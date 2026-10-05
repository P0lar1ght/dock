//! Slash catalog + execution for the web client.
//!
//! 命令表是 spine 的 `"slash"`：各功能插件登记自己的命令，宿主无关的带 handler。
//! 这里只做投影：`slash/list` 列出表里的命令（`surface` 是 `gateway` / `terminal`）、
//! 子命令提示与附加命令；`slash/execute` 在 `threadId` 那一页上跑一次，把
//! [`SlashOutcome`] 投成 dock.1 的 `applied` / `notice` / `filled` / `submitted` /
//! `menu`。截图命令（`/screenshot`）是宿主页自己的，像素留在 JS 里，这里只列出来。

use serde_json::{json, Value};

use cordis_spine::{
    SessionRef, Slash, SlashCommand, SlashEntry, SlashOutcome, SlashSurface, SESSION_PORT, SLASH,
};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;
use crate::threads;

/// 宿主页（embed）自己执行的截图命令。
#[derive(Clone, Copy)]
struct Item {
    name: &'static str,
    description: &'static str,
    capture: &'static str,
}

const CAPTURE: &[Item] = &[
    Item {
        name: "screenshot",
        description: "截取当前 viewport；参数是发给模型的问题。",
        capture: "viewport",
    },
    Item {
        name: "screenshot --region",
        description: "拖动选择一个区域，只把该区域发给本轮模型。",
        capture: "region",
    },
    Item {
        name: "screenshot --reuse",
        description: "不重新截图，复用当前页面最近一次图片。",
        capture: "reuse",
    },
    Item {
        name: "screenshot --screen",
        description: "打开浏览器共享选择器并捕获一帧。",
        capture: "screen",
    },
    Item {
        name: "screenshot --full-page",
        description: "分段截取当前页面的完整纵向内容。",
        capture: "full-page",
    },
];

pub fn list(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let mut commands = Vec::new();
    if let Some(slash) = gateway.ctx().get::<Slash>(SLASH) {
        let builtins = slash.commands();
        commands.extend(builtins.iter().map(command_json));
        for command in &builtins {
            commands.extend(command.hints.iter().map(|hint| {
                json!({
                    "name": hint.name,
                    "display": format!("/{}", hint.name),
                    "aliases": [],
                    "description": hint.description,
                    "takesArgs": hint.takes_args,
                    "surface": surface(command.surface),
                    "kind": "command",
                    "capture": Value::Null,
                })
            }));
        }
        commands.extend(CAPTURE.iter().map(capture_json));
        commands.extend(slash.list().iter().map(extra_json));
    } else {
        commands.extend(CAPTURE.iter().map(capture_json));
    }
    Ok(json!({ "commands": commands }))
}

pub async fn execute(gateway: GatewayHandle, params: Value) -> Result<Value, RpcError> {
    // 斜杠命令作用在 `threadId` 那一页：命令体拿到的是这一页的 ctx。
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
    let Some((name, args)) = parse_slash_line(&text) else {
        return Ok(passthrough());
    };
    if name == "screenshot" {
        return Ok(json!({ "ok": true, "kind": "capture" }));
    }
    let Some(slash) = gateway.ctx().get::<Slash>(SLASH) else {
        return Ok(passthrough());
    };
    match slash.run(gateway.ctx(), &name, &args).await {
        None => Ok(passthrough()),
        Some(outcome) => project(&gateway, &name, outcome),
    }
}

fn project(gateway: &GatewayHandle, name: &str, outcome: SlashOutcome) -> Result<Value, RpcError> {
    Ok(match outcome {
        SlashOutcome::Applied(message) => applied(message),
        SlashOutcome::Notice { title, body } => notice(title, body),
        SlashOutcome::Fill(text) => filled(text),
        SlashOutcome::Submit(text) => return submit_text(gateway, text),
        SlashOutcome::Menu(id) => menu(&id),
        SlashOutcome::OpenSlot(_) => notice("终端插槽", format!("/{name} 请在 Dock 终端打开。")),
        SlashOutcome::TerminalOnly(name) => terminal_only(&name),
    })
}

fn submit_text(gateway: &GatewayHandle, text: String) -> Result<Value, RpcError> {
    let port = gateway
        .ctx()
        .get::<SessionRef>(SESSION_PORT)
        .ok_or_else(|| RpcError::app("unavailable", "session.port is not mounted"))?;
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

fn surface(surface: SlashSurface) -> &'static str {
    match surface {
        SlashSurface::Host => "gateway",
        SlashSurface::Terminal => "terminal",
    }
}

fn command_json(command: &SlashCommand) -> Value {
    json!({
        "name": command.name,
        "display": command.display(),
        "aliases": command.aliases,
        "description": command.description,
        "takesArgs": command.takes_args,
        "surface": surface(command.surface),
        "kind": "command",
        "capture": Value::Null,
    })
}

fn capture_json(item: &Item) -> Value {
    json!({
        "name": item.name,
        "display": format!("/{}", item.name),
        "aliases": [],
        "description": item.description,
        "takesArgs": true,
        "surface": "embed",
        "kind": "capture",
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
