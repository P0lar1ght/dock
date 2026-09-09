//! MCP wire helpers. Preferred protocol is `2026-07-28` (stateless).
//! Initialize-era (`2025-11-25` and earlier) is a fallback only.

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use base64::Engine;
use serde_json::{json, Value};

use crate::tool_images::{self, IMAGE_SEPARATE_PLACEHOLDER};
use crate::types::UserImage;

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

/// Split `mcp_{server}__{tool}` into `(server, tool)`.
pub fn split_mcp_public_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix(MCP_NAME_PREFIX)?;
    rest.split_once(MCP_TOOL_NAME_DELIMITER)
        .filter(|(server, tool)| !server.is_empty() && !tool.is_empty())
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
    format_call_result_parts(v).0
}

/// Text + extracted images for a tools/call JSON-RPC result.
/// Keeps `type:image` (and CUA `png_base64` / path) instead of dropping them.
pub fn format_call_result_parts(v: &Value) -> (String, Vec<UserImage>) {
    if let Some(err) = v.get("error") {
        return (err.to_string(), Vec::new());
    }
    let Some(content) = v.pointer("/result/content").and_then(|c| c.as_array()) else {
        let text = v
            .pointer("/result")
            .map(|r| r.to_string())
            .unwrap_or_else(|| v.to_string());
        let (text, images) = extract_from_text_and_structured(&text, v.pointer("/result"));
        return (text, images);
    };

    let mut texts: Vec<String> = Vec::new();
    let mut images: Vec<UserImage> = Vec::new();

    for item in content {
        let kind = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match kind {
            "image" => {
                if let Some(img) = image_from_mcp_item(item) {
                    images.push(img);
                    texts.push(IMAGE_SEPARATE_PLACEHOLDER.to_string());
                }
            }
            "text" => {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    let (cleaned, extracted) = extract_data_uri_images(t);
                    texts.push(cleaned);
                    images.extend(extracted);
                }
            }
            _ => {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    let (cleaned, extracted) = extract_data_uri_images(t);
                    texts.push(cleaned);
                    images.extend(extracted);
                }
            }
        }
    }

    if let Some(result) = v.pointer("/result") {
        promote_cua_fields(result, &mut texts, &mut images);
    }

    let text = if texts.is_empty() {
        serde_json::to_string(content).unwrap_or_else(|_| {
            content
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })
    } else {
        texts.join("\n")
    };
    (text, tool_images::cap_images(images))
}

fn image_from_mcp_item(item: &Value) -> Option<UserImage> {
    let mime = item
        .get("mimeType")
        .or_else(|| item.get("mime_type"))
        .and_then(|m| m.as_str())
        .unwrap_or("image/png");
    if let Some(data) = item.get("data").and_then(|d| d.as_str()) {
        if data.starts_with("data:") {
            let (_cleaned, imgs) = extract_data_uri_images(data);
            return imgs.into_iter().next();
        }
        return tool_images::user_image_from_base64(data, mime);
    }
    None
}

fn extract_from_text_and_structured(
    text: &str,
    result: Option<&Value>,
) -> (String, Vec<UserImage>) {
    let (cleaned, mut images) = extract_data_uri_images(text);
    let mut texts = vec![cleaned.clone()];
    if let Some(result) = result {
        promote_cua_fields(result, &mut texts, &mut images);
    }
    (texts.into_iter().next().unwrap_or(cleaned), tool_images::cap_images(images))
}

fn promote_cua_fields(result: &Value, texts: &mut Vec<String>, images: &mut Vec<UserImage>) {
    let structured = result
        .get("structuredContent")
        .or_else(|| result.get("structured_content"))
        .unwrap_or(result);

    if let Some(b64) = structured
        .get("png_base64")
        .or_else(|| structured.get("pngBase64"))
        .and_then(|v| v.as_str())
    {
        if let Some(img) = tool_images::user_image_from_base64(b64, "image/png") {
            images.push(img);
            if texts.is_empty() {
                texts.push(IMAGE_SEPARATE_PLACEHOLDER.to_string());
            }
        }
    }

    if let Some(path) = structured
        .get("path")
        .or_else(|| structured.get("screenshot_path"))
        .or_else(|| structured.get("file"))
        .and_then(|v| v.as_str())
    {
        let p = std::path::Path::new(path);
        if p.exists() {
            if let Some(img) = tool_images::user_image_from_path(p) {
                images.push(img);
                if !texts.iter().any(|t| t.contains(path)) {
                    texts.push(format!("saved {path}"));
                }
            }
        }
    }
}

/// Pull `data:image/...;base64,...` out of free text (Grok extract_base64_images).
fn extract_data_uri_images(text: &str) -> (String, Vec<UserImage>) {
    let mut images = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < text.len() {
        let rest = &text[i..];
        let lower = rest.to_ascii_lowercase();
        let Some(rel) = lower.find("data:image/") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&text[i..i + rel]);
        let start = i + rel;
        let after = &text[start..];
        let Some(semi) = after.to_ascii_lowercase().find(";base64,") else {
            out.push(text.as_bytes()[start] as char);
            i = start + 1;
            continue;
        };
        let mime_end = semi;
        let mime = after
            .get("data:".len()..mime_end)
            .unwrap_or("image/png");
        let payload_start = start + semi + ";base64,".len();
        let mut j = payload_start;
        let b = text.as_bytes();
        while j < b.len() {
            let c = b[j];
            if c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=' | b'\n' | b'\r') {
                j += 1;
            } else {
                break;
            }
        }
        let payload = text[payload_start..j].replace(['\n', '\r'], "");
        if let Some(img) = tool_images::user_image_from_base64(&payload, mime) {
            images.push(img);
            out.push_str(IMAGE_SEPARATE_PLACEHOLDER);
        } else {
            out.push_str(&text[start..j]);
        }
        i = j;
    }
    if images.is_empty() {
        (text.to_string(), images)
    } else {
        (out, images)
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
    fn format_keeps_image_type() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        png.extend(std::iter::repeat(0u8).take(40));
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let v = json!({
            "result": { "content": [
                {"type":"text","text":"shot"},
                {"type":"image","mimeType":"image/png","data": b64}
            ]}
        });
        let (text, images) = format_call_result_parts(&v);
        assert!(text.contains("shot"), "{text}");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime, "image/png");
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
        assert_eq!(
            split_mcp_public_name("mcp_sungods__search"),
            Some(("sungods", "search"))
        );
        assert_eq!(split_mcp_public_name("bash"), None);
    }
}
