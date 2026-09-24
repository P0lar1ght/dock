use std::collections::HashMap;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};

use cordis_spine::{
    apply_restored_preset, session_cwd, AgentPresets, ApplyRestoredPreset, ArchivedSession, Roster,
    RosterEntry, Sessions, AGENT_PRESETS, ROSTER, SESSIONS,
};
use cordis_tui::{Tabs, TUI_TABS};

use crate::handle::GatewayHandle;
use crate::protocol::{self, RpcError, LIVE_THREAD_ID};
use crate::threads::{self, Page};

/// 订阅：分页身份 → 客户端订阅时用的 `threadId`（第 1 页可以按 `live` 也可以按
/// 会话 id 订，推送时用它订的那个）。
pub type Subscriptions = HashMap<String, String>;

pub fn list(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    if params.get("scope").and_then(Value::as_str) == Some("all") {
        return list_all(gateway);
    }
    let sessions = live_sessions(gateway)?;
    let workspace_id = protocol::DEFAULT_WORKSPACE_ID;
    let mut threads = vec![live_summary(&sessions, workspace_id)];
    for item in sessions.archived() {
        threads.push(archived_summary(&item, workspace_id));
    }
    Ok(json!({ "threads": threads }))
}

/// `thread/list { scope: "all" }`：所有开着的页 + 所有目录下的落盘会话，`id` 一律是
/// 会话 id（第 1 页另带 `alias: "live"`），带 `cwd` 与 `open`。GUI 左侧项目树用。
fn list_all(gateway: &GatewayHandle) -> Result<Value, RpcError> {
    let mut threads = Vec::new();
    let mut open_ids = std::collections::HashSet::new();
    for page in threads::open_pages(gateway) {
        let sessions = page.sessions()?;
        let id = page.session_id();
        open_ids.insert(id.clone());
        let mut item = live_summary(&sessions, protocol::DEFAULT_WORKSPACE_ID);
        item["id"] = json!(id);
        item["cwd"] = json!(session_cwd(&page.ctx).display().to_string());
        item["open"] = json!(true);
        if page.is_root() {
            item["alias"] = json!(LIVE_THREAD_ID);
        }
        threads.push(item);
    }
    for entry in roster_entries(gateway) {
        if open_ids.contains(&entry.id) {
            continue;
        }
        threads.push(roster_summary(&entry));
    }
    Ok(json!({ "threads": threads }))
}

/// `thread/start` 不带 `cwd`：老语义，第 1 页换成一个新会话。带 `cwd` 的走
/// [`start_at`]。
pub fn start(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let sessions = live_sessions(gateway)?;
    sessions.archive_current();
    sessions.clear();
    cordis_spine::clear_plan_for_session_switch(gateway.ctx());
    if let Some(title) = params.get("title").and_then(Value::as_str) {
        sessions.set_live_title(title);
    }
    gateway.reset_transcript();
    Ok(json!({
        "thread": live_summary(&sessions, protocol::DEFAULT_WORKSPACE_ID)
    }))
}

pub fn rename(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = canonical(gateway, text(&params, "threadId")?);
    let title = text(&params, "title")?;
    if let Some(page) = open_other_page(gateway, &id) {
        page.sessions()?.set_live_title(&title);
        return Ok(json!({ "ok": true, "thread": open_summary(&page)? }));
    }
    let sessions = live_sessions(gateway)?;
    if id == LIVE_THREAD_ID {
        sessions.set_live_title(&title);
        return Ok(json!({
            "ok": true,
            "thread": live_summary(&sessions, protocol::DEFAULT_WORKSPACE_ID)
        }));
    }
    let item = sessions
        .rename_archived(&id, &title)
        .ok_or_else(|| RpcError::app("not_found", format!("thread {id} not found")))?;
    Ok(json!({
        "ok": true,
        "thread": archived_summary(&item, protocol::DEFAULT_WORKSPACE_ID)
    }))
}

