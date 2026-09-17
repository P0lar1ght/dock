//! MCP client: one plugin, tools register into `"tools"`. Connect/list fail-open.
//!
//! Protocol: offer `2026-07-28` first; fall back to initialize-era `2025-11-25`.
//! Transports: stdio (Content-Length or NDJSON) and Streamable HTTP (`url`).
//! Public names: `mcp_{server}__{tool}`. Sampler sees `search_tool` / `use_tool`
//! (Grok progressive disclosure); MCP extras stay registered for dispatch,
//! omitted from `specs_for_model` and occupancy. Enabled MCP tools bypass Agent
//! preset allowlists on execute.

mod credentials;
mod discover;
mod elicitation;
mod http;
mod incoming;
mod oauth;
mod protocol;
mod sse;
mod stdio;
mod tool_index;
mod tools_list;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cordis::{plugin, plugin_async, Context, Disposable, Inject, Plugin};

use crate::names::{MCP, PRE_STEP, SESSIONS, TOOLS};
use crate::session::Sessions;
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::config::{self, McpServer, McpTransport};
use cordis_base::types::{LogEvent, PreStep, ToolCall, ToolResult, ToolSpec};

use incoming::LiveHooks;

pub(super) type CallFn = std::sync::Arc<
    dyn Fn(String, ToolCall) -> crate::runtime::BoxFuture<'static, ToolResult> + Send + Sync,
>;

pub(super) type RelistFn = std::sync::Arc<
    dyn Fn() -> crate::runtime::BoxFuture<'static, Result<Vec<protocol::ListedTool>, String>>
        + Send
        + Sync,
>;

pub use discover::{SEARCH_TOOL_NAME, USE_TOOL_NAME};
pub use elicitation::{ElicitPrompt, Elicitation};
pub use protocol::{is_mcp_public_name, public_tool_name, raw_tool_name, split_mcp_public_name};

#[derive(Clone, Debug)]
pub struct McpToolStatus {
    pub name: String,
    pub public_name: String,
    pub description: String,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct McpStatus {
    pub name: String,
    pub command: String,
    pub ok: bool,
    pub enabled: bool,
    pub needs_auth: bool,
    pub detail: String,
    pub tools: Vec<McpToolStatus>,
}

/// 一次 [`Mcp::reload`] 做了什么。`added` 记的是「配置里新出现」，与 `failed`
/// 不互斥：新服务器连不上时两边都有它。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpReloadReport {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// 配置改过、或上次没连上这次重试成功的。
    pub reconnected: Vec<String>,
    /// 配置里被改成 `enabled = false` 的。
    pub disabled: Vec<String>,
    /// 配置一致且连接健康，一个字节都没碰。
    pub unchanged: usize,
    pub failed: Vec<(String, String)>,
}

impl McpReloadReport {
    pub fn is_noop(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.reconnected.is_empty()
            && self.disabled.is_empty()
            && self.failed.is_empty()
    }

    /// 给 TUI flash 用的一行中文摘要。
    pub fn summary(&self) -> String {
        if self.is_noop() {
            return "MCP 配置无变化".into();
        }
        let mut parts = Vec::new();
        let mut push = |label: &str, names: &[String]| {
            if !names.is_empty() {
                parts.push(format!("{label} {}", names.join("、")));
            }
        };
        push("新增", &self.added);
        push("移除", &self.removed);
        push("重连", &self.reconnected);
        push("禁用", &self.disabled);
        if !self.failed.is_empty() {
            let detail: Vec<String> = self
                .failed
                .iter()
                .map(|(name, err)| format!("{name}（{}）", clip(err, 40)))
                .collect();
            parts.push(format!("失败 {}", detail.join("、")));
        }
        format!("MCP 配置已重载：{}", parts.join("；"))
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if out.chars().count() < text.chars().count() {
        out.push('…');
    }
    out
}

/// 一个 slot 在重载时该怎么处理。判定见 [`plan_for`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SlotPlan {
    /// 配置里新出现。
    Add,
    /// 配置里整条没了。
    Remove,
    /// 配置里转成 `enabled = false`。
    Unplug,
    /// 配置内容变了（命令 / args / env / url / headers / timeout / oauth / 转成启用）。
    Reconnect,
    /// 配置一致但没连上（从没连过，或上次报错）—— 改完命令行重载就能自愈。
    Retry,
    /// 配置一致且连接健康 —— 不碰。重连 stdio 会杀掉子进程，
    /// 连带丢掉它自己的会话状态（cua-driver 的浏览器就是这样）。
    Keep,
}

/// `current` 是 `(当前配置, 是否已连上)`，两侧的 `None` 表示这个名字在
/// slots / 配置文件里不存在。`McpServer` 的 `Eq` 含 `enabled`，所以
/// 「禁用转启用」自然落进 `Reconnect`。
pub(super) fn plan_for(
    current: Option<(&McpServer, bool)>,
    desired: Option<&McpServer>,
) -> SlotPlan {
    match (current, desired) {
        (None, Some(_)) => SlotPlan::Add,
        (Some(_), None) => SlotPlan::Remove,
        (None, None) => SlotPlan::Keep,
        (Some((current, connected)), Some(desired)) => {
            if !desired.enabled {
                return if current.enabled {
                    SlotPlan::Unplug
                } else {
                    SlotPlan::Keep
                };
            }
            if current != desired {
                SlotPlan::Reconnect
            } else if connected {
                SlotPlan::Keep
            } else {
                SlotPlan::Retry
            }
        }
    }
}

