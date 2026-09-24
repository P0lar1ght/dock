//! Slash catalog + execution for the web client.
//!
//! Builtin names come from `cordis_tui::slash_catalog()` (the pager dropdown).
//! `slash/list` is autocomplete; `slash/execute` talks to spine services.
//! Capture commands (`/screenshot`) are listed here so embed does not keep a
//! parallel harness catalog; the actual pixels stay in JS. `/cd` and `/settings`
//! (including `/settings timestamps`) are listed but refused: list `terminal`
//! never mutates process state. Use `/timestamps` `/think` `/model` `/protocol`
//! `/effort`.

use serde_json::{json, Value};

use cordis_spine::{
    apply_restored_preset, extra_tool_slash_arguments, goal_composer_fill, loop_composer_fill,
    loop_schedule_instruction, session_usage_block_text, workflow_command_arguments, AgentPresets,
    ApiBackend, AppSettings, ApplyRestoredPreset, ExtraSlashKind, Goal, LoopFireMode, PlanMode,
    Sessions, Slash, SlashEntry, ToolCall, Tools, AGENT_PRESETS, GOAL, GOAL_RESERVED_SUBCOMMANDS,
    PLAN_MODE, SESSIONS, SETTINGS, SLASH, TOOLS, WORKFLOW_TOOL_NAME,
};
use cordis_tui::{resolve_slash, slash_catalog, SessionRef, SlashCatalogEntry, SESSION_PORT};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;
use crate::threads;

#[derive(Clone, Copy)]
struct Item {
    name: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    takes_args: bool,
    /// `gateway` = this process. `terminal` = TUI overlay. `embed` = host capture.
    surface: &'static str,
    capture: Option<&'static str>,
}

/// Harness-facing builtins. List `surface` and execute both go through
/// `gateway_cmd`: omitted TUI names are fail-closed to terminal (`/settings`
/// with args, `/cd`, overlays, `/quit`). Adding a variant without an execute
/// arm is a compile error.
#[derive(Clone, Copy)]
enum GatewayCmd {
    New,
    Resume,
    Model,
    Protocol,
    Loop,
    Plan,
    ViewPlan,
    Goal,
    Compact,
    Effort,
    Think,
    Help,
    Usage,
    Context,
    Workflow,
    Timestamps,
}

fn gateway_cmd(name: &str) -> Option<GatewayCmd> {
    Some(match name {
        "new" => GatewayCmd::New,
        "resume" => GatewayCmd::Resume,
        "model" => GatewayCmd::Model,
        "protocol" => GatewayCmd::Protocol,
        "loop" => GatewayCmd::Loop,
        "plan" => GatewayCmd::Plan,
        "view-plan" => GatewayCmd::ViewPlan,
        "goal" => GatewayCmd::Goal,
        "compact" => GatewayCmd::Compact,
        "effort" => GatewayCmd::Effort,
        "think" => GatewayCmd::Think,
        "help" => GatewayCmd::Help,
        "usage" => GatewayCmd::Usage,
        "context" => GatewayCmd::Context,
        "workflow" => GatewayCmd::Workflow,
        "timestamps" => GatewayCmd::Timestamps,
        _ => return None,
    })
}

fn surface_for(name: &str) -> &'static str {
    if gateway_cmd(name).is_some() {
        "gateway"
    } else {
        "terminal"
    }
}

const GOAL_HINTS: &[Item] = &[
    Item {
        name: "goal pause",
        aliases: &[],
        description: "暂停自动推进",
        takes_args: false,
        surface: "gateway",
        capture: None,
    },
    Item {
        name: "goal resume",
        aliases: &[],
        description: "恢复当前目标",
        takes_args: false,
        surface: "gateway",
        capture: None,
    },
    Item {
        name: "goal clear",
        aliases: &[],
        description: "清除当前目标",
        takes_args: false,
        surface: "gateway",
        capture: None,
    },
    Item {
        name: "goal edit",
        aliases: &[],
        description: "在面板里修改当前目标",
        takes_args: true,
        surface: "gateway",
        capture: None,
    },
];

