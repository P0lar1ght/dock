//! Model-facing plugin tools: `cordis_inspect` (read) and `cordis_plugin`
//! (reload / disable a plugin directory). Injects `"dynamicCordisRunner"` +
//! `"tools"`; does not own the registry.
//!
//! 对齐 DSH 现在的做法：模型只有「只读查看」和「管理插件目录」两颗工具，插件
//! 本身就是普通文件，用文件工具写。以前那套 define / run / call / stop / promote
//! 让模型在内存里的会话插件和磁盘插件之间来回搬：一次修改要三次调用，插件 id
//! 跟着分裂（`safe-1` → `safeguard`，`cyberspace` → `cyber-2`），define 和 run
//! 校验的路径还不一样。DSH 撤掉同一套工具的理由原话是「生成代码工具在普通已安装
//! 组合包之外引入了第二套插件生命周期」。

mod inspect;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use cordis::{plugin, Context, Inject, Plugin};
use serde_json::Value;

use crate::names::{DYNAMIC_CORDIS_RUNNER, PRE_STEP, SESSIONS, TOOLS};
use crate::session::log::Sessions;
use crate::tools::dynamic_runner::{
    project_root, DiskGrants, DiskSpec, DynamicRunner, PersistScope, ReloadReceipt, ReloadStatus,
    Surface,
};
use crate::tools::registry::{exec_ctx, own_registered, tool_result, ToolBody, Tools};
use cordis_base::config::dock_home;
use cordis_base::types::{LogEvent, PreStep, ToolCall, ToolResult, ToolSpec};

use inspect::{render_inspect, render_overview, render_plugin};

type ExecFut = Pin<Box<dyn Future<Output = ToolResult> + Send + 'static>>;

pub(crate) const INSPECT: &str = "cordis_inspect";
pub(crate) const PLUGIN: &str = "cordis_plugin";

const INSPECT_DESC: &str = "Read-only view of the plugin host; never runs plugin code. No arguments: this session's plugin directories and every plugin with its status (running, failed and why, disabled). `plugin`: one plugin — directory, enabled, what its running version registered (tools, slash commands, slots, hooks), declared HTTP hosts, last error. `what`: one reference section — services (what a Rhai host can reach), tools (live tool names), builtins (Rhai host API signatures), events (hook names and payloads), slots (panels). Load the cordis-plugin-development skill before writing a plugin.";
const INSPECT_PARAMS: &str = r#"{"type":"object","properties":{"plugin":{"type":"string","description":"Plugin id (its directory name under .dock/plugins/)."},"what":{"type":"string","enum":["plugins","services","tools","builtins","events","slots"],"description":"One reference section. Ignored when plugin is given."}}}"#;

const PLUGIN_DESC: &str = "Load, hot-reload, or disable a plugin. A plugin is a directory: .dock/plugins/<id>/ in the project (scope \"user\": ~/.dock/plugins/<id>/) holding source.rhai and an optional plugin.toml. Write and edit those files with the file tools, then action \"reload\" compiles them as they are now and starts them, replacing the running version; if the new version fails, the previous one keeps running and the error (with line and position) comes back. Reload also sets enabled = true. Action \"disable\" stops the plugin and writes enabled = false; files stay. Asks the user like bash; the request lists the plugin's hooks and declared HTTP hosts, and choosing always covers later reloads of that plugin until it adds a hook or an HTTP host. Changes reach every session and survive restart. After reload, call the tools it registered through use_tool to verify. Load the cordis-plugin-development skill before writing a plugin.";
const PLUGIN_PARAMS: &str = r#"{"type":"object","required":["action","id"],"properties":{"action":{"type":"string","enum":["reload","disable"]},"id":{"type":"string","description":"Plugin id = directory name: 2–32 chars, lowercase letter first, then a-z 0-9 -."},"scope":{"type":"string","enum":["project","user"],"description":"Which plugins root. Default: project if .dock/plugins/<id>/ exists there, else user."}}}"#;

/// The registered surface, for the text guard below.
#[cfg(test)]
const TOOL_NAMES: [&str; 2] = [INSPECT, PLUGIN];

