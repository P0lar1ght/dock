//! Grok progressive disclosure: `search_tool` + `use_tool`.
//!
//! MCP extras stay registered for dispatch but are omitted from the sampler
//! tools array (`Tools::specs_for_model`). Catalog changes are announced as
//! server-level `<system-reminder>` deltas, not per-tool schema dumps.

use serde_json::{json, Value};

use crate::tools::{tool_result, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};
use crate::workspace;

use super::protocol::{is_mcp_public_name, public_tool_name, split_mcp_public_name};
use super::{Mcp, McpStatus};

pub const SEARCH_TOOL_NAME: &str = "search_tool";
pub const USE_TOOL_NAME: &str = "use_tool";

const SEARCH_TOOL_DESC: &str = "Search for MCP tools by keyword and retrieve their input schemas. \
If status is \"partial\", some servers may still be connecting. \
Call matched tools with use_tool (tool_name is the qualified mcp_server__tool name).";

const SEARCH_TOOL_PARAMS: &str = r#"{"type":"object","properties":{"query":{"type":"string","description":"Keywords to match against tool names, server names, and descriptions (e.g. \"linear create issue\")."},"limit":{"type":"integer","description":"Maximum number of results (default 5)."}},"required":["query"]}"#;

const USE_TOOL_DESC: &str = "Call an MCP integration tool. \
tool_name must be the qualified mcp_server__tool name returned by search_tool \
(e.g. mcp_linear__save_issue). tool_input must match that tool's input_schema. \
Do not route native tools (bash, read_file, …) through use_tool — call them directly.";

const USE_TOOL_PARAMS: &str = r#"{"type":"object","properties":{"tool_name":{"type":"string","description":"Qualified MCP name from search_tool (mcp_server__tool)."},"tool_input":{"type":"object","description":"Arguments conforming to the tool's input_schema.","additionalProperties":true}},"required":["tool_name"]}"#;

pub fn search_spec() -> ToolSpec {
    ToolSpec {
        name: SEARCH_TOOL_NAME.into(),
        description: SEARCH_TOOL_DESC.into(),
        parameters_json: SEARCH_TOOL_PARAMS.into(),
    }
}

pub fn use_spec() -> ToolSpec {
    ToolSpec {
        name: USE_TOOL_NAME.into(),
        description: USE_TOOL_DESC.into(),
        parameters_json: USE_TOOL_PARAMS.into(),
    }
}

pub fn run_search(tools: &Tools, mcp: Option<&Mcp>, arguments: &str) -> String {
    let query = parse_query(arguments);
    let limit = parse_limit(arguments);
    let ready = mcp.is_none_or(Mcp::catalog_ready);
    if query.is_empty() {
        return pretty(json!({
            "results": [],
            "total_hidden_tools": mcp_count(tools),
            "status": if ready { "ready" } else { "partial" },
            "note": "query is required.",
        }));
    }
    let hits = search_hits(tools, &query, limit);
    let total_hidden = mcp_count(tools);
    let mut groups: Vec<(String, f32, Vec<Value>)> = Vec::new();
    for hit in &hits {
        let row = json!({
            "tool_name": hit.name,
            "description": truncate_description(&hit.description),
            "score": hit.score,
            "input_schema": hit.schema,
        });
        if let Some(group) = groups
            .iter_mut()
            .find(|(server, _, _)| server == &hit.server)
        {
            group.2.push(row);
        } else {
            groups.push((hit.server.clone(), hit.score, vec![row]));
        }
    }
    groups.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let result_groups: Vec<Value> = groups
        .into_iter()
        .map(|(server, _, tools)| json!({ "server": server, "tools": tools }))
        .collect();
    let note = if !ready {
        Some("Some MCP servers are still connecting. Results may be incomplete.")
    } else if total_hidden == 0 && result_groups.is_empty() {
        Some("No MCP tools are available in this session. Enable servers in /mcps.")
    } else {
        None
    };
    pretty(json!({
        "results": result_groups,
        "total_hidden_tools": total_hidden,
        "status": if ready { "ready" } else { "partial" },
        "note": note,
    }))
}

