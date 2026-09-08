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
mod tools_list;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cordis::{plugin, plugin_async, Context, Disposable, Inject, Plugin};

use crate::config::{self, McpServer, McpTransport};
use crate::names::{MCP, PRE_STEP, SESSIONS, TOOLS};
use crate::session::Sessions;
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{LogEvent, PreStep, ToolCall, ToolResult, ToolSpec};

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
                    let body =
                        discover::run_search(tools.as_ref(), mcp.as_deref(), &call.arguments);
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
                if next.enter {
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
    use crate::types::ToolCall;
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
                framing: crate::config::McpStdioFraming::Auto,
            },
            startup_timeout_sec: 1,
            enabled: true,
            oauth: crate::config::McpOAuthConfig {
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
}
