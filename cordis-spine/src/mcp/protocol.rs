//! MCP wire helpers. Preferred protocol is `2026-07-28` (stateless).
//! Initialize-era (`2025-11-25` and earlier) is a fallback only.

use base64::Engine;
use serde_json::{json, Value};

/// Latest published MCP spec. Offered first on both stdio and Streamable HTTP.
pub const PROTOCOL_LATEST: &str = "2026-07-28";
/// Last initialize-era revision (Grok's pin). Used when the server has no 2026 support.
pub const PROTOCOL_LEGACY: &str = "2025-11-25";

const META_PROTOCOL: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
const META_CLIENT_CAPS: &str = "io.modelcontextprotocol/clientCapabilities";

pub const ERR_UNSUPPORTED_PROTOCOL: i64 = -32022;
pub const ERR_METHOD_NOT_FOUND: i64 = -32601;

/// Newest first. Dock speaks modern `2026-07-28` plus initialize-era fallbacks.
pub const SPOKEN: &[&str] = &[
    PROTOCOL_LATEST,
    PROTOCOL_LEGACY,
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];

pub type ListedTool = (String, String, String);

/// Model-facing MCP name: `mcp_{server}__{tool}`.
pub const MCP_NAME_PREFIX: &str = "mcp_";
pub const MCP_TOOL_NAME_DELIMITER: &str = "__";

pub fn public_tool_name(server: &str, raw: &str) -> String {
    format!("{MCP_NAME_PREFIX}{server}{MCP_TOOL_NAME_DELIMITER}{raw}")
}

pub fn is_mcp_public_name(name: &str) -> bool {
    name.starts_with(MCP_NAME_PREFIX) && name.contains(MCP_TOOL_NAME_DELIMITER)
}