pub async fn run_use_tool(tools: &Tools, exec: &cordis::Context, call: ToolCall) -> ToolResult {
    let (tool_name, tool_input) = parse_use_args(&call.arguments);
    if tool_name.is_empty() {
        return tool_result(
            call,
            "tool_name is required. Use search_tool to discover MCP tools.",
        );
    }
    if !tool_name.contains("__") {
        if is_native_tool(tools, &tool_name) {
            return tool_result(
                call,
                format!(
                    "`{tool_name}` is a native tool, not an MCP integration tool. \
                     Call `{tool_name}` directly as its own tool call instead of \
                     routing it through `use_tool`."
                ),
            );
        }
        return tool_result(
            call,
            format!(
                "'{tool_name}' is not a valid MCP tool name. \
                 Tool names must be qualified as `mcp_server__tool` \
                 (e.g. `mcp_linear__save_issue`). Use `{SEARCH_TOOL_NAME}` to discover available tools."
            ),
        );
    }
    let resolved = resolve_mcp_name(tools, &tool_name);
    if !tools.is_mcp(&resolved) {
        return tool_result(
            call,
            format!(
                "MCP tool '{tool_name}' is not enabled. Use `{SEARCH_TOOL_NAME}` to discover available tools."
            ),
        );
    }
    let inner = ToolCall {
        id: call.id.clone(),
        name: resolved,
        arguments: tool_input,
    };
    let mut result = tools.execute_on(exec, inner).await;
    result.name = USE_TOOL_NAME.into();
    result
}

pub(super) struct ServerSummary {
    pub name: String,
    pub description: Option<String>,
    pub tool_count: usize,
    pub tool_names: Vec<String>,
}

pub(super) type ServerFingerprint = (usize, u64, u64);

pub(super) fn summaries_from_status(list: &[McpStatus]) -> Vec<ServerSummary> {
    list.iter()
        .filter(|s| s.enabled && s.ok)
        .map(|s| {
            let mut tool_names: Vec<String> = s
                .tools
                .iter()
                .filter(|t| t.enabled)
                .map(|t| t.public_name.clone())
                .collect();
            tool_names.sort();
            ServerSummary {
                name: s.name.clone(),
                description: None,
                tool_count: tool_names.len(),
                tool_names,
            }
        })
        .collect()
}

pub(super) fn fingerprint_servers(
    servers: &[ServerSummary],
) -> std::collections::HashMap<String, ServerFingerprint> {
    servers
        .iter()
        .map(|s| {
            (
                s.name.clone(),
                (
                    s.tool_count,
                    hash_value(&s.description),
                    hash_value(&s.tool_names),
                ),
            )
        })
        .collect()
}

pub(super) fn build_delta_reminder(
    old: &std::collections::HashMap<String, ServerFingerprint>,
    new_summaries: &[ServerSummary],
) -> Option<String> {
    let new_map = fingerprint_servers(new_summaries);
    let added: Vec<&ServerSummary> = new_summaries
        .iter()
        .filter(|s| !old.contains_key(&s.name))
        .collect();
    let updated: Vec<&ServerSummary> = new_summaries
        .iter()
        .filter(|s| {
            old.get(&s.name)
                .is_some_and(|old_fp| Some(old_fp) != new_map.get(&s.name))
        })
        .collect();
    let removed: Vec<&String> = old
        .keys()
        .filter(|name| !new_map.contains_key(*name))
        .collect();
    if added.is_empty() && updated.is_empty() && removed.is_empty() {
        return None;
    }
    let mut text = String::new();
    if !added.is_empty() {
        text.push_str("MCP 服务器已连接：\n");
        for server in &added {
            text.push_str(&format_server_line(server));
        }
    }
    if !updated.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("MCP 服务器已更新：\n");
        for server in &updated {
            text.push_str(&format_server_line(server));
        }
    }
    if !removed.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        let mut names: Vec<&str> = removed.iter().map(|s| s.as_str()).collect();
        names.sort_unstable();
        text.push_str(&format!("MCP 服务器已断开：{}", names.join("、")));
    }
    Some(text)
}

