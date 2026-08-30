//! stdio MCP: Content-Length JSON-RPC. Probe `server/discover` (2026-07-28),
//! then fall back to `initialize` if the server is still initialize-era.
//! A standing reader demuxes server requests (`elicitation/create`, `ping`)
//! and `tools/list_changed` while `tools/call` waits.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{oneshot, watch};

use crate::config::{McpServer, McpTransport};
use crate::mcp::protocol::{
    self, client_capabilities, client_info, discover_versions, pick_version, raw_tool_name,
    with_meta, Incoming, PROTOCOL_LATEST, PROTOCOL_LEGACY,
};
use crate::tools::tool_result;
use crate::types::ToolCall;

use super::incoming::{self, LiveHooks};
use super::tools_list;
use super::{CallFn, RelistFn};

pub(super) async fn connect(
    server: &McpServer,
    hooks: LiveHooks,
    stop: watch::Receiver<bool>,
) -> Result<(Vec<protocol::ListedTool>, CallFn, RelistFn), String> {
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
    let shared = Arc::new(Shared {
        stdin: tokio::sync::Mutex::new(stdin),
        pending: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
        protocol: Mutex::new(PROTOCOL_LATEST.to_string()),
        modern: AtomicBool::new(true),
        hooks,
        _child: Mutex::new(Some(child)),
    });
    let reader_shared = shared.clone();
    let mut stop_reader = stop.clone();
    tokio::spawn(async move {
        reader_loop(BufReader::new(stdout), reader_shared, &mut stop_reader).await;
    });
    let timeout = Duration::from_secs(server.startup_timeout_sec.max(1));
    handshake(&shared, timeout).await?;
    let listed = tokio::time::timeout(timeout, list_all(&shared, &server.name))
        .await
        .map_err(|_| "tools/list timeout".to_string())??;
    let session_call = shared.clone();
    let call: CallFn = std::sync::Arc::new(move |public: String, c: ToolCall| {
        let session = session_call.clone();
        Box::pin(async move {
            let raw_name = raw_tool_name(&public);
            let args: Value = serde_json::from_str(&c.arguments).unwrap_or(json!({}));
            let body = json!({ "name": raw_name, "arguments": args });
            let params = if session.modern.load(Ordering::Relaxed) {
                with_meta(body, &session.protocol.lock().unwrap())
            } else {
                body
            };
            match rpc(&session, "tools/call", params).await {
                Ok(v) => tool_result(c, protocol::format_call_result(&v)),
                Err(e) => tool_result(c, e),
            }
        })
    });
    let session_list = shared.clone();
    let name = server.name.clone();
    let relist: RelistFn = std::sync::Arc::new(move || {
        let session = session_list.clone();
        let name = name.clone();
        Box::pin(async move { list_all(&session, &name).await })
    });
    Ok((listed, call, relist))
}

struct Shared {
    stdin: tokio::sync::Mutex<ChildStdin>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    next_id: AtomicU64,
    protocol: Mutex<String>,
    modern: AtomicBool,
    hooks: LiveHooks,
    _child: Mutex<Option<tokio::process::Child>>,
}

async fn handshake(s: &Shared, timeout: Duration) -> Result<(), String> {
    let discover = tokio::time::timeout(
        timeout,
        rpc(s, "server/discover", with_meta(json!({}), PROTOCOL_LATEST)),
    )
    .await;
    match discover {
        Ok(Ok(v)) if v.get("error").is_none() => {
            let versions = discover_versions(&v);
            if let Some(picked) = pick_version(&versions) {
                *s.protocol.lock().unwrap() = picked.to_string();
                s.modern
                    .store(protocol::is_modern(picked), Ordering::Relaxed);
            }
            if !s.modern.load(Ordering::Relaxed) {
                initialize_legacy(s, timeout).await?;
            }
            Ok(())
        }
        Ok(Ok(v)) => {
            if let Some(supported) = protocol::unsupported_versions(&v) {
                let picked = pick_version(&supported).ok_or_else(|| {
                    format!("server does not speak a protocol Dock supports: {supported:?}")
                })?;
                *s.protocol.lock().unwrap() = picked.to_string();
                s.modern
                    .store(protocol::is_modern(picked), Ordering::Relaxed);
                if !s.modern.load(Ordering::Relaxed) {
                    initialize_legacy(s, timeout).await?;
                }
                return Ok(());
            }
            initialize_legacy(s, timeout).await
        }
        Ok(Err(_)) | Err(_) => initialize_legacy(s, timeout).await,
    }
}