struct Slot {
    config: McpServer,
    call: Option<CallFn>,
    relist: Option<RelistFn>,
    stop: Option<tokio::sync::watch::Sender<bool>>,
    listed: Vec<protocol::ListedTool>,
    disabled_tools: HashSet<String>,
    registered: HashMap<String, Disposable>,
    last_error: Option<String>,
}

struct Inner {
    slots: Vec<Slot>,
    tools: Option<Tools>,
}

#[derive(Clone)]
pub struct Mcp {
    ctx: Context,
    inner: Arc<Mutex<Inner>>,
    elicit: Elicitation,
    changed_tx: tokio::sync::mpsc::UnboundedSender<String>,
    dirty: Arc<AtomicBool>,
    announced: Arc<Mutex<HashMap<String, discover::ServerFingerprint>>>,
    reloading: Arc<AtomicBool>,
}

/// drop 时放开重载闸，panic 也不会把标志位卡死。
struct ReloadGuard(Arc<AtomicBool>);

impl Drop for ReloadGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Mcp {
    fn new(ctx: Context, tools: Option<Tools>, elicit: Elicitation) -> Self {
        let (changed_tx, changed_rx) = tokio::sync::mpsc::unbounded_channel();
        let mcp = Self {
            ctx,
            inner: Arc::new(Mutex::new(Inner {
                slots: Vec::new(),
                tools,
            })),
            elicit,
            changed_tx,
            dirty: Arc::new(AtomicBool::new(false)),
            announced: Arc::new(Mutex::new(HashMap::new())),
            reloading: Arc::new(AtomicBool::new(false)),
        };
        spawn_changed_refresh(mcp.clone(), changed_rx);
        mcp
    }

    pub fn elicitation(&self) -> Elicitation {
        self.elicit.clone()
    }

    pub fn list(&self) -> Vec<McpStatus> {
        self.inner
            .lock()
            .unwrap()
            .slots
            .iter()
            .map(slot_status)
            .collect()
    }

    pub fn catalog_ready(&self) -> bool {
        self.inner
            .lock()
            .unwrap()
            .slots
            .iter()
            .all(|s| !s.config.enabled || s.call.is_some() || s.last_error.is_some())
    }

    fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    fn maybe_inject_reminder(&self) {
        if !self.dirty.swap(false, Ordering::SeqCst) {
            return;
        }
        let summaries = discover::summaries_from_status(&self.list());
        let mut announced = self.announced.lock().unwrap();
        let Some(text) = discover::build_delta_reminder(&announced, &summaries) else {
            return;
        };
        *announced = discover::fingerprint_servers(&summaries);
        drop(announced);
        let Some(sessions) = self.ctx.get::<Sessions>(SESSIONS) else {
            return;
        };
        sessions.append(LogEvent::SystemReminder(format!(
            "<system-reminder>\n{text}\n</system-reminder>"
        )));
    }

    fn shutdown(&self) {
        let mut inner = self.inner.lock().unwrap();
        for slot in &mut inner.slots {
            stop_slot(slot);
            drop_registered(slot);
            slot.call = None;
            slot.relist = None;
        }
    }

    pub async fn set_server_enabled(&self, name: &str, enabled: bool) -> Result<(), String> {
        config::persist_mcp_server_enabled(name, enabled)?;
        let result = if enabled {
            self.plug_server(name).await
        } else {
            self.unplug_server(name)
        };
        self.mark_dirty();
        self.maybe_inject_reminder();
        result
    }

    pub async fn set_tool_enabled(
        &self,
        server: &str,
        tool: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let disabled = {
            let inner = self.inner.lock().unwrap();
            let Some(slot) = inner.slots.iter().find(|s| s.config.name == server) else {
                return Err(format!("unknown MCP server {server}"));
            };
            let mut names = slot.disabled_tools.clone();
            if enabled {
                names.remove(tool);
            } else {
                names.insert(tool.to_string());
            }
            let mut list: Vec<String> = names.into_iter().collect();
            list.sort();
            list
        };
        config::persist_disabled_mcp_tools(server, &disabled)?;
        self.apply_tool_enabled(server, tool, enabled)?;
        self.mark_dirty();
        self.maybe_inject_reminder();
        Ok(())
    }

    /// 重读配置文件并与当前 slots 对账：新增 / 移除 / 改动 / 禁用都就地生效，
    /// 配置一致且连着的服务器一个字节都不碰。目录变化走和 `/mcps` 开关同一条
    /// `<system-reminder>` 通道，模型下一轮就知道它刚配的服务器上线了。
    ///
    /// 同时只允许一个重载在跑；第二个调用立刻拿到 `Err`。
    pub async fn reload(&self) -> Result<McpReloadReport, String> {
        if self.reloading.swap(true, Ordering::SeqCst) {
            return Err("MCP 配置正在重载中".into());
        }
        let _guard = ReloadGuard(self.reloading.clone());
        Ok(self.reload_inner().await)
    }

