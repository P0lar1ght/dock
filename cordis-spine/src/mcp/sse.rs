//! Streamable HTTP standing GET (server → client SSE) and session recovery.
//!
//! GET is how the server pushes `elicitation/create` and `tools/list_changed`
//! while no POST is in flight. A 404 on a POST that already had
//! `Mcp-Session-Id` means the session is gone — handshake again.

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue};
use serde_json::Value;
use tokio::sync::watch;

use crate::config::McpServer;

use super::credentials;
use super::protocol;

const EVENT_STREAM_MIME: &str = "text/event-stream";
const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
const HEADER_PROTOCOL: &str = "MCP-Protocol-Version";

const STABLE_STREAM: Duration = Duration::from_secs(2);
const BASE_DELAY: Duration = Duration::from_millis(500);
const MAX_DELAY: Duration = Duration::from_secs(30);

pub fn is_session_expired(status: u16, had_session: bool) -> bool {
    status == 404 && had_session
}

pub fn reconnect_delay(consecutive_rapid: u32) -> Duration {
    if consecutive_rapid < 2 {
        return Duration::ZERO;
    }
    let exp = consecutive_rapid.saturating_sub(2).min(6);
    (BASE_DELAY * 2u32.pow(exp)).min(MAX_DELAY)
}

/// Incremental SSE parser (GET stream). Complements [`protocol::parse_sse_json_values`]
/// which expects a finished POST body.
#[derive(Default)]
pub struct SseParser {
    data_lines: Vec<String>,
}

impl SseParser {
    pub fn push_line(&mut self, line: &str) -> Vec<Value> {
        let mut out = Vec::new();
        if line.is_empty() {
            if let Some(v) = self.flush() {
                out.push(v);
            }
            return out;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            self.data_lines.push(rest.trim_start().to_string());
        }
        out
    }

    pub fn push_chunk(&mut self, chunk: &str, carry: &mut String) -> Vec<Value> {
        carry.push_str(chunk);
        let mut out = Vec::new();
        while let Some(pos) = carry.find('\n') {
            let mut line = carry[..pos].to_string();
            *carry = carry[pos + 1..].to_string();
            if line.ends_with('\r') {
                line.pop();
            }
            out.extend(self.push_line(&line));
        }
        out
    }

    fn flush(&mut self) -> Option<Value> {
        if self.data_lines.is_empty() {
            return None;
        }
        let data = self.data_lines.join("\n");
        self.data_lines.clear();
        if data.trim() == "[DONE]" {
            return None;
        }
        serde_json::from_str(&data).ok()
    }
}

/// Standing GET until `stop` is true. 405 / 404 without a session ends the loop.
pub async fn listen_get(
    client: reqwest::Client,
    url: String,
    mut protocol: watch::Receiver<String>,
    mut session_id: watch::Receiver<Option<String>>,
    skip_oauth: bool,
    server: McpServer,
    tx: tokio::sync::mpsc::UnboundedSender<Value>,
    mut stop: watch::Receiver<bool>,
) {
    let mut consecutive_rapid = 0u32;
    loop {
        if *stop.borrow() {
            return;
        }
        let sid = session_id.borrow().clone();
        let proto = protocol.borrow().clone();
        let delay = reconnect_delay(consecutive_rapid);
        if !delay.is_zero() {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = stop.changed() => return,
            }
            if *stop.borrow() {
                return;
            }
        }
        let started = std::time::Instant::now();
        match get_once(
            &client,
            &url,
            &proto,
            sid.as_deref(),
            skip_oauth,
            &server,
            &tx,
            &mut stop,
        )
        .await
        {
            GetEnd::Stop => return,
            GetEnd::Unsupported => return,
            GetEnd::Ok | GetEnd::Retry => {
                if started.elapsed() < STABLE_STREAM {
                    consecutive_rapid = consecutive_rapid.saturating_add(1);
                } else {
                    consecutive_rapid = 0;
                }
            }
        }
        tokio::select! {
            _ = session_id.changed() => {}
            _ = protocol.changed() => {}
            _ = stop.changed() => return,
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }
}

enum GetEnd {
    Stop,
    Unsupported,
    Ok,
    Retry,
}

async fn get_once(
    client: &reqwest::Client,
    url: &str,
    protocol: &str,
    session_id: Option<&str>,
    skip_oauth: bool,
    server: &McpServer,
    tx: &tokio::sync::mpsc::UnboundedSender<Value>,
    stop: &mut watch::Receiver<bool>,
) -> GetEnd {
    let mut req = client
        .get(url)
        .header(ACCEPT, EVENT_STREAM_MIME)
        .header(HEADER_PROTOCOL, protocol);
    if let Some(sid) = session_id {
        req = req.header(HEADER_SESSION_ID, sid);
    }
    if !skip_oauth {
        if let Some(token) = credentials::access_token(&server.name, url) {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {token}")) {
                req = req.header(AUTHORIZATION, val);
            }
        }
    }
    let response = match req.send().await {
        Ok(r) => r,
        Err(_) => return GetEnd::Retry,
    };
    let status = response.status().as_u16();
    if *stop.borrow() {
        return GetEnd::Stop;
    }
    if status == 405 || status == 404 && session_id.is_none() {
        return GetEnd::Unsupported;
    }
    if status == 404 {
        return GetEnd::Retry;
    }
    if !response.status().is_success() {
        return GetEnd::Retry;
    }
    let mut stream = response.bytes_stream();
    let mut parser = SseParser::default();
    let mut carry = String::new();
    loop {
        tokio::select! {
            _ = stop.changed() => return GetEnd::Stop,
            chunk = stream.next() => {
                let Some(chunk) = chunk else {
                    return GetEnd::Ok;
                };
                let Ok(bytes) = chunk else {
                    return GetEnd::Retry;
                };
                let text = String::from_utf8_lossy(&bytes);
                for v in parser.push_chunk(&text, &mut carry) {
                    if tx.send(v).is_err() {
                        return GetEnd::Stop;
                    }
                }
            }
        }
    }
}

/// Re-export so POST SSE bodies stay parsed in one place.
pub fn parse_post_sse(body: &str) -> Vec<Value> {
    protocol::parse_sse_json_values(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_404_only_with_existing_id() {
        assert!(is_session_expired(404, true));
        assert!(!is_session_expired(404, false));
        assert!(!is_session_expired(401, true));
    }

    #[test]
    fn backoff_stays_zero_then_grows() {
        assert_eq!(reconnect_delay(0), Duration::ZERO);
        assert_eq!(reconnect_delay(1), Duration::ZERO);
        assert_eq!(reconnect_delay(2), BASE_DELAY);
        assert_eq!(reconnect_delay(3), BASE_DELAY * 2);
        assert_eq!(reconnect_delay(99), MAX_DELAY);
    }

    #[test]
    fn sse_parser_splits_events() {
        let mut p = SseParser::default();
        let mut carry = String::new();
        let vals = p.push_chunk(
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
            &mut carry,
        );
        assert_eq!(vals.len(), 1);
        assert_eq!(vals[0]["method"], "notifications/tools/list_changed");
    }
}