async fn initialize_legacy(s: &Shared, timeout: Duration) -> Result<(), String> {
    s.modern.store(false, Ordering::Relaxed);
    {
        let mut proto = s.protocol.lock().unwrap();
        if protocol::is_modern(&proto) {
            *proto = PROTOCOL_LEGACY.to_string();
        }
    }
    let protocol = s.protocol.lock().unwrap().clone();
    let init = tokio::time::timeout(
        timeout,
        rpc(
            s,
            "initialize",
            json!({
                "protocolVersion": protocol,
                "capabilities": client_capabilities(),
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
        *s.protocol.lock().unwrap() = pv.to_string();
        s.modern.store(protocol::is_modern(pv), Ordering::Relaxed);
    }
    notify(s, "notifications/initialized", json!({})).await.ok();
    Ok(())
}

async fn list_all(s: &Shared, server_name: &str) -> Result<Vec<protocol::ListedTool>, String> {
    let modern = s.modern.load(Ordering::Relaxed);
    let protocol = s.protocol.lock().unwrap().clone();
    tools_list::list_all(server_name, |params| {
        let params = if modern {
            with_meta(params, &protocol)
        } else {
            params
        };
        rpc(s, "tools/list", params)
    })
    .await
}

async fn rpc(s: &Shared, method: &str, params: Value) -> Result<Value, String> {
    let id = s.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    s.pending.lock().unwrap().insert(id, tx);
    let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    if let Err(e) = write_msg(s, &msg).await {
        s.pending.lock().unwrap().remove(&id);
        return Err(e);
    }
    rx.await.map_err(|_| "MCP stdio closed".to_string())
}

async fn notify(s: &Shared, method: &str, params: Value) -> Result<(), String> {
    write_msg(
        s,
        &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
    )
    .await
}

async fn write_msg(s: &Shared, v: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    let head = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut stdin = s.stdin.lock().await;
    stdin
        .write_all(head.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stdin.write_all(&body).await.map_err(|e| e.to_string())?;
    stdin.flush().await.map_err(|e| e.to_string())
}

async fn reader_loop(
    mut stdout: BufReader<tokio::process::ChildStdout>,
    shared: Arc<Shared>,
    stop: &mut watch::Receiver<bool>,
) {
    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            _ = stop.changed() => {
                if *stop.borrow() {
                    break;
                }
            }
            frame = read_frame(&mut stdout) => {
                let Ok(line) = frame else {
                    break;
                };
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match protocol::classify(v) {
                    Incoming::Response(v) => {
                        if let Some(id) = incoming::pending_id(&v) {
                            if let Some(tx) = shared.pending.lock().unwrap().remove(&id) {
                                let _ = tx.send(v);
                            }
                        }
                    }
                    Incoming::Notification { method, params } => {
                        incoming::note(&shared.hooks, &method, &params);
                    }
                    Incoming::Request { id, method, params } => {
                        let reply = incoming::request(&shared.hooks, id, method, params).await;
                        let _ = write_msg(&shared, &reply).await;
                    }
                }
            }
        }
    }
    let pending = std::mem::take(&mut *shared.pending.lock().unwrap());
    drop(pending);
    if let Some(mut child) = shared._child.lock().unwrap().take() {
        let _ = child.start_kill();
    }
}

async fn read_frame(stdout: &mut BufReader<tokio::process::ChildStdout>) -> Result<String, String> {
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        stdout
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;
        if line.is_empty() {
            return Err("MCP stdio eof".into());
        }
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
    tokio::io::AsyncReadExt::read_exact(stdout, &mut buf)
        .await
        .map_err(|e| e.to_string())?;
    String::from_utf8(buf).map_err(|e| e.to_string())
}
