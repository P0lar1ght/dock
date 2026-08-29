//! MCP client: one plugin, tools register into `"tools"`. Connect/list fail-open.

use std::sync::Mutex;
use std::time::Duration;

use cordis::{plugin, plugin_async, Inject, Plugin};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

use crate::config::{self, McpServer};
use crate::names::{MCP, TOOLS};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

#[derive(Clone, Debug)]
pub struct McpStatus {
    pub name: String,
    pub command: String,
    pub ok: bool,
    pub detail: String,
    pub tools: Vec<String>,
}

pub struct Mcp {
    servers: Mutex<Vec<McpStatus>>,
}

impl Mcp {
    pub fn list(&self) -> Vec<McpStatus> {
        self.servers.lock().unwrap().clone()
    }
}

pub fn mcp_client() -> Plugin {
    plugin_async(
        "mcp-client",
        Inject::from([TOOLS]),
        |ctx, _: &()| async move {
            let configured = config::load_mcp_servers();
            let mut statuses = Vec::new();
            let mut disposers = Vec::new();
            let tools = ctx.require::<Tools>(TOOLS)?;
            for server in configured {
                match connect_and_list(&server).await {
                    Ok((listed, call)) => {
                        let names: Vec<String> = listed.iter().map(|t| t.0.clone()).collect();
                        statuses.push(McpStatus {
                            name: server.name.clone(),
                            command: server.command.clone(),
                            ok: true,
                            detail: format!("{} tools", names.len()),
                            tools: names,
                        });
                        for (public, schema) in listed {
                            let call = call.clone();
                            let pname = public.clone();
                            let body: ToolBody = std::sync::Arc::new(move |c| {
                                let call = call.clone();
                                let pname = pname.clone();
                                Box::pin(async move { call(pname, c).await })
                            });
                            match tools.register(
                                ToolSpec {
                                    name: public,
                                    description: schema,
                                    parameters_json: r#"{"type":"object"}"#.into(),
                                },
                                body,
                            ) {
                                Ok(d) => disposers.push(d),
                                Err(_) => {}
                            }
                        }
                    }
                    Err(e) => {
                        statuses.push(McpStatus {
                            name: server.name.clone(),
                            command: server.command.clone(),
                            ok: false,
                            detail: e,
                            tools: Vec::new(),
                        });
                    }
                }
            }
            ctx.provide(
                MCP,
                Mcp {
                    servers: Mutex::new(statuses),
                },
            )?;
            own_registered(&ctx, disposers)?;
            Ok(None)
        },
    )
}

type CallFn = std::sync::Arc<
    dyn Fn(String, ToolCall) -> crate::runtime::BoxFuture<'static, ToolResult> + Send + Sync,
>;

async fn connect_and_list(server: &McpServer) -> Result<(Vec<(String, String)>, CallFn), String> {
    let mut child = tokio::process::Command::new(&server.command)
        .args(&server.args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let session = std::sync::Arc::new(tokio::sync::Mutex::new(McpSession {
        stdin,
        stdout: BufReader::new(stdout),
        next_id: 1,
        _child: child,
    }));
    {
        let mut s = session.lock().await;
        let init = tokio::time::timeout(
            Duration::from_secs(8),
            s.rpc(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "dock", "version": "0.1.0" }
                }),
            ),
        )
        .await
        .map_err(|_| "initialize timeout".to_string())?
        .map_err(|e| format!("initialize: {e}"))?;
        if init.get("error").is_some() {
            return Err(init["error"].to_string());
        }
        s.notify("notifications/initialized", json!({})).await.ok();
        let listed = tokio::time::timeout(Duration::from_secs(8), s.rpc("tools/list", json!({})))
            .await
            .map_err(|_| "tools/list timeout".to_string())?
            .map_err(|e| format!("tools/list: {e}"))?;
        let tools = listed
            .pointer("/result/tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for tool in tools {
            let Some(name) = tool.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            let desc = tool
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or(name);
            let public = format!("{}__{name}", server.name);
            out.push((public, desc.to_string()));
        }
        drop(s);
        let session_call = session.clone();
        let call: CallFn = std::sync::Arc::new(move |public: String, c: ToolCall| {
            let session = session_call.clone();
            Box::pin(async move {
                let raw_name = public
                    .rsplit_once("__")
                    .map(|(_, n)| n.to_string())
                    .unwrap_or(public);
                let args: Value = serde_json::from_str(&c.arguments).unwrap_or(json!({}));
                let mut s = session.lock().await;
                match s
                    .rpc("tools/call", json!({ "name": raw_name, "arguments": args }))
                    .await
                {
                    Ok(v) => {
                        let text = v
                            .pointer("/result/content")
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| v.to_string());
                        tool_result(c, text)
                    }
                    Err(e) => tool_result(c, e),
                }
            })
        });
        Ok((out, call))
    }
}

struct McpSession {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    _child: tokio::process::Child,
}

impl McpSession {
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write(&msg).await?;
        loop {
            let line = self.read_line().await?;
            let v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            if v.get("id").and_then(|x| x.as_u64()) == Some(id)
                || v.get("id").and_then(|x| x.as_i64()) == Some(id as i64)
            {
                return Ok(v);
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn write(&mut self, v: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(v).map_err(|e| e.to_string())?;
        let head = format!("Content-Length: {}\r\n\r\n", body.len());
        self.stdin
            .write_all(head.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        self.stdin
            .write_all(&body)
            .await
            .map_err(|e| e.to_string())?;
        self.stdin.flush().await.map_err(|e| e.to_string())
    }

    async fn read_line(&mut self) -> Result<String, String> {
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            self.stdout
                .read_line(&mut line)
                .await
                .map_err(|e| e.to_string())?;
            if line == "\r\n" || line == "\n" {
                break;
            }
            headers.push_str(&line);
        }
        let len = headers
            .lines()
            .find_map(|l| {
                l.split_once(':').and_then(|(k, v)| {
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                })
            })
            .flatten()
            .ok_or("missing Content-Length")?;
        let mut buf = vec![0u8; len];
        tokio::io::AsyncReadExt::read_exact(&mut self.stdout, &mut buf)
            .await
            .map_err(|e| e.to_string())?;
        String::from_utf8(buf).map_err(|e| e.to_string())
    }
}

/// Empty MCP status plugin when we only need the service key for `/mcps`.
#[allow(dead_code)]
pub fn mcp_empty() -> Plugin {
    plugin("mcp-client", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(
            MCP,
            Mcp {
                servers: Mutex::new(Vec::new()),
            },
        )?))
    })
}
