//! stdio MCP: Content-Length and/or NDJSON JSON-RPC.
//! Probe `server/discover` (2026-07-28), then fall back to `initialize` if the
//! server is still initialize-era. A standing reader demuxes server requests
//! (`elicitation/create`, `ping`) and `tools/list_changed` while `tools/call`
//! waits.
//!
//! Framing: `auto` (default) spawns with Content-Length; on JSON-RPC parse error
//! (-32700 / "parse") kills the child and respawns with NDJSON. Explicit
//! `content-length` / `ndjson` skip probing. Never flips wire framing on the
//! same stdin. Reads accept either shape (first line starting with `{` → NDJSON).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{oneshot, watch};

use crate::config::{McpServer, McpStdioFraming, McpTransport};
use crate::mcp::protocol::{
    self, client_capabilities, client_info, discover_versions, pick_version, raw_tool_name,
    with_meta, Incoming, PROTOCOL_LATEST, PROTOCOL_LEGACY,
};
use crate::tools::tool_result;
use crate::types::ToolCall;

use super::incoming::{self, LiveHooks};
use super::tools_list;
use super::{CallFn, RelistFn};

/// Fixed on-the-wire framing for one child process (never flipped mid-connection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireFraming {
    ContentLength,
    Ndjson,
}

pub(super) async fn connect(
    server: &McpServer,
    hooks: LiveHooks,
    stop: watch::Receiver<bool>,
) -> Result<(Vec<protocol::ListedTool>, CallFn, RelistFn), String> {
    let McpTransport::Stdio { framing, .. } = &server.transport else {
        return Err("not a stdio MCP server".into());
    };
    let timeout = Duration::from_secs(server.startup_timeout_sec.max(1));
    match *framing {
        McpStdioFraming::ContentLength => {
            connect_once(server, hooks, stop, WireFraming::ContentLength, timeout).await
        }
        McpStdioFraming::Ndjson => {
            connect_once(server, hooks, stop, WireFraming::Ndjson, timeout).await
        }
        McpStdioFraming::Auto => {
            match connect_once(
                server,
                hooks.clone(),
                stop.clone(),
                WireFraming::ContentLength,
                timeout,
            )
            .await
            {
                Ok(ok) => Ok(ok),
                Err(e) if looks_like_framing_parse_error(&e) => {
                    tracing::info!(
                        "MCP stdio parse error on Content-Length; killing child and respawning as NDJSON"
                    );
                    connect_once(server, hooks, stop, WireFraming::Ndjson, timeout).await
                }
                Err(e) => Err(e),
            }
        }
    }
}

async fn connect_once(
    server: &McpServer,
    hooks: LiveHooks,
    stop: watch::Receiver<bool>,
    wire: WireFraming,
    timeout: Duration,
) -> Result<(Vec<protocol::ListedTool>, CallFn, RelistFn), String> {
    let McpTransport::Stdio {
        command,
        args,
        env,
        ..
    } = &server.transport
    else {
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
        wire,
        hooks,
        _child: Mutex::new(Some(child)),
    });
    let reader_shared = shared.clone();
    let mut stop_reader = stop.clone();
    tokio::spawn(async move {
        reader_loop(BufReader::new(stdout), reader_shared, &mut stop_reader).await;
    });
    if let Err(e) = handshake(&shared, timeout).await {
        shared.kill_child();
        return Err(e);
    }
    let listed = match tokio::time::timeout(timeout, list_all(&shared, &server.name)).await {
        Ok(Ok(listed)) => listed,
        Ok(Err(e)) => {
            shared.kill_child();
            return Err(e);
        }
        Err(_) => {
            shared.kill_child();
            return Err("tools/list timeout".to_string());
        }
    };
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
    wire: WireFraming,
    hooks: LiveHooks,
    _child: Mutex<Option<tokio::process::Child>>,
}

impl Shared {
    fn kill_child(&self) {
        if let Some(mut child) = self._child.lock().unwrap().take() {
            let _ = child.start_kill();
        }
    }
}

fn looks_like_framing_parse_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("-32700")
        || lower.contains("parse error")
        || (lower.contains("parse") && lower.contains("error"))
}

fn value_is_framing_parse_error(v: &Value) -> bool {
    if protocol::jsonrpc_error_code(v) == Some(-32700) {
        return true;
    }
    v.pointer("/error/message")
        .and_then(|m| m.as_str())
        .is_some_and(|m| {
            let lower = m.to_ascii_lowercase();
            lower.contains("parse")
        })
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
            if value_is_framing_parse_error(&v) {
                return Err(v["error"].to_string());
            }
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
        Ok(Err(e)) if looks_like_framing_parse_error(&e) => Err(e),
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
    let mut stdin = s.stdin.lock().await;
    write_frame(&mut *stdin, v, s.wire).await
}

