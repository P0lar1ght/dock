//! Grok `workflow` tool + Rhai engine, mounted as Cordis `"workflows"` + `"tools"`.

mod args;
mod command;
mod drain;
mod grok_tool;
mod host;
mod listing;
mod registry;
mod schema_contract;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};

use crate::host::settings::AppSettings;
use crate::host::slash::{ExtraSlashKind, Slash, SlashEntry};
use crate::names::{
    SESSIONS, SETTINGS, SLASH, STEP_START, TOOLS, TOOLS_EXECUTE, WORKFLOWS, WORKFLOW_CHANGED,
};
use crate::prompt::listing::wants_listing;
use crate::session::log::Sessions;
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{
    LogEvent, StepStart, ToolCall, ToolResult, ToolSpec, ORDER_STEP_START_WORKFLOWS,
};

pub use args::{workflow_command_arguments, workflow_slash_arguments};
pub use command::workflow_command;
pub use drain::{WorkflowAgentRow, WorkflowRunSnap};

/// [`crate::WORKFLOW_CHANGED`] 的载荷。
#[derive(Clone, Debug)]
pub struct WorkflowChanged {
    pub run_id: Arc<str>,
}
pub use grok_tool::{render_ack, WorkflowLaunchHandle, WorkflowToolInput, WORKFLOW_TOOL_NAME};
pub use host::DEFAULT_MAX_CONCURRENT_AGENTS;

use registry::WorkflowListing;

/// Names + descriptions of workflows the model can launch (builtin + disk).
pub(crate) fn catalog_listing() -> Vec<(String, String)> {
    scan_catalog()
        .into_iter()
        .map(|w| (w.name, w.description))
        .collect()
}

fn scan_catalog() -> Vec<WorkflowListing> {
    registry::list_workflows(Some(&crate::session::cwd::current_cwd()))
}

/// One workflow's metadata plus its full Rhai script (builtin + disk), for the
/// `/context` drill-down.
pub fn workflow_detail(name: &str) -> Option<(WorkflowInfo, String)> {
    let cwd = crate::session::cwd::current_cwd();
    let (registry, listings) = registry::workflow_snapshot(Some(&cwd));
    let listing = listings.into_iter().find(|l| l.name == name)?;
    let resolved = registry.resolve_by_name(name).ok()?;
    Some((WorkflowInfo::from(listing), resolved.script))
}

/// Map typed slash args for an extra `kind: tool` command.
/// Workflow extras (`text` = `workflow`) build `source.type=name` JSON.
pub fn extra_tool_slash_arguments(entry: &SlashEntry, args: &str) -> Result<String, String> {
    if entry.text.trim() == WORKFLOW_TOOL_NAME {
        workflow_slash_arguments(&entry.command, args)
    } else {
        Ok(crate::host::slash::tool_slash_arguments(args))
    }
}

/// Named `"workflows"` service. TUI live-looks `list()` of runs.
pub struct Workflows {
    ctx: Context,
    state: Arc<drain::WorkflowState>,
    pub handle: WorkflowLaunchHandle,
    extras: Mutex<Vec<Disposable>>,
    announced: Mutex<HashSet<String>>,
    /// 本会话**中途**发现的工作流名。启动时就在目录里的不算——那些在目录
    /// reminder 里，压缩后还会重新注入。
    discovered: Mutex<Vec<String>>,
    frozen_listing: Mutex<Option<String>>,
}

impl Workflows {
    pub fn list(&self) -> Vec<WorkflowRunSnap> {
        self.state.list()
    }

    pub fn catalog(&self) -> Vec<WorkflowInfo> {
        scan_catalog().into_iter().map(WorkflowInfo::from).collect()
    }

    /// 停掉一次运行中的 run。`needle` 可以是 run id，也可以是显示名。
    ///
    /// 返回被停掉的 run id；没有匹配的活跃 run 时返回 `None`。
    pub fn stop(&self, needle: &str) -> Option<String> {
        let run_id = self.state.resolve(needle)?;
        self.state.cancel(&run_id).then_some(run_id)
    }

