//! stdio MCP: Content-Length JSON-RPC. Probe `server/discover` (2026-07-28),
//! then fall back to `initialize` if the server is still initialize-era.

use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

use crate::config::{McpServer, McpTransport};
use crate::mcp::protocol::{
    self, client_info, discover_versions, pick_version, raw_tool_name, tools_from_list, with_meta,
    PROTOCOL_LATEST, PROTOCOL_LEGACY,
};
use crate::tools::tool_result;
use crate::types::ToolCall;

use super::CallFn;

pub(super) async fn connect(
    server: &McpServer,
) -> Result<(Vec<protocol::ListedTool>, CallFn), String> {
    let McpTransport::Stdio { command, args, env } = &server.transport else {
        return Err("not a stdio MCP server".into());
    };
    let mut child = tokio::process::Command::new(command);
    child
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    for (k, v) in env {
        child.env(k, v);
    }
    let mut child = child.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let session = std::sync::Arc::new(tokio::sync::Mutex::new(McpSession {
        stdin,
        stdout: BufReader::new(stdout),
        next_id: 1,
        protocol: PROTOCOL_LATEST.to_string(),
        modern: true,
        _child: child,
    }));
    let timeout = Duration::from_secs(server.startup_timeout_sec.max(1));
    {
        let mut s = session.lock().await;
        handshake(&mut s, timeout).await?;
        let listed = tokio::time::timeout(timeout, list_tools(&mut s))
            .await
            .map_err(|_| "tools/list timeout".to_string())??;
        if listed.get("error").is_some() {
            return Err(listed["error"].to_string());
        }
        let out = tools_from_list(&server.name, &listed);
        drop(s);
        let session_call = session.clone();
        let call: CallFn = std::sync::Arc::new(move |public: String, c: ToolCall| {
            let session = session_call.clone();
            Box::pin(async move {
                let raw_name = raw_tool_name(&public);
                let args: Value = serde_json::from_str(&c.arguments).unwrap_or(json!({}));
                let mut s = session.lock().await;
                let params = {
                    let body = json!({ "name": raw_name, "arguments": args });
                    if s.modern {
                        with_meta(body, &s.protocol)
                    } else {
                        body
                    }
                };
                match s.rpc("tools/call", params).await {
                    Ok(v) => tool_result(c, protocol::format_call_result(&v)),
                    Err(e) => tool_result(c, e),
                }
            })
        });
        Ok((out, call))
    }
}

async fn handshake(s: &mut McpSession, timeout: Duration) -> Result<(), String> {
    let discover = tokio::time::timeout(
        timeout,
        s.rpc("server/discover", with_meta(json!({}), PROTOCOL_LATEST)),
    )
    .await;
    match discover {
        Ok(Ok(v)) if v.get("error").is_none() => {
            let versions = discover_versions(&v);
            if let Some(picked) = pick_version(&versions) {
                s.protocol = picked.to_string();
                s.modern = protocol::is_modern(picked);
            }
            if !s.modern {
                initialize_legacy(s, timeout).await?;
            }
            Ok(())
        }
        Ok(Ok(v)) => {
            if let Some(supported) = protocol::unsupported_versions(&v) {
                let picked = pick_version(&supported).ok_or_else(|| {
                    format!("server does not speak a protocol Dock supports: {supported:?}")
                })?;
                s.protocol = picked.to_string();
                s.modern = protocol::is_modern(picked);
                if !s.modern {
                    initialize_legacy(s, timeout).await?;
                }
                return Ok(());
            }
            initialize_legacy(s, timeout).await
        }
        Ok(Err(_)) | Err(_) => initialize_legacy(s, timeout).await,
    }
}

async fn initialize_legacy(s: &mut McpSession, timeout: Duration) -> Result<(), String> {
    s.modern = false;
    if protocol::is_modern(&s.protocol) {
        s.protocol = PROTOCOL_LEGACY.to_string();
    }
    let init = tokio::time::timeout(
        timeout,
        s.rpc(
            "initialize",
            json!({
                "protocolVersion": s.protocol,
                "capabilities": {},
                "clientInfo": client_info(),
            }),
        ),
    )
    .await
    .map_err(|_| "initialize timeout".to_string())?
    .map_err(|e| format!("initialize: {e}"))?;
    if init.get("error").is_some() {
        return Err(init["error"].to_string());
    }
    if let Some(pv) = init
        .pointer("/result/protocolVersion")
        .and_then(|v| v.as_str())
    {
        s.protocol = pv.to_string();
        s.modern = protocol::is_modern(pv);
    }
    s.notify("notifications/initialized", json!({})).await.ok();
    Ok(())
}

async fn list_tools(s: &mut McpSession) -> Result<Value, String> {
    let params = if s.modern {
        with_meta(json!({}), &s.protocol)
    } else {
        json!({})
    };
    s.rpc("tools/list", params).await
}

struct McpSession {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    protocol: String,
    modern: bool,
    _child: tokio::process::Child,
}

impl McpSession {
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write(&msg).await?;
        loop {
            let line = self.read_frame().await?;
            let v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            if protocol::id_matches(&v, id) {
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

    async fn read_frame(&mut self) -> Result<String, String> {
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