pub fn tool_cordis() -> Plugin {
    plugin(
        "tool-cordis",
        Inject::from([TOOLS, DYNAMIC_CORDIS_RUNNER]),
        |ctx, _: &()| {
            let ctx_pre = ctx.clone();
            let _ = ctx.on_waterfall(PRE_STEP, move |step: PreStep, args| {
                let next = args.next::<PreStep>().unwrap_or(step);
                // Main session only: a `@pluginId` reminder has to land in the
                // session that mentioned it.
                if next.enter && next.is_main_session() {
                    let page = exec_ctx().unwrap_or_else(|| ctx_pre.clone());
                    inject_plugin_mentions(&ctx_pre, &page, &next.user);
                }
                next
            });
            let tools = ctx.require::<Tools>(TOOLS)?;
            own_registered(
                ctx,
                vec![
                    spec(
                        ctx,
                        tools.as_ref(),
                        INSPECT,
                        INSPECT_DESC,
                        INSPECT_PARAMS,
                        inspect_tool,
                    )?,
                    spec(
                        ctx,
                        tools.as_ref(),
                        PLUGIN,
                        PLUGIN_DESC,
                        PLUGIN_PARAMS,
                        plugin_tool,
                    )?,
                ],
            )?;
            Ok(None)
        },
    )
}

fn spec(
    ctx: &Context,
    tools: &Tools,
    name: &str,
    description: &str,
    params: &str,
    exec: fn(Context, ToolCall) -> ExecFut,
) -> cordis::Result<cordis::Disposable> {
    let ctx = ctx.clone();
    let body: ToolBody = Arc::new(move |call| {
        let ctx = ctx.clone();
        Box::pin(async move { exec(ctx, call).await })
    });
    tools.register(
        ToolSpec {
            name: name.into(),
            description: description.into(),
            parameters_json: params.into(),
        },
        body,
    )
}

/// 发起这次调用的那一页：项目目录、会话身份都按它算。插件 fiber 和这两颗工具都
/// 挂在根上，拿根算的话 GUI 里每个会话都会被当成第 1 页。
fn page(ctx: &Context) -> Context {
    exec_ctx().unwrap_or_else(|| ctx.clone())
}

/// 这一页能看到的两个插件根：用户级在前、项目级在后（后者覆盖前者）。
pub(crate) fn roots_for(page: &Context) -> Vec<(PersistScope, PathBuf)> {
    vec![
        (PersistScope::User, dock_home().join("plugins")),
        (
            PersistScope::Project,
            project_root(&crate::session::cwd::session_cwd(page)),
        ),
    ]
}

fn inspect_tool(ctx: Context, call: ToolCall) -> ExecFut {
    Box::pin(async move {
        let v = json(&call);
        let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
            return tool_result(call, "错误：动态插件服务没有挂上");
        };
        let page = page(&ctx);
        let roots = roots_for(&page);
        if let Some(id) = text_arg(&v, "plugin").or_else(|| text_arg(&v, "pluginId")) {
            return match render_plugin(&runner, &roots, &id) {
                Ok(text) => tool_result(call, text),
                Err(e) => error_result(call, e),
            };
        }
        let Some(tools) = ctx.get::<Tools>(TOOLS) else {
            return error_result(call, "工具表没有挂上");
        };
        let text = match text_arg(&v, "what") {
            None => render_overview(&runner, &roots),
            Some(what) => render_inspect(&ctx, &runner, &tools, &roots, &what),
        };
        tool_result(call, text)
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Reload,
    Disable,
}

/// 宽松地认动作名：模型常写 enable / load / stop，意思都明白，就别为一个同义词
/// 再让它重来一轮。
fn parse_action(raw: &str) -> Result<Action, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "reload" | "load" | "enable" | "start" | "run" | "update" => Ok(Action::Reload),
        "disable" | "stop" | "unload" => Ok(Action::Disable),
        "" => Err("action 是必填的：\"reload\" 或 \"disable\"".into()),
        other => Err(format!(
            "action 只能是 \"reload\" 或 \"disable\"，收到 {other:?}"
        )),
    }
}

fn parse_scope(v: &Value) -> Result<Option<PersistScope>, String> {
    match text_arg(v, "scope").as_deref() {
        None => Ok(None),
        Some("project") => Ok(Some(PersistScope::Project)),
        Some("user") => Ok(Some(PersistScope::User)),
        Some(other) => Err(format!(
            "scope 只能是 \"project\" 或 \"user\"，收到 {other:?}"
        )),
    }
}