const CAPTURE: &[Item] = &[
    Item {
        name: "screenshot",
        aliases: &[],
        description: "截取当前 viewport；参数是发给模型的问题。",
        takes_args: true,
        surface: "embed",
        capture: Some("viewport"),
    },
    Item {
        name: "screenshot --region",
        aliases: &[],
        description: "拖动选择一个区域，只把该区域发给本轮模型。",
        takes_args: true,
        surface: "embed",
        capture: Some("region"),
    },
    Item {
        name: "screenshot --reuse",
        aliases: &[],
        description: "不重新截图，复用当前页面最近一次图片。",
        takes_args: true,
        surface: "embed",
        capture: Some("reuse"),
    },
    Item {
        name: "screenshot --screen",
        aliases: &[],
        description: "打开浏览器共享选择器并捕获一帧。",
        takes_args: true,
        surface: "embed",
        capture: Some("screen"),
    },
    Item {
        name: "screenshot --full-page",
        aliases: &[],
        description: "分段截取当前页面的完整纵向内容。",
        takes_args: true,
        surface: "embed",
        capture: Some("full-page"),
    },
];

pub fn list(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let mut commands = Vec::new();
    for entry in slash_catalog() {
        commands.push(catalog_json(&entry));
    }
    for item in GOAL_HINTS.iter().chain(CAPTURE) {
        commands.push(item_json(item));
    }
    if let Some(slash) = gateway.ctx().get::<Slash>(SLASH) {
        for entry in slash.list() {
            commands.push(extra_json(&entry));
        }
    }
    Ok(json!({ "commands": commands }))
}

