//! Streamable HTTP MCP client (rmcp `post_message` shape, reqwest 0.12).
//!
//! Prefer protocol `2026-07-28` (no session; `_meta`, `MCP-Protocol-Version`,
//! `Mcp-Method` / `Mcp-Name`). On a 400 that is not a modern JSON-RPC error,
//! fall back to initialize-era `2025-11-25` with `Mcp-Session-Id`.
//! Standing GET SSE + session 404 recover.
//!
//! POST SSE **不再**串行等 elicitation 完成。原来的「按序处理，elicitation 先
//! 完成再处理该流上的后续事件」在真实场景下是死锁源：`elicitation/create` 要等
//! 用户填完整张表单，这期间流的消费停摆，服务器发来的任何东西（包括本次
//! `tools/call` 的结果）都读不到，表单填得越久越像卡死。改成把服务器请求交给
//! 单独的任务处理、回复走 `HttpShared::reply_tx`。JSON-RPC 按 id 配对，顺序不是
//! 正确性的前提；服务器也不会在拿到 elicitation 答复前发依赖它的事件。

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch};

use crate::tools::mcp::protocol::{
    self, client_capabilities, client_info, encode_header_value, id_matches, pick_version,
    raw_tool_name, unsupported_versions, with_meta, Incoming, PROTOCOL_LATEST, PROTOCOL_LEGACY,
};
use crate::tools::registry::{tool_result, tool_result_with_images};
use cordis_base::config::{McpServer, McpTransport};
use cordis_base::types::ToolCall;

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
    let (reply_tx, mut reply_rx) = mpsc::unbounded_channel::<Value>();
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
        call_lock: Arc::new(tokio::sync::Mutex::new(())),
        handshake: tokio::sync::Mutex::new(()),
        reply_tx,
    });
    // 回复写侧：所有 sender 随 Arc<HttpShared> 一起 drop 时自然收尾。
    let reply_shared = shared.clone();
    tokio::spawn(async move {
        while let Some(reply) = reply_rx.recv().await {
            let _ = post_raw(&reply_shared, &reply).await;
        }
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
            // 锁单独拿一份 Arc：守卫如果借在 `session` 的字段上，就没法再把
            // 整个 `session` 借给 `rpc`。
            //
            // 同一服务器一次只跑一页的调用。elicitation 的来源页是「当前在飞
            // 的调用」里最后 push 的那一页；两页同时打进来，提问会盖到后一页。
            // 锁覆盖整次 `rpc`（含服务器等用户填完表单才回的结果），后一页排队。
            let lock = Arc::clone(&session.call_lock);
            let _call_lock = lock.lock().await;
            let raw_name = raw_tool_name(&public);
            let args: Value = serde_json::from_str(&c.arguments).unwrap_or(json!({}));
            let _page = session.hooks.elicit.scope_caller();
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
    /// 一次只让一页的 `tools/call` 在飞（见 call fn）。
    ///
    /// `Arc` 是为了让守卫不借 `HttpShared` 本身，调用期间还能把 `self` 传给 `rpc`。
    call_lock: Arc<tokio::sync::Mutex<()>>,
    handshake: tokio::sync::Mutex<()>,
    /// 服务器发来的请求（`elicitation/create`）的回复出口。
    ///
    /// 不能在 `ingest` 里直接 `post_raw` 回复：那会让读侧一直等到用户填完整张
    /// 表单。走通道还顺带打断了 `ingest → post_raw → post → ingest` 这个递归
    /// async 环 —— 有环的时候编译器推不出 `Send`，什么都 spawn 不出去。
    reply_tx: mpsc::UnboundedSender<Value>,
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
        // 结果走 SSE 回来：和 stdio 一样要有兜底 deadline，否则服务器不回就是
        // 永久挂起，无人值守时没有任何恢复手段。
        Ok(None) => {
            let Some(budget) = protocol::call_timeout() else {
                return rx.await.map_err(|_| "MCP HTTP closed".into());
            };
            match tokio::time::timeout(budget, rx).await {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(_)) => Err("MCP HTTP closed".into()),
                Err(_) => {
                    s.pending.lock().unwrap().remove(&id);
                    let _ = Box::pin(post_raw(
                        s,
                        &protocol::cancelled_notification(id, "client timeout"),
                    ))
                    .await;
                    Err(format!(
                        "MCP {method} 超时（{}s 未响应）。调长或取消上限：{}=<秒数>（0 = 不限）",
                        budget.as_secs(),
                        protocol::CALL_TIMEOUT_ENV
                    ))
                }
            }
        }
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
            // 同 stdio：`elicitation/create` 会一直等到用户填完整张表，在这里
            // await 会把 SSE 的消费停摆，服务器发来的任何东西（包括这次
            // tools/call 的结果）都读不到。JSON-RPC 按 id 配对，回复不需要保序。
            let hooks = s.hooks.clone();
            let tx = s.reply_tx.clone();
            tokio::spawn(async move {
                let reply = incoming::request(&hooks, id, method, params).await;
                let _ = tx.send(reply);
            });
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
    use cordis_base::config::{McpOAuthConfig, McpServer};
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
        /// `tools/call` 的回复是一条 SSE 流，里面先是一个**没人会回答**的
        /// `elicitation/create`，然后才是调用结果。用来钉住「读侧不被
        /// elicitation 挡住」。
        ElicitThenResult,
    }

    async fn connect_with(
        server: &McpServer,
        hooks: LiveHooks,
    ) -> Result<(Vec<protocol::ListedTool>, CallFn), String> {
        let (stop_tx, stop_rx) = watch::channel(false);
        let (listed, call, _) = connect(server, hooks, stop_rx).await?;
        std::mem::forget(stop_tx);
        Ok((listed, call))
    }

    async fn connect_test(
        server: &McpServer,
    ) -> Result<(Vec<protocol::ListedTool>, CallFn), String> {
        connect_with(server, incoming::dummy_hooks(&server.name)).await
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

    /// 一条 SSE 响应，按顺序携带若干 JSON-RPC 消息。
    fn sse_response(events: &[Value]) -> Vec<u8> {
        let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
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
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer\r\nContent-Length: 0\r\n\r\n".to_vec()
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
            Mode::ElicitThenResult => match method {
                "tools/list" => json_response(
                    200,
                    "",
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "tools": [{
                            "name": "echo",
                            "description": "echo text",
                            "inputSchema": { "type": "object" }
                        }] }
                    })
                    .to_string(),
                ),
                "tools/call" => sse_response(&[
                    // 没有 TUI 来回答它 —— `Elicitation::create` 会一直挂着。
                    json!({
                        "jsonrpc": "2.0",
                        "id": 9001,
                        "method": "elicitation/create",
                        "params": {
                            "message": "确认？",
                            "requestedSchema": {
                                "type": "object",
                                "properties": { "ok": { "type": "boolean" } }
                            }
                        }
                    }),
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "content": [{ "type": "text", "text": "结果送达" }] }
                    }),
                ]),
                _ => json_response(200, "", &json!({ "jsonrpc": "2.0", "id": id, "result": {} }).to_string()),
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

    /// 服务器在同一条 SSE 流上先发 `elicitation/create` 再发调用结果。
    ///
    /// 改之前 `ingest` 会在读侧内联 await `incoming::request`，而
    /// `elicitation/create` 要等用户填完整张表单 —— 没有 TUI 就是永远。结果事件
    /// 排在它后面，于是 `tools/call` 永久挂起：工具调用卡住、整轮对话结束不了，
    /// 正是线上报的那个现象。
    #[tokio::test]
    async fn elicitation_does_not_block_the_call_result() {
        let (url, handle) = spawn_server(Mode::ElicitThenResult).await;
        let (_, call) = connect_test(&http_server(url)).await.expect("connect");
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            call(
                "mcp_local__echo".into(),
                ToolCall {
                    id: "1".into(),
                    name: "mcp_local__echo".into(),
                    arguments: "{}".into(),
                },
            ),
        )
        .await
        .expect("结果不该被没人回答的 elicitation 挡住");
        assert_eq!(result.content, "结果送达");
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

    /// 两页同时打同一台服务器时，先发起那页的提问必须留在它自己的页上。
    ///
    /// 服务器把第一次 `tools/call` 按住，等第二次进来（或超时），再只先推
    /// `elicitation/create`，结果留到测试看完提问才发。这样盖来源页的时候第一次
    /// 调用还在飞。没加锁时第二次已经 push 了来源页，提问盖到后一页；加锁后
    /// 第二次还排着队，提问留在第一页，且两次调用不重叠。
    #[tokio::test]
    async fn concurrent_calls_keep_elicitation_on_the_calling_page() {
        let overlap = Arc::new(CallOverlap::default());
        let (url, handle) = spawn_exclusive_server(overlap.clone()).await;
        let hooks = incoming::dummy_hooks("local");
        let elicit = hooks.elicit.clone();
        let (_, call) = connect_with(&http_server(url), hooks)
            .await
            .expect("connect");

        let call_a = call.clone();
        let call_b = call;
        let main = page_ctx("main");
        let other = page_ctx("main#2");

        let a = tokio::spawn(async move {
            crate::tools::registry::with_exec_ctx_async(main, async move {
                call_a(
                    "mcp_local__echo".into(),
                    ToolCall {
                        id: "a".into(),
                        name: "mcp_local__echo".into(),
                        arguments: "{}".into(),
                    },
                )
                .await
            })
            .await
        });

        tokio::time::timeout(Duration::from_secs(3), async {
            while !overlap.first_seen.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("first call never reached the server");

        let b = tokio::spawn(async move {
            crate::tools::registry::with_exec_ctx_async(other, async move {
                call_b(
                    "mcp_local__echo".into(),
                    ToolCall {
                        id: "b".into(),
                        name: "mcp_local__echo".into(),
                        arguments: "{}".into(),
                    },
                )
                .await
            })
            .await
        });

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if elicit.front_for(Some("main")).is_some()
                    || elicit.front_for(Some("main#2")).is_some()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("elicitation never queued");
        assert!(
            elicit.front_for(Some("main")).is_some(),
            "提问该盖在先调用的那一页"
        );
        assert!(
            elicit.front_for(Some("main#2")).is_none(),
            "后一页不该看见先一页的提问"
        );
        assert_eq!(
            overlap.max_inflight.load(Ordering::SeqCst),
            1,
            "同一连接的 tools/call 必须串行"
        );
        overlap.release_first.store(true, Ordering::SeqCst);

        let a = tokio::time::timeout(Duration::from_secs(5), a)
            .await
            .expect("call a timed out")
            .expect("call a join");
        let b = tokio::time::timeout(Duration::from_secs(5), b)
            .await
            .expect("call b timed out")
            .expect("call b join");
        assert_eq!(a.content, "先一页");
        assert_eq!(b.content, "后一页");
        elicit.cancel();
        handle.abort();
    }

    #[derive(Default)]
    struct CallOverlap {
        calls: AtomicU32,
        inflight: AtomicU32,
        max_inflight: AtomicU32,
        first_seen: AtomicBool,
        second_arrived: AtomicBool,
        first_done: AtomicBool,
        /// 测试看完提问之后才放行第一次调用的结果。
        release_first: AtomicBool,
    }

    /// 第一次调用按住，给第二次一个重叠窗口；elicitation 与结果拆成两段 SSE，
    /// 结果要等测试看完提问才写。第二次停到第一次整段响应写完。
    async fn spawn_exclusive_server(
        overlap: Arc<CallOverlap>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let _ = stream.set_nodelay(true);
                let overlap = overlap.clone();
                tokio::spawn(async move {
                    while let Ok((headers, body)) = read_http(&mut stream).await {
                        if !exclusive_turn(&mut stream, &headers, &body, &overlap).await {
                            break;
                        }
                    }
                });
            }
        });
        (format!("http://{addr}/mcp"), handle)
    }

    async fn exclusive_turn(
        stream: &mut tokio::net::TcpStream,
        headers: &str,
        body: &[u8],
        overlap: &CallOverlap,
    ) -> bool {
        if headers.starts_with("GET ") {
            return stream
                .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n")
                .await
                .is_ok();
        }
        let v: Value = serde_json::from_slice(body).unwrap_or(json!({}));
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        if method != "tools/call" {
            let resp = if method == "tools/list" {
                json_response(
                    200,
                    "",
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "tools": [{
                            "name": "echo",
                            "description": "echo",
                            "inputSchema": { "type": "object" }
                        }] }
                    })
                    .to_string(),
                )
            } else {
                json_response(
                    200,
                    "",
                    &json!({ "jsonrpc": "2.0", "id": id, "result": {} }).to_string(),
                )
            };
            return stream.write_all(&resp).await.is_ok();
        }

        let seq = overlap.calls.fetch_add(1, Ordering::SeqCst);
        let now = overlap.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        overlap.max_inflight.fetch_max(now, Ordering::SeqCst);
        let ok = if seq == 0 {
            overlap.first_seen.store(true, Ordering::SeqCst);
            let wait_second = async {
                while !overlap.second_arrived.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            };
            let _ = tokio::time::timeout(Duration::from_millis(500), wait_second).await;
            held_elicit_then_result(stream, &id, overlap).await
        } else {
            overlap.second_arrived.store(true, Ordering::SeqCst);
            let wait_first = async {
                while !overlap.first_done.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            };
            let _ = tokio::time::timeout(Duration::from_secs(3), wait_first).await;
            let resp = json_response(
                200,
                "",
                &json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "content": [{ "type": "text", "text": "后一页" }] }
                })
                .to_string(),
            );
            stream.write_all(&resp).await.is_ok()
        };
        overlap.inflight.fetch_sub(1, Ordering::SeqCst);
        ok
    }

    /// 先把 elicitation 推出去并停住，等测试看完提问再写调用结果。
    /// 结果如果跟 elicitation 在同一段里写完，`create` 还没盖上来源页调用就返回了。
    async fn held_elicit_then_result(
        stream: &mut tokio::net::TcpStream,
        id: &Value,
        overlap: &CallOverlap,
    ) -> bool {
        let header = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n";
        if stream.write_all(header).await.is_err() {
            return false;
        }
        let elicit = json!({
            "jsonrpc": "2.0",
            "id": 9001,
            "method": "elicitation/create",
            "params": {
                "message": "确认？",
                "requestedSchema": {
                    "type": "object",
                    "properties": { "ok": { "type": "boolean" } }
                }
            }
        });
        if !write_sse_chunk(stream, &elicit).await {
            return false;
        }
        let wait_release = async {
            while !overlap.release_first.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(3), wait_release).await;
        let result = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": [{ "type": "text", "text": "先一页" }] }
        });
        let ok =
            write_sse_chunk(stream, &result).await && stream.write_all(b"0\r\n\r\n").await.is_ok();
        overlap.first_done.store(true, Ordering::SeqCst);
        ok
    }

    async fn write_sse_chunk(stream: &mut tokio::net::TcpStream, event: &Value) -> bool {
        let payload = format!("data: {event}\n\n");
        let frame = format!("{:X}\r\n{payload}\r\n", payload.len());
        stream.write_all(frame.as_bytes()).await.is_ok()
    }

    fn page_ctx(identity: &str) -> cordis::Context {
        let ctx = cordis::Context::new();
        let sessions = if identity == "main" {
            crate::Sessions::new(ctx.clone())
        } else {
            let n: usize = identity
                .strip_prefix("main#")
                .and_then(|n| n.parse().ok())
                .unwrap_or(2);
            crate::Sessions::tab(ctx.clone(), n)
        };
        // Drop 不会反注册，服务留在这份 ctx 上。
        let _reg = ctx
            .provide(crate::SESSIONS, sessions)
            .expect("provide sessions");
        ctx
    }
}
