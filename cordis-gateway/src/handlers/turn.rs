use serde_json::{json, Value};

use cordis_spine::{Sessions, UserImage, SESSIONS};
use cordis_tui::{SessionRef, SESSION_PORT};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;
use crate::threads::{self, Page};

pub fn start(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    submit(gateway, params, true)
}

pub fn enqueue(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    submit(gateway, params, false)
}

pub fn steer(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    submit(gateway, params, true)
}

pub fn cancel(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    // 关着的页没有在跑的一轮：没什么可停的。
    let Some(page) = open_or_none(threads::resolve_param(gateway, &params))? else {
        return Ok(json!({ "cancelled": false }));
    };
    let port = session_port(&page)?;
    port.cancel();
    Ok(json!({ "cancelled": true }))
}

pub fn queue_list(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let thread_id = threads::thread_param(&params);
    // 队列跟着页走，关着的页队列是空的。
    let Some(page) = open_or_none(threads::resolve(gateway, &thread_id))? else {
        return Ok(json!({ "active": Value::Null, "items": [] }));
    };
    let port = session_port(&page)?;
    let items: Vec<Value> = port
        .queued_prompts()
        .into_iter()
        .map(|q| {
            json!({
                "id": q.id,
                "threadId": thread_id,
                "message": q.text,
                "kind": "queue",
                "status": "queued",
                "createdAt": 0,
                "updatedAt": 0
            })
        })
        .collect();
    let active = if port.working() {
        json!({
            "threadId": thread_id,
            "turnId": "live",
            "status": "running"
        })
    } else {
        Value::Null
    };
    Ok(json!({ "active": active, "items": items }))
}

pub fn queue_remove(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = params
        .get("queueId")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let thread_id = threads::thread_param(&params);
    let removed = match open_or_none(threads::resolve(gateway, &thread_id))? {
        Some(page) => session_port(&page)?.take_queued(id.clone()).is_some(),
        None => false,
    };
    Ok(json!({
        "threadId": thread_id,
        "queueId": id.unwrap_or_default(),
        "removed": removed
    }))
}

/// `thread_not_open` → `None`（停止、队列这类方法对关着的页回空结果，不开页）。
fn open_or_none(page: Result<Page, RpcError>) -> Result<Option<Page>, RpcError> {
    match page {
        Ok(page) => Ok(Some(page)),
        Err(e) if e.details_code == "thread_not_open" => Ok(None),
        Err(e) => Err(e),
    }
}

fn submit(gateway: &GatewayHandle, params: Value, send_now: bool) -> Result<Value, RpcError> {
    let thread_id = threads::thread_param(&params);
    let page = threads::resolve(gateway, &thread_id)?;
    let mut message = params
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let images = resolve_turn_images(gateway, &page, &params)?;
    if !images.is_empty() {
        message = with_image_chips(&message, images.len());
        if let Some(sessions) = page.ctx.get::<Sessions>(SESSIONS) {
            sessions.queue_user_images(images);
        }
    }
    if message.trim().is_empty() {
        return Err(RpcError::invalid_params("message is required"));
    }
    apply_turn_intent(&page, &params, &message)?;
    let port = session_port(&page)?;
    let working = port.working();
    port.submit(message, send_now);
    let status = if working && !send_now {
        "queued"
    } else {
        "running"
    };
    Ok(json!({
        "threadId": thread_id,
        "turnId": format!("t{}", gateway.latest_seq(&page.identity).saturating_add(1)),
        "status": status
    }))
}

fn resolve_turn_images(
    gateway: &GatewayHandle,
    page: &Page,
    params: &Value,
) -> Result<Vec<UserImage>, RpcError> {
    let Some(list) = params.get("imageInputs").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    if list.len() > 4 {
        return Err(RpcError::app(
            "image_input_invalid",
            "A Turn accepts 1-4 images",
        ));
    }
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        if let Some(reuse) = item.get("reuseTurnId").and_then(Value::as_str) {
            out.extend(reuse_turn_images(page, reuse)?);
            continue;
        }
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("imageInputs.id is required"))?;
        let digest = item
            .get("digest")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("imageInputs.digest is required"))?;
        out.push(gateway.take_image(id, digest)?.into());
    }
    Ok(out)
}

fn reuse_turn_images(page: &Page, turn_id: &str) -> Result<Vec<UserImage>, RpcError> {
    let Some(sessions) = page.ctx.get::<Sessions>(SESSIONS) else {
        return Err(RpcError::app("unavailable", "sessions is not mounted"));
    };
    let index = turn_id
        .strip_prefix('t')
        .and_then(|n| n.parse::<usize>().ok())
        .and_then(|n| n.checked_sub(1));
    let Some(index) = index else {
        return Err(RpcError::invalid_params("reuseTurnId is invalid"));
    };
    let rows = sessions.user_images();
    rows.get(index)
        .cloned()
        .filter(|row| !row.is_empty())
        .ok_or_else(|| {
            RpcError::app(
                "image_input_invalid",
                "No reusable screenshot Turn is available",
            )
        })
}

fn with_image_chips(message: &str, count: usize) -> String {
    let mut chips = String::new();
    for i in 1..=count {
        let chip = format!("[Image #{i}]");
        if message.contains(&chip) {
            continue;
        }
        if !chips.is_empty() {
            chips.push(' ');
        }
        chips.push_str(&chip);
    }
    if chips.is_empty() {
        return message.to_string();
    }
    if message.trim().is_empty() {
        chips
    } else {
        format!("{chips} {message}")
    }
}

fn apply_turn_intent(page: &Page, params: &Value, message: &str) -> Result<(), RpcError> {
    let intent = params.get("intent");
    if intent.and_then(|v| v.get("mode")).and_then(Value::as_str) == Some("plan") {
        if let Some(plan) = page
            .ctx
            .get::<cordis_spine::PlanMode>(cordis_spine::PLAN_MODE)
        {
            plan.set(true);
        }
    }
    let goal_intent = intent.and_then(|v| v.get("goal"));
    let op = goal_intent
        .and_then(|g| g.get("operation"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if op.is_empty() {
        return Ok(());
    }
    let Some(goal) = page.ctx.get::<cordis_spine::Goal>(cordis_spine::GOAL) else {
        return Ok(());
    };
    match op {
        "start" => {
            let objective = goal_intent
                .and_then(|g| g.get("objective"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(message.trim());
            goal.start(objective);
        }
        "resume" => {
            let _ = goal.resume();
        }
        _ => {}
    }
    Ok(())
}

fn session_port(page: &Page) -> Result<std::sync::Arc<SessionRef>, RpcError> {
    page.ctx
        .get::<SessionRef>(SESSION_PORT)
        .ok_or_else(|| RpcError::app("unavailable", "session.port is not mounted"))
}
