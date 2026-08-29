//! MCP client: one plugin, tools register into `"tools"`. Connect/list fail-open.
//!
//! Protocol: offer `2026-07-28` first; fall back to initialize-era `2025-11-25`.
//! Transports: stdio (Content-Length) and Streamable HTTP (`url`).
//! Public names: `mcp_{server}__{tool}`. Enabled tools bypass Agent preset allowlists.

mod http;
mod protocol;
mod stdio;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use cordis::{plugin, plugin_async, Disposable, Inject, Plugin};

use crate::config::{self, McpServer, McpTransport};
use crate::names::{MCP, TOOLS};
use crate::tools::{ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

pub(super) type CallFn = std::sync::Arc<
    dyn Fn(String, ToolCall) -> crate::runtime::BoxFuture<'static, ToolResult> + Send + Sync,
>;

pub use protocol::{is_mcp_public_name, public_tool_name, raw_tool_name};

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
    pub detail: String,
    pub tools: Vec<McpToolStatus>,
}

struct Slot {
    config: McpServer,
    call: Option<CallFn>,
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
}

impl Mcp {
    fn new(tools: Option<Tools>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                slots: Vec::new(),
                tools,
            })),
        }
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
            drop_registered(slot);
            slot.call = None;
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
            slot.call = None;
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

    async fn connect_slot(&self, config: &McpServer) -> Result<(), String> {
        let result = connect_and_list(config).await;
        let mut inner = self.inner.lock().unwrap();
        let tools = inner.tools.clone();
        let Some(slot) = inner
            .slots
            .iter_mut()
            .find(|s| s.config.name == config.name)
        else {
            return Ok(());
        };
        match result {
            Ok((listed, call)) => {
                drop_registered(slot);
                slot.listed = listed;
                slot.call = Some(call);
                slot.last_error = None;
                if let Some(tools) = tools {
                    register_enabled(slot, &tools);
                }
                Ok(())
            }
            Err(e) => {
                drop_registered(slot);
                slot.call = None;
                slot.last_error = Some(e.clone());
                Err(e)
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
            let mcp = Mcp::new(Some((*tools).clone()));
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
) -> Result<(Vec<protocol::ListedTool>, CallFn), String> {
    match &server.transport {
        McpTransport::Stdio { .. } => stdio::connect(server).await,
        McpTransport::Http { .. } => http::connect(server).await,
    }
}

/// Empty MCP status plugin when we only need the service key for `/mcps`.
#[allow(dead_code)]
pub fn mcp_empty() -> Plugin {
    plugin("mcp-client", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(MCP, Mcp::new(None))?))
    })
}