pub fn archive(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = canonical(
        gateway,
        text(&params, "threadId").unwrap_or_else(|_| LIVE_THREAD_ID.into()),
    );
    reject_open_other_page(gateway, &id)?;
    let sessions = live_sessions(gateway)?;
    if id != LIVE_THREAD_ID {
        let item = sessions
            .archived()
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| RpcError::app("not_found", format!("thread {id} not found")))?;
        return Ok(json!({ "thread": archived_summary(&item, protocol::DEFAULT_WORKSPACE_ID) }));
    }
    match sessions.archive_current() {
        Some(item) => {
            sessions.clear();
            gateway.reset_transcript();
            Ok(json!({
                "thread": archived_summary(&item, protocol::DEFAULT_WORKSPACE_ID)
            }))
        }
        None => Ok(json!({
            "thread": live_summary(&sessions, protocol::DEFAULT_WORKSPACE_ID)
        })),
    }
}

pub fn delete(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = canonical(gateway, text(&params, "threadId")?);
    reject_open_other_page(gateway, &id)?;
    if id == LIVE_THREAD_ID {
        return Err(RpcError::app(
            "invalid_params",
            "cannot delete the live thread; archive it first",
        ));
    }
    let sessions = live_sessions(gateway)?;
    let item = sessions
        .remove_archived(&id)
        .ok_or_else(|| RpcError::app("not_found", format!("thread {id} not found")))?;
    Ok(json!({
        "ok": true,
        "thread": archived_summary(&item, protocol::DEFAULT_WORKSPACE_ID)
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
    let stamped = sessions.archived_preset_id(&id);
    sessions.archive_current();
    if !sessions.restore(&id) {
        return Err(RpcError::app("not_found", format!("thread {id} not found")));
    }
    cordis_spine::clear_plan_for_session_switch(gateway.ctx());
    if let Some(presets) = gateway.ctx().get::<AgentPresets>(AGENT_PRESETS) {
        if let ApplyRestoredPreset::Failed { id, error } =
            apply_restored_preset(&presets, stamped.as_deref())
        {
            eprintln!("dock: thread/restore preset `{id}` not applied: {error}");
        }
    }
    gateway.reset_transcript();
    Ok(json!({
        "thread": live_summary(&sessions, protocol::DEFAULT_WORKSPACE_ID)
    }))
}

/// `thread/start { cwd, title? }`：在 `cwd` 另开一页（不动第 1 页、不切终端里
/// 正在看的页），回它的会话 id。
pub async fn start_at(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let cwd = PathBuf::from(text(&params, "cwd")?);
    let page = open_page(gateway, &cwd, None).await?;
    if let Some(title) = params.get("title").and_then(Value::as_str) {
        page.sessions()?.set_live_title(title);
    }
    Ok(json!({ "thread": open_summary(&page)? }))
}

/// `thread/open { threadId }`：把一份落盘会话开成一页（在它自己的 cwd 下），已经
/// 开着就回那一页。之后 `turn/start` 等都用这个 `threadId`。
pub async fn open(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = text(&params, "threadId")?;
    if let Ok(page) = threads::resolve(gateway, &id) {
        return Ok(json!({ "thread": open_summary(&page)? }));
    }
    let entry = roster_entries(gateway)
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| RpcError::app("not_found", format!("thread {id} not found")))?;
    let page = open_page(gateway, &entry.cwd, Some(&id)).await?;
    Ok(json!({ "thread": open_summary(&page)? }))
}

/// `thread/close { threadId }`：关掉那一页（会话留在磁盘上，随时能再 open）。
/// 第 1 页关不掉。
pub async fn close(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = text(&params, "threadId")?;
    let page = threads::resolve(gateway, &id)?;
    if page.is_root() {
        return Err(RpcError::app("invalid_params", "第 1 页（live）关不掉"));
    }
    let tabs = tabs(gateway)?;
    tabs.close_session(&page.session_id())
        .await
        .map_err(|e| RpcError::app("close_failed", e))?;
    gateway.drop_page(&page.identity);
    Ok(json!({ "ok": true, "threadId": id }))
}

async fn open_page(
    gateway: &GatewayHandle,
    cwd: &std::path::Path,
    archived: Option<&str>,
) -> Result<Page, RpcError> {
    let tabs = tabs(gateway)?;
    let ctx = tabs
        .open_at(cwd, archived)
        .await
        .map_err(|e| RpcError::app("open_failed", e))?;
    let identity = ctx
        .get::<Sessions>(SESSIONS)
        .map(|s| s.identity().to_string())
        .unwrap_or_default();
    let page = Page { ctx, identity };
    // 从历史开的页带着整段对话：投影按它重建，订阅时才回放得出来。
    gateway.reset_page(&page);
    Ok(page)
}

fn tabs(gateway: &GatewayHandle) -> Result<std::sync::Arc<Tabs>, RpcError> {
    gateway
        .ctx()
        .get::<Tabs>(TUI_TABS)
        .ok_or_else(|| RpcError::app("unavailable", "分页服务没有挂载，开不了新线程"))
}

pub fn history(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = threads::thread_param(&params);
    if let Ok(page) = threads::resolve(gateway, &id) {
        let sessions = page.sessions()?;
        let events: Vec<Value> = gateway
            .history_since(&page.identity, 0)
            .into_iter()
            .map(|e| e.as_history_item_as(&id))
            .collect();
        let mut thread = live_summary(&sessions, protocol::DEFAULT_WORKSPACE_ID);
        thread["id"] = json!(id);
        return Ok(json!({
            "thread": thread,
            "messages": live_messages(&sessions, &id),
            "events": events
        }));
    }
    let sessions = live_sessions(gateway)?;
    if let Some(item) = sessions.archived().into_iter().find(|s| s.id == id) {
        return Ok(json!({
            "thread": archived_summary(&item, protocol::DEFAULT_WORKSPACE_ID),
            "messages": archived_messages(&item),
            "events": []
        }));
    }
    // 别的目录下的会话：从跨目录名册读。
    let entry = roster_entries(gateway)
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| RpcError::app("not_found", format!("thread {id} not found")))?;
    let events = fresh_roster(gateway).transcript(&entry.id, &entry.cwd);
    Ok(json!({
        "thread": roster_summary(&entry),
        "messages": messages_of(&events, &id),
        "events": []
    }))
}

pub fn subscribe(
    gateway: &GatewayHandle,
    params: Value,
    subscribed: &mut Subscriptions,
) -> Result<Value, RpcError> {
    let id = threads::thread_param(&params);
    let since = params.get("sinceSeq").and_then(Value::as_u64).unwrap_or(0);
    let page = threads::resolve(gateway, &id)?;
    subscribed.insert(page.identity.clone(), id.clone());
    Ok(json!({
        "ok": true,
        "threadId": id,
        "replayed": gateway.history_since(&page.identity, since).len(),
        "latestSeq": gateway.latest_seq(&page.identity)
    }))
}

pub fn unsubscribe(params: Value, subscribed: &mut Subscriptions) -> Result<Value, RpcError> {
    let id = threads::thread_param(&params);
    subscribed.retain(|_, alias| *alias != id);
    Ok(json!({ "ok": true }))
}

/// 第 1 页也能按它的会话 id 称呼；老逻辑只认 `live`，这里先归一。
fn canonical(gateway: &GatewayHandle, id: String) -> String {
    match threads::resolve(gateway, &id) {
        Ok(page) if page.is_root() => LIVE_THREAD_ID.into(),
        _ => id,
    }
}

/// `id` 是一张开着的、不是第 1 页的页。
fn open_other_page(gateway: &GatewayHandle, id: &str) -> Option<Page> {
    threads::resolve(gateway, id)
        .ok()
        .filter(|page| !page.is_root())
}

/// 归档 / 删除作用在第 1 页或磁盘上的会话；别的页开着时先关页。
fn reject_open_other_page(gateway: &GatewayHandle, id: &str) -> Result<(), RpcError> {
    if open_other_page(gateway, id).is_some() {
        return Err(RpcError::app(
            "thread_open",
            format!("线程 {id} 还开着，先 thread/close"),
        ));
    }
    Ok(())
}

/// 跨目录名册，**不用备忘**：名册的 2 秒备忘是给 TUI 每帧重绘挡重扫的，网关的
/// 请求是一次次的用户操作——刚关掉的页、刚写进去的话都得看得见。
fn roster_entries(gateway: &GatewayHandle) -> Vec<RosterEntry> {
    fresh_roster(gateway).list()
}

/// 名册服务（先丢掉备忘）；没挂载（精简装配）就临时扫一遍磁盘——名册只是读
/// `$DOCK_HOME/sessions`，没挂服务不等于没有会话。
pub(crate) fn fresh_roster(gateway: &GatewayHandle) -> std::sync::Arc<Roster> {
    match gateway.ctx().get::<Roster>(ROSTER) {
        Some(roster) => {
            roster.invalidate();
            roster
        }
        None => std::sync::Arc::new(Roster::new()),
    }
}

fn roster_summary(entry: &RosterEntry) -> Value {
    let updated = entry
        .updated
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    json!({
        "id": entry.id,
        "title": entry.title,
        "workspaceId": protocol::DEFAULT_WORKSPACE_ID,
        "cwd": entry.cwd.display().to_string(),
        "open": false,
        "createdAt": 0,
        "updatedAt": updated,
        "archivedAt": 1
    })
}

fn open_summary(page: &Page) -> Result<Value, RpcError> {
    let sessions = page.sessions()?;
    let mut item = live_summary(&sessions, protocol::DEFAULT_WORKSPACE_ID);
    item["id"] = json!(page.thread_id());
    item["cwd"] = json!(session_cwd(&page.ctx).display().to_string());
    item["open"] = json!(true);
    Ok(item)
}

fn live_sessions(gateway: &GatewayHandle) -> Result<std::sync::Arc<Sessions>, RpcError> {
    gateway
        .ctx()
        .get::<Sessions>(SESSIONS)
        .ok_or_else(|| RpcError::app("unavailable", "sessions service is not mounted"))
}

fn live_summary(sessions: &Sessions, workspace_id: &str) -> Value {
    let title = sessions.live_title().unwrap_or_else(|| {
        sessions
            .events()
            .iter()
            .find_map(|e| match e {
                cordis_spine::LogEvent::User(t) if !t.trim().is_empty() => {
                    Some(t.chars().take(40).collect::<String>())
                }
                _ => None,
            })
            .unwrap_or_else(|| "当前会话".into())
    });
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

fn live_messages(sessions: &Sessions, thread_id: &str) -> Vec<Value> {
    messages_of(&sessions.events(), thread_id)
}

fn messages_of(events: &[cordis_spine::LogEvent], thread_id: &str) -> Vec<Value> {
    events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            cordis_spine::LogEvent::User(content) => Some(json!({
                "id": format!("u{i}"),
                "threadId": thread_id,
                "role": "user",
                "content": content,
                "createdAt": 0
            })),
            cordis_spine::LogEvent::LlmStream(out) if !out.text.is_empty() => Some(json!({
                "id": format!("a{i}"),
                "threadId": thread_id,
                "role": "assistant",
                "content": out.text,
                "createdAt": 0
            })),
            _ => None,
        })
        .collect()
}

fn archived_messages(item: &ArchivedSession) -> Vec<Value> {
    messages_of(&item.events, &item.id)
}

fn text(params: &Value, key: &str) -> Result<String, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("{key} is required")))
}
