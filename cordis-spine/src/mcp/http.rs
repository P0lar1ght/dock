//! Streamable HTTP MCP client (rmcp `post_message` shape, reqwest 0.12).
//!
//! Prefer protocol `2026-07-28` (no session, `_meta` + `MCP-Protocol-Version`
//! + `Mcp-Method` / `Mcp-Name`). On a 400 that is not a modern JSON-RPC error,
//! fall back to initialize-era `2025-11-25` with `Mcp-Session-Id`.
//! Standing GET SSE + session 404 recover; POST SSE is processed in order
//! (elicitation must complete before later events on that stream).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde_json::{json, Value};
use tokio::sync::{oneshot, watch};

use crate::config::{McpServer, McpTransport};
use crate::mcp::protocol::{
    self, client_capabilities, client_info, encode_header_value, id_matches, pick_version,
    raw_tool_name, unsupported_versions, with_meta, Incoming, PROTOCOL_LATEST, PROTOCOL_LEGACY,
};
use crate::tools::{tool_result, tool_result_with_images};
use crate::types::ToolCall;

use super::incoming::{self, LiveHooks};
use super::sse::{self, SseParser};
use super::tools_list;
use super::{CallFn, RelistFn};

const JSON_MIME: &str = "application/json";
const EVENT_STREAM_MIME: &str = "text/event-stream";
const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
const HEADER_PROTOCOL: &str = "MCP-Protocol-Version";
const HEADER_METHOD: &str = "Mcp-Method";
const HEADER_NAME: &str = "Mcp-Name";

const RESERVED_HEADERS: &[&str] = &["accept", "mcp-session-id", "last-event-id"];