    async fn reload_inner(&self) -> McpReloadReport {
        let desired = config::load_mcp_servers();
        let disabled_map = config::load_disabled_mcp_tools();
        let mut report = McpReloadReport::default();
        let mut to_connect: Vec<(McpServer, SlotPlan)> = Vec::new();
        {
            let mut inner = self.inner.lock().unwrap();
            let tools = inner.tools.clone();
            let by_name: HashMap<&str, &McpServer> =
                desired.iter().map(|s| (s.name.as_str(), s)).collect();

            inner.slots.retain_mut(|slot| {
                let want = by_name.get(slot.config.name.as_str()).copied();
                let plan = plan_for(Some((&slot.config, slot.call.is_some())), want);
                let Some(want) = want else {
                    // Remove：配置里整条没了。
                    drop_registered(slot);
                    stop_slot(slot);
                    report.removed.push(slot.config.name.clone());
                    return false;
                };
                let name = slot.config.name.clone();
                // 先落新配置，`connect_slot` 只写连接结果、不碰 `slot.config`。
                slot.config = want.clone();
                let want_disabled: HashSet<String> = disabled_map
                    .get(&name)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                let disabled_changed = slot.disabled_tools != want_disabled;
                slot.disabled_tools = want_disabled;
                match plan {
                    SlotPlan::Unplug => {
                        drop_registered(slot);
                        stop_slot(slot);
                        slot.call = None;
                        slot.relist = None;
                        slot.last_error = None;
                        report.disabled.push(name);
                    }
                    SlotPlan::Keep => {
                        report.unchanged += 1;
                        // `[disabled_mcp_tools.*]` 的编辑也要跟着生效；
                        // Reconnect / Retry 走 connect_slot，那边自会 sync。
                        if disabled_changed && slot.config.enabled {
                            if let Some(tools) = tools.as_ref() {
                                sync_listed(slot, tools);
                            }
                        }
                    }
                    SlotPlan::Reconnect | SlotPlan::Retry => {
                        to_connect.push((want.clone(), plan));
                    }
                    // 走不到：`plan_for(Some(..), Some(..))` 不返回这两个。
                    // 但这里正持着 `inner` 的锁，panic 会把整个 MCP 子系统毒死，
                    // 所以留原样并记一条，不 `unreachable!`。
                    SlotPlan::Add | SlotPlan::Remove => {
                        tracing::warn!(server = name.as_str(), ?plan, "MCP 重载遇到意外的 plan");
                    }
                }
                true
            });

            for server in &desired {
                if inner.slots.iter().any(|s| s.config.name == server.name) {
                    continue;
                }
                let disabled_tools = disabled_map
                    .get(&server.name)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                inner.slots.push(Slot {
                    config: server.clone(),
                    call: None,
                    relist: None,
                    stop: None,
                    listed: Vec::new(),
                    disabled_tools,
                    registered: HashMap::new(),
                    last_error: None,
                });
                report.added.push(server.name.clone());
                if server.enabled {
                    to_connect.push((server.clone(), SlotPlan::Add));
                }
            }

            // 让 `/mcps` 的行序跟着配置文件走，重载后不无故洗牌。
            let order: HashMap<&str, usize> = desired
                .iter()
                .enumerate()
                .map(|(i, s)| (s.name.as_str(), i))
                .collect();
            inner.slots.sort_by_key(|s| {
                order
                    .get(s.config.name.as_str())
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        }

        // 串行连接的话，3 个坏服务器 × 默认 30s startup_timeout 就是 90s 卡死。
        // `connect_slot` 全程不跨 await 持锁，可以并发。
        let results = futures_util::future::join_all(to_connect.into_iter().map(
            |(server, plan)| async move {
                let result = self.connect_slot(&server).await;
                (server.name, plan, result)
            },
        ))
        .await;
        for (name, plan, result) in results {
            match result {
                Ok(()) if plan == SlotPlan::Add => {}
                Ok(()) => report.reconnected.push(name),
                Err(e) => report.failed.push((name, e)),
            }
        }

        if !report.is_noop() {
            self.mark_dirty();
            self.maybe_inject_reminder();
        }
        report
    }

    /// Browser PKCE for an HTTP MCP server. Reconnects when the server is enabled.
    pub async fn authenticate(&self, name: &str) -> Result<(), String> {
        let config = {
            let inner = self.inner.lock().unwrap();
            let slot = inner
                .slots
                .iter()
                .find(|s| s.config.name == name)
                .ok_or_else(|| format!("unknown MCP server {name}"))?;
            if !matches!(slot.config.transport, McpTransport::Http { .. }) {
                return Err("stdio MCP 服务器不需要浏览器登录".into());
            }
            slot.config.clone()
        };
        match oauth::browser_login(&config).await {
            Ok(()) => {
                if config.enabled {
                    let result = self.connect_slot(&config).await;
                    self.mark_dirty();
                    self.maybe_inject_reminder();
                    result
                } else {
                    Ok(())
                }
            }
            Err(e) => {
                let mut inner = self.inner.lock().unwrap();
                if let Some(slot) = inner.slots.iter_mut().find(|s| s.config.name == name) {
                    slot.last_error = Some(e.clone());
                }
                Err(e)
            }
        }
    }

    async fn plug_server(&self, name: &str) -> Result<(), String> {
        let config = {
            let mut inner = self.inner.lock().unwrap();
            let Some(slot) = inner.slots.iter_mut().find(|s| s.config.name == name) else {
                return Err(format!("unknown MCP server {name}"));
            };
            slot.config.enabled = true;
            slot.config.clone()
        };
        self.connect_slot(&config).await
    }

    fn unplug_server(&self, name: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let Some(slot) = inner.slots.iter_mut().find(|s| s.config.name == name) else {
            return Err(format!("unknown MCP server {name}"));
        };
        drop_registered(slot);
        stop_slot(slot);
        slot.call = None;
        slot.relist = None;
        slot.config.enabled = false;
        slot.last_error = None;
        drop(inner);
        self.mark_dirty();
        Ok(())
    }

    fn apply_tool_enabled(&self, server: &str, tool: &str, enabled: bool) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let tools = inner.tools.clone();
        let Some(slot) = inner.slots.iter_mut().find(|s| s.config.name == server) else {
            return Err(format!("unknown MCP server {server}"));
        };
        if enabled {
            slot.disabled_tools.remove(tool);
        } else {
            slot.disabled_tools.insert(tool.to_string());
        }
        let public = public_tool_name(server, tool);
        if !enabled {
            if let Some(d) = slot.registered.remove(&public) {
                d.dispose_sync();
            }
            drop(inner);
            self.mark_dirty();
            return Ok(());
        }
        if slot.config.enabled {
            if let Some(tools) = tools.as_ref() {
                upsert_one(slot, tools, &public);
            }
        }
        drop(inner);
        self.mark_dirty();
        Ok(())
    }

    async fn connect_slot(&self, config: &McpServer) -> Result<(), String> {
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(slot) = inner
                .slots
                .iter_mut()
                .find(|s| s.config.name == config.name)
            {
                stop_slot(slot);
            }
        }
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let hooks = LiveHooks {
            server: config.name.clone(),
            elicit: self.elicit.clone(),
            tools_changed: self.changed_tx.clone(),
        };
        let result = connect_and_list(config, hooks, stop_rx).await;
        let mut inner = self.inner.lock().unwrap();
        let tools = inner.tools.clone();
        let Some(slot) = inner
            .slots
            .iter_mut()
            .find(|s| s.config.name == config.name)
        else {
            let _ = stop_tx.send(true);
            return Ok(());
        };
        match result {
            Ok((listed, call, relist)) => {
                slot.listed = listed;
                slot.call = Some(call);
                slot.relist = Some(relist);
                slot.stop = Some(stop_tx);
                slot.last_error = None;
                if let Some(tools) = tools {
                    sync_listed(slot, &tools);
                }
                drop(inner);
                self.mark_dirty();
                Ok(())
            }
            Err(e) => {
                drop_registered(slot);
                stop_slot(slot);
                slot.call = None;
                slot.relist = None;
                slot.last_error = Some(e.clone());
                let _ = stop_tx.send(true);
                drop(inner);
                self.mark_dirty();
                Err(e)
            }
        }
    }

