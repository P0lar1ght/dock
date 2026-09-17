//! MCP `tools/list` pagination (`nextCursor`) and `notifications/tools/list_changed`.

use std::future::Future;
use std::time::Duration;

use serde_json::{json, Value};

use super::protocol::{self, ListedTool};

pub const METHOD_CHANGED: &str = "notifications/tools/list_changed";
const MAX_PAGES: usize = 64;

/// Coalesce a burst of `tools/list_changed` before relisting.
pub fn coalesce_delay() -> Duration {
    Duration::from_millis(50)
}

pub fn is_changed(method: &str) -> bool {
    method == METHOD_CHANGED
}

pub fn list_params(cursor: Option<&str>) -> Value {
    match cursor {
        Some(c) if !c.is_empty() => json!({ "cursor": c }),
        _ => json!({}),
    }
}

pub fn next_cursor(listed: &Value) -> Option<String> {
    listed
        .pointer("/result/nextCursor")
        .or_else(|| listed.pointer("/result/next_cursor"))
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Continue after a first `tools/list` result (the handshake probe).
pub async fn list_all_from_first<F, Fut>(
    server_name: &str,
    first: Value,
    mut rpc: F,
) -> Result<Vec<ListedTool>, String>
where
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    if first.get("error").is_some() {
        return Err(first["error"].to_string());
    }
    let mut out = protocol::tools_from_list(server_name, &first);
    let mut cursor = next_cursor(&first);
    for _ in 1..MAX_PAGES {
        let Some(c) = cursor else {
            return Ok(out);
        };
        let listed = rpc(list_params(Some(&c))).await?;
        if listed.get("error").is_some() {
            return Err(listed["error"].to_string());
        }
        out.extend(protocol::tools_from_list(server_name, &listed));
        cursor = next_cursor(&listed);
    }
    Err("tools/list exceeded page limit".into())
}

/// Walk `tools/list` until the server stops sending a cursor.
pub async fn list_all<F, Fut>(server_name: &str, mut rpc: F) -> Result<Vec<ListedTool>, String>
where
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    let mut cursor = None;
    let mut out = Vec::new();
    for _ in 0..MAX_PAGES {
        let listed = rpc(list_params(cursor.as_deref())).await?;
        if listed.get("error").is_some() {
            return Err(listed["error"].to_string());
        }
        out.extend(protocol::tools_from_list(server_name, &listed));
        match next_cursor(&listed) {
            Some(next) => cursor = Some(next),
            None => return Ok(out),
        }
    }
    Err("tools/list exceeded page limit".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cursor_round_trip() {
        assert_eq!(list_params(None), json!({}));
        assert_eq!(list_params(Some("abc")), json!({ "cursor": "abc" }));
        let page = json!({ "result": { "tools": [], "nextCursor": "p2" } });
        assert_eq!(next_cursor(&page).as_deref(), Some("p2"));
        assert!(next_cursor(&json!({ "result": { "tools": [] } })).is_none());
    }

    #[test]
    fn changed_method() {
        assert!(is_changed("notifications/tools/list_changed"));
        assert!(!is_changed("notifications/resources/list_changed"));
    }

    #[tokio::test]
    async fn list_all_walks_pages() {
        let mut calls = 0usize;
        let listed = list_all("local", |params| {
            calls += 1;
            let cursor = params.get("cursor").and_then(|c| c.as_str());
            let body = if cursor.is_none() {
                json!({
                    "result": {
                        "tools": [{ "name": "a", "description": "a", "inputSchema": {} }],
                        "nextCursor": "2"
                    }
                })
            } else {
                json!({
                    "result": {
                        "tools": [{ "name": "b", "description": "b", "inputSchema": {} }]
                    }
                })
            };
            async move { Ok(body) }
        })
        .await
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(listed[0].0, "mcp_local__a");
        assert_eq!(listed[1].0, "mcp_local__b");
    }
}
