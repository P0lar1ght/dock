//! `subagent/*`：一个线程启动的子代理。只认启动它的那一页（`threadId`），别的线程
//! 拿着 agent id 也查不到、发不了。
//!
//! 活着的子代理在内存里；父会话落盘的，子代理的对话也落在 `<会话>/subagents/<id>/`。
//! `list` / `history` 内存里没有就读盘（线程关着、Dock 重启过都行）；`send` /
//! `interrupt` / `stop` 只认活着的。

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::{json, Value};

use cordis_spine::{Sessions, SubagentMeta, SubagentSnap, Subagents, SESSIONS, SUBAGENTS};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;
use crate::subagents;
use crate::threads::{self, Page};

/// `subagent/list { threadId }` → `{ agents }`，先启动的在前。活着的照内存报，
/// 只在盘上的（关掉的线程、Dock 重启前的）带 `persisted: true`。
pub fn list(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let thread_id = threads::thread_param(&params);
    let page = threads::resolve(gateway, &thread_id).ok();
    let dir = session_dir(gateway, &thread_id);
    if page.is_none() && dir.is_none() {
        return Err(RpcError::app(
            "not_found",
            format!("thread {thread_id} not found"),
        ));
    }
    let mut agents: Vec<Value> = Vec::new();
    let mut live: HashSet<String> = HashSet::new();
    if let Some(page) = &page {
        let sub = service(gateway)?;
        for snap in subagents::children_of(gateway.ctx(), &page.identity) {
            let role = subagents::role_label(&page.ctx, &snap.subagent_type);
            agents.push(subagents::info(&snap, &sub.events(&snap.id), role));
            live.insert(snap.id.clone());
        }
    }
    for meta in dir.iter().flat_map(|d| cordis_spine::load_subagents(d)) {
        if !live.contains(&meta.id) {
            let role = role_for(gateway, page.as_ref(), &thread_id, &meta.subagent_type);
            agents.push(subagents::persisted_info(&meta, role));
        }
    }
    agents.sort_by_key(|a| a["startedAt"].as_u64().unwrap_or(0));
    Ok(json!({ "agents": agents }))
}

/// `thread/list` 一项上的子代理计数：`subagentCount`（内存里的 + 只在盘上的）、
/// `subagentsRunning`（正在跑的，只有开着的页才有）。没有子代理就不加这两个键。
pub fn stamp_counts(
    gateway: &GatewayHandle,
    item: &mut Value,
    page: Option<&Page>,
    dir: Option<PathBuf>,
) {
    // 关着的：只数目录（名册一次要列全部会话，别逐个读抬头）。
    let Some(page) = page else {
        let n = dir.map(|d| cordis_spine::count_subagents(&d)).unwrap_or(0);
        if n > 0 {
            item["subagentCount"] = json!(n);
            item["subagentsRunning"] = json!(0);
        }
        return;
    };
    let mut ids: HashSet<String> = HashSet::new();
    let mut running = 0usize;
    {
        for snap in subagents::children_of(gateway.ctx(), &page.identity) {
            running += usize::from(snap.running());
            ids.insert(snap.id);
        }
    }
    if let Some(dir) = dir {
        ids.extend(cordis_spine::load_subagents(&dir).into_iter().map(|m| m.id));
    }
    if !ids.is_empty() {
        item["subagentCount"] = json!(ids.len());
        item["subagentsRunning"] = json!(running);
    }
}

/// 角色显示名：开着的页看它现在的预设；关着的看它落盘时的预设（不是根上那份——按页预设时
/// 两者可能不同，同一个子代理开着和关着时叫法会不一样）。
fn role_for(
    gateway: &GatewayHandle,
    page: Option<&Page>,
    thread_id: &str,
    subagent_type: &str,
) -> Option<String> {
    if let Some(page) = page {
        return subagents::role_label(&page.ctx, subagent_type);
    }
    match crate::handlers::thread::roster_entries(gateway)
        .into_iter()
        .find(|e| e.id == thread_id)
    {
        Some(entry) => subagents::role_label_at(
            gateway.ctx(),
            &entry.cwd,
            entry.preset_id.as_deref(),
            subagent_type,
        ),
        None => subagents::role_label(gateway.ctx(), subagent_type),
    }
}