fn plugin_tool(ctx: Context, call: ToolCall) -> ExecFut {
    Box::pin(async move {
        let v = json(&call);
        let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
            return error_result(call, "动态插件服务没有挂上");
        };
        let action = match parse_action(&text_arg(&v, "action").unwrap_or_default()) {
            Ok(a) => a,
            Err(e) => return error_result(call, e),
        };
        let Some(id) = text_arg(&v, "id").or_else(|| text_arg(&v, "pluginId")) else {
            return error_result(
                call,
                "id 是必填的：插件目录名，比如 .dock/plugins/memo/ 的 memo",
            );
        };
        let scope = match parse_scope(&v) {
            Ok(s) => s,
            Err(e) => return error_result(call, e),
        };
        let page = page(&ctx);
        let roots = roots_for(&page);
        let (spec, shadowed) = match crate::tools::dynamic_runner::find_disk(&id, scope, &roots) {
            Ok(found) => found,
            Err(e) => return error_result(call, e),
        };
        match action {
            Action::Disable => match runner.disable_disk(&spec).await {
                Ok(r) => tool_result(
                    call,
                    format!(
                        "已停用插件 {}（{}）{}。plugin.toml 已写 enabled = false，文件都还在；要再用就 cordis_plugin reload {}。",
                        r.id,
                        r.dir.display(),
                        if r.was_running { "，它登记的工具 / 命令 / 面板已经撤下" } else { "（本来就没在跑）" },
                        r.id
                    ),
                ),
                Err(e) => error_result(call, e),
            },
            Action::Reload => match runner.reload_disk(&spec).await {
                Ok(r) => {
                    let failed = r.status == ReloadStatus::Failed;
                    let text = render_reload(&spec, &r, shadowed.as_deref());
                    if failed {
                        ToolResult {
                            is_error: true,
                            ..tool_result(call, text)
                        }
                    } else {
                        tool_result(call, text)
                    }
                }
                Err(e) => error_result(call, e),
            },
        }
    })
}

/// reload 回执：状态、登记了什么、下一步。给模型的「下一步」只有一条，按结果挑。
fn render_reload(spec: &DiskSpec, r: &ReloadReceipt, shadowed: Option<&std::path::Path>) -> String {
    let mut lines = Vec::new();
    let place = format!("{}级 {}", scope_label(r.scope), r.dir.display());
    match r.status {
        ReloadStatus::Failed => {
            lines.push(format!("插件 {} 这一版没跑起来（{place}）：", r.id));
            lines.push(r.error.clone().unwrap_or_else(|| "原因不明".into()));
            match &r.kept_previous {
                Some(prev) => lines.push(format!(
                    "上一版 {prev} 仍在运行（已自动回滚），用户手上的功能没断。"
                )),
                None => lines.push("现在没有在跑的版本。".into()),
            }
        }
        ReloadStatus::Running | ReloadStatus::Waiting => {
            lines.push(format!(
                "已重载插件 {}（{place}），{}。",
                r.id,
                status_label(r.status)
            ));
            if r.enabled_now {
                lines.push(
                    "plugin.toml 原来是 enabled = false，已改成 true（以后启动会自动装上）。"
                        .into(),
                );
            }
        }
    }
    if r.status != ReloadStatus::Failed || r.kept_previous.is_some() {
        lines.push(format!("登记：{}", surface_line(&r.surface)));
    }
    if !r.http_allow.is_empty() {
        lines.push(format!("HTTP 免逐次确认：{}", r.http_allow.join("、")));
    }
    if let Some(other) = shadowed {
        lines.push(format!(
            "注意：同名的另一份 {} 被这份盖住了，不会加载。",
            other.display()
        ));
    }
    for w in &r.warnings {
        lines.push(format!("注意：{w}"));
    }
    lines.push(format!("下一步：{}", next_step(spec, r)));
    lines.join("\n")
}