    async fn refresh_list(&self, name: &str) {
        let relist = {
            let inner = self.inner.lock().unwrap();
            inner
                .slots
                .iter()
                .find(|s| s.config.name == name)
                .and_then(|s| s.relist.clone())
        };
        let Some(relist) = relist else {
            return;
        };
        let listed = match relist().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(server = name, "MCP tools/list refresh failed: {e}");
                return;
            }
        };
        {
            let mut inner = self.inner.lock().unwrap();
            let tools = inner.tools.clone();
            let Some(slot) = inner.slots.iter_mut().find(|s| s.config.name == name) else {
                return;
            };
            slot.listed = listed;
            if let (Some(tools), Some(_)) = (tools, slot.call.as_ref()) {
                if slot.config.enabled {
                    sync_listed(slot, &tools);
                } else {
                    drop_registered(slot);
                }
            }
        }
        self.mark_dirty();
        self.maybe_inject_reminder();
    }
}

fn drop_registered(slot: &mut Slot) {
    for (_, d) in slot.registered.drain() {
        d.dispose_sync();
    }
}

fn stop_slot(slot: &mut Slot) {
    if let Some(tx) = slot.stop.take() {
        let _ = tx.send(true);
    }
}

fn spawn_changed_refresh(mcp: Mcp, mut rx: tokio::sync::mpsc::UnboundedReceiver<String>) {
    tokio::spawn(async move {
        loop {
            let Some(first) = rx.recv().await else {
                return;
            };
            let mut names = HashSet::from([first]);
            tokio::time::sleep(tools_list::coalesce_delay()).await;
            while let Ok(n) = rx.try_recv() {
                names.insert(n);
            }
            for name in names {
                mcp.refresh_list(&name).await;
            }
        }
    });
}

