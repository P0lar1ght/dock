//! Grok `workflow` tool + Rhai engine, mounted as Cordis `"workflows"` + `"tools"`.

mod args;
mod drain;
mod grok_tool;
mod listing;
mod registry;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};

use crate::context_book::{own_sections, ContextBook};
use crate::names::{CONTEXT, SESSIONS, SETTINGS, SLASH, TOOLS, TOOLS_EXECUTE, WORKFLOWS};
use crate::prompt::ORDER_WORKFLOWS;
use crate::session::Sessions;
use crate::settings::AppSettings;
use crate::slash::{slash_name_reserved, ExtraSlashKind, Slash, SlashEntry};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{LogEvent, ToolCall, ToolResult, ToolSpec};

pub use args::{workflow_command_arguments, workflow_slash_arguments};
pub use drain::WorkflowRunSnap;
pub use grok_tool::{render_ack, WorkflowLaunchHandle, WorkflowToolInput, WORKFLOW_TOOL_NAME};

use registry::WorkflowListing;

/// Names + descriptions of workflows the model can launch (builtin + disk).
pub(crate) fn catalog_listing() -> Vec<(String, String)> {
    scan_catalog()
        .into_iter()
        .map(|w| (w.name, w.description))
        .collect()
}

fn scan_catalog() -> Vec<WorkflowListing> {
    let cwd = std::env::current_dir().ok();
    registry::list_workflows(cwd.as_deref())
}

/// Map typed slash args for an extra `kind: tool` command.
/// Workflow extras (`text` = `workflow`) build `source.type=name` JSON.
pub fn extra_tool_slash_arguments(entry: &SlashEntry, args: &str) -> Result<String, String> {
    if entry.text.trim() == WORKFLOW_TOOL_NAME {
        workflow_slash_arguments(&entry.command, args)
    } else {
        Ok(crate::slash::tool_slash_arguments(args))
    }
}

/// Named `"workflows"` service. TUI live-looks `list()` of runs.
pub struct Workflows {
    ctx: Context,
    state: Arc<drain::WorkflowState>,
    pub handle: WorkflowLaunchHandle,
    extras: Mutex<Vec<Disposable>>,
    announced: Mutex<HashSet<String>>,
    frozen_listing: Mutex<Option<String>>,
}

impl Workflows {
    pub fn list(&self) -> Vec<WorkflowRunSnap> {
        self.state.list()
    }

    pub fn catalog(&self) -> Vec<WorkflowInfo> {
        scan_catalog().into_iter().map(WorkflowInfo::from).collect()
    }

    fn listing_text(&self) -> String {
        if let Some(frozen) = self.frozen_listing.lock().unwrap().clone() {
            return frozen;
        }
        let window = self
            .ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| s.usage().window)
            .filter(|w| *w > 0)
            .or_else(|| {
                self.ctx.get::<AppSettings>(SETTINGS).and_then(|s| {
                    s.catalog()
                        .into_iter()
                        .find(|m| m.id == s.model())
                        .and_then(|m| m.context_window)
                })
            })
            .unwrap_or(128_000);
        let text = listing::render_listing(&scan_catalog(), listing::listing_budget_chars(window));
        *self.frozen_listing.lock().unwrap() = Some(text.clone());
        text
    }

    fn sync_slash(&self) {
        let Some(slash) = self.ctx.get::<Slash>(SLASH) else {
            return;
        };
        {
            let mut extras = self.extras.lock().unwrap();
            extras.clear();
        }
        let mut extras = Vec::new();
        for workflow in scan_catalog() {
            if slash_name_reserved(&workflow.name) || workflow.name == "skills" {
                continue;
            }
            let entry = SlashEntry {
                command: workflow.name.clone(),
                description: workflow.description.clone(),
                kind: ExtraSlashKind::Tool,
                text: WORKFLOW_TOOL_NAME.into(),
                title: workflow.name.clone(),
                send: false,
            };
            if let Ok(d) = slash.register(entry) {
                extras.push(d);
            }
        }
        *self.extras.lock().unwrap() = extras;
    }

    fn notice_paths(&self, paths: &[PathBuf]) {
        if !paths.iter().any(|p| path_near_workflows(p)) {
            return;
        }
        let catalog = scan_catalog();
        let mut new_names = Vec::new();
        {
            let mut announced = self.announced.lock().unwrap();
            for workflow in &catalog {
                if announced.insert(workflow.name.clone()) {
                    new_names.push(workflow.name.clone());
                }
            }
        }
        self.sync_slash();
        if new_names.is_empty() {
            return;
        }
        if let Some(sessions) = self.ctx.get::<Sessions>(SESSIONS) {
            sessions.append(LogEvent::SystemReminder(format!(
                "发现新工作流：{}。用 `/name` 启动，或 `workflow` 工具 source.type=name。",
                new_names.join("、")
            )));
        }
    }
}

