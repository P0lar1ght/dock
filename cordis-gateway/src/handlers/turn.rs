use serde_json::{json, Value};

use cordis_tui::{SessionRef, SESSION_PORT};

use crate::handle::GatewayHandle;
use crate::protocol::{RpcError, LIVE_THREAD_ID};

pub fn start(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    submit(gateway, params, true)
}

pub fn enqueue(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    submit(gateway, params, false)
}

pub fn steer(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    submit(gateway, params, true)
}

pub fn cancel(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let port = session_port(gateway)?;
    port.cancel();
    Ok(json!({ "cancelled": true }))
}

pub fn queue_list(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let port = session_port(gateway)?;
    let items: Vec<Value> = port
        .queued_prompts()
        .into_iter()
        .map(|q| {
            json!({
                "id": q.id,
                "threadId": LIVE_THREAD_ID,
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
            "threadId": LIVE_THREAD_ID,
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
    let port = session_port(gateway)?;
    let removed = port.take_queued(id.clone()).is_some();
    Ok(json!({
        "threadId": LIVE_THREAD_ID,
        "queueId": id.unwrap_or_default(),
        "removed": removed
    }))
}

fn submit(gateway: &GatewayHandle, params: Value, send_now: bool) -> Result<Value, RpcError> {
    let message = params
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if message.trim().is_empty() {
        return Err(RpcError::invalid_params("message is required"));
    }
    apply_turn_intent(gateway, &params, &message)?;
    let port = session_port(gateway)?;
    let working = port.working();
    port.submit(message, send_now);
    let status = if working && !send_now { "queued" } else { "running" };
    Ok(json!({
        "threadId": LIVE_THREAD_ID,
        "turnId": format!("t{}", gateway.latest_seq().saturating_add(1)),
        "status": status
    }))
}

fn apply_turn_intent(
    gateway: &GatewayHandle,
    params: &Value,
    message: &str,
) -> Result<(), RpcError> {
    let intent = params.get("intent");
    if intent.and_then(|v| v.get("mode")).and_then(Value::as_str) == Some("plan") {
        if let Some(plan) = gateway.ctx().get::<cordis_spine::PlanMode>(cordis_spine::PLAN_MODE) {
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
    let Some(goal) = gateway.ctx().get::<cordis_spine::Goal>(cordis_spine::GOAL) else {
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

fn session_port(gateway: &GatewayHandle) -> Result<std::sync::Arc<SessionRef>, RpcError> {
    gateway
        .ctx()
        .get::<SessionRef>(SESSION_PORT)
        .ok_or_else(|| RpcError::app("unavailable", "session.port is not mounted"))
}