fn format_server_line(server: &ServerSummary) -> String {
    match server.description.as_deref().filter(|s| !s.is_empty()) {
        Some(d) => format!("- {} ({} 个工具): {}\n", server.name, server.tool_count, d),
        None => format!("- {} ({} 个工具)\n", server.name, server.tool_count),
    }
}

struct Hit {
    name: String,
    server: String,
    description: String,
    score: f32,
    schema: Value,
}

fn search_hits(tools: &Tools, query: &str, limit: usize) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let mut exact: Option<Hit> = None;
    let mut scored = Vec::new();
    for spec in tools.specs() {
        if !tools.is_mcp(&spec.name) {
            continue;
        }
        let (server, raw) = split_mcp_public_name(&spec.name).unwrap_or(("", spec.name.as_str()));
        let schema: Value = serde_json::from_str(&spec.parameters_json)
            .unwrap_or_else(|_| json!({"type": "object"}));
        let hit = |score: f32| Hit {
            name: spec.name.clone(),
            server: server.to_string(),
            description: spec.description.clone(),
            score,
            schema: schema.clone(),
        };
        if spec.name.to_lowercase() == q || raw.to_lowercase() == q {
            exact = Some(hit(1.0));
            break;
        }
        let score = token_score(&q, &spec.name, server, raw, &spec.description);
        if score > 0.0 {
            scored.push(hit(score));
        }
    }
    if let Some(hit) = exact {
        return vec![hit];
    }
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit.max(1));
    scored
}

fn token_score(query: &str, public: &str, server: &str, raw: &str, desc: &str) -> f32 {
    let hay = format!("{public} {server} {raw} {desc}").to_lowercase();
    let mut score = 0.0f32;
    for tok in query.split_whitespace().filter(|t| !t.is_empty()) {
        if public.to_lowercase().contains(tok) || raw.to_lowercase() == tok {
            score += 2.0;
        } else if hay.contains(tok) {
            score += 1.0;
        }
    }
    score
}

fn mcp_count(tools: &Tools) -> usize {
    tools
        .specs()
        .into_iter()
        .filter(|s| tools.is_mcp(&s.name))
        .count()
}

fn parse_query(arguments: &str) -> String {
    let v: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    v.get("query")
        .and_then(|q| q.as_str())
        .unwrap_or(arguments.trim())
        .trim()
        .to_string()
}

fn parse_limit(arguments: &str) -> usize {
    let v: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    v.get("limit")
        .and_then(|n| n.as_u64())
        .map(|n| n as usize)
        .filter(|n| *n > 0)
        .unwrap_or(5)
        .min(32)
}

fn parse_use_args(arguments: &str) -> (String, String) {
    let v: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let name = v
        .get("tool_name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let input = match v.get("tool_input") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => "{}".into(),
    };
    (name, input)
}

fn resolve_mcp_name(tools: &Tools, name: &str) -> String {
    if tools.is_mcp(name) || is_mcp_public_name(name) {
        return name.to_string();
    }
    if let Some((server, tool)) = name.split_once("__") {
        return public_tool_name(server, tool);
    }
    name.to_string()
}

fn is_native_tool(tools: &Tools, name: &str) -> bool {
    workspace::handles(name)
        || tools
            .specs()
            .iter()
            .any(|s| s.name == name && !tools.is_mcp(name))
}

fn pretty(v: Value) -> String {
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())
}

fn truncate_description(s: &str) -> String {
    const MAX: usize = 2048;
    const SUFFIX: &str = "\u{2026} [truncated]";
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let budget = MAX.saturating_sub(SUFFIX.chars().count());
    format!("{}{SUFFIX}", s.chars().take(budget).collect::<String>())
}