/// Public catalog row for tests and live-lookup.
#[derive(Clone, Debug)]
pub struct WorkflowInfo {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub source: &'static str,
    pub path: Option<String>,
}

impl From<WorkflowListing> for WorkflowInfo {
    fn from(w: WorkflowListing) -> Self {
        Self {
            name: w.name,
            description: w.description,
            when_to_use: w.when_to_use,
            source: w.source,
            path: w.path,
        }
    }
}

fn path_near_workflows(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    if raw.contains(".dock/workflows") || raw.contains(".dock\\workflows") {
        return true;
    }
    let home = crate::config::dock_home();
    path.starts_with(home.join("workflows"))
        || path.starts_with(home.join("bundled").join("workflows"))
}

fn arguments_for(ctx: &Context, result: &ToolResult) -> String {
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return String::new();
    };
    sessions
        .events()
        .into_iter()
        .rev()
        .find_map(|e| match e {
            LogEvent::ToolExecute { id, arguments, .. } if id == result.call_id => Some(arguments),
            _ => None,
        })
        .unwrap_or_default()
}

fn extract_paths_from_tool(name: &str, arguments: &str, content: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) {
        walk_json_strings(&v, &mut out);
    }
    for prefix in ["updated ", "created ", "wrote "] {
        if let Some(rest) = content.lines().next().and_then(|l| l.strip_prefix(prefix)) {
            if !rest.starts_with("Error") {
                out.push(PathBuf::from(rest.trim()));
            }
        }
    }
    if matches!(name, "list_dir" | "glob" | "grep" | "read_file") {
        for line in content.lines().take(80) {
            let line = line.trim();
            if line.is_empty() || line.starts_with("Error") {
                continue;
            }
            let candidate = line.split(':').next().unwrap_or(line);
            if path_near_workflows(Path::new(candidate)) {
                out.push(PathBuf::from(candidate));
            }
        }
    }
    out.into_iter()
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