/// 线程的落盘会话目录：开着的页问它的会话；关着的按名册找。
fn session_dir(gateway: &GatewayHandle, thread_id: &str) -> Option<PathBuf> {
    if let Ok(page) = threads::resolve(gateway, thread_id) {
        return page.ctx.get::<Sessions>(SESSIONS)?.existing_session_dir();
    }
    crate::handlers::thread::roster_entries(gateway)
        .into_iter()
        .find(|e| e.id == thread_id)
        .map(|e| cordis_spine::sessions_cwd_dir(&e.cwd).join(&e.id))
}

/// 盘上的一个子代理：抬头 + 回放好的事件。
fn persisted(
    gateway: &GatewayHandle,
    params: &Value,
) -> Result<(SubagentMeta, Vec<Value>), RpcError> {
    let thread_id = threads::thread_param(params);
    let agent_id = agent_param(params)?;
    let (meta, events, times) = session_dir(gateway, &thread_id)
        .and_then(|dir| cordis_spine::load_subagent(&dir, agent_id))
        .ok_or_else(|| RpcError::app("not_found", format!("这个线程没有子代理 {agent_id}")))?;
    let (tx, _) = tokio::sync::broadcast::channel(1);
    let mut t = crate::transcript::Transcript::for_child("", "", agent_id, tx);
    t.replay(&events, &times, &[]);
    let events = t
        .history_since(0)
        .into_iter()
        .map(|e| e.as_history_item_as(&thread_id))
        .collect();
    Ok((meta, events))
}

/// `subagent/history { threadId, agentId }` → `{ agent, events }`。`events` 和
/// `thread/history` 的同形，之后的推送（`subagent/event`）接着它的 `seq`。
pub fn history(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let Ok((page, snap)) = resolve(gateway, &params) else {
        // 不在内存里：读盘（线程关着、Dock 重启过、早就收掉的）。
        let (meta, events) = persisted(gateway, &params)?;
        let thread_id = threads::thread_param(&params);
        let page = threads::resolve(gateway, &thread_id).ok();
        let role = role_for(gateway, page.as_ref(), &thread_id, &meta.subagent_type);
        return Ok(json!({ "agent": subagents::persisted_info(&meta, role), "events": events }));
    };
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
/// 浮层的输入框同款）。在跑就在下一步读到，空闲就开下一轮；已经收掉的、只在盘上的
/// 同一个 id 复活接着聊（回 `revived: true`）。用户开的一轮结果送达父级但不叫醒它。
pub fn send(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let message = params
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if message.is_empty() {
        return Err(RpcError::invalid_params("消息不能为空"));
    }
    let sub = service(gateway)?;
    match resolve(gateway, &params) {
        Ok((_, snap)) if !snap.done => {
            let was_running = snap.running();
            sub.send_message(&snap.id, message, true)
                .map_err(|e| RpcError::app("subagent_closed", e))?;
            Ok(json!({ "ok": true, "wasRunning": was_running }))
        }
        // 收掉了 / 只在盘上：同一个 id 接着聊（线程得开着，复活要挂到那一页）。
        _ => {
            let page = threads::resolve_param(gateway, &params)?;
            sub.revive(&page.ctx, agent_param(&params)?, message)
                .map_err(|e| RpcError::app("subagent_closed", e))?;
            Ok(json!({ "ok": true, "wasRunning": false, "revived": true }))
        }
    }
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
    let agent_id = agent_param(params)?;
    let snap = service(gateway)?
        .snapshot(agent_id)
        .filter(|snap| snap.parent == page.identity)
        .ok_or_else(|| RpcError::app("not_found", format!("这个线程没有子代理 {agent_id}")))?;
    Ok((page, snap))
}

fn agent_param(params: &Value) -> Result<&str, RpcError> {
    params
        .get("agentId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("agentId 不能为空"))
}