pub fn client_info() -> Value {
    json!({
        "name": "dock",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// Host capabilities advertised on `initialize` and in `_meta`.
pub fn client_capabilities() -> Value {
    json!({
        "elicitation": {
            "form": {},
            "url": {}
        }
    })
}

pub fn request_meta(protocol: &str) -> Value {
    json!({
        META_PROTOCOL: protocol,
        META_CLIENT_INFO: client_info(),
        META_CLIENT_CAPS: client_capabilities(),
    })
}

pub fn with_meta(mut params: Value, protocol: &str) -> Value {
    if !params.is_object() {
        params = json!({});
    }
    params["_meta"] = request_meta(protocol);
    params
}

pub fn is_modern(protocol: &str) -> bool {
    protocol >= PROTOCOL_LATEST
}

pub fn jsonrpc_error_code(v: &Value) -> Option<i64> {
    v.pointer("/error/code").and_then(|c| c.as_i64())
}

pub fn unsupported_versions(v: &Value) -> Option<Vec<String>> {
    if jsonrpc_error_code(v) != Some(ERR_UNSUPPORTED_PROTOCOL) {
        return None;
    }
    v.pointer("/error/data/supported")
        .and_then(|s| s.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
}

pub fn discover_versions(result: &Value) -> Vec<String> {
    let from = result
        .pointer("/result/supportedVersions")
        .or_else(|| result.pointer("/result/supported"));
    from.and_then(|s| s.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn pick_version(supported: &[String]) -> Option<&'static str> {
    SPOKEN
        .iter()
        .copied()
        .find(|v| supported.iter().any(|s| s == v))
}

pub fn id_matches(v: &Value, id: u64) -> bool {
    match v.get("id") {
        Some(Value::Number(n)) => n.as_u64() == Some(id) || n.as_i64() == Some(id as i64),
        Some(Value::String(s)) => s.parse::<u64>().ok() == Some(id),
        _ => false,
    }
}

/// Server → client JSON-RPC on a shared stream (stdio / SSE).
#[derive(Debug, Clone)]
pub enum Incoming {
    Response(Value),
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

pub fn classify(v: Value) -> Incoming {
    let method = v
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    if method.is_empty() {
        return Incoming::Response(v);
    }
    if v.get("id").is_none_or(|id| id.is_null()) {
        let params = v.get("params").cloned().unwrap_or(json!({}));
        Incoming::Notification { method, params }
    } else {
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        let params = v.get("params").cloned().unwrap_or(json!({}));
        Incoming::Request { id, method, params }
    }
}

pub fn jsonrpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn jsonrpc_method_not_found(id: Value, method: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": ERR_METHOD_NOT_FOUND, "message": method }
    })
}

pub fn ping_result() -> Value {
    json!({})
}

pub fn tools_from_list(server_name: &str, listed: &Value) -> Vec<ListedTool> {
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
        let schema = tool
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object"}));
        out.push((
            public_tool_name(server_name, name),
            desc.to_string(),
            schema.to_string(),
        ));
    }
    out
}

pub fn format_call_result(v: &Value) -> String {
    if let Some(err) = v.get("error") {
        return err.to_string();
    }
    let Some(content) = v.pointer("/result/content").and_then(|c| c.as_array()) else {
        return v
            .pointer("/result")
            .map(|r| r.to_string())
            .unwrap_or_else(|| v.to_string());
    };
    let texts: Vec<&str> = content
        .iter()
        .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
        .collect();
    if texts.is_empty() {
        serde_json::to_string(content).unwrap_or_else(|_| {
            content
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })
    } else {
        texts.join("\n")
    }
}

pub fn raw_tool_name(public: &str) -> String {
    public
        .rsplit_once("__")
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| public.to_string())
}

/// Streamable HTTP `Mcp-Name` / `Mcp-Param-*` encoding (2026-07-28).
pub fn encode_header_value(value: &str) -> String {
    let sentinel = value.starts_with("=?base64?") && value.ends_with("?=");
    let padded = value.starts_with([' ', '\t']) || value.ends_with([' ', '\t']);
    let unsafe_char = !value.chars().all(|c| {
        let u = c as u32;
        u == 0x09 || u == 0x20 || (0x21..=0x7E).contains(&u)
    });
    if sentinel || padded || unsafe_char {
        format!(
            "=?base64?{}?=",
            base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
        )
    } else {
        value.to_string()
    }
}

pub fn parse_sse_json_values(body: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut data_lines: Vec<String> = Vec::new();
    let mut flush = |data_lines: &mut Vec<String>| {
        if data_lines.is_empty() {
            return;
        }
        let data = data_lines.join("\n");
        data_lines.clear();
        if data.trim() == "[DONE]" {
            return;
        }
        if let Ok(v) = serde_json::from_str(&data) {
            out.push(v);
        }
    };
    for line in body.lines() {
        if line.is_empty() {
            flush(&mut data_lines);
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start().to_string());
        }
    }
    flush(&mut data_lines);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_prefers_latest_spoken() {
        let supported = vec![
            "2024-11-05".into(),
            "2026-07-28".into(),
            "2025-11-25".into(),
        ];
        assert_eq!(pick_version(&supported), Some(PROTOCOL_LATEST));
    }

    #[test]
    fn pick_falls_back_to_legacy() {
        let supported = vec!["2025-11-25".into(), "2025-03-26".into()];
        assert_eq!(pick_version(&supported), Some(PROTOCOL_LEGACY));
    }

    #[test]
    fn encode_plain_ascii_name() {
        assert_eq!(encode_header_value("get_weather"), "get_weather");
    }

    #[test]
    fn encode_non_ascii_name() {
        assert!(encode_header_value("你好").starts_with("=?base64?"));
    }

    #[test]
    fn sse_picks_data_json() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n";
        let vals = parse_sse_json_values(body);
        assert_eq!(vals.len(), 1);
        assert_eq!(vals[0]["id"], 1);
    }

    #[test]
    fn format_joins_text_content() {
        let v = json!({
            "result": { "content": [
                {"type":"text","text":"a"},
                {"type":"text","text":"b"}
            ]}
        });
        assert_eq!(format_call_result(&v), "a\nb");
    }

    #[test]
    fn with_meta_sets_latest() {
        let p = with_meta(json!({"name": "echo"}), PROTOCOL_LATEST);
        assert_eq!(p["_meta"][META_PROTOCOL], PROTOCOL_LATEST);
        assert_eq!(p["name"], "echo");
    }

    #[test]
    fn public_mcp_name_prefixes_server() {
        assert_eq!(public_tool_name("local", "echo"), "mcp_local__echo");
        assert_eq!(raw_tool_name("mcp_local__echo"), "echo");
        assert!(is_mcp_public_name("mcp_local__sungods_search"));
        assert!(!is_mcp_public_name("bash"));
    }
}
