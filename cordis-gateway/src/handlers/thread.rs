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
use crate::transcript::Transcript;

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

/// `thread/search { query, cwd?, limit? }`：按标题和用户消息搜落盘会话（全部目录），
/// 每条带一小段命中处的纯文本片段（只命中标题时为 `null`）。还没落过盘的内容搜不到。
pub fn search(params: Value) -> Result<Value, RpcError> {
    let query = text(&params, "query")?;
    let cwd = params.get("cwd").and_then(Value::as_str);
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, 200) as usize;
    cordis_spine::ensure_session_search();
    let hits: Vec<Value> = cordis_spine::session_search::search(&query, cwd, limit)
        .into_iter()
        .map(|h| {
            json!({
                "threadId": h.session_id,
                "title": h.title,
                "cwd": h.cwd,
                "snippet": h.snippet,
                "updatedAt": h.updated_at_unix.max(0) as u64 * 1000
            })
        })
        .collect();
    Ok(json!({ "hits": hits }))
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
        item["presetId"] = page_preset(&page);
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
    if let Some(item) = sessions.rename_archived(&id, &title) {
        return Ok(json!({
            "ok": true,
            "thread": archived_summary(&item, protocol::DEFAULT_WORKSPACE_ID)
        }));
    }
    // 不在第 1 页目录的历史里：按名册找（别的目录下的落盘会话）。
    let title = title.trim();
    if title.is_empty() {
        return Err(RpcError::invalid_params("title is required"));
    }
    let (roster, mut entry) = roster_entry(gateway, &id)?;
    roster
        .rename(&id, &entry.cwd, title)
        .map_err(|e| RpcError::app("rename_failed", e.to_string()))?;
    entry.title = title.to_string();
    Ok(json!({ "ok": true, "thread": roster_summary(&entry) }))
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
    if let Some(item) = sessions.remove_archived(&id) {
        return Ok(json!({
            "ok": true,
            "thread": archived_summary(&item, protocol::DEFAULT_WORKSPACE_ID)
        }));
    }
    let (roster, entry) = roster_entry(gateway, &id)?;
    roster
        .remove(&id, &entry.cwd)
        .map_err(|e| RpcError::app("delete_failed", e.to_string()))?;
    Ok(json!({ "ok": true, "thread": roster_summary(&entry) }))
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