pub async fn execute(gateway: GatewayHandle, params: Value) -> Result<Value, RpcError> {
    // 斜杠命令作用在 `threadId` 那一页：下面整串 `cmd_*` 读的都是 `gateway.ctx()`。
    let page = threads::resolve_param(&gateway, &params)?;
    let gateway = gateway.scoped(page);
    let text = params
        .get("text")
        .or_else(|| params.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if text.trim().is_empty() {
        return Err(RpcError::invalid_params("text is required"));
    }
    match parse_slash_line(&text) {
        None => Ok(passthrough()),
        Some((name, args)) => dispatch_named(&gateway, &name, &args).await,
    }
}

async fn dispatch_named(
    gateway: &GatewayHandle,
    name: &str,
    args: &str,
) -> Result<Value, RpcError> {
    if name == "screenshot" {
        return Ok(json!({ "ok": true, "kind": "capture" }));
    }
    if let Some(entry) = resolve_slash(name) {
        return execute_builtin(gateway, entry.name, args).await;
    }
    if let Some(slash) = gateway.ctx().get::<Slash>(SLASH) {
        if let Some(entry) = slash.list().into_iter().find(|e| e.command == name) {
            return execute_extra(gateway, &entry, args).await;
        }
    }
    Ok(passthrough())
}

async fn execute_builtin(
    gateway: &GatewayHandle,
    name: &str,
    args: &str,
) -> Result<Value, RpcError> {
    let Some(cmd) = gateway_cmd(name) else {
        return Ok(terminal_only(name));
    };
    let args = args.trim();
    match cmd {
        GatewayCmd::New => cmd_new(gateway),
        GatewayCmd::Resume => cmd_resume(gateway),
        GatewayCmd::Model => cmd_model(gateway, args),
        GatewayCmd::Protocol => cmd_protocol(gateway, args),
        GatewayCmd::Loop => cmd_loop(gateway, args),
        GatewayCmd::Plan => cmd_plan(gateway, args),
        GatewayCmd::ViewPlan => cmd_view_plan(gateway),
        GatewayCmd::Goal => cmd_goal(gateway, args),
        GatewayCmd::Compact => cmd_compact(gateway, args),
        GatewayCmd::Effort => cmd_effort(gateway, args),
        GatewayCmd::Think => cmd_think(gateway),
        GatewayCmd::Help => Ok(notice("斜杠命令", help_body(gateway))),
        GatewayCmd::Usage => cmd_usage(gateway),
        GatewayCmd::Context => Ok(menu("context")),
        GatewayCmd::Workflow => cmd_workflow(gateway, args).await,
        GatewayCmd::Timestamps => cmd_timestamps(gateway),
    }
}

async fn execute_extra(
    gateway: &GatewayHandle,
    entry: &SlashEntry,
    args: &str,
) -> Result<Value, RpcError> {
    match entry.kind {
        ExtraSlashKind::Prompt => {
            let text = entry.expand(args);
            if entry.send {
                submit_text(gateway, text)
            } else {
                Ok(filled(text))
            }
        }
        ExtraSlashKind::Overlay => Ok(notice(entry.overlay_title(), entry.expand(args))),
        ExtraSlashKind::Slot => Ok(notice(
            "终端插槽",
            format!("{} 请在 Dock 终端打开。", entry.display()),
        )),
        ExtraSlashKind::Tool => {
            let Some(tools) = gateway.ctx().get::<Tools>(TOOLS) else {
                return Ok(notice(entry.tool_title(), "tools 未挂载".to_string()));
            };
            let arguments = match extra_tool_slash_arguments(entry, args) {
                Ok(arguments) => arguments,
                Err(body) => return Ok(notice(entry.tool_title(), body)),
            };
            let result = tools
                .execute(ToolCall {
                    id: format!("slash-tool-{}", entry.command),
                    name: entry.text.trim().to_string(),
                    arguments,
                })
                .await;
            Ok(notice(entry.tool_title(), result.content))
        }
    }
}

fn cmd_new(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let sessions = sessions(gateway)?;
    sessions.archive_current();
    sessions.clear();
    cordis_spine::clear_plan_for_session_switch(gateway.ctx());
    if let Some(goal) = gateway.ctx().get::<Goal>(GOAL) {
        goal.clear();
    }
    gateway.reset_transcript();
    Ok(applied("已开始新会话"))
}

fn cmd_resume(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let sessions = sessions(gateway)?;
    let Some(item) = sessions.archived().into_iter().next() else {
        return Ok(notice("恢复会话", "没有可恢复的会话。"));
    };
    let stamped = item.preset_id.clone();
    sessions.archive_current();
    if !sessions.restore(&item.id) {
        return Ok(notice("恢复会话", "没有可恢复的会话。"));
    }
    cordis_spine::clear_plan_for_session_switch(gateway.ctx());
    if let Some(presets) = gateway.ctx().get::<AgentPresets>(AGENT_PRESETS) {
        if let ApplyRestoredPreset::Failed { id, error } =
            apply_restored_preset(&presets, stamped.as_deref())
        {
            eprintln!("dock: /resume preset `{id}` not applied: {error}");
        }
    }
    gateway.reset_transcript();
    Ok(applied("已恢复上次会话"))
}

fn cmd_model(gateway: &GatewayHandle, args: &str) -> Result<Value, RpcError> {
    if args.is_empty() {
        return Ok(menu("model"));
    }
    let settings = settings(gateway)?;
    settings.set_model(args);
    Ok(applied(format!("已切换 {args}")))
}

/// `/protocol` —— 在当前模型 `api_backends` 声明过的协议之间切。
///
/// 空参数回一条 notice 列可选项，不新开一个 menu id：`menu` 的取值是 dock.1
/// 的一部分，为这个功能扩协议不值得，宿主页也不需要为它加渲染。
fn cmd_protocol(gateway: &GatewayHandle, args: &str) -> Result<Value, RpcError> {
    let settings = settings(gateway)?;
    let choices = settings.backend_choices();
    if args.is_empty() {
        let current = settings.backend();
        let body = choices
            .iter()
            .map(|b| {
                let mark = if *b == current { " ·当前" } else { "" };
                format!("/protocol {}{mark} — {}", b.name(), b.description())
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(notice("推理协议", body));
    }
    let Some(backend) = ApiBackend::from_name(args) else {
        return Ok(applied(format!("未知协议 {args}")));
    };
    if !choices.contains(&backend) {
        return Ok(applied(format!(
            "{} 未在该模型的 api_backends 里声明",
            backend.name()
        )));
    }
    settings.set_backend(backend);
    Ok(applied(format!("协议 {}", backend.name())))
}

fn cmd_loop(gateway: &GatewayHandle, args: &str) -> Result<Value, RpcError> {
    if args.is_empty() {
        return Ok(filled(loop_composer_fill()));
    }
    let visible = format!("/loop {args}");
    if let Some(sessions) = gateway.ctx().get::<Sessions>(SESSIONS) {
        sessions.arm_user_addon(
            visible.clone(),
            loop_schedule_instruction(args, LoopFireMode::InSession),
        );
    }
    submit_text(gateway, visible)
}

fn cmd_plan(gateway: &GatewayHandle, args: &str) -> Result<Value, RpcError> {
    let Some(plan) = gateway.ctx().get::<PlanMode>(PLAN_MODE) else {
        return Ok(notice("计划模式", "计划模式未挂载。"));
    };
    if args.is_empty() {
        plan.enter_pending();
        return Ok(applied("计划模式"));
    }
    plan.enter_active();
    submit_text(gateway, args.to_string())
}

fn cmd_view_plan(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let Some(plan) = gateway.ctx().get::<PlanMode>(PLAN_MODE) else {
        return Ok(notice("计划", "计划模式未挂载。"));
    };
    if let Some(prompt) = plan.front().or_else(|| plan.disk_preview()) {
        let body = if prompt.empty {
            "计划文件是空的。".to_string()
        } else {
            prompt.body
        };
        return Ok(notice("当前计划", body));
    }
    Ok(notice("计划", "没有可查看的计划。"))
}

fn cmd_goal(gateway: &GatewayHandle, args: &str) -> Result<Value, RpcError> {
    if args.is_empty() {
        if let Some(goal) = gateway.ctx().get::<Goal>(GOAL) {
            goal.arm_composer();
        }
        return Ok(filled(goal_composer_fill()));
    }
    if args == "<目标>" {
        return Ok(filled(goal_composer_fill()));
    }
    let first = args.split_whitespace().next().unwrap_or("");
    match first {
        "status" => Ok(menu("goal")),
        "edit" => Ok(menu("goal")),
        "pause" => {
            let Some(goal) = gateway.ctx().get::<Goal>(GOAL) else {
                return Ok(notice("目标", "目标服务未挂载。"));
            };
            goal.disarm_composer();
            if goal.pause() {
                Ok(applied("目标已暂停"))
            } else {
                Ok(notice("目标", "没有进行中的目标。"))
            }
        }
        "resume" => {
            let Some(goal) = gateway.ctx().get::<Goal>(GOAL) else {
                return Ok(notice("目标", "目标服务未挂载。"));
            };
            goal.disarm_composer();
            if goal.resume() {
                Ok(applied("目标已继续"))
            } else {
                Ok(notice("目标", "没有已暂停的目标。"))
            }
        }
        "clear" => {
            let Some(goal) = gateway.ctx().get::<Goal>(GOAL) else {
                return Ok(notice("目标", "目标服务未挂载。"));
            };
            if goal.present() {
                goal.clear();
                Ok(applied("已清除目标"))
            } else {
                goal.disarm_composer();
                Ok(notice("目标", "没有活动目标。"))
            }
        }
        _ => {
            if GOAL_RESERVED_SUBCOMMANDS.contains(&first) {
                return Ok(menu("goal"));
            }
            let Some(goal) = gateway.ctx().get::<Goal>(GOAL) else {
                return Ok(notice("目标", "目标服务未挂载。"));
            };
            goal.disarm_composer();
            goal.start(args);
            submit_text(gateway, args.to_string())
        }
    }
}

fn cmd_compact(gateway: &GatewayHandle, args: &str) -> Result<Value, RpcError> {
    let port = session_port(gateway)?;
    port.compact(args.to_string());
    Ok(applied("正在压缩上下文…"))
}

fn cmd_effort(gateway: &GatewayHandle, args: &str) -> Result<Value, RpcError> {
    if args.is_empty() {
        return Ok(menu("reasoning"));
    }
    let settings = settings(gateway)?;
    settings.set_effort(args);
    Ok(applied(format!("推理强度 {args}")))
}

fn cmd_think(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let settings = settings(gateway)?;
    let on = settings.toggle_thinking();
    Ok(applied(if on {
        "思考模式已开"
    } else {
        "思考模式已关"
    }))
}

fn cmd_usage(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let sessions = sessions(gateway)?;
    Ok(notice(
        "本会话用量",
        session_usage_block_text(&sessions.prompt_usage()),
    ))
}

async fn cmd_workflow(gateway: &GatewayHandle, args: &str) -> Result<Value, RpcError> {
    let args = args.trim();
    if args.is_empty() || args.eq_ignore_ascii_case("runs") {
        return Ok(terminal_only("workflow"));
    }
    let first = args.split_whitespace().next().unwrap_or("");
    if matches!(first, "pause" | "resume" | "stop" | "save") {
        return Ok(terminal_only("workflow"));
    }
    let (name, arguments) = match workflow_command_arguments(args) {
        Ok(v) => v,
        Err(body) => return Ok(notice("工作流", body)),
    };
    let Some(tools) = gateway.ctx().get::<Tools>(TOOLS) else {
        return Ok(notice(format!("/{name}"), "tools 未挂载".to_string()));
    };
    let result = tools
        .execute(ToolCall {
            id: format!("slash-tool-{name}"),
            name: WORKFLOW_TOOL_NAME.into(),
            arguments,
        })
        .await;
    Ok(notice(format!("/{name}"), result.content))
}

fn cmd_timestamps(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let settings = settings(gateway)?;
    let on = !settings.timestamps();
    settings.set_timestamps(on);
    Ok(applied(if on {
        "时间戳已开"
    } else {
        "时间戳已关"
    }))
}

fn submit_text(gateway: &GatewayHandle, text: String) -> Result<Value, RpcError> {
    let port = session_port(gateway)?;
    let working = port.working();
    port.submit(text, false);
    // Embed refreshes environment on `submitted`. User/assistant text arrives
    // via `thread/subscribe` transcript events (SessionCoordinator.open already
    // subscribes); this is not a PolarVigil Runtime replay.
    Ok(json!({
        "ok": true,
        "kind": "submitted",
        "turn": {
            "threadId": crate::threads::Page::thread_id_of(gateway.ctx()),
            "turnId": format!(
                "t{}",
                gateway
                    .latest_seq(gateway.page_identity())
                    .saturating_add(1)
            ),
            "status": if working { "queued" } else { "running" }
        }
    }))
}

fn parse_slash_line(text: &str) -> Option<(String, String)> {
    let goal_usage = cordis_spine::goal_usage_message();
    let loop_usage = cordis_spine::loop_usage_message();
    let mut body = text.trim();
    if let Some(rest) = body.strip_prefix(goal_usage) {
        body = rest.trim();
    } else if let Some(rest) = body.strip_prefix(loop_usage) {
        body = rest.trim();
    }
    if body.is_empty() {
        return None;
    }
    let line = body.lines().last().unwrap_or(body).trim();
    let rest = line.strip_prefix('/')?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").trim();
    if name.is_empty() {
        return None;
    }
    let args = parts.next().unwrap_or("").trim().to_string();
    Some((name.to_string(), args))
}

fn catalog_json(entry: &SlashCatalogEntry) -> Value {
    json!({
        "name": entry.name,
        "display": format!("/{}", entry.name),
        "aliases": entry.aliases,
        "description": entry.description,
        "takesArgs": entry.takes_args,
        "surface": surface_for(entry.name),
        "kind": "command",
        "capture": Value::Null,
    })
}

fn item_json(item: &Item) -> Value {
    json!({
        "name": item.name,
        "display": format!("/{}", item.name),
        "aliases": item.aliases,
        "description": item.description,
        "takesArgs": item.takes_args,
        "surface": item.surface,
        "kind": item.capture.map(|_| "capture").unwrap_or("command"),
        "capture": item.capture,
    })
}

fn extra_json(entry: &SlashEntry) -> Value {
    json!({
        "name": entry.command,
        "display": entry.display(),
        "aliases": [],
        "description": entry.description,
        "takesArgs": true,
        "surface": "gateway",
        "kind": entry.kind.as_str(),
        "capture": Value::Null,
    })
}

fn help_body(gateway: &GatewayHandle) -> String {
    let mut lines = Vec::new();
    for entry in slash_catalog() {
        lines.push(format!("/{}  {}", entry.name, entry.description));
    }
    for item in CAPTURE {
        lines.push(format!("/{}  {}", item.name, item.description));
    }
    if let Some(slash) = gateway.ctx().get::<Slash>(SLASH) {
        for entry in slash.list() {
            lines.push(format!("{}  {}", entry.display(), entry.description));
        }
    }
    lines.join("\n")
}

fn filled(text: impl Into<String>) -> Value {
    json!({ "ok": true, "kind": "filled", "fill": text.into() })
}

fn notice(title: impl Into<String>, body: impl Into<String>) -> Value {
    json!({
        "ok": true,
        "kind": "notice",
        "notice": { "title": title.into(), "body": body.into() }
    })
}

fn menu(id: &str) -> Value {
    json!({ "ok": true, "kind": "menu", "menu": id })
}

fn applied(message: impl Into<String>) -> Value {
    json!({ "ok": true, "kind": "applied", "notice": { "title": "", "body": message.into() } })
}

fn passthrough() -> Value {
    json!({ "ok": true, "kind": "passthrough" })
}

fn terminal_only(name: &str) -> Value {
    notice("终端命令", format!("/{name} 请在 Dock 终端使用。"))
}

fn sessions(gateway: &GatewayHandle) -> Result<std::sync::Arc<Sessions>, RpcError> {
    gateway
        .ctx()
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| RpcError::app("unavailable", "sessions service is not mounted"))
}

fn settings(gateway: &GatewayHandle) -> Result<std::sync::Arc<AppSettings>, RpcError> {
    gateway
        .ctx()
        .get::<AppSettings>(SETTINGS)
        .ok_or_else(|| RpcError::app("unavailable", "settings service is not mounted"))
}

fn session_port(gateway: &GatewayHandle) -> Result<std::sync::Arc<SessionRef>, RpcError> {
    gateway
        .ctx()
        .get::<SessionRef>(SESSION_PORT)
        .ok_or_else(|| RpcError::app("unavailable", "session.port is not mounted"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn tokens(entry: SlashCatalogEntry) -> impl Iterator<Item = &'static str> {
        std::iter::once(entry.name).chain(entry.aliases.iter().copied())
    }

    #[test]
    fn builtin_tokens_match_tui_catalog() {
        let tui: HashSet<String> = slash_catalog()
            .flat_map(tokens)
            .map(str::to_string)
            .collect();
        let listed: HashSet<String> = slash_catalog()
            .map(|entry| catalog_json(&entry))
            .flat_map(|row| {
                let name = row["name"].as_str().unwrap().to_string();
                let aliases: Vec<String> = row["aliases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|a| a.as_str().map(str::to_string))
                    .collect();
                std::iter::once(name).chain(aliases)
            })
            .collect();
        assert_eq!(listed, tui);
        assert_eq!(surface_for("cd"), "terminal");
        assert_eq!(surface_for("settings"), "terminal");
        assert_eq!(surface_for("new"), "gateway");
        assert_eq!(surface_for("pair"), "terminal");
        assert!(gateway_cmd("settings").is_none());
        assert!(gateway_cmd("new").is_some());
        for entry in slash_catalog() {
            if gateway_cmd(entry.name).is_some() {
                assert_eq!(surface_for(entry.name), "gateway");
            } else {
                assert_eq!(surface_for(entry.name), "terminal");
            }
        }
    }
}