fn next_step(spec: &DiskSpec, r: &ReloadReceipt) -> String {
    let source = spec
        .source_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| r.dir.join("plugin.toml").display().to_string());
    match r.status {
        ReloadStatus::Failed => {
            let error = r.error.as_deref().unwrap_or("");
            if error.contains("Function not found") || error.contains("Syntax error") || error.contains("rhai syntax") {
                format!(
                    "按报错的行号改 {source}，再 cordis_plugin reload {}。Rhai 不是 JS：报 Function not found 先读技能里的 references/rhai-pitfalls.md。别另起一个插件目录。",
                    r.id
                )
            } else {
                format!(
                    "改 {source} 后再 cordis_plugin reload {}；要看完整状态用 cordis_inspect plugin \"{}\"。别另起一个插件目录。",
                    r.id, r.id
                )
            }
        }
        ReloadStatus::Waiting => format!(
            "它在等这些服务挂上：{}。inject 里写了没有的名字就删掉再 reload（cordis_inspect what \"services\" 看哪些够得着）。",
            r.waiting_for.join("、")
        ),
        ReloadStatus::Running => match r.surface.tools.first() {
            Some(tool) => format!(
                "用 use_tool 试调刚登记的工具，例如 use_tool {{\"tool_name\":\"{tool}\",\"tool_input\":{{}}}}，看结果对不对。改了文件就再 reload 一次。"
            ),
            None if !r.surface.hooks.is_empty() => format!(
                "触发一次它挂的钩子（{}）看效果，比如直接调被拦的那个工具。改了文件就再 reload 一次。",
                r.surface.hooks.join("、")
            ),
            None if !r.surface.slash.is_empty() || !r.surface.slots.is_empty() => {
                "请用户打开它登记的斜杠命令 / 面板看一眼；你这边用 cordis_inspect 确认登记无误。".into()
            }
            None => "它没有登记任何东西——apply 里是不是漏了 host.register_tool / register_slash / on？".into(),
        },
    }
}

fn status_label(status: ReloadStatus) -> &'static str {
    match status {
        ReloadStatus::Running => "运行中",
        ReloadStatus::Waiting => "已启动、在等依赖",
        ReloadStatus::Failed => "失败",
    }
}

fn scope_label(scope: PersistScope) -> &'static str {
    match scope {
        PersistScope::Project => "项目",
        PersistScope::User => "用户",
    }
}

pub(crate) fn surface_line(s: &Surface) -> String {
    if s.is_empty() {
        return "（没有）".into();
    }
    let mut parts = Vec::new();
    let mut push = |label: &str, items: &[String], prefix: &str| {
        if !items.is_empty() {
            let items: Vec<String> = items.iter().map(|i| format!("{prefix}{i}")).collect();
            parts.push(format!("{label} {}", items.join("、")));
        }
    };
    push("工具", &s.tools, "");
    push("斜杠", &s.slash, "/");
    push("面板", &s.slots, "");
    push("钩子", &s.hooks, "");
    push("服务", &s.provides, "");
    if s.settings {
        parts.push("设置卡".into());
    }
    parts.join("；")
}

/// `cordis_plugin` 的权限请求：按插件记「始终允许」，迭代同一个插件不用每改一次点
/// 一次，换一个插件还会再问。摘要把这个插件挂的钩子和声明的 HTTP host 列出来——批准
/// 加载，批的就是这些；所以 reload 的键是 `cordis_plugin <id>` 再带上它们的指纹
/// （[`DiskGrants::fingerprint`]），多挂一个钩子、多声明一个 host 就换了键、要再问。
/// 两样都没有的（和停用）就是 `cordis_plugin <id>`。
pub(crate) fn permission_request(exec: &Context, arguments: &str) -> (String, String) {
    let v: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let id = text_arg(&v, "id")
        .or_else(|| text_arg(&v, "pluginId"))
        .unwrap_or_default();
    let mut key = format!("{PLUGIN} {id}");
    let action = parse_action(&text_arg(&v, "action").unwrap_or_default()).ok();
    let scope = parse_scope(&v).ok().flatten();
    let found = crate::tools::dynamic_runner::find_disk(&id, scope, &roots_for(exec)).ok();
    let mut summary = match (action, found.as_ref()) {
        (Some(Action::Disable), Some((spec, _))) => {
            format!("停用插件 {id}（{}）", spec.path.display())
        }
        (Some(Action::Disable), None) => format!("停用插件 {id}"),
        (_, Some((spec, _))) => format!(
            "加载插件 {id}（{}级 {}）：以宿主权限运行它的代码，对所有会话生效",
            scope_label(spec.scope),
            spec.path.display()
        ),
        (_, None) => format!("加载插件 {id}（目录还不存在）"),
    };
    if let (Some(Action::Reload), Some((spec, _))) = (action, found.as_ref()) {
        let grants = DiskGrants::of(spec);
        summary.push_str(&grants.describe());
        if let Some(fp) = grants.fingerprint() {
            key = format!("{key} #{fp}");
        }
    }
    (key, summary)
}

