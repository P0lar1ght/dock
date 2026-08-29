//! Streamable HTTP MCP client (rmcp `post_message` shape, reqwest 0.12).
//!
//! Prefer protocol `2026-07-28` (no session, `_meta` + `MCP-Protocol-Version`
//! + `Mcp-Method` / `Mcp-Name`). On a 400 that is not a modern JSON-RPC error,
//! fall back to initialize-era `2025-11-25` with `Mcp-Session-Id`.

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, USER_AGENT};
use serde_json::{json, Value};

use crate::config::{McpServer, McpTransport};
use crate::mcp::protocol::{
    self, client_info, encode_header_value, id_matches, parse_sse_json_values, pick_version,
    raw_tool_name, tools_from_list, unsupported_versions, with_meta, PROTOCOL_LATEST,
    PROTOCOL_LEGACY,
};
use crate::tools::tool_result;
use crate::types::ToolCall;

use super::CallFn;

const JSON_MIME: &str = "application/json";
const EVENT_STREAM_MIME: &str = "text/event-stream";
const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
const HEADER_PROTOCOL: &str = "MCP-Protocol-Version";
const HEADER_METHOD: &str = "Mcp-Method";
const HEADER_NAME: &str = "Mcp-Name";

const RESERVED_HEADERS: &[&str] = &["accept", "mcp-session-id", "last-event-id"];

pub(super) async fn connect(
    server: &McpServer,
) -> Result<(Vec<protocol::ListedTool>, CallFn), String> {
    let McpTransport::Http { url, headers } = &server.transport else {
        return Err("not an HTTP MCP server".into());
    };
    let client = build_client(&server.name, url, headers)?;
    let timeout = Duration::from_secs(server.startup_timeout_sec.max(1));
    let session = std::sync::Arc::new(tokio::sync::Mutex::new(HttpSession {
        client,
        url: url.clone(),
        protocol: PROTOCOL_LATEST.to_string(),
        modern: true,
        session_id: None,
        next_id: 1,
    }));
    {
        let mut s = session.lock().await;
        let listed = tokio::time::timeout(timeout, handshake(&mut s))
            .await
            .map_err(|_| "MCP HTTP handshake timeout".to_string())??;
        let out = tools_from_list(&server.name, &listed);
        drop(s);
        let session_call = session.clone();
        let call: CallFn = std::sync::Arc::new(move |public: String, c: ToolCall| {
            let session = session_call.clone();
            Box::pin(async move {
                let raw_name = raw_tool_name(&public);
                let args: Value = serde_json::from_str(&c.arguments).unwrap_or(json!({}));
                let mut s = session.lock().await;
                match s
                    .rpc("tools/call", json!({ "name": raw_name, "arguments": args }))
                    .await
                {
                    Ok(v) => tool_result(c, protocol::format_call_result(&v)),
                    Err(e) => tool_result(c, e),
                }
            })
        });
        Ok((out, call))
    }
}

async fn handshake(s: &mut HttpSession) -> Result<Value, String> {
    match s.rpc("tools/list", json!({})).await {
        Ok(v) if v.get("error").is_none() => Ok(v),
        Ok(v) => {
            if let Some(supported) = unsupported_versions(&v) {
                return negotiate(s, &supported).await;
            }
            if looks_legacy_error(&v) {
                initialize_legacy(s).await?;
                return list_after_legacy(s).await;
            }
            Err(v["error"].to_string())
        }
        Err(e) if looks_legacy_http(&e) => {
            initialize_legacy(s).await?;
            list_after_legacy(s).await
        }
        Err(e) => Err(e),
    }
}

async fn negotiate(s: &mut HttpSession, supported: &[String]) -> Result<Value, String> {
    let picked = pick_version(supported)
        .ok_or_else(|| format!("server does not speak a protocol Dock supports: {supported:?}"))?;
    s.protocol = picked.to_string();
    s.modern = protocol::is_modern(picked);
    if s.modern {
        let listed = s.rpc("tools/list", json!({})).await?;
        if listed.get("error").is_some() {
            return Err(listed["error"].to_string());
        }
        Ok(listed)
    } else {
        initialize_legacy(s).await?;
        list_after_legacy(s).await
    }
}

async fn list_after_legacy(s: &mut HttpSession) -> Result<Value, String> {
    let listed = s.rpc("tools/list", json!({})).await?;
    if listed.get("error").is_some() {
        return Err(listed["error"].to_string());
    }
    Ok(listed)
}