    pub(crate) fn listing_text(&self) -> String {
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
            if slash.is_builtin(&workflow.name) || workflow.name == "skills" {
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

    /// 压缩后重发的「中途发现」提示，没有中途发现就返回 `None`。
    ///
    /// 目录在会话里冻结（不随磁盘扫描重写），所以这些工作流只靠历史里那条
    /// reminder 让模型知道；压缩抹掉它之后得再说一遍。
    pub fn rediscovery_notice(&self) -> Option<String> {
        let found = self.discovered.lock().unwrap().clone();
        if found.is_empty() {
            return None;
        }
        Some(format!(
            "本会话中途发现的工作流：{}。用 `/name` 启动，或 `workflow` 工具 source.type=name。",
            found.join("、")
        ))
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
        {
            let mut discovered = self.discovered.lock().unwrap();
            discovered.extend(new_names.iter().cloned());
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
    let home = cordis_base::config::dock_home();
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
source 只能有一个：已注册 name、内联 script、script_path。\
可选 args（绑到脚本 args）和 agent_budget（子代理调用上限，默认 128，最大 1024）。\
调用立即返回；进度看 /workflow runs，完成后会自动汇报，不要轮询 job。\
**动手写或改脚本之前先读 `create-workflow` 技能的 SKILL.md**：脚本形状、host API、\
方言规则、保存位置和 dock 的限制都在那儿。\
validate_only: true 只做冒烟检查（元数据、编译、一条 canned-host 路径），不证明每个分支或真实工具可用。\
停一次在跑的 run 让用户走 /workflow stop <name>。";

const PARAMS: &str = r#"{"type":"object","properties":{"source":{"description":"Exactly one workflow source.","oneOf":[{"type":"object","required":["type","name"],"properties":{"type":{"const":"name"},"name":{"type":"string"}}},{"type":"object","required":["type","script"],"properties":{"type":{"const":"script"},"script":{"type":"string"}}},{"type":"object","required":["type","script_path"],"properties":{"type":{"const":"script_path"},"script_path":{"type":"string"}}}]},"agent_budget":{"type":"integer","minimum":1,"maximum":1024},"args":{},"validate_only":{"type":"boolean"}},"required":[]}"#;

/// 历史里那份工作流目录的开头。结构化标记：会话里要认出「哪一条 reminder 是
/// 目录副本」才谈得上判断该不该重注（压缩抹掉旧副本、`/resume` 回放旧版）。
const LISTING_MARKER: &str = "<system-reminder>\n可用工作流";
/// 切换预设后旧目录还在历史里，补一句让它失效，免得模型照着调一颗已经不存在的工具。
const RETRACT_MARKER: &str = "<system-reminder>\n工作流目录不再适用";

/// 历史里最近一条目录相关提醒：`Some(Some(copy))` 是目录副本，`Some(None)` 是
/// 停用说明，`None` 是还没发过。
fn latest_state(history: &[LogEvent]) -> Option<Option<String>> {
    history.iter().rev().find_map(|e| match e {
        LogEvent::SystemReminder(text) if text.starts_with(LISTING_MARKER) => {
            Some(Some(text.clone()))
        }
        LogEvent::SystemReminder(text) if text.starts_with(RETRACT_MARKER) => Some(None),
        _ => None,
    })
}

fn latest_listing(history: &[LogEvent]) -> Option<String> {
    latest_state(history).flatten()
}

/// 本会话实际注入的那份目录（`/context` 用它报「已计入消息」）。
pub(crate) fn injected_copy(exec: &Context) -> Option<String> {
    latest_listing(&exec.get::<Sessions>(SESSIONS)?.model_history())
}

/// 这一步该注入的目录。`None` = 预设调不动 `workflow`、目录为空、或历史里最近
/// 一份已经一致。目录内容首帧冻结；压缩抹掉旧副本后重新注入同一份。
fn pending_listing(exec: &Context) -> Option<String> {
    let state = exec
        .get::<Sessions>(SESSIONS)
        .and_then(|s| latest_state(&s.model_history()));
    if !wants_listing(exec, WORKFLOW_TOOL_NAME) {
        return state.flatten().map(|_| {
            format!(
                "{RETRACT_MARKER}：当前预设调不动 `workflow`，上面那份清单照着调会被拒绝。\n</system-reminder>"
            )
        });
    }
    let listing = exec.get::<Workflows>(WORKFLOWS)?.listing_text();
    if listing.trim().is_empty() {
        return None;
    }
    let body = format!("<system-reminder>\n{listing}</system-reminder>");
    (state != Some(Some(body.clone()))).then_some(body)
}

pub fn tool_workflow() -> Plugin {
    plugin("tool-workflow", Inject::from([TOOLS]), |ctx, _: &()| {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(drain::WorkflowState::new());
        {
            let ctx = ctx.clone();
            state.set_on_change(Arc::new(move |run_id: &str| {
                ctx.emit(
                    WORKFLOW_CHANGED,
                    WorkflowChanged {
                        run_id: run_id.into(),
                    },
                );
            }));
        }
        {
            let state = state.clone();
            let ctx = ctx.clone();
            tokio::spawn(drain::drain_loop_with_ctx(state, ctx, rx));
        }
        let catalog_names: HashSet<String> = scan_catalog().into_iter().map(|w| w.name).collect();
        let handle = Workflows {
            ctx: ctx.clone(),
            state,
            handle: WorkflowLaunchHandle(tx),
            extras: Mutex::new(Vec::new()),
            announced: Mutex::new(catalog_names),
            discovered: Mutex::new(Vec::new()),
            frozen_listing: Mutex::new(None),
        };
        handle.sync_slash();
        let provided = ctx.provide(WORKFLOWS, handle)?;
        let ctx_step = ctx.clone();
        let step = ctx.on_waterfall(STEP_START, move |start: StepStart, args| {
            let mut next = args.next::<StepStart>().unwrap_or(start);
            let exec = crate::tools::registry::exec_ctx().unwrap_or_else(|| ctx_step.clone());
            if let Some(body) = pending_listing(&exec) {
                next.remind_preamble(ORDER_STEP_START_WORKFLOWS, body);
            }
            next
        })?;
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
            // 内置预设把它设成按需（`on_demand_tools`）。listing 认 `use_tool` 可达，
            // 见 `Tools::reachable_on`。
            vec![
                step,
                tools.register(
                    ToolSpec {
                        name: WORKFLOW_TOOL_NAME.into(),
                        description: DESC.into(),
                        parameters_json: PARAMS.into(),
                    },
                    body,
                )?,
            ],
        )?;
        Ok(Some(provided))
    })
}

/// Copied from Grok `WorkflowTool::run` (handle → oneshot ack → render).
async fn run_workflow_tool(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    // 子代理不许再起工作流。宽口径角色（`general-purpose`）的工具集里就有
    // `workflow`，而 workflow 的子代理默认就是这个角色——不挡的话每一层都能拿
    // 到一份全新的 `agent_budget` 与一个全新的并发池，run 级预算正好管不住。
    let depth = crate::tools::task::current_depth();
    if depth > 0 {
        return tool_result(
            call,
            format!(
                "Error: workflow_depth_exceeded: 子代理不能再启动工作流（当前深度 {depth}）。\
                 把需要的工作交给这一层完成，或让主代理另起一条工作流。"
            ),
        );
    }
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
        .send((
            grok_tool::WorkflowLaunchRequest {
                input,
                cwd: crate::session::cwd::current_cwd(),
                page: crate::tools::registry::exec_ctx(),
                tool_call_id: Some(call.id.clone()),
            },
            ack_tx,
        ))
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
                serde_json::to_string_pretty(&out).unwrap_or(out.message),
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
    use crate::host::slash::slash;

    /// 子代理不许再起工作流。
    ///
    /// 宽口径角色（`general-purpose`）的工具集里就有 `workflow`，而 workflow 的
    /// 子代理默认就是这个角色；不挡的话每一层拿一份全新的 `agent_budget` 和一个
    /// 全新的并发池，run 级预算管不住。
    #[tokio::test]
    async fn a_subagent_cannot_launch_another_workflow() {
        let dir = tempfile::tempdir().unwrap();
        let _env = env_in(dir.path());
        let ctx = Context::new();
        mount_workflows(&ctx).await;
        let call = || ToolCall {
            id: "nested".into(),
            name: WORKFLOW_TOOL_NAME.into(),
            arguments: r#"{"source":{"type":"name","name":"deep-research"}}"#.into(),
        };

        let nested = crate::tools::task::DEPTH
            .scope(1, run_workflow_tool(&ctx, call()))
            .await;
        assert!(
            nested.content.contains("workflow_depth_exceeded"),
            "{}",
            nested.content
        );

        // 主会话（depth 0）不受影响。
        let top = run_workflow_tool(&ctx, call()).await;
        assert!(
            !top.content.contains("workflow_depth_exceeded"),
            "{}",
            top.content
        );
    }

    use crate::names::AGENT_PRESETS;
    use crate::prompt::context_usage::{occupancy_detail, snapshot_context, OccupancyKind};
    /// `DOCK_HOME` 指向临时目录 + 切 cwd，退出时还原（进程级状态由 test_env 串行化）。
    fn env_in(dir: &std::path::Path) -> cordis_base::test_env::EnvScope {
        std::fs::create_dir_all(dir.join("dock-home")).unwrap();
        cordis_base::test_env::scoped()
            .cwd(dir)
            .set("DOCK_HOME", dir.join("dock-home"))
    }

    fn script(name: &str, desc: &str) -> String {
        format!("let meta = #{{ name: \"{name}\", description: \"{desc}\" }};\ncomplete(\"ok\");")
    }

    async fn mount_workflows(ctx: &Context) {
        crate::install_without_llm(ctx).await.unwrap();
        ctx.plugin(slash(), ()).unwrap().wait().await.unwrap();
        // `workflow` 按需加载：listing 要看得到 `use_tool` 才算叫得动它。
        ctx.plugin(crate::tools::mcp::mcp_client(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        ctx.plugin(tool_workflow(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
    }

    /// 工作流目录排在技能之后、子代理名册之前。
    #[test]
    fn catalog_reminder_sits_after_skills_before_roster() {
        const { assert!(ORDER_STEP_START_WORKFLOWS > cordis_base::types::ORDER_STEP_START_SKILLS) };
        const { assert!(ORDER_STEP_START_WORKFLOWS < cordis_base::types::ORDER_STEP_START_ROSTER) };
    }

    #[tokio::test]
    async fn builtin_deep_research_is_a_slash_extra() {
        let dir = tempfile::tempdir().unwrap();
        let _env = env_in(dir.path());
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
        let _env = env_in(dir.path());
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
        let _env = env_in(dir.path());
        let ctx = cordis::Context::new();
        mount_workflows(&ctx).await;
        let extras = ctx.get::<Slash>(SLASH).unwrap().list();
        assert!(!extras.iter().any(|e| e.command == "help"));
        assert!(!extras.iter().any(|e| e.command == "workflow"));
    }

    #[tokio::test]
    async fn catalog_rides_a_reminder_and_occupancy_counts_messages() {
        let dir = tempfile::tempdir().unwrap();
        let _env = env_in(dir.path());
        let ctx = cordis::Context::new();
        mount_workflows(&ctx).await;
        let assembled = ctx
            .get::<crate::prompt::assemble::SystemPrompt>(crate::names::SYSTEM_PROMPT)
            .unwrap()
            .assemble();
        assert!(
            !assembled.contains("可用工作流"),
            "目录走 reminder，不再进系统提示: {assembled}"
        );
        let start = StepStart::new(0, crate::session::log::ROOT_IDENTITY);
        let seed = start.clone();
        let reminders = ctx
            .waterfall(STEP_START, start, move || seed)
            .into_reminders();
        let catalog = reminders
            .iter()
            .find(|t| t.contains("可用工作流"))
            .expect("工作流目录 reminder")
            .clone();
        assert!(catalog.starts_with("<system-reminder>"), "{catalog}");
        assert!(catalog.contains("deep-research"), "{catalog}");
        // 模拟循环把这条 reminder 写进历史；`/context` 报的是实际注入的这份。
        ctx.require::<Sessions>(SESSIONS)
            .unwrap()
            .append(LogEvent::SystemReminder(catalog));
        let snap = snapshot_context(&ctx);
        let extra = snap
            .categories
            .iter()
            .find(|c| c.label == "工作流")
            .expect("工作流 legend");
        assert!(extra.tokens > 0);
        assert!(
            extra
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("已计入消息")),
            "{extra:?}"
        );
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
        let _env = env_in(dir.path());
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
            ..Default::default()
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

    fn catalog_reminder(exec: &Context, identity: &str) -> Option<String> {
        let start = StepStart::new(0, identity);
        let seed = start.clone();
        crate::tools::registry::with_exec_ctx(exec, || {
            exec.waterfall(STEP_START, start, move || seed)
                .into_reminders()
                .into_iter()
                .find(|text| text.starts_with(LISTING_MARKER) || text.starts_with(RETRACT_MARKER))
        })
    }

    /// 目录 reminder 只发一次；压缩抹掉旧副本后重注。
    #[tokio::test]
    async fn catalog_injects_once_and_again_after_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let _env = env_in(dir.path());
        let ctx = cordis::Context::new();
        mount_workflows(&ctx).await;
        let first = catalog_reminder(&ctx, crate::session::log::ROOT_IDENTITY).expect("first");
        let sessions = ctx.require::<Sessions>(SESSIONS).unwrap();
        sessions.append(LogEvent::SystemReminder(first));
        assert!(
            catalog_reminder(&ctx, crate::session::log::ROOT_IDENTITY).is_none(),
            "历史里已有就不再注入"
        );
        sessions.replace_compacted(vec![LogEvent::SystemReminder("summary".into())]);
        assert!(
            catalog_reminder(&ctx, crate::session::log::ROOT_IDENTITY).is_some(),
            "压缩后要重注"
        );
    }

    /// 子代理默认不带目录；`agents/<type>.yml` 开了 `listings: true` 才带。
    #[tokio::test]
    async fn child_catalog_is_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let _env = env_in(dir.path());
        let root = cordis::Context::new();
        mount_workflows(&root).await;

        let child = root.isolate("sessions").isolate("agentPresets");
        child
            .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-1"))
            .unwrap();
        assert!(
            catalog_reminder(&child, "child-1").is_none(),
            "子代理默认不带目录"
        );

        let mut wide = crate::agent::presets::SubagentDef {
            name: "通用".into(),
            listings: true,
            ..Default::default()
        };
        wide.tools = Some(vec![WORKFLOW_TOOL_NAME.into(), "use_tool".into()]);
        child
            .provide(
                AGENT_PRESETS,
                crate::agent::presets::AgentPresets::overlay(wide.to_preset("general-purpose")),
            )
            .unwrap();
        assert!(
            catalog_reminder(&child, "child-1").is_some(),
            "listings: true 才带"
        );
    }

    /// 切到调不动 `workflow` 的预设：补一句「目录不再适用」，且只补一次。
    #[tokio::test]
    async fn preset_without_workflow_retracts_the_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let _env = env_in(dir.path());
        let ctx = cordis::Context::new();
        mount_workflows(&ctx).await;
        let first = catalog_reminder(&ctx, crate::session::log::ROOT_IDENTITY).expect("first");
        let sessions = ctx.require::<Sessions>(SESSIONS).unwrap();
        sessions.append(LogEvent::SystemReminder(first));

        let mut narrow = crate::agent::presets::AgentPreset::new("narrow");
        narrow.tools = Some(vec!["read_file".into()]);
        ctx.provide(
            AGENT_PRESETS,
            crate::agent::presets::AgentPresets::overlay(narrow),
        )
        .unwrap();

        let retract =
            catalog_reminder(&ctx, crate::session::log::ROOT_IDENTITY).expect("retraction");
        assert!(retract.contains("工作流目录不再适用"), "{retract}");
        sessions.append(LogEvent::SystemReminder(retract));
        assert!(
            catalog_reminder(&ctx, crate::session::log::ROOT_IDENTITY).is_none(),
            "停用说明只发一次"
        );
    }
}