fn json(call: &ToolCall) -> Value {
    serde_json::from_str(&call.arguments).unwrap_or(Value::Null)
}

fn text_arg(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn error_result(call: ToolCall, message: impl Into<String>) -> ToolResult {
    ToolResult {
        is_error: true,
        ..tool_result(call, format!("错误：{}", message.into()))
    }
}

fn inject_plugin_mentions(ctx: &Context, page: &Context, user: &str) {
    let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) else {
        return;
    };
    let Some(sessions) = page.get::<Sessions>(SESSIONS) else {
        return;
    };
    let roots = roots_for(page);
    for id in mentioned_plugin_ids(user) {
        let Ok((spec, _)) = crate::tools::dynamic_runner::find_disk(&id, None, &roots) else {
            continue;
        };
        let running = runner
            .disk_plugins_in(&roots)
            .into_iter()
            .any(|v| v.id == spec.id && v.running && v.path == spec.path);
        let text = format!(
            "# @{id}\n插件目录：{}（{}级）\n名称：{}\n用途：{}\n状态：{}\n改它就改这个目录里的文件，然后 cordis_plugin reload {id}；cordis_inspect plugin \"{id}\" 看它登记了什么、最近的错误。不要另起一个插件目录。",
            spec.path.display(),
            scope_label(spec.scope),
            spec.name,
            if spec.purpose.is_empty() { "（未写）" } else { spec.purpose.as_str() },
            if running { "运行中" } else if spec.enabled { "未在运行" } else { "已停用" },
        );
        sessions.append(LogEvent::SystemReminder(text));
    }
}