fn sync_listed(slot: &mut Slot, tools: &Tools) {
    if slot.call.is_none() {
        drop_registered(slot);
        return;
    }
    let want: HashSet<String> = slot
        .listed
        .iter()
        .filter(|(public, ..)| !slot.disabled_tools.contains(&raw_tool_name(public)))
        .map(|(public, ..)| public.clone())
        .collect();
    let stale: Vec<String> = slot
        .registered
        .keys()
        .filter(|name| !want.contains(*name))
        .cloned()
        .collect();
    for name in stale {
        if let Some(d) = slot.registered.remove(&name) {
            d.dispose_sync();
        }
    }
    for (public, _, _) in slot.listed.clone() {
        if want.contains(&public) {
            upsert_one(slot, tools, &public);
        }
    }
}

fn upsert_one(slot: &mut Slot, tools: &Tools, public: &str) {
    let Some(call) = slot.call.clone() else {
        return;
    };
    let Some((_, description, parameters_json)) =
        slot.listed.iter().find(|(name, ..)| name == public)
    else {
        return;
    };
    if slot.disabled_tools.contains(&raw_tool_name(public)) {
        return;
    }
    let pname = public.to_string();
    let body: ToolBody = std::sync::Arc::new(move |c| {
        let call = call.clone();
        let pname = pname.clone();
        Box::pin(async move { call(pname, c).await })
    });
    let spec = ToolSpec {
        name: public.to_string(),
        description: description.clone(),
        parameters_json: parameters_json.clone(),
    };
    if let Some(d) = tools.patch_mcp(spec.clone(), body.clone()) {
        let _ = slot.registered.insert(public.to_string(), d);
        return;
    }
    match tools.register_mcp(spec, body) {
        Ok(d) => {
            slot.registered.insert(public.to_string(), d);
        }
        Err(e) => {
            tracing::warn!(tool = public, "MCP register failed: {e}");
        }
    }
}

fn slot_status(slot: &Slot) -> McpStatus {
    let tools: Vec<McpToolStatus> = slot
        .listed
        .iter()
        .map(|(public, desc, _)| {
            let raw = raw_tool_name(public);
            McpToolStatus {
                name: raw.clone(),
                public_name: public.clone(),
                description: desc.clone(),
                enabled: !slot.disabled_tools.contains(&raw),
            }
        })
        .collect();
    let enabled_count = tools.iter().filter(|t| t.enabled).count();
    let tools_detail = if tools.is_empty() {
        if slot.config.enabled {
            "没有工具（服务器可能未连接）".into()
        } else {
            String::new()
        }
    } else if enabled_count == tools.len() {
        format!("{} 个工具", tools.len())
    } else {
        format!("{} 个工具（{} 个已启用）", tools.len(), enabled_count)
    };
    let detail = if slot.config.enabled {
        if let Some(err) = &slot.last_error {
            err.clone()
        } else {
            tools_detail
        }
    } else {
        tools_detail
    };
    McpStatus {
        name: slot.config.name.clone(),
        command: slot.config.endpoint().to_string(),
        ok: slot.config.enabled && slot.call.is_some(),
        enabled: slot.config.enabled,
        needs_auth: slot.last_error.as_deref().is_some_and(oauth::is_auth_error),
        detail,
        tools,
    }
}

pub fn mcp_client() -> Plugin {
    plugin_async(
        "mcp-client",
        Inject::from([TOOLS]),
        |ctx, _: &()| async move {
            let configured = config::load_mcp_servers();
            let disabled_map = config::load_disabled_mcp_tools();
            let tools = ctx.require::<Tools>(TOOLS)?;
            let mcp = Mcp::new(
                ctx.clone(),
                Some((*tools).clone()),
                Elicitation::new(ctx.clone()),
            );
            let search_body: ToolBody = std::sync::Arc::new(|call| {
                Box::pin(async move {
                    let Some(exec) = crate::tools::exec_ctx() else {
                        return tool_result(call, "search_tool: missing exec context");
                    };
                    let Some(tools) = exec.get::<Tools>(TOOLS) else {
                        return tool_result(call, "search_tool: tools unavailable");
                    };
                    let mcp = exec.get::<Mcp>(MCP);
                    // schema 是否已在上下文里，以本 agent 自己的模型历史为准：
                    // 子代理 isolate 看到的是它那份 `"sessions"`。
                    let seen = exec
                        .get::<Sessions>(SESSIONS)
                        .map(|s| discover::schemas_in_context(&s.model_history()))
                        .unwrap_or_default();
                    let body = discover::run_search(
                        tools.as_ref(),
                        mcp.as_deref(),
                        &seen,
                        &call.arguments,
                    );
                    tool_result(call, body)
                })
            });
            let use_body: ToolBody = std::sync::Arc::new(|call| {
                Box::pin(async move {
                    let Some(exec) = crate::tools::exec_ctx() else {
                        return tool_result(call, "use_tool: missing exec context");
                    };
                    let Some(tools) = exec.get::<Tools>(TOOLS) else {
                        return tool_result(call, "use_tool: tools unavailable");
                    };
                    discover::run_use_tool(tools.as_ref(), &exec, call).await
                })
            });
            own_registered(
                &ctx,
                vec![
                    tools.register(discover::search_spec(), search_body)?,
                    tools.register(discover::use_spec(), use_body)?,
                ],
            )?;
            {
                let mut inner = mcp.inner.lock().unwrap();
                for server in configured {
                    let disabled_tools = disabled_map
                        .get(&server.name)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .collect();
                    inner.slots.push(Slot {
                        config: server,
                        call: None,
                        relist: None,
                        stop: None,
                        listed: Vec::new(),
                        disabled_tools,
                        registered: HashMap::new(),
                        last_error: None,
                    });
                }
            }
            let to_connect: Vec<McpServer> = mcp
                .inner
                .lock()
                .unwrap()
                .slots
                .iter()
                .filter(|s| s.config.enabled)
                .map(|s| s.config.clone())
                .collect();
            for server in to_connect {
                let _ = mcp.connect_slot(&server).await;
            }
            ctx.provide(MCP, mcp.clone())?;
            let mcp_pre = mcp.clone();
            let _ = ctx.on_waterfall(PRE_STEP, move |step: PreStep, args| {
                let next = args.next::<PreStep>().unwrap_or(step);
                // Main session only: the catalog notice is drained once. A
                // child turn would spend it on the parent's transcript, and the
                // user would never see why `/mcps` changed.
                if next.enter && next.is_main_session() {
                    mcp_pre.maybe_inject_reminder();
                }
                next
            });
            let mcp_drop = mcp;
            ctx.effect("mcp.servers", move |scope| {
                scope.own(Disposable::from_fn(move || mcp_drop.shutdown()));
                Ok(())
            })?;
            Ok(None)
        },
    )
}