fn walk_json_strings(v: &serde_json::Value, out: &mut Vec<PathBuf>) {
    match v {
        serde_json::Value::String(s) if !s.is_empty() => out.push(PathBuf::from(s)),
        serde_json::Value::Array(items) => {
            for item in items {
                walk_json_strings(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for val in map.values() {
                walk_json_strings(val, out);
            }
        }
        _ => {}
    }
}

const DESC: &str = "启动工作流：一段 Rhai 脚本，把子代理编排成一次后台运行。\
source 只能有一个：已注册 name、内联 script、script_path，或同进程 resume。\
可选 args（绑到脚本 args）和 agent_budget（子代理调用上限，默认 128，最大 1024）。\
调用立即返回；进度看 /workflow runs，完成后会自动汇报，不要轮询 wait_tasks。\
validate_only: true 只做冒烟检查（元数据、编译、一条 canned-host 路径），不证明每个分支或真实工具可用。\
可复用脚本放到 .dock/workflows/<name>.rhai 或 ~/.dock/workflows/<name>.rhai；斜杠 `/name` 直接启动。";

const PARAMS: &str = r#"{"type":"object","properties":{"source":{"description":"Exactly one workflow source.","oneOf":[{"type":"object","required":["type","name"],"properties":{"type":{"const":"name"},"name":{"type":"string"}}},{"type":"object","required":["type","script"],"properties":{"type":{"const":"script"},"script":{"type":"string"}}},{"type":"object","required":["type","script_path"],"properties":{"type":{"const":"script_path"},"script_path":{"type":"string"}}},{"type":"object","required":["type","resume_from_run_id"],"properties":{"type":{"const":"resume"},"resume_from_run_id":{"type":"string"}}}]},"agent_budget":{"type":"integer","minimum":1,"maximum":1024},"args":{},"validate_only":{"type":"boolean"},"name":{"type":"string"},"script":{"type":"string"},"script_path":{"type":"string"},"resume_from_run_id":{"type":"string"}},"required":[]}"#;

pub fn tool_workflow() -> Plugin {
    plugin(
        "tool-workflow",
        Inject::from([TOOLS, CONTEXT]),
        |ctx, _: &()| {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let state = Arc::new(drain::WorkflowState::new());
            {
                let state = state.clone();
                let ctx = ctx.clone();
                tokio::spawn(drain::drain_loop_with_ctx(state, ctx, rx));
            }
            let catalog_names: HashSet<String> =
                scan_catalog().into_iter().map(|w| w.name).collect();
            let handle = Workflows {
                ctx: ctx.clone(),
                state,
                handle: WorkflowLaunchHandle(tx),
                extras: Mutex::new(Vec::new()),
                announced: Mutex::new(catalog_names),
                frozen_listing: Mutex::new(None),
            };
            handle.sync_slash();
            let provided = ctx.provide(WORKFLOWS, handle)?;
            let book = ctx.require::<ContextBook>(CONTEXT)?;
            own_sections(
                ctx,
                vec![book.section(ORDER_WORKFLOWS, "workflows", |exec| {
                    exec.get::<Workflows>(WORKFLOWS).and_then(|wf| {
                        let listing = wf.listing_text();
                        if listing.trim().is_empty() {
                            None
                        } else {
                            Some(listing)
                        }
                    })
                })?],
            )?;
            let ctx_exec = ctx.clone();
            let _ = ctx.on_waterfall(TOOLS_EXECUTE, move |result: ToolResult, args| {
                let result = args.next::<ToolResult>().unwrap_or(result);
                if let Some(wf) = ctx_exec.get::<Workflows>(WORKFLOWS) {
                    let call_args = arguments_for(&ctx_exec, &result);
                    let paths = extract_paths_from_tool(&result.name, &call_args, &result.content);
                    if !paths.is_empty() {
                        wf.notice_paths(&paths);
                    }
                }
                result
            });
            let tools = ctx.require::<Tools>(TOOLS)?;
            let body: ToolBody = {
                let ctx = ctx.clone();
                std::sync::Arc::new(move |call| {
                    let ctx = ctx.clone();
                    Box::pin(async move { run_workflow_tool(&ctx, call).await })
                })
            };
            own_registered(
                ctx,
                vec![tools.register_deferred(
                    ToolSpec {
                        name: WORKFLOW_TOOL_NAME.into(),
                        description: DESC.into(),
                        parameters_json: PARAMS.into(),
                    },
                    body,
                )?],
            )?;
            Ok(Some(provided))
        },
    )
}

/// Copied from Grok `WorkflowTool::run` (handle → oneshot ack → render).
async fn run_workflow_tool(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let input: WorkflowToolInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: workflow_invalid_input: {e}")),
    };
    let Some(wf) = ctx.get::<Workflows>(WORKFLOWS) else {
        return tool_result(
            call,
            "Error: workflow_not_available: workflows is not mounted",
        );
    };
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    if wf
        .handle
        .0
        .send((grok_tool::WorkflowLaunchRequest { input }, ack_tx))
        .is_err()
    {
        return tool_result(
            call,
            "Error: workflow_channel_closed: the session may be shutting down",
        );
    }
    match ack_rx.await {
        Ok(ack) => match render_ack(ack) {
            Ok(out) => tool_result(
                call,
                serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.message),
            ),
            Err((code, detail)) => tool_result(call, format!("Error: {code}: {detail}")),
        },
        Err(_) => tool_result(
            call,
            "Error: workflow_launch_no_ack: the session dropped the launch channel before answering",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_usage::{occupancy_detail, snapshot_context, OccupancyKind};
    use crate::prompt::{SystemPrompt, ORDER_SKILLS, ORDER_WORKFLOWS};
    use crate::slash::slash;
    use std::sync::{Mutex as StdMutex, MutexGuard};

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    struct EnvGuard {
        prev_cwd: std::path::PathBuf,
        prev_home: Option<String>,
        _lock: MutexGuard<'static, ()>,
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.prev_cwd);
            match &self.prev_home {
                Some(v) => std::env::set_var("DOCK_HOME", v),
                None => std::env::remove_var("DOCK_HOME"),
            }
        }
    }

    fn lock_env(dir: &std::path::Path) -> EnvGuard {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_cwd = std::env::current_dir().unwrap();
        let prev_home = std::env::var("DOCK_HOME").ok();
        std::env::set_current_dir(dir).unwrap();
        std::env::set_var("DOCK_HOME", dir.join("dock-home"));
        std::fs::create_dir_all(dir.join("dock-home")).unwrap();
        EnvGuard {
            prev_cwd,
            prev_home,
            _lock: lock,
        }
    }

    fn script(name: &str, desc: &str) -> String {
        format!("let meta = #{{ name: \"{name}\", description: \"{desc}\" }};\ncomplete(\"ok\");")
    }

    async fn mount_workflows(ctx: &Context) {
        crate::install_without_llm(ctx).await.unwrap();
        ctx.plugin(slash(), ()).unwrap().wait().await.unwrap();
        ctx.plugin(tool_workflow(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
    }

    #[test]
    fn order_workflows_follow_roster() {
        assert!(ORDER_WORKFLOWS > crate::prompt::ORDER_ROSTER);
        assert!(ORDER_SKILLS > ORDER_WORKFLOWS);
    }

    #[tokio::test]
    async fn builtin_deep_research_is_a_slash_extra() {
        let dir = tempfile::tempdir().unwrap();
        let _env = lock_env(dir.path());
        let ctx = cordis::Context::new();
        mount_workflows(&ctx).await;
        let extras = ctx.get::<Slash>(SLASH).unwrap().list();
        let extra = extras
            .iter()
            .find(|e| e.command == "deep-research")
            .expect("deep-research extra");
        assert_eq!(extra.kind, ExtraSlashKind::Tool);
        assert_eq!(extra.text, WORKFLOW_TOOL_NAME);
        assert!(!extras.iter().any(|e| e.command == "workflow"));
        let catalog = ctx.get::<Workflows>(WORKFLOWS).unwrap().catalog();
        assert!(catalog.iter().any(|w| w.name == "deep-research"));
    }

    #[tokio::test]
    async fn project_custom_workflow_registers_extra() {
        let dir = tempfile::tempdir().unwrap();
        let wf_dir = dir.path().join(".dock").join("workflows");
        std::fs::create_dir_all(&wf_dir).unwrap();
        std::fs::write(
            wf_dir.join("review-pr.rhai"),
            script("review-pr", "Review a PR."),
        )
        .unwrap();
        let _env = lock_env(dir.path());
        let ctx = cordis::Context::new();
        mount_workflows(&ctx).await;
        let extras = ctx.get::<Slash>(SLASH).unwrap().list();
        assert!(
            extras
                .iter()
                .any(|e| e.command == "review-pr" && e.kind == ExtraSlashKind::Tool),
            "{extras:?}"
        );
    }

    #[tokio::test]
    async fn reserved_workflow_filename_is_not_an_extra() {
        let dir = tempfile::tempdir().unwrap();
        let wf_dir = dir.path().join(".dock").join("workflows");
        std::fs::create_dir_all(&wf_dir).unwrap();
        std::fs::write(
            wf_dir.join("help.rhai"),
            script("help", "Must not shadow /help."),
        )
        .unwrap();
        let _env = lock_env(dir.path());
        let ctx = cordis::Context::new();
        mount_workflows(&ctx).await;
        let extras = ctx.get::<Slash>(SLASH).unwrap().list();
        assert!(!extras.iter().any(|e| e.command == "help"));
        assert!(!extras.iter().any(|e| e.command == "workflow"));
    }

    #[tokio::test]
    async fn assemble_lists_workflows_and_occupancy_does_not_double_count() {
        let dir = tempfile::tempdir().unwrap();
        let _env = lock_env(dir.path());
        let ctx = cordis::Context::new();
        mount_workflows(&ctx).await;
        let assembled = ctx
            .get::<SystemPrompt>(crate::names::SYSTEM_PROMPT)
            .unwrap()
            .assemble();
        assert!(assembled.contains("deep-research"), "{assembled}");
        assert!(assembled.contains("可用工作流"), "{assembled}");
        let snap = snapshot_context(&ctx);
        let extra = snap
            .categories
            .iter()
            .find(|c| c.label == "工作流")
            .expect("工作流 legend");
        assert!(extra.tokens > 0);
        assert_eq!(
            snap.used,
            snap.system_prompt_tokens
                .saturating_add(snap.message_tokens)
                .saturating_add(snap.tool_definitions_tokens)
        );
        let detail = occupancy_detail(&ctx, OccupancyKind::Workflows);
        assert_eq!(detail.kind, OccupancyKind::Workflows);
        assert!(
            detail
                .groups
                .iter()
                .flat_map(|g| &g.rows)
                .any(|r| r.label == "deep-research"),
            "{detail:?}"
        );
    }

    #[tokio::test]
    async fn tools_execute_discovers_new_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let _env = lock_env(dir.path());
        let ctx = cordis::Context::new();
        mount_workflows(&ctx).await;
        let wf_dir = dir.path().join(".dock").join("workflows");
        std::fs::create_dir_all(&wf_dir).unwrap();
        let path = wf_dir.join("late-flow.rhai");
        std::fs::write(&path, script("late-flow", "Discovered mid session.")).unwrap();
        let result = ToolResult {
            call_id: "w1".into(),
            name: "write_file".into(),
            content: format!("created {}", path.display()),
        };
        ctx.waterfall(TOOLS_EXECUTE, result.clone(), move || result);
        let extras = ctx.get::<Slash>(SLASH).unwrap().list();
        assert!(
            extras.iter().any(|e| e.command == "late-flow"),
            "{extras:?}"
        );
        let events = ctx.get::<Sessions>(SESSIONS).unwrap().events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LogEvent::SystemReminder(t) if t.contains("late-flow"))),
            "{events:?}"
        );
    }

    #[test]
    fn extra_tool_arguments_build_named_source() {
        let entry = SlashEntry {
            command: "deep-research".into(),
            description: "d".into(),
            kind: ExtraSlashKind::Tool,
            text: WORKFLOW_TOOL_NAME.into(),
            title: "deep-research".into(),
            send: false,
        };
        let json = extra_tool_slash_arguments(&entry, "why rust").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["source"]["name"], "deep-research");
        assert_eq!(v["args"]["query"], "why rust");
        let other = SlashEntry {
            command: "test".into(),
            description: "d".into(),
            kind: ExtraSlashKind::Tool,
            text: "dyn_echo".into(),
            title: String::new(),
            send: false,
        };
        assert_eq!(extra_tool_slash_arguments(&other, "").unwrap(), "{}");
    }
}
