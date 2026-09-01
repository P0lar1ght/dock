//! MCP client: one plugin, tools register into `"tools"`. Connect/list fail-open.
//!
//! Protocol: offer `2026-07-28` first; fall back to initialize-era `2025-11-25`.
//! Transports: stdio (Content-Length) and Streamable HTTP (`url`).
//! Public names: `mcp_{server}__{tool}`. Enabled tools bypass Agent preset allowlists.

mod credentials;
mod elicitation;
mod http;
mod incoming;
mod oauth;
mod protocol;
mod sse;
mod stdio;
mod tools_list;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use cordis::{plugin, plugin_async, Disposable, Inject, Plugin};

use crate::config::{self, McpServer, McpTransport};
use crate::names::{MCP, TOOLS};
use crate::tools::{ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

use incoming::LiveHooks;

pub(super) type CallFn = std::sync::Arc<
    dyn Fn(String, ToolCall) -> crate::runtime::BoxFuture<'static, ToolResult> + Send + Sync,
>;

pub(super) type RelistFn = std::sync::Arc<
    dyn Fn() -> crate::runtime::BoxFuture<'static, Result<Vec<protocol::ListedTool>, String>>
        + Send
        + Sync,
>;

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
    inner: Arc<Mutex<Inner>>,
    elicit: Elicitation,
    changed_tx: tokio::sync::mpsc::UnboundedSender<String>,
}

impl Mcp {
    fn new(tools: Option<Tools>, elicit: Elicitation) -> Self {
        let (changed_tx, changed_rx) = tokio::sync::mpsc::unbounded_channel();
        let mcp = Self {
            inner: Arc::new(Mutex::new(Inner {
                slots: Vec::new(),
                tools,
            })),
            elicit,
            changed_tx,
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
        if !enabled {
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
            return Ok(());
        }
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

    pub async fn set_tool_enabled(
        &self,
        server: &str,
        tool: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let (disabled, server_on) = {
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
            (list, slot.config.enabled)
        };
        config::persist_disabled_mcp_tools(server, &disabled)?;
        {
            let mut inner = self.inner.lock().unwrap();
            let Some(slot) = inner.slots.iter_mut().find(|s| s.config.name == server) else {
                return Err(format!("unknown MCP server {server}"));
            };
            if enabled {
                slot.disabled_tools.remove(tool);
            } else {
                slot.disabled_tools.insert(tool.to_string());
            }
        }
        if server_on {
            self.sync_registrations(server);
        }
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
                    self.connect_slot(&config).await
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
                drop_registered(slot);
                slot.listed = listed;
                slot.call = Some(call);
                slot.relist = Some(relist);
                slot.stop = Some(stop_tx);
                slot.last_error = None;
                if let Some(tools) = tools {
                    register_enabled(slot, &tools);
                }
                Ok(())
            }
            Err(e) => {
                drop_registered(slot);
                stop_slot(slot);
                slot.call = None;
                slot.relist = None;
                slot.last_error = Some(e.clone());
                let _ = stop_tx.send(true);
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
        let mut inner = self.inner.lock().unwrap();
        let tools = inner.tools.clone();
        let Some(slot) = inner.slots.iter_mut().find(|s| s.config.name == name) else {
            return;
        };
        drop_registered(slot);
        slot.listed = listed;
        if let (Some(tools), Some(_)) = (tools, slot.call.as_ref()) {
            if slot.config.enabled {
                register_enabled(slot, &tools);
            }
        }
    }

    fn sync_registrations(&self, server: &str) {
        let mut inner = self.inner.lock().unwrap();
        let tools = inner.tools.clone();
        let Some(slot) = inner.slots.iter_mut().find(|s| s.config.name == server) else {
            return;
        };
        drop_registered(slot);
        if slot.config.enabled {
            if let (Some(tools), Some(_)) = (tools, slot.call.as_ref()) {
                register_enabled(slot, &tools);
            }
        }
    }
}

fn drop_registered(slot: &mut Slot) {
    slot.registered.clear();
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

fn register_enabled(slot: &mut Slot, tools: &Tools) {
    let Some(call) = slot.call.clone() else {
        return;
    };
    for (public, description, parameters_json) in &slot.listed {
        let raw = raw_tool_name(public);
        if slot.disabled_tools.contains(&raw) {
            continue;
        }
        let call = call.clone();
        let pname = public.clone();
        let body: ToolBody = std::sync::Arc::new(move |c| {
            let call = call.clone();
            let pname = pname.clone();
            Box::pin(async move { call(pname, c).await })
        });
        match tools.register_mcp(
            ToolSpec {
                name: public.clone(),
                description: description.clone(),
                parameters_json: parameters_json.clone(),
            },
            body,
        ) {
            Ok(d) => {
                slot.registered.insert(public.clone(), d);
            }
            Err(_) => {}
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
            let mcp = Mcp::new(Some((*tools).clone()), Elicitation::new(ctx.clone()));
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
            Mcp::new(None, Elicitation::new(ctx.clone())),
        )?))
    })
}
