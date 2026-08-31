use serde_json::{json, Value};

use cordis_spine::{ArchivedSession, Sessions, SESSIONS};

use crate::handle::GatewayHandle;
use crate::protocol::{self, RpcError, LIVE_THREAD_ID};

pub fn list(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let sessions = live_sessions(gateway)?;
    let workspace_id = protocol::DEFAULT_WORKSPACE_ID;
    let mut threads = vec![live_summary(&sessions, workspace_id)];
    for item in sessions.archived() {
        threads.push(archived_summary(&item, workspace_id));
    }
    Ok(json!({ "threads": threads }))
}

pub fn start(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let sessions = live_sessions(gateway)?;
    sessions.archive_current();
    sessions.clear();
    gateway.reset_transcript();
    Ok(json!({
        "thread": live_summary(&sessions, protocol::DEFAULT_WORKSPACE_ID)
    }))
}

pub fn restore(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = text(&params, "threadId")?;
    if id == LIVE_THREAD_ID {
        let sessions = live_sessions(gateway)?;
        return Ok(json!({
            "thread": live_summary(&sessions, protocol::DEFAULT_WORKSPACE_ID)
        }));
    }
    let sessions = live_sessions(gateway)?;
    sessions.archive_current();
    if !sessions.restore(&id) {
        return Err(RpcError::app("not_found", format!("thread {id} not found")));
    }
    gateway.reset_transcript();
    Ok(json!({
        "thread": live_summary(&sessions, protocol::DEFAULT_WORKSPACE_ID)
    }))
}

pub fn history(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = text(&params, "threadId").unwrap_or_else(|_| LIVE_THREAD_ID.into());
    let sessions = live_sessions(gateway)?;
    if id != LIVE_THREAD_ID {
        let item = sessions
            .archived()
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| RpcError::app("not_found", format!("thread {id} not found")))?;
        return Ok(json!({
            "thread": archived_summary(&item, protocol::DEFAULT_WORKSPACE_ID),
            "messages": archived_messages(&item),
            "events": []
        }));
    }
    let events: Vec<Value> = gateway
        .history_since(0)
        .into_iter()
        .map(|e| e.as_history_item())
        .collect();
    Ok(json!({
        "thread": live_summary(&sessions, protocol::DEFAULT_WORKSPACE_ID),
        "messages": live_messages(&sessions),
        "events": events
    }))
}

pub fn subscribe(
    gateway: &GatewayHandle,
    params: Value,
    subscribed: &mut std::collections::HashSet<String>,
) -> Result<Value, RpcError> {
    let id = text(&params, "threadId").unwrap_or_else(|_| LIVE_THREAD_ID.into());
    let since = params
        .get("sinceSeq")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    subscribed.insert(id.clone());
    let replayed = if id == LIVE_THREAD_ID {
        gateway.history_since(since).len()
    } else {
        0
    };
    Ok(json!({
        "ok": true,
        "threadId": id,
        "replayed": replayed,
        "latestSeq": gateway.latest_seq()
    }))
}

pub fn unsubscribe(
    params: Value,
    subscribed: &mut std::collections::HashSet<String>,
) -> Result<Value, RpcError> {
    let id = text(&params, "threadId").unwrap_or_else(|_| LIVE_THREAD_ID.into());
    subscribed.remove(&id);
    Ok(json!({ "ok": true }))
}

fn live_sessions(gateway: &GatewayHandle) -> Result<std::sync::Arc<Sessions>, RpcError> {
    gateway
        .ctx()
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| RpcError::app("unavailable", "sessions service is not mounted"))
}

fn live_summary(sessions: &Sessions, workspace_id: &str) -> Value {
    let title = sessions
        .events()
        .iter()
        .find_map(|e| match e {
            cordis_spine::LogEvent::User(t) if !t.trim().is_empty() => {
                Some(t.chars().take(40).collect::<String>())
            }
            _ => None,
        })
        .unwrap_or_else(|| "当前会话".into());
    json!({
        "id": LIVE_THREAD_ID,
        "title": title,
        "workspaceId": workspace_id,
        "createdAt": 0,
        "updatedAt": 0
    })
}

fn archived_summary(item: &ArchivedSession, workspace_id: &str) -> Value {
    json!({
        "id": item.id,
        "title": item.title,
        "workspaceId": workspace_id,
        "createdAt": 0,
        "updatedAt": 0,
        "archivedAt": 1
    })
}

fn live_messages(sessions: &Sessions) -> Vec<Value> {
    sessions
        .events()
        .into_iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            cordis_spine::LogEvent::User(content) => Some(json!({
                "id": format!("u{i}"),
                "threadId": LIVE_THREAD_ID,
                "role": "user",
                "content": content,
                "createdAt": 0
            })),
            cordis_spine::LogEvent::LlmStream(out) if !out.text.is_empty() => Some(json!({
                "id": format!("a{i}"),
                "threadId": LIVE_THREAD_ID,
                "role": "assistant",
                "content": out.text,
                "createdAt": 0
            })),
            _ => None,
        })
        .collect()
}

fn archived_messages(item: &ArchivedSession) -> Vec<Value> {
    item.events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            cordis_spine::LogEvent::User(content) => Some(json!({
                "id": format!("u{i}"),
                "threadId": item.id,
                "role": "user",
                "content": content,
                "createdAt": 0
            })),
            cordis_spine::LogEvent::LlmStream(out) if !out.text.is_empty() => Some(json!({
                "id": format!("a{i}"),
                "threadId": item.id,
                "role": "assistant",
                "content": out.text,
                "createdAt": 0
            })),
            _ => None,
        })
        .collect()
}

fn text(params: &Value, key: &str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("{key} is required")))
}