pub(super) async fn connect(
    server: &McpServer,
    hooks: LiveHooks,
    stop: watch::Receiver<bool>,
) -> Result<(Vec<protocol::ListedTool>, CallFn, RelistFn), String> {
    let McpTransport::Http { url, headers } = &server.transport else {
        return Err("not an HTTP MCP server".into());
    };
    let skip_oauth = has_authorization_header(headers);
    let client = build_client(&server.name, url, headers)?;
    let (protocol_watch, proto_rx) = watch::channel(PROTOCOL_LATEST.to_string());
    let (session_watch, sid_rx) = watch::channel(None);
    let shared = Arc::new(HttpShared {
        client: client.clone(),
        url: url.clone(),
        protocol_watch,
        session_watch,
        modern: AtomicBool::new(true),
        next_id: AtomicU64::new(1),
        server: server.clone(),
        skip_oauth,
        pending: Mutex::new(HashMap::new()),
        hooks,
        handshake: tokio::sync::Mutex::new(()),
    });
    let timeout = Duration::from_secs(server.startup_timeout_sec.max(1));
    let listed = tokio::time::timeout(timeout, handshake(&shared))
        .await
        .map_err(|_| "MCP HTTP handshake timeout".to_string())??;

    let (get_tx, mut get_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(sse::listen_get(
        client,
        url.clone(),
        proto_rx,
        sid_rx,
        skip_oauth,
        server.clone(),
        get_tx,
        stop,
    ));
    let get_shared = shared.clone();
    tokio::spawn(async move {
        while let Some(v) = get_rx.recv().await {
            ingest(&get_shared, v, None).await;
        }
    });

    let session_call = shared.clone();
    let call: CallFn = std::sync::Arc::new(move |public: String, c: ToolCall| {
        let session = session_call.clone();
        Box::pin(async move {
            let raw_name = raw_tool_name(&public);
            let args: Value = serde_json::from_str(&c.arguments).unwrap_or(json!({}));
            match rpc(
                &session,
                "tools/call",
                json!({ "name": raw_name, "arguments": args }),
            )
            .await
            {
                Ok(v) => {
                    let (text, images) = protocol::format_call_result_parts(&v);
                    if images.is_empty() {
                        tool_result(c, text)
                    } else {
                        tool_result_with_images(c, text, images)
                    }
                }
                Err(e) => tool_result(c, e),
            }
        })
    });
    let session_list = shared.clone();
    let name = server.name.clone();
    let relist: RelistFn = std::sync::Arc::new(move || {
        let session = session_list.clone();
        let name = name.clone();
        Box::pin(async move { list_pages(&session, &name).await })
    });
    Ok((listed, call, relist))
}

struct HttpShared {
    client: reqwest::Client,
    url: String,
    protocol_watch: watch::Sender<String>,
    session_watch: watch::Sender<Option<String>>,
    modern: AtomicBool,
    next_id: AtomicU64,
    server: McpServer,
    skip_oauth: bool,
    pending: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    hooks: LiveHooks,
    handshake: tokio::sync::Mutex<()>,
}

impl HttpShared {
    fn protocol(&self) -> String {
        self.protocol_watch.borrow().clone()
    }

    fn set_protocol(&self, p: String) {
        self.modern
            .store(protocol::is_modern(&p), Ordering::Relaxed);
        let _ = self.protocol_watch.send(p);
    }

    fn session_id(&self) -> Option<String> {
        self.session_watch.borrow().clone()
    }

    fn set_session_id(&self, id: Option<String>) {
        let _ = self.session_watch.send(id);
    }
}

async fn handshake(s: &HttpShared) -> Result<Vec<protocol::ListedTool>, String> {
    match rpc(s, "tools/list", json!({})).await {
        Ok(v) if v.get("error").is_none() => list_from_first(s, v).await,
        Ok(v) => {
            if let Some(supported) = unsupported_versions(&v) {
                return negotiate(s, &supported).await;
            }
            if looks_legacy_error(&v) {
                initialize_legacy(s).await?;
                return list_pages(s, &s.server.name).await;
            }
            Err(v["error"].to_string())
        }
        Err(e) if looks_legacy_http(&e) => {
            initialize_legacy(s).await?;
            list_pages(s, &s.server.name).await
        }
        Err(e) => Err(e),
    }
}

async fn negotiate(
    s: &HttpShared,
    supported: &[String],
) -> Result<Vec<protocol::ListedTool>, String> {
    let picked = pick_version(supported)
        .ok_or_else(|| format!("server does not speak a protocol Dock supports: {supported:?}"))?;
    s.set_protocol(picked.to_string());
    if s.modern.load(Ordering::Relaxed) {
        let listed = rpc(s, "tools/list", json!({})).await?;
        if listed.get("error").is_some() {
            return Err(listed["error"].to_string());
        }
        list_from_first(s, listed).await
    } else {
        initialize_legacy(s).await?;
        list_pages(s, &s.server.name).await
    }
}

async fn list_from_first(
    s: &HttpShared,
    first: Value,
) -> Result<Vec<protocol::ListedTool>, String> {
    tools_list::list_all_from_first(&s.server.name, first, |params| rpc(s, "tools/list", params))
        .await
}

async fn list_pages(
    s: &HttpShared,
    server_name: &str,
) -> Result<Vec<protocol::ListedTool>, String> {
    tools_list::list_all(server_name, |params| rpc(s, "tools/list", params)).await
}

async fn initialize_legacy(s: &HttpShared) -> Result<(), String> {
    s.modern.store(false, Ordering::Relaxed);
    s.set_session_id(None);
    if protocol::is_modern(&s.protocol()) {
        s.set_protocol(PROTOCOL_LEGACY.to_string());
    }
    let init = rpc_inner(
        s,
        "initialize",
        json!({
            "protocolVersion": s.protocol(),
            "capabilities": client_capabilities(),
            "clientInfo": client_info(),
        }),
        true,
    )
    .await
    .map_err(|e| format!("initialize: {e}"))?;
    if init.get("error").is_some() {
        return Err(init["error"].to_string());
    }
    if let Some(pv) = init
        .pointer("/result/protocolVersion")
        .and_then(|v| v.as_str())
    {
        s.set_protocol(pv.to_string());
    }
    notify(s, "notifications/initialized", json!({})).await.ok();
    Ok(())
}

async fn recover_session(s: &HttpShared) -> Result<(), String> {
    let _g = s.handshake.lock().await;
    initialize_legacy(s).await
}

fn looks_legacy_http(err: &str) -> bool {
    err.starts_with("HTTP 400") || err.starts_with("HTTP 404") || err.starts_with("HTTP 405")
}

fn looks_legacy_error(v: &Value) -> bool {
    protocol::jsonrpc_error_code(v) == Some(protocol::ERR_METHOD_NOT_FOUND)
}

fn build_client(
    server_name: &str,
    url: &str,
    pairs: &BTreeMap<String, String>,
) -> Result<reqwest::Client, String> {
    let mut headers = parse_config_headers(server_name, pairs);
    if !headers.contains_key(USER_AGENT) {
        let ua = format!("dock/{}", env!("CARGO_PKG_VERSION"));
        if let Ok(val) = HeaderValue::from_str(&ua) {
            headers.insert(USER_AGENT, val);
        }
    }
    let _ = url;
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .default_headers(headers)
        .build()
        .map_err(|e| format!("HTTP client: {e}"))
}

fn has_authorization_header(pairs: &BTreeMap<String, String>) -> bool {
    pairs
        .keys()
        .any(|k| k.eq_ignore_ascii_case("authorization"))
}

fn parse_config_headers(server_name: &str, pairs: &BTreeMap<String, String>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (key, value) in pairs {
        if RESERVED_HEADERS.iter().any(|r| key.eq_ignore_ascii_case(r)) {
            continue;
        }
        match (
            HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            (Ok(name), Ok(val)) => {
                headers.insert(name, val);
            }
            _ => {
                tracing::warn!(
                    server = server_name,
                    "Skipping invalid MCP HTTP header: {key}"
                );
            }
        }
    }
    headers
}

async fn rpc(s: &HttpShared, method: &str, params: Value) -> Result<Value, String> {
    rpc_inner(s, method, params, false).await
}

async fn rpc_inner(
    s: &HttpShared,
    method: &str,
    params: Value,
    recovered: bool,
) -> Result<Value, String> {
    let id = s.next_id.fetch_add(1, Ordering::Relaxed);
    let params = if s.modern.load(Ordering::Relaxed) && method != "initialize" {
        with_meta(params, &s.protocol())
    } else {
        params
    };
    let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
    let (tx, rx) = oneshot::channel();
    s.pending.lock().unwrap().insert(id, tx);
    match post(s, Some(method), &params, &msg, Some(id), recovered).await {
        Ok(Some(v)) => {
            s.pending.lock().unwrap().remove(&id);
            Ok(v)
        }
        Ok(None) => rx.await.map_err(|_| "MCP HTTP closed".into()),
        Err(e) => {
            s.pending.lock().unwrap().remove(&id);
            Err(e)
        }
    }
}

async fn notify(s: &HttpShared, method: &str, params: Value) -> Result<(), String> {
    let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
    post(s, Some(method), &params, &msg, None, false)
        .await
        .map(|_| ())
}

async fn post_raw(s: &HttpShared, message: &Value) -> Result<(), String> {
    post(s, None, &json!({}), message, None, false)
        .await
        .map(|_| ())
}

fn complete_pending(s: &HttpShared, v: Value) {
    if let Some(id) = incoming::pending_id(&v) {
        if let Some(tx) = s.pending.lock().unwrap().remove(&id) {
            let _ = tx.send(v);
        }
    }
}

async fn ingest(s: &HttpShared, v: Value, want_id: Option<u64>) -> Option<Value> {
    match protocol::classify(v) {
        Incoming::Response(v) => {
            if want_id.is_some_and(|id| id_matches(&v, id)) {
                return Some(v);
            }
            complete_pending(s, v);
            None
        }
        Incoming::Notification { method, params } => {
            incoming::note(&s.hooks, &method, &params);
            None
        }
        Incoming::Request { id, method, params } => {
            let reply = incoming::request(&s.hooks, id, method, params).await;
            let _ = Box::pin(post_raw(s, &reply)).await;
            None
        }
    }
}

/// `Ok(Some(v))` = JSON-RPC response on this POST. `Ok(None)` = 202, wait GET.
async fn post(
    s: &HttpShared,
    method: Option<&str>,
    params: &Value,
    message: &Value,
    want_id: Option<u64>,
    mut recovered: bool,
) -> Result<Option<Value>, String> {
    let mut retried_auth = false;
    loop {
        let mut req = s
            .client
            .post(&s.url)
            .header(ACCEPT, format!("{EVENT_STREAM_MIME}, {JSON_MIME}"))
            .header(HEADER_PROTOCOL, s.protocol());
        if s.modern.load(Ordering::Relaxed) {
            if let Some(method) = method {
                req = req.header(HEADER_METHOD, method);
                if method == "tools/call" {
                    if let Some(name) = params.get("name").and_then(|n| n.as_str()) {
                        req = req.header(HEADER_NAME, encode_header_value(name));
                    }
                }
            }
        }
        if let Some(sid) = s.session_id() {
            req = req.header(HEADER_SESSION_ID, sid.as_str());
        }
        if !s.skip_oauth {
            if let Some(token) = super::credentials::access_token(&s.server.name, &s.url) {
                if let Ok(val) = HeaderValue::from_str(&format!("Bearer {token}")) {
                    req = req.header(AUTHORIZATION, val);
                }
            }
        }
        let had_session = s.session_id().is_some();
        let response = req
            .json(message)
            .send()
            .await
            .map_err(|e| format!("MCP HTTP {e}"))?;
        let status = response.status();
        if status.as_u16() == 401 && !s.skip_oauth && !retried_auth {
            retried_auth = true;
            drop(response);
            if super::oauth::refresh_stored(&s.server).await.is_ok() {
                continue;
            }
            return Err("MCP HTTP 401 unauthorized".into());
        }
        if let Some(sid) = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
        {
            s.set_session_id(Some(sid.to_string()));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let content_length = response.content_length();

        if status.as_u16() == 401 {
            return Err("MCP HTTP 401 unauthorized".into());
        }
        if sse::is_session_expired(status.as_u16(), had_session) {
            drop(response);
            if recovered {
                return Err("MCP HTTP session expired".into());
            }
            recovered = true;
            Box::pin(recover_session(s)).await?;
            continue;
        }
        if matches!(status.as_u16(), 202 | 204) {
            return Ok(None);
        }

        if status.is_success() && content_length == Some(0) {
            return Ok(None);
        }

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read body>".into());
            if let Ok(v) = serde_json::from_str::<Value>(&body) {
                if v.get("error").is_some() || v.get("result").is_some() {
                    if let Some(hit) = ingest(s, v.clone(), want_id).await {
                        return Ok(Some(hit));
                    }
                    return Ok(Some(v));
                }
            }
            return Err(format!("HTTP {status}: {body}"));
        }

        if content_type
            .as_bytes()
            .starts_with(EVENT_STREAM_MIME.as_bytes())
        {
            let mut stream = response.bytes_stream();
            let mut parser = SseParser::default();
            let mut carry = String::new();
            let mut found = None;
            while let Some(chunk) = stream.next().await {
                let Ok(bytes) = chunk else {
                    break;
                };
                let text = String::from_utf8_lossy(&bytes);
                for v in parser.push_chunk(&text, &mut carry) {
                    if let Some(hit) = ingest(s, v, want_id).await {
                        found = Some(hit);
                    }
                }
            }
            return Ok(found);
        }
        if content_type.as_bytes().starts_with(JSON_MIME.as_bytes()) {
            match response.json::<Value>().await {
                Ok(v) => {
                    if let Some(hit) = ingest(s, v.clone(), want_id).await {
                        return Ok(Some(hit));
                    }
                    if want_id.is_some_and(|id| id_matches(&v, id))
                        || v.get("error").is_some()
                        || v.get("result").is_some()
                    {
                        return Ok(Some(v));
                    }
                    return Ok(None);
                }
                Err(e) => {
                    tracing::warn!("MCP HTTP JSON parse failed, treating as accepted: {e}");
                    return Ok(None);
                }
            }
        } else {
            return Err(format!(
                "MCP HTTP unexpected content type: {content_type:?}"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{McpOAuthConfig, McpServer};
    use serde_json::json;
    use std::io;
    use std::sync::atomic::AtomicU32;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Clone, Copy)]
    enum Mode {
        Modern,
        Legacy,
        UnsupportedThenLegacy,
        Unauthorized,
        Paged,
        SessionDrop,
    }

    async fn connect_test(
        server: &McpServer,
    ) -> Result<(Vec<protocol::ListedTool>, CallFn), String> {
        let (stop_tx, stop_rx) = watch::channel(false);
        let hooks = incoming::dummy_hooks(&server.name);
        let (listed, call, _) = connect(server, hooks, stop_rx).await?;
        std::mem::forget(stop_tx);
        Ok((listed, call))
    }

    async fn spawn_server(mode: Mode) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicU32::new(0));
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let calls = calls.clone();
                tokio::spawn(async move {
                    while let Ok((headers, body)) = read_http(&mut stream).await {
                        let resp = handle_req(mode, &headers, &body, &calls);
                        if stream.write_all(&resp).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        (format!("http://{addr}/mcp"), handle)
    }

    async fn read_http(stream: &mut tokio::net::TcpStream) -> io::Result<(String, Vec<u8>)> {
        let mut buf = Vec::new();
        loop {
            let mut tmp = [0u8; 2048];
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof"));
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = find_header_end(&buf) {
                let header = String::from_utf8_lossy(&buf[..pos]).into_owned();
                let mut content_length = 0usize;
                for line in header.lines().skip(1) {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.eq_ignore_ascii_case("content-length") {
                            content_length = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut body = buf[pos + 4..].to_vec();
                while body.len() < content_length {
                    let mut tmp = [0u8; 2048];
                    let n = stream.read(&mut tmp).await?;
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&tmp[..n]);
                }
                body.truncate(content_length);
                return Ok((header, body));
            }
        }
    }

    fn find_header_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n")
    }

    fn json_response(status: u16, extra_headers: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra_headers}\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn handle_req(mode: Mode, headers: &str, body: &[u8], calls: &AtomicU32) -> Vec<u8> {
        if headers.starts_with("GET ") {
            return b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n".to_vec();
        }
        let v: Value = serde_json::from_slice(body).unwrap_or(json!({}));
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        match mode {
            Mode::Unauthorized => {
                return b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer\r\nContent-Length: 0\r\n\r\n".to_vec();
            }
            Mode::Paged => match method {
                "tools/list" => {
                    let cursor = v.pointer("/params/cursor").and_then(|c| c.as_str());
                    let (tools, next) = if cursor == Some("2") {
                        (
                            json!([{
                                "name": "b",
                                "description": "b",
                                "inputSchema": { "type": "object" }
                            }]),
                            None,
                        )
                    } else {
                        (
                            json!([{
                                "name": "a",
                                "description": "a",
                                "inputSchema": { "type": "object" }
                            }]),
                            Some("2"),
                        )
                    };
                    let mut result = json!({ "tools": tools });
                    if let Some(c) = next {
                        result["nextCursor"] = json!(c);
                    }
                    json_response(
                        200,
                        "",
                        &json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
                    )
                }
                _ => json_response(
                    400,
                    "",
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": method }
                    })
                    .to_string(),
                ),
            },
            Mode::Modern => match method {
                "tools/list" => {
                    assert!(
                        headers
                            .to_ascii_lowercase()
                            .contains("mcp-method: tools/list"),
                        "modern tools/list must send Mcp-Method: {headers}"
                    );
                    assert!(
                        headers.contains("2026-07-28"),
                        "modern request must send 2026-07-28: {headers}"
                    );
                    json_response(
                        200,
                        "",
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "tools": [{
                                    "name": "echo",
                                    "description": "echo text",
                                    "inputSchema": {
                                        "type": "object",
                                        "properties": { "text": { "type": "string" } }
                                    }
                                }]
                            }
                        })
                        .to_string(),
                    )
                }
                "tools/call" => {
                    assert!(
                        headers.to_ascii_lowercase().contains("mcp-name: echo"),
                        "tools/call must send Mcp-Name: {headers}"
                    );
                    let text = v
                        .pointer("/params/arguments/text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    json_response(
                        200,
                        "",
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "content": [{ "type": "text", "text": text }] }
                        })
                        .to_string(),
                    )
                }
                _ => json_response(
                    400,
                    "",
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": method }
                    })
                    .to_string(),
                ),
            },
            Mode::Legacy => legacy_handle(headers, &v, &id, false, calls, false),
            Mode::SessionDrop => legacy_handle(headers, &v, &id, false, calls, true),
            Mode::UnsupportedThenLegacy => {
                let proto = v
                    .pointer("/params/_meta/io.modelcontextprotocol/protocolVersion")
                    .and_then(|p| p.as_str());
                if proto == Some(PROTOCOL_LATEST) {
                    json_response(
                        400,
                        "",
                        &json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32022,
                                "message": "unsupported",
                                "data": { "supported": ["2025-11-25"], "requested": "2026-07-28" }
                            }
                        })
                        .to_string(),
                    )
                } else {
                    legacy_handle(headers, &v, &id, true, calls, false)
                }
            }
        }
    }

    fn legacy_handle(
        headers: &str,
        v: &Value,
        id: &Value,
        after_negotiate: bool,
        calls: &AtomicU32,
        drop_first_call: bool,
    ) -> Vec<u8> {
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let has_session = headers.to_ascii_lowercase().contains("mcp-session-id:");
        if drop_first_call && method == "tools/call" && calls.fetch_add(1, Ordering::Relaxed) == 0 {
            return b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec();
        }
        match method {
            "initialize" => json_response(
                200,
                "Mcp-Session-Id: sess-1\r\n",
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "serverInfo": { "name": "fake", "version": "0" }
                    }
                })
                .to_string(),
            ),
            "notifications/initialized" => {
                b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n".to_vec()
            }
            "tools/list" if has_session || after_negotiate => json_response(
                200,
                "Mcp-Session-Id: sess-1\r\n",
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [{
                            "name": "ping",
                            "description": "ping",
                            "inputSchema": { "type": "object" }
                        }]
                    }
                })
                .to_string(),
            ),
            "tools/list" => b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n".to_vec(),
            "tools/call" => json_response(
                200,
                "Mcp-Session-Id: sess-1\r\n",
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "content": [{ "type": "text", "text": "pong" }] }
                })
                .to_string(),
            ),
            _ => json_response(400, "", "{}"),
        }
    }

    fn http_server(url: String) -> McpServer {
        McpServer {
            name: "local".into(),
            transport: McpTransport::Http {
                url,
                headers: BTreeMap::new(),
            },
            startup_timeout_sec: 5,
            enabled: true,
            oauth: McpOAuthConfig::default(),
        }
    }

    #[tokio::test]
    async fn unauthorized_is_401() {
        let (url, handle) = spawn_server(Mode::Unauthorized).await;
        let err = match connect_test(&http_server(url)).await {
            Ok(_) => panic!("expected 401"),
            Err(e) => e,
        };
        assert!(err.contains("401"), "{err}");
        handle.abort();
    }

    #[tokio::test]
    async fn modern_list_and_call() {
        let (url, handle) = spawn_server(Mode::Modern).await;
        let (listed, call) = connect_test(&http_server(url)).await.expect("connect");
        assert_eq!(listed[0].0, "mcp_local__echo");
        assert!(listed[0].2.contains("text"));
        let result = call(
            "mcp_local__echo".into(),
            ToolCall {
                id: "1".into(),
                name: "mcp_local__echo".into(),
                arguments: r#"{"text":"hi"}"#.into(),
            },
        )
        .await;
        assert_eq!(result.content, "hi");
        handle.abort();
    }

    #[tokio::test]
    async fn legacy_fallback_on_empty_400() {
        let (url, handle) = spawn_server(Mode::Legacy).await;
        let (listed, call) = connect_test(&http_server(url))
            .await
            .expect("legacy connect");
        assert_eq!(listed[0].0, "mcp_local__ping");
        let result = call(
            "mcp_local__ping".into(),
            ToolCall {
                id: "1".into(),
                name: "mcp_local__ping".into(),
                arguments: "{}".into(),
            },
        )
        .await;
        assert_eq!(result.content, "pong");
        handle.abort();
    }

    #[tokio::test]
    async fn unsupported_version_negotiates_legacy() {
        let (url, handle) = spawn_server(Mode::UnsupportedThenLegacy).await;
        let (listed, _) = connect_test(&http_server(url)).await.expect("negotiate");
        assert_eq!(listed[0].0, "mcp_local__ping");
        handle.abort();
    }

    #[tokio::test]
    async fn list_walks_next_cursor() {
        let (url, handle) = spawn_server(Mode::Paged).await;
        let (listed, _) = connect_test(&http_server(url)).await.expect("paged");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].0, "mcp_local__a");
        assert_eq!(listed[1].0, "mcp_local__b");
        handle.abort();
    }

    #[tokio::test]
    async fn session_404_rehandshakes_and_retries() {
        let (url, handle) = spawn_server(Mode::SessionDrop).await;
        let (listed, call) = connect_test(&http_server(url)).await.expect("drop");
        assert_eq!(listed[0].0, "mcp_local__ping");
        let result = call(
            "mcp_local__ping".into(),
            ToolCall {
                id: "1".into(),
                name: "mcp_local__ping".into(),
                arguments: "{}".into(),
            },
        )
        .await;
        assert_eq!(result.content, "pong");
        handle.abort();
    }
}
