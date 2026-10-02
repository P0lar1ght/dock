//! `subagent/*`：一个线程启动的子代理。只认启动它的那一页（`threadId`），别的线程
//! 拿着 agent id 也查不到、发不了。子代理只活在内存里，线程得开着。

use serde_json::{json, Value};

use cordis_spine::{SubagentSnap, Subagents, SUBAGENTS};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;
use crate::subagents;
use crate::threads::{self, Page};

/// `subagent/list { threadId }` → `{ agents }`，先启动的在前。
pub fn list(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let sub = service(gateway)?;
    let agents: Vec<Value> = subagents::children_of(gateway.ctx(), &page.identity)
        .iter()
        .map(|snap| {
            let role = subagents::role_label(&page.ctx, &snap.subagent_type);
            subagents::info(snap, &sub.events(&snap.id), role)
        })
        .collect();
    Ok(json!({ "agents": agents }))
}

/// `subagent/history { threadId, agentId }` → `{ agent, events }`。`events` 和
/// `thread/history` 的同形，之后的推送（`subagent/event`）接着它的 `seq`。
pub fn history(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let (page, snap) = resolve(gateway, &params)?;
    let sub = service(gateway)?;
    let thread_id = threads::thread_param(&params);
    let events: Vec<Value> = gateway
        .child_history(&snap.id)
        .into_iter()
        .map(|e| e.as_history_item_as(&thread_id))
        .collect();
    let role = subagents::role_label(&page.ctx, &snap.subagent_type);
    Ok(json!({
        "agent": subagents::info(&snap, &sub.events(&snap.id), role),
        "events": events
    }))
}

/// `subagent/send { threadId, agentId, message }`：用户对子代理说一句（TUI 子代理
/// 浮层的输入框同款）。在跑就在下一步读到，空闲就开下一轮；已经收掉的报错。
pub fn send(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let message = params
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if message.is_empty() {
        return Err(RpcError::invalid_params("消息不能为空"));
    }
    let (_, snap) = resolve(gateway, &params)?;
    let was_running = snap.running();
    service(gateway)?
        .send_message(&snap.id, message, true)
        .map_err(|e| RpcError::app("subagent_closed", e))?;
    Ok(json!({ "ok": true, "wasRunning": was_running }))
}

/// `subagent/interrupt { threadId, agentId }`：停下这一轮，子代理留着能接着聊。
/// 没在跑就什么都不做（`interrupted: false`）。
pub fn interrupt(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let (_, snap) = resolve(gateway, &params)?;
    if !snap.running() {
        return Ok(json!({ "ok": true, "interrupted": false }));
    }
    service(gateway)?.interrupt_agent(&snap.id);
    Ok(json!({ "ok": true, "interrupted": true }))
}

/// `subagent/stop { threadId, agentId }`：收掉子代理，之后不能再接着聊。已经收掉的
/// 什么都不做（`stopped: false`）。
pub fn stop(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let (_, snap) = resolve(gateway, &params)?;
    if snap.done {
        return Ok(json!({ "ok": true, "stopped": false }));
    }
    service(gateway)?.kill(&snap.id);
    Ok(json!({ "ok": true, "stopped": true }))
}

fn service(gateway: &GatewayHandle) -> Result<std::sync::Arc<Subagents>, RpcError> {
    gateway
        .ctx()
        .get::<Subagents>(SUBAGENTS)
        .ok_or_else(|| RpcError::app("unavailable", "subagents service is not mounted"))
}

/// 参数里的线程和 `agentId`：子代理得是这一页启动的。
fn resolve(gateway: &GatewayHandle, params: &Value) -> Result<(Page, SubagentSnap), RpcError> {
    let page = threads::resolve_param(gateway, params)?;
    let agent_id = params
        .get("agentId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("agentId 不能为空"))?;
    let snap = service(gateway)?
        .snapshot(agent_id)
        .filter(|snap| snap.parent == page.identity)
        .ok_or_else(|| RpcError::app("not_found", format!("这个线程没有子代理 {agent_id}")))?;
    Ok((page, snap))
}