async fn connect_and_list(
    server: &McpServer,
    hooks: LiveHooks,
    stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(Vec<protocol::ListedTool>, CallFn, RelistFn), String> {
    match &server.transport {
        McpTransport::Stdio { .. } => stdio::connect(server, hooks, stop).await,
        McpTransport::Http { .. } => http::connect(server, hooks, stop).await,
    }
}

/// Empty MCP status plugin when we only need the service key for `/mcps`.
#[allow(dead_code)]
pub fn mcp_empty() -> Plugin {
    plugin("mcp-client", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(
            MCP,
            Mcp::new(ctx.clone(), None, Elicitation::new(ctx.clone())),
        )?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::{SESSIONS, TOOLS};
    use crate::session::Sessions;
    use crate::tools::tool_result;
    use cordis_base::types::ToolCall;
    use std::sync::Arc;

    fn stub_call() -> CallFn {
        Arc::new(|_, c| Box::pin(async move { tool_result(c, "ok") }))
    }

    fn fake_config(name: &str) -> McpServer {
        McpServer {
            name: name.into(),
            transport: McpTransport::Stdio {
                command: "true".into(),
                args: Vec::new(),
                env: Default::default(),
                framing: cordis_base::config::McpStdioFraming::Auto,
            },
            startup_timeout_sec: 1,
            enabled: true,
            oauth: cordis_base::config::McpOAuthConfig {
                client_id: None,
                client_secret: None,
                scopes: Vec::new(),
                callback_port: None,
            },
        }
    }

    fn attach_fake(mcp: &Mcp, name: &str, raw_tools: &[(&str, &str)]) {
        let listed: Vec<protocol::ListedTool> = raw_tools
            .iter()
            .map(|(raw, desc)| {
                (
                    public_tool_name(name, raw),
                    (*desc).to_string(),
                    r#"{"type":"object"}"#.into(),
                )
            })
            .collect();
        let mut inner = mcp.inner.lock().unwrap();
        if !inner.slots.iter().any(|s| s.config.name == name) {
            inner.slots.push(Slot {
                config: fake_config(name),
                call: None,
                relist: None,
                stop: None,
                listed: Vec::new(),
                disabled_tools: HashSet::new(),
                registered: HashMap::new(),
                last_error: None,
            });
        }
        let tools = inner.tools.clone();
        let slot = inner
            .slots
            .iter_mut()
            .find(|s| s.config.name == name)
            .unwrap();
        slot.listed = listed;
        slot.call = Some(stub_call());
        slot.config.enabled = true;
        if let Some(tools) = tools {
            sync_listed(slot, &tools);
        }
        drop(inner);
        mcp.mark_dirty();
    }

    fn spec_names(tools: &Tools) -> Vec<String> {
        tools.specs().into_iter().map(|s| s.name).collect()
    }

    /// `DOCK_HOME` + cwd 都指向空临时目录，再把 `body` 写成用户级 catalog。
    /// 返回的 guard 与 cwd 目录要一直持到用例结束。
    ///
    /// 同时关掉内置 cua-driver 行：它是「发现得到就注入」，开发机上装了 driver
    /// 的话每个 reload 用例都会真的去拉守护进程。
    fn isolated_config(body: &str) -> (cordis_base::test_env::EnvScope, tempfile::TempDir) {
        let cwd = tempfile::tempdir().unwrap();
        let env = cordis_base::test_env::scoped()
            .home()
            .cwd(cwd.path())
            .set(cordis_base::cua::DRIVER_ENV, "off");
        std::fs::write(config::dock_home().join("config.toml"), body).unwrap();
        (env, cwd)
    }

    /// 与 `fake_config("probe")` 完全等价的一行配置 —— `plan_for` 必须判成 `Keep`。
    const PROBE_TOML: &str = r#"
[mcp_servers.probe]
command = "true"
framing = "auto"
startup_timeout_sec = 1
"#;

    fn slot_call(mcp: &Mcp, name: &str) -> Option<CallFn> {
        mcp.inner
            .lock()
            .unwrap()
            .slots
            .iter()
            .find(|s| s.config.name == name)
            .and_then(|s| s.call.clone())
    }

    async fn boot() -> (cordis::Context, Mcp, Tools) {
        let ctx = cordis::Context::new();
        crate::install_without_llm(&ctx).await.unwrap();
        let tools = (*ctx.require::<Tools>(TOOLS).unwrap()).clone();
        let mcp = Mcp::new(
            ctx.clone(),
            Some(tools.clone()),
            Elicitation::new(ctx.clone()),
        );
        (ctx, mcp, tools)
    }

    #[tokio::test]
    async fn toggle_unregisters_and_reregisters_without_reshuffling_prefix() {
        let (_ctx, mcp, tools) = boot().await;
        tools
            .register(
                ToolSpec {
                    name: "zeta".into(),
                    description: "z".into(),
                    parameters_json: r#"{"type":"object"}"#.into(),
                },
                Arc::new(|c| Box::pin(async move { tool_result(c, "z") })),
            )
            .unwrap();
        let prefix = spec_names(&tools);
        attach_fake(&mcp, "probe", &[("ping", "ping"), ("pong", "pong")]);
        let with_mcp = spec_names(&tools);
        assert_eq!(&with_mcp[..prefix.len()], prefix.as_slice());
        assert_eq!(
            &with_mcp[prefix.len()..],
            ["mcp_probe__ping", "mcp_probe__pong"]
        );

        mcp.unplug_server("probe").unwrap();
        assert_eq!(spec_names(&tools), prefix);
        let blocked = tools
            .execute(ToolCall {
                id: "1".into(),
                name: "mcp_probe__ping".into(),
                arguments: "{}".into(),
            })
            .await;
        assert_eq!(blocked.content, "MCP 工具未启用或已关闭");

        attach_fake(&mcp, "probe", &[("ping", "ping"), ("pong", "pong")]);
        let again = spec_names(&tools);
        assert_eq!(&again[..prefix.len()], prefix.as_slice());
        assert_eq!(
            &again[prefix.len()..],
            ["mcp_probe__ping", "mcp_probe__pong"]
        );
        let ok = tools
            .execute(ToolCall {
                id: "2".into(),
                name: "mcp_probe__ping".into(),
                arguments: "{}".into(),
            })
            .await;
        assert_eq!(ok.content, "ok");

        mcp.apply_tool_enabled("probe", "ping", false).unwrap();
        let after_one = spec_names(&tools);
        assert_eq!(&after_one[..prefix.len()], prefix.as_slice());
        assert_eq!(&after_one[prefix.len()..], ["mcp_probe__pong"]);
        let model: Vec<String> = tools
            .specs_for_model()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert!(
            !model.iter().any(|n| n.starts_with("mcp_")),
            "sampler tools must hide MCP extras: {model:?}"
        );
    }

    #[tokio::test]
    async fn toggle_appends_server_delta_reminder_not_tool_names() {
        let (ctx, mcp, tools) = boot().await;
        attach_fake(&mcp, "probe", &[("ping", "ping")]);
        mcp.maybe_inject_reminder();
        mcp.unplug_server("probe").unwrap();
        mcp.maybe_inject_reminder();
        let events = ctx.require::<Sessions>(SESSIONS).unwrap().events();
        let reminders: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                LogEvent::SystemReminder(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            reminders
                .iter()
                .any(|t| t.contains("MCP 服务器已连接") && t.contains("probe")),
            "{reminders:?}"
        );
        assert!(
            reminders
                .iter()
                .any(|t| t.contains("MCP 服务器已断开：probe")),
            "{reminders:?}"
        );
        assert!(
            reminders.iter().all(|t| !t.contains("mcp_probe__ping")),
            "{reminders:?}"
        );
        assert!(
            reminders.iter().all(|t| t.contains("<system-reminder>")),
            "{reminders:?}"
        );
        assert!(!tools.is_mcp("mcp_probe__ping"));
    }

    #[test]
    fn plan_for_covers_the_reconcile_table() {
        let base = fake_config("probe");
        let mut disabled = base.clone();
        disabled.enabled = false;
        let mut changed = base.clone();
        changed.startup_timeout_sec = 9;

        assert_eq!(plan_for(None, Some(&base)), SlotPlan::Add);
        assert_eq!(plan_for(Some((&base, true)), None), SlotPlan::Remove);
        assert_eq!(
            plan_for(Some((&base, true)), Some(&disabled)),
            SlotPlan::Unplug
        );
        assert_eq!(
            plan_for(Some((&disabled, false)), Some(&disabled)),
            SlotPlan::Keep,
            "本来就禁用、配置也还是禁用，没事可做"
        );
        assert_eq!(
            plan_for(Some((&base, true)), Some(&changed)),
            SlotPlan::Reconnect
        );
        assert_eq!(
            plan_for(Some((&disabled, false)), Some(&base)),
            SlotPlan::Reconnect,
            "禁用转启用要连上来"
        );
        assert_eq!(
            plan_for(Some((&base, true)), Some(&base)),
            SlotPlan::Keep,
            "配置没变且连着就别动它 —— 重连 stdio 会杀掉子进程状态"
        );
        assert_eq!(
            plan_for(Some((&base, false)), Some(&base)),
            SlotPlan::Retry,
            "配置没变但没连上要重试，这样改完命令行重载就能自愈"
        );
    }

    #[tokio::test]
    async fn reload_picks_up_a_server_added_to_config() {
        let (_env, _cwd) = isolated_config(
            r#"
[mcp_servers.later]
command = "true"
enabled = false
"#,
        );
        let (_ctx, mcp, _tools) = boot().await;
        assert!(mcp.list().is_empty(), "Mcp::new 不读配置，起点必须是空的");

        let report = mcp.reload().await.unwrap();
        assert_eq!(report.added, ["later"]);
        assert!(report.failed.is_empty(), "{report:?}");
        let names: Vec<String> = mcp.list().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["later"], "重载后配置里的新服务器要出现在 /mcps 里");
    }

    #[tokio::test]
    async fn reload_drops_a_server_removed_from_config() {
        let (_env, _cwd) = isolated_config("");
        let (_ctx, mcp, tools) = boot().await;
        attach_fake(&mcp, "probe", &[("ping", "ping")]);
        assert!(tools.is_mcp("mcp_probe__ping"));

        let report = mcp.reload().await.unwrap();
        assert_eq!(report.removed, ["probe"]);
        assert!(mcp.list().is_empty(), "配置里没了就不该还列在 /mcps 上");
        assert!(
            !tools.is_mcp("mcp_probe__ping"),
            "它的工具也要从 \"tools\" 注销"
        );
    }

    #[tokio::test]
    async fn reload_keeps_a_healthy_unchanged_server_connected() {
        let (_env, _cwd) = isolated_config(PROBE_TOML);
        let (_ctx, mcp, _tools) = boot().await;
        attach_fake(&mcp, "probe", &[("ping", "ping")]);
        let before = slot_call(&mcp, "probe").expect("attach_fake 应当装上 call");

        let report = mcp.reload().await.unwrap();
        assert_eq!(report.unchanged, 1, "{report:?}");
        assert!(report.is_noop(), "配置一致时重载应当什么都不做：{report:?}");
        let after = slot_call(&mcp, "probe").expect("连接不该被拆掉");
        assert!(
            Arc::ptr_eq(&before, &after),
            "健康且配置没变的服务器不能重连 —— 重连 stdio 会杀掉它的子进程状态"
        );
    }

    #[tokio::test]
    async fn reload_reports_connect_failure_without_poisoning_the_slot() {
        // `true` 立刻退出，握不上手；1s 预算让失败来得快。
        let (_env, _cwd) = isolated_config(
            r#"
[mcp_servers.broken]
command = "true"
startup_timeout_sec = 1
"#,
        );
        let (_ctx, mcp, _tools) = boot().await;

        let report = mcp.reload().await.unwrap();
        assert_eq!(report.added, ["broken"]);
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(report.failed[0].0, "broken");
        let status = mcp.list();
        assert_eq!(status.len(), 1, "fail-open：连不上也要留在列表里");
        assert!(!status[0].ok);
        assert!(!status[0].detail.is_empty(), "错误详情要显示出来");
        assert!(
            report.summary().contains("失败 broken"),
            "{}",
            report.summary()
        );
    }

    #[tokio::test]
    async fn reload_announces_the_delta_to_the_model() {
        let (_env, _cwd) = isolated_config("");
        let (ctx, mcp, _tools) = boot().await;
        attach_fake(&mcp, "probe", &[("ping", "ping")]);
        mcp.maybe_inject_reminder();

        mcp.reload().await.unwrap();
        let events = ctx.require::<Sessions>(SESSIONS).unwrap().events();
        let reminders: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                LogEvent::SystemReminder(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            reminders
                .iter()
                .any(|t| t.contains("MCP 服务器已断开：probe")),
            "重载要复用目录 delta 通道告诉模型：{reminders:?}"
        );
        assert!(
            reminders.iter().all(|t| !t.contains("mcp_probe__ping")),
            "delta 只报服务器，不铺工具名：{reminders:?}"
        );
    }

    #[tokio::test]
    async fn concurrent_reloads_do_not_overlap() {
        let (_env, _cwd) = isolated_config(
            r#"
[mcp_servers.slow]
command = "sleep"
args = ["5"]
startup_timeout_sec = 2
"#,
        );
        let (_ctx, mcp, _tools) = boot().await;

        let first = tokio::spawn({
            let mcp = mcp.clone();
            async move { mcp.reload().await }
        });
        // 等第一个把闸推上去（连接要 2s，窗口很宽）。
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let second = mcp.reload().await;
        assert_eq!(second.unwrap_err(), "MCP 配置正在重载中");
        assert!(first.await.unwrap().is_ok());

        // 闸要放开，下一次还能重载。
        assert!(mcp.reload().await.is_ok());
    }
}