fn hash_value<H: std::hash::Hash>(val: &H) -> u64 {
    use std::hash::Hasher;
    struct Fnv1a(u64);
    impl Hasher for Fnv1a {
        fn finish(&self) -> u64 {
            self.0
        }
        fn write(&mut self, bytes: &[u8]) {
            for &b in bytes {
                self.0 ^= b as u64;
                self.0 = self.0.wrapping_mul(0x00000100000001B3);
            }
        }
    }
    let mut h = Fnv1a(0xcbf29ce484222325);
    val.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tool_result;
    use std::sync::Arc;

    fn stub_body() -> crate::tools::ToolBody {
        Arc::new(|call| Box::pin(async move { tool_result(call, "pong") }))
    }

    fn spec(name: &str, desc: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: desc.into(),
            parameters_json: r#"{"type":"object"}"#.into(),
        }
    }

    #[test]
    fn search_groups_by_server_and_exact_match() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx);
        let _a = tools
            .register_mcp(spec("mcp_linear__save_issue", "save an issue"), stub_body())
            .unwrap();
        let _b = tools
            .register_mcp(spec("mcp_linear__list_issues", "list issues"), stub_body())
            .unwrap();
        let _c = tools
            .register_mcp(
                spec("mcp_github__create_issue", "create a github issue"),
                stub_body(),
            )
            .unwrap();
        let exact = run_search(&tools, None, r#"{"query":"mcp_linear__save_issue"}"#);
        assert!(exact.contains("mcp_linear__save_issue"), "{exact}");
        assert!(!exact.contains("mcp_github__create_issue"), "{exact}");
        let ranked = run_search(&tools, None, r#"{"query":"linear issue"}"#);
        assert!(ranked.contains("\"server\": \"linear\""), "{ranked}");
        assert!(ranked.contains("total_hidden_tools"), "{ranked}");
    }

    #[test]
    fn delta_reminder_is_server_level() {
        let empty = std::collections::HashMap::new();
        let added = vec![ServerSummary {
            name: "probe".into(),
            description: None,
            tool_count: 2,
            tool_names: vec!["mcp_probe__ping".into(), "mcp_probe__pong".into()],
        }];
        let text = build_delta_reminder(&empty, &added).unwrap();
        assert!(text.contains("MCP 服务器已连接"), "{text}");
        assert!(text.contains("probe"), "{text}");
        assert!(text.contains("2 个工具"), "{text}");
        assert!(!text.contains("mcp_probe__ping"), "{text}");

        let old = fingerprint_servers(&added);
        let gone = build_delta_reminder(&old, &[]).unwrap();
        assert!(gone.contains("MCP 服务器已断开：probe"), "{gone}");
    }

    #[tokio::test]
    async fn use_tool_dispatches_and_steers_native() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx.clone());
        let _mcp = tools
            .register_mcp(spec("mcp_probe__ping", "ping"), stub_body())
            .unwrap();
        let ok = run_use_tool(
            &tools,
            &ctx,
            ToolCall {
                id: "1".into(),
                name: USE_TOOL_NAME.into(),
                arguments: r#"{"tool_name":"mcp_probe__ping","tool_input":{}}"#.into(),
            },
        )
        .await;
        assert_eq!(ok.name, USE_TOOL_NAME);
        assert_eq!(ok.content, "pong");

        let grok_style = run_use_tool(
            &tools,
            &ctx,
            ToolCall {
                id: "2".into(),
                name: USE_TOOL_NAME.into(),
                arguments: r#"{"tool_name":"probe__ping","tool_input":{}}"#.into(),
            },
        )
        .await;
        assert_eq!(grok_style.content, "pong");

        let native = run_use_tool(
            &tools,
            &ctx,
            ToolCall {
                id: "3".into(),
                name: USE_TOOL_NAME.into(),
                arguments: r#"{"tool_name":"bash","tool_input":{}}"#.into(),
            },
        )
        .await;
        assert!(native.content.contains("native tool"), "{native:?}");
        assert!(native.content.contains("bash"), "{native:?}");
    }
}