async fn initialize_legacy(s: &mut HttpSession) -> Result<(), String> {
    s.modern = false;
    s.session_id = None;
    if protocol::is_modern(&s.protocol) {
        s.protocol = PROTOCOL_LEGACY.to_string();
    }
    let init = s
        .rpc(
            "initialize",
            json!({
                "protocolVersion": s.protocol,
                "capabilities": {},
                "clientInfo": client_info(),
            }),
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
        s.protocol = pv.to_string();
    }
    s.notify("notifications/initialized", json!({})).await.ok();
    Ok(())
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

struct HttpSession {
    client: reqwest::Client,
    url: String,
    protocol: String,
    modern: bool,
    session_id: Option<String>,
    next_id: u64,
}

enum PostOutcome {
    Accepted,
    Json(Value),
}

impl HttpSession {
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let params = if self.modern && method != "initialize" {
            with_meta(params, &self.protocol)
        } else {
            params
        };
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        match self.post(method, &params, &msg).await? {
            PostOutcome::Accepted => Err("empty MCP HTTP response".into()),
            PostOutcome::Json(v) => {
                if id_matches(&v, id) || v.get("id").is_none() {
                    Ok(v)
                } else {
                    Ok(v)
                }
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        match self.post(method, &params, &msg).await? {
            PostOutcome::Accepted | PostOutcome::Json(_) => Ok(()),
        }
    }

    async fn post(
        &mut self,
        method: &str,
        params: &Value,
        message: &Value,
    ) -> Result<PostOutcome, String> {
        let mut req = self
            .client
            .post(&self.url)
            .header(ACCEPT, format!("{EVENT_STREAM_MIME}, {JSON_MIME}"))
            .header(HEADER_PROTOCOL, self.protocol.as_str());
        if self.modern {
            req = req.header(HEADER_METHOD, method);
            if method == "tools/call" {
                if let Some(name) = params.get("name").and_then(|n| n.as_str()) {
                    req = req.header(HEADER_NAME, encode_header_value(name));
                }
            }
        }
        if let Some(sid) = &self.session_id {
            req = req.header(HEADER_SESSION_ID, sid.as_str());
        }
        let had_session = self.session_id.is_some();
        let response = req
            .json(message)
            .send()
            .await
            .map_err(|e| format!("MCP HTTP {e}"))?;
        let status = response.status();
        if let Some(sid) = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
        {
            self.session_id = Some(sid.to_string());
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
        if status.as_u16() == 404 && had_session {
            return Err("MCP HTTP session expired".into());
        }
        if matches!(status.as_u16(), 202 | 204) {
            return Ok(PostOutcome::Accepted);
        }

        if status.is_success() && content_length == Some(0) {
            return Ok(PostOutcome::Accepted);
        }

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read body>".into());
            if let Ok(v) = serde_json::from_str::<Value>(&body) {
                if v.get("error").is_some() || v.get("result").is_some() {
                    return Ok(PostOutcome::Json(v));
                }
            }
            return Err(format!("HTTP {status}: {body}"));
        }

        if content_type
            .as_bytes()
            .starts_with(EVENT_STREAM_MIME.as_bytes())
        {
            let body = response.text().await.map_err(|e| e.to_string())?;
            let id = message.get("id").and_then(|i| i.as_u64());
            for v in parse_sse_json_values(&body) {
                if id.is_none_or(|want| id_matches(&v, want)) {
                    return Ok(PostOutcome::Json(v));
                }
            }
            return Err("MCP HTTP SSE had no JSON-RPC response".into());
        }
        if content_type.as_bytes().starts_with(JSON_MIME.as_bytes()) {
            match response.json::<Value>().await {
                Ok(v) => Ok(PostOutcome::Json(v)),
                Err(e) => {
                    tracing::warn!("MCP HTTP JSON parse failed, treating as accepted: {e}");
                    Ok(PostOutcome::Accepted)
                }
            }
        } else {
            Err(format!(
                "MCP HTTP unexpected content type: {content_type:?}"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::McpServer;
    use serde_json::json;
    use std::io;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Clone, Copy)]
    enum Mode {
        Modern,
        Legacy,
        UnsupportedThenLegacy,
    }

    async fn spawn_server(mode: Mode) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    while let Ok((headers, body)) = read_http(&mut stream).await {
                        let resp = handle_req(mode, &headers, &body);
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

    fn handle_req(mode: Mode, headers: &str, body: &[u8]) -> Vec<u8> {
        let v: Value = serde_json::from_slice(body).unwrap_or(json!({}));
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        match mode {
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
            Mode::Legacy => legacy_handle(headers, &v, &id, false),
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
                    legacy_handle(headers, &v, &id, true)
                }
            }
        }
    }

    fn legacy_handle(headers: &str, v: &Value, id: &Value, after_negotiate: bool) -> Vec<u8> {
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let has_session = headers.to_ascii_lowercase().contains("mcp-session-id:");
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
        }
    }

    #[tokio::test]
    async fn modern_list_and_call() {
        let (url, handle) = spawn_server(Mode::Modern).await;
        let (listed, call) = connect(&http_server(url)).await.expect("connect");
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
        let (listed, call) = connect(&http_server(url)).await.expect("legacy connect");
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
        let (listed, _) = connect(&http_server(url)).await.expect("negotiate");
        assert_eq!(listed[0].0, "mcp_local__ping");
        handle.abort();
    }
}