async fn write_frame<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    v: &Value,
    framing: WireFraming,
) -> Result<(), String> {
    let body = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    match framing {
        WireFraming::ContentLength => {
            let head = format!("Content-Length: {}\r\n\r\n", body.len());
            w.write_all(head.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            w.write_all(&body).await.map_err(|e| e.to_string())?;
        }
        WireFraming::Ndjson => {
            w.write_all(&body).await.map_err(|e| e.to_string())?;
            w.write_all(b"\n").await.map_err(|e| e.to_string())?;
        }
    }
    w.flush().await.map_err(|e| e.to_string())
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
                        deliver_response(&shared, v);
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

fn deliver_response(shared: &Shared, v: Value) {
    if let Some(id) = incoming::pending_id(&v) {
        if let Some(tx) = shared.pending.lock().unwrap().remove(&id) {
            let _ = tx.send(v);
        }
        return;
    }
    // JSON-RPC parse errors often use `id: null` — hand to oldest pending so
    // `auto` can see -32700 and respawn with NDJSON.
    if value_is_framing_parse_error(&v) {
        let mut pending = shared.pending.lock().unwrap();
        if let Some(id) = pending.keys().copied().min() {
            if let Some(tx) = pending.remove(&id) {
                let _ = tx.send(v);
            }
        }
    }
}

/// Dual-read: if the first line starts with `{`, treat as one NDJSON message;
/// otherwise parse Content-Length headers + body as today.
async fn read_frame<R: AsyncBufReadExt + Unpin>(stdout: &mut R) -> Result<String, String> {
    let mut first = String::new();
    stdout
        .read_line(&mut first)
        .await
        .map_err(|e| e.to_string())?;
    if first.is_empty() {
        return Err("MCP stdio eof".into());
    }
    if first.trim_start().starts_with('{') {
        while first.ends_with('\n') || first.ends_with('\r') {
            first.pop();
        }
        return Ok(first);
    }
    let mut headers = first;
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
    stdout
        .read_exact(&mut buf)
        .await
        .map_err(|e| e.to_string())?;
    String::from_utf8(buf).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn read_ndjson_line() {
        let (client, mut server) = tokio::io::duplex(1024);
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        server.write_all(body.as_bytes()).await.unwrap();
        server.write_all(b"\n").await.unwrap();
        drop(server);
        let mut reader = BufReader::new(client);
        let got = read_frame(&mut reader).await.unwrap();
        assert_eq!(got, body);
    }

    #[tokio::test]
    async fn read_content_length_frame() {
        let (client, mut server) = tokio::io::duplex(1024);
        let body = r#"{"jsonrpc":"2.0","id":2,"result":{}}"#;
        let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        server.write_all(frame.as_bytes()).await.unwrap();
        drop(server);
        let mut reader = BufReader::new(client);
        let got = read_frame(&mut reader).await.unwrap();
        assert_eq!(got, body);
    }

    #[tokio::test]
    async fn write_content_length_format() {
        let (mut client, server) = tokio::io::duplex(1024);
        let msg = json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}});
        write_frame(&mut client, &msg, WireFraming::ContentLength)
            .await
            .unwrap();
        drop(client);
        let mut reader = BufReader::new(server);
        let got = read_frame(&mut reader).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&got).unwrap(), msg);
        // Raw bytes must include Content-Length header when written that way —
        // re-read via a fresh duplex to assert header shape.
        let (mut w, mut r) = tokio::io::duplex(1024);
        write_frame(&mut w, &msg, WireFraming::ContentLength)
            .await
            .unwrap();
        drop(w);
        let mut buf = Vec::new();
        AsyncReadExt::read_to_end(&mut r, &mut buf).await.unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("Content-Length: "), "{s}");
        assert!(s.contains("\r\n\r\n"));
        assert!(!s.ends_with('\n') || s.contains("\r\n\r\n{"));
    }

    #[tokio::test]
    async fn write_ndjson_format() {
        let (mut w, mut r) = tokio::io::duplex(1024);
        let msg = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        write_frame(&mut w, &msg, WireFraming::Ndjson).await.unwrap();
        drop(w);
        let mut buf = Vec::new();
        AsyncReadExt::read_to_end(&mut r, &mut buf).await.unwrap();
        assert_eq!(buf.last().copied(), Some(b'\n'));
        let line = std::str::from_utf8(&buf[..buf.len() - 1]).unwrap();
        assert!(!line.contains("Content-Length"));
        assert_eq!(serde_json::from_str::<Value>(line).unwrap(), msg);
    }

    #[test]
    fn parse_error_detection() {
        assert!(value_is_framing_parse_error(&json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {"code": -32700, "message": "Parse error"}
        })));
        assert!(looks_like_framing_parse_error(
            r#"{"code":-32700,"message":"Parse error"}"#
        ));
        assert!(!value_is_framing_parse_error(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32601, "message": "Method not found"}
        })));
    }

    #[test]
    fn auto_respawn_uses_fixed_wire_per_child() {
        // `auto` never flips WireFraming mid-connection; first child is CL, second Ndjson.
        assert_ne!(WireFraming::ContentLength, WireFraming::Ndjson);
        assert!(looks_like_framing_parse_error(
            r#"{"code":-32700,"message":"Parse error"}"#
        ));
        // Framing stays fixed for the life of one Shared / child.
        let shared_wire = WireFraming::ContentLength;
        assert_eq!(shared_wire, WireFraming::ContentLength);
        let respawn_wire = WireFraming::Ndjson;
        assert_eq!(respawn_wire, WireFraming::Ndjson);
    }

    #[tokio::test]
    async fn dual_read_prefers_ndjson_when_brace() {
        // Content-Length servers never start a frame with `{`; NDJSON always does.
        let (client, mut server) = tokio::io::duplex(256);
        server
            .write_all(br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#)
            .await
            .unwrap();
        server.write_all(b"\n").await.unwrap();
        drop(server);
        let mut reader = BufReader::new(client);
        let got = read_frame(&mut reader).await.unwrap();
        let v: Value = serde_json::from_str(&got).unwrap();
        assert!(value_is_framing_parse_error(&v));
    }
}