fn mentioned_plugin_ids(user: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let bytes = user.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            let start = i + 1;
            if start < bytes.len() && bytes[start].is_ascii_lowercase() {
                let mut j = start;
                while j < bytes.len()
                    && j - start < 32
                    && (bytes[j].is_ascii_lowercase()
                        || bytes[j].is_ascii_digit()
                        || bytes[j] == b'-')
                {
                    j += 1;
                }
                let len = j - start;
                if (2..=32).contains(&len) && (j == bytes.len() || bytes[j].is_ascii_whitespace()) {
                    let id = user[start..j].to_string();
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Model-facing text may only name tools that exist. Descriptions, receipts
    /// and the bundled skill all outlive the tool table they were written
    /// against: `cordis_undefine` survived in four places after it was folded
    /// away, and the model was told to call something that answered "is not
    /// registered".
    #[test]
    fn every_cordis_name_in_model_text_is_a_live_tool() {
        let model_text: &[(&str, &str)] = &[
            ("INSPECT_DESC", INSPECT_DESC),
            ("INSPECT_PARAMS", INSPECT_PARAMS),
            ("PLUGIN_DESC", PLUGIN_DESC),
            ("PLUGIN_PARAMS", PLUGIN_PARAMS),
            (
                "INLINE_TOO_LARGE",
                crate::tools::dynamic_runner::rhai_host_inline_too_large(),
            ),
        ];
        // Retired verbs. Prefixed-name matching alone would wave through a bare
        // "promote it" or "define it", so the bare forms are checked too.
        const RETIRED: &[(&str, &str)] = &[
            ("cordis_define", "write files + cordis_plugin reload"),
            ("cordis_run", "cordis_plugin reload"),
            ("cordis_call", "use_tool"),
            ("cordis_stop", "cordis_plugin disable"),
            (
                "cordis_promote",
                "files under .dock/plugins are already permanent",
            ),
            ("undefine", "cordis_plugin disable"),
            ("inspect_self", "cordis_inspect with plugin"),
            ("source_path", "the plugin directory"),
            ("idPrefix", "the directory name"),
        ];
        let check = |label: &str, text: &str| {
            for name in cordis_names(text) {
                assert!(
                    TOOL_NAMES.contains(&name.as_str()),
                    "{label} names {name:?}, which is not a registered tool: {TOOL_NAMES:?}"
                );
            }
            for (gone, now) in RETIRED {
                assert!(
                    !text.contains(gone),
                    "{label} still says {gone:?}; that is now {now}"
                );
            }
        };
        for (label, text) in model_text {
            check(label, text);
        }
        // The bundled skill is the authoring contract (tool descriptions point
        // at it). References and examples get the same two checks.
        let skill_files: Vec<_> = crate::tools::skills::builtin::BUILTIN_FILES
            .iter()
            .filter(|(rel, _)| rel.starts_with("cordis-plugin-development/"))
            .collect();
        assert!(
            skill_files
                .iter()
                .any(|(rel, _)| rel.ends_with("/SKILL.md")),
            "cordis-plugin-development is not a bundled skill"
        );
        for (rel, text) in skill_files {
            check(rel, text);
        }
    }

    /// `cordis_` followed by the identifier characters a tool name can use.
    fn cordis_names(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(at) = rest.find("cordis_") {
            let tail = &rest[at..];
            let end = tail
                .find(|c: char| !c.is_ascii_lowercase() && c != '_')
                .unwrap_or(tail.len());
            out.push(tail[..end].to_string());
            rest = &tail[end..];
        }
        out
    }

    #[test]
    fn action_names_are_forgiving_but_not_anything_goes() {
        assert_eq!(parse_action("reload").unwrap(), Action::Reload);
        assert_eq!(parse_action(" Enable ").unwrap(), Action::Reload);
        assert_eq!(parse_action("stop").unwrap(), Action::Disable);
        assert!(parse_action("delete").is_err());
        assert!(parse_action("").is_err());
    }

    /// 「始终允许」按「加载它批了什么」记：只改代码照旧免问；多挂一个钩子、多声明一个
    /// HTTP host（之后对它免逐次确认）就是扩大权限，换一个键、再问一次。以前键只有
    /// `cordis_plugin <id>`，点过一次以后加什么都不再问。
    #[tokio::test]
    async fn always_allow_does_not_cover_new_hooks_or_http_hosts() {
        let project = tempfile::tempdir().unwrap();
        let ctx = Context::new();
        let sessions = Sessions::tab(ctx.clone(), 2);
        sessions.pin_workspace_cwd(project.path());
        let _reg = ctx.provide(SESSIONS, sessions).unwrap();
        let dir = project.path().join(".dock").join("plugins").join("memo");
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
        let key =
            || permission_request(&ctx, r#"{"action":"reload","id":"memo","scope":"project"}"#).0;

        write("source.rhai", "#{ inject: [], apply: |host| { } }");
        let plain = key();
        assert_eq!(plain, "cordis_plugin memo");
        write(
            "source.rhai",
            "#{ inject: [], apply: |host| { host.set_status(\"x\"); } }",
        );
        assert_eq!(key(), plain, "只改代码不扩大权限，迭代不用每次点");

        write(
            "plugin.toml",
            "[permissions]\nhttp = [\"api.example.com\"]\n",
        );
        let with_http = key();
        assert_ne!(with_http, plain, "新声明的 HTTP host 要再问一次");
        assert!(with_http.starts_with("cordis_plugin memo "), "{with_http}");

        write(
            "source.rhai",
            "#{ inject: [], apply: |host| { host.on(\"tools/pre-execute\", |c| { () }); } }",
        );
        let with_hook = key();
        assert_ne!(with_hook, with_http, "新挂的流程钩子要再问一次");
        write(
            "source.rhai",
            "#{ inject: [], apply: |host| {\n  host.on(\"tools/pre-execute\", |c| { () });\n} }",
        );
        assert_eq!(key(), with_hook, "钩子没变，只是排版");

        let (disable, _) = permission_request(
            &ctx,
            r#"{"action":"disable","id":"memo","scope":"project"}"#,
        );
        assert_eq!(disable, "cordis_plugin memo", "停用不扩大权限");
    }

    #[test]
    fn surface_line_names_each_kind() {
        let s = Surface {
            tools: vec!["fofa_info".into()],
            slash: vec!["cyber".into()],
            hooks: vec!["tools/pre-execute".into()],
            settings: true,
            ..Surface::default()
        };
        assert_eq!(
            surface_line(&s),
            "工具 fofa_info；斜杠 /cyber；钩子 tools/pre-execute；设置卡"
        );
        assert_eq!(surface_line(&Surface::default()), "（没有）");
    }
}