/// `thread/start { cwd, title?, presetId? }`：在 `cwd` 另开一页（不动第 1 页、不切
/// 终端里正在看的页），回它的会话 id。预设在创建时定下：给了 `presetId` 就只在这一
/// 页切过去并记进这份会话的 `meta.json`（不改全局默认）；不给就沿用新页继承来的。
pub async fn start_at(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let cwd = PathBuf::from(text(&params, "cwd")?);
    let preset = params
        .get("presetId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    // 先验再开页：预设不存在就别留下一个空页。
    if let Some(id) = preset {
        let presets = gateway
            .ctx()
            .get::<AgentPresets>(AGENT_PRESETS)
            .ok_or_else(|| RpcError::app("unavailable", "预设服务没有挂载"))?;
        if presets.get(id).is_none() {
            return Err(RpcError::invalid_params(format!("没有预设 {id}")));
        }
    }
    let page = open_page(gateway, &cwd, None).await?;
    if let Some(id) = preset {
        if let Err(e) = apply_page_preset(&page, id) {
            // 开出来了但切不过去（预设坏了）：把这页关掉，不留半成品。
            if let Ok(tabs) = tabs(gateway) {
                let _ = tabs.close_session(&page.session_id()).await;
            }
            gateway.drop_page(&page.identity);
            return Err(RpcError::invalid_params(e));
        }
    }
    if let Some(title) = params.get("title").and_then(Value::as_str) {
        page.sessions()?.set_live_title(title);
    }
    Ok(json!({ "thread": open_summary(&page)? }))
}

/// `thread/open { threadId }`：把一份落盘会话开成一页（在它自己的 cwd 下），已经
/// 开着就回那一页。之后 `turn/start` 等都用这个 `threadId`。预设跟着会话走：
/// 这一页切回它 `meta.json` 记的那个（老会话没记就沿用新页继承的）。
pub async fn open(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let id = text(&params, "threadId")?;
    let page = open_thread(gateway, &id).await?;
    Ok(json!({ "thread": open_summary(&page)? }))
}

/// 线程级方法按需开页：`threadId` 是关着的落盘会话就先照 `thread/open` 开成一页，
/// 客户端不用自己先 open。已开着（含 `live`）什么也不做；认不得的 id 回 `not_found`。
pub async fn open_on_demand(gateway: &GatewayHandle, params: &Value) -> Result<(), RpcError> {
    let id = threads::thread_param(params);
    if threads::resolve(gateway, &id).is_err() {
        open_thread(gateway, &id).await?;
    }
    Ok(())
}

/// 把落盘会话开成一页（在它自己的 cwd 下，预设切回会话记的那个）；已开着就回那一页。
async fn open_thread(gateway: &GatewayHandle, id: &str) -> Result<Page, RpcError> {
    let _opening = gateway.lock_opening().await;
    if let Ok(page) = threads::resolve(gateway, id) {
        return Ok(page);
    }
    let entry = roster_entries(gateway)
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| RpcError::app("not_found", format!("thread {id} not found")))?;
    let page = open_page(gateway, &entry.cwd, Some(id)).await?;
    if let Some(preset) = entry.preset_id.as_deref() {
        // fail-open，和 `/resume` 一样：预设没了 / 坏了就用继承来的，只记一笔。
        if let Err(e) = apply_page_preset(&page, preset) {
            eprintln!("dock: thread/open preset `{preset}` not applied: {e}");
        }
    }
    Ok(page)
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

fn apply_page_preset(page: &Page, id: &str) -> Result<(), String> {
    let presets = page
        .ctx
        .get::<AgentPresets>(AGENT_PRESETS)
        .ok_or_else(|| "预设服务没有挂载".to_string())?;
    let applied = presets.pin(id)?;
    if let Ok(sessions) = page.sessions() {
        sessions.set_preset_id(Some(applied.id));
    }
    Ok(())
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
            "events": replayed_events(&id, &item)
        }));
    }
    // 别的目录下的会话：从跨目录名册读。
    let entry = roster_entries(gateway)
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| RpcError::app("not_found", format!("thread {id} not found")))?;
    let archived = fresh_roster(gateway).session(&entry.id, &entry.cwd);
    let (messages, events) = match &archived {
        Some(item) => (messages_of(&item.events, &id), replayed_events(&id, item)),
        None => (Vec::new(), Vec::new()),
    };
    Ok(json!({
        "thread": roster_summary(&entry),
        "messages": messages,
        "events": events
    }))
}

/// 关着的会话：落盘事件按真实时间回放成和开着的页同一种 `events`，客户端只要
/// 一条路径。用一份临时投影（自带 broadcast），不碰任何页、不推给订阅者。
fn replayed_events(id: &str, item: &ArchivedSession) -> Vec<Value> {
    let mut t = Transcript::new();
    t.set_thread_id(id);
    t.replay(&item.events, &item.times, &[]);
    t.history_since(0)
        .into_iter()
        .map(|e| e.as_history_item_as(id))
        .collect()
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
/// 名册里的一个落盘会话（任意目录），找不到就是 `not_found`。
fn roster_entry(
    gateway: &GatewayHandle,
    id: &str,
) -> Result<(std::sync::Arc<Roster>, RosterEntry), RpcError> {
    let roster = fresh_roster(gateway);
    let entry = roster
        .list()
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| RpcError::app("not_found", format!("thread {id} not found")))?;
    Ok((roster, entry))
}

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
        "presetId": entry.preset_id,
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
    item["presetId"] = page_preset(page);
    Ok(item)
}

/// 开着的页当前用的预设（预设按页，见 `AgentPresets::fork`）。没挂预设服务是 `null`。
fn page_preset(page: &Page) -> Value {
    page.ctx
        .get::<AgentPresets>(AGENT_PRESETS)
        .map(|p| json!(p.current_id()))
        .unwrap_or(Value::Null)
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
