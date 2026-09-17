//! Grok progressive disclosure: `search_tool` + `use_tool`.
//!
//! Hidden extras (MCP + infrequent local tools) stay registered for dispatch
//! but are omitted from the sampler tools array (`Tools::specs_for_model`).
//! Catalog changes for MCP are announced as server-level `<system-reminder>`
//! deltas, not per-tool schema dumps.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::tools::registry::{tool_result, Tools};
use crate::tools::workspace;
use cordis_base::types::{LogEvent, ToolCall, ToolResult, ToolSpec};

use super::protocol::{is_mcp_public_name, public_tool_name, split_mcp_public_name};
use super::tool_index::{self, IndexedTool, Ranking};
use super::{Mcp, McpStatus};

pub const SEARCH_TOOL_NAME: &str = "search_tool";
pub const USE_TOOL_NAME: &str = "use_tool";

const SEARCH_TOOL_DESC: &str = "Search on-demand tools by keyword and retrieve their input schemas. \
Matches MCP integrations and infrequent local tools (scheduler, memory, lsp, skill, workflow, cordis_*, browser_*, …). \
Returns only hits, each with a full input_schema, capped by limit (default 5, max 255). \
Unmatched tools stay hidden; total_hidden_tools is the catalog size. \
A hit whose schema is already earlier in this conversation comes back as schema_in_context \
instead of repeating the schema — call it with use_tool as-is. \
Searching an exact tool name always returns that tool's full schema. \
If status is \"partial\", some MCP servers may still be connecting. \
Call matched tools with use_tool. Do not guess parameter names.";

const SEARCH_TOOL_PARAMS: &str = r#"{"type":"object","properties":{"query":{"type":"string","description":"Keywords to match against tool names, server/group names, and descriptions (e.g. \"linear create issue\", \"scheduler\", \"browser\", \"cordis define\")."},"limit":{"type":"integer","minimum":1,"maximum":255,"description":"Maximum number of results (default 5, max 255)."}},"required":["query"]}"#;

const USE_TOOL_DESC: &str = "Call an on-demand tool discovered via search_tool. \
tool_name is the name search_tool returned (mcp_server__tool, or a local name like scheduler_create). \
tool_input must match that tool's input_schema. \
If the schema is already in this conversation you may call use_tool again without searching; \
after a new session, subagent, or compaction, search again. Never guess parameter names. \
Do not route first-class tools (bash, read_file, …) through use_tool — call them directly. \
Output is capped at 20KB; anything past the cap is written to a file whose path is in the truncation notice.";

const USE_TOOL_PARAMS: &str = r#"{"type":"object","properties":{"tool_name":{"type":"string","description":"Name from search_tool (mcp_server__tool or a deferred local tool)."},"tool_input":{"type":"object","description":"Arguments conforming to the tool's input_schema.","additionalProperties":true}},"required":["tool_name"]}"#;

/// Grok `MCP_MAX_OUTPUT_BYTES`.
pub const USE_TOOL_MAX_OUTPUT_BYTES: usize = 20_000;
/// `search_tool` 输出预算。`limit` 上限保持 255（Grok parity），预算兜底：
/// 整个目录灌不进历史。
pub const SEARCH_MAX_OUTPUT_BYTES: usize = 20_000;
/// 预算里能花在 schema 上的部分。剩下的留给工具名——超出后的命中先降级成无
/// schema 行（还认得出叫什么、能直接 use_tool），预算真的耗尽才整条不发。
const SEARCH_MAX_SCHEMA_BYTES: usize = SEARCH_MAX_OUTPUT_BYTES * 3 / 5;
const DEFAULT_SEARCH_LIMIT: usize = 5;
const MAX_SEARCH_LIMIT: usize = 255;
/// Grok `MAX_MCP_DESCRIPTION_LENGTH`。
const MAX_DESC_CHARS: usize = 2048;
/// 没发 schema 的降级行只留短描述：够认出工具，不够再吃一份预算。
const DEGRADED_DESC_CHARS: usize = 200;

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

/// `seen` 是 schema 已经在模型上下文里的工具名（见 [`schemas_in_context`]）：
/// 命中它们时只回名字与描述，不重复 dump schema。
pub fn run_search(
    tools: &Tools,
    mcp: Option<&Mcp>,
    seen: &HashSet<String>,
    arguments: &str,
) -> String {
    let query = parse_query(arguments);
    let limit = parse_limit(arguments);
    let ready = mcp.is_none_or(Mcp::catalog_ready);
    let docs = hidden_index(tools);
    let total_hidden = docs.len();
    if query.is_empty() {
        return pretty(json!({
            "results": [],
            "total_hidden_tools": total_hidden,
            "status": if ready { "ready" } else { "partial" },
            "note": "query is required.",
        }));
    }
    // 精确同名命中是重复搜索的逃生舱：永远回完整 schema，不看 `seen`、不看预算。
    let (hits, exact) = match tool_index::rank(&docs, &query, limit) {
        Ranking::Exact(i) => (vec![(i, 1.0f32)], true),
        Ranking::Ranked(hits) => (hits, false),
    };
    let mut in_context = 0usize;
    let mut over_budget = 0usize;
    let mut dropped = 0usize;
    let mut used = 0usize;
    let mut groups: Vec<(String, f32, Vec<Value>)> = Vec::new();
    for (i, score) in hits {
        let Some(doc) = docs.get(i) else { continue };
        let mut row = json!({
            "tool_name": doc.name,
            "description": truncate_description(&doc.description),
            "score": score,
        });
        let degraded: Option<(&str, Value)> = if !exact && seen.contains(&doc.name) {
            in_context += 1;
            Some(("schema_in_context", json!(true)))
        } else {
            let mut with_schema = row.clone();
            with_schema["input_schema"] = doc.schema.clone();
            let len = row_bytes(&with_schema);
            if !exact && used.saturating_add(len) > SEARCH_MAX_SCHEMA_BYTES {
                over_budget += 1;
                Some(("schema_omitted", json!("budget")))
            } else {
                row = with_schema;
                None
            }
        };
        if let Some((key, marker)) = degraded {
            // 没发 schema 的行不值得再占一大段描述：名字才是 use_tool 要的。
            row["description"] = json!(truncate_chars(&doc.description, DEGRADED_DESC_CHARS));
            row[key] = marker;
        }
        // 降级行同样计进预算：`limit: 255` 灌不出一份超大输出。预算耗尽后连名字
        // 也不再加，note 会说清丢了几条。第一条永远发得出去。
        let bytes = row_bytes(&row);
        if !exact && !groups.is_empty() && used.saturating_add(bytes) > SEARCH_MAX_OUTPUT_BYTES {
            dropped += 1;
            continue;
        }
        used = used.saturating_add(bytes);
        match groups
            .iter_mut()
            .find(|(server, _, _)| server == &doc.group)
        {
            Some(group) => group.2.push(row),
            None => groups.push((doc.group.clone(), score, vec![row])),
        }
    }
    groups.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let result_groups: Vec<Value> = groups
        .into_iter()
        .map(|(server, _, tools)| json!({ "server": server, "tools": tools }))
        .collect();
    let note = search_note(
        ready,
        total_hidden,
        result_groups.is_empty(),
        desktop_driver_missing(&query, mcp),
        in_context,
        over_budget,
        dropped,
    );
    pretty(json!({
        "results": result_groups,
        "total_hidden_tools": total_hidden,
        "status": if ready { "ready" } else { "partial" },
        "note": note,
    }))
}

/// 桌面类查询的关键词。搜不到东西**又**是这类词时，模型该被告知本机还没装
/// driver —— 这是它唯一会撞上「桌面控制不可用」的地方。
const DESKTOP_QUERY_HINTS: &[&str] = &[
    "desktop",
    "screen",
    "screenshot",
    "click",
    "keyboard",
    "mouse",
    "window",
    "gui",
    "computer",
    "cua",
    "桌面",
    "截图",
    "点击",
    "鼠标",
    "键盘",
    "窗口",
];

/// 查询像在找桌面控制，而 cua-driver 没连上。
fn desktop_driver_missing(query: &str, mcp: Option<&Mcp>) -> bool {
    let query = query.to_lowercase();
    if !DESKTOP_QUERY_HINTS.iter().any(|k| query.contains(k)) {
        return false;
    }
    !mcp.map(Mcp::list).unwrap_or_default().iter().any(|s| {
        s.name == cordis_base::cua::CUA_DRIVER_SERVER && s.enabled && s.ok && !s.tools.is_empty()
    })
}

#[allow(clippy::too_many_arguments)]
fn search_note(
    ready: bool,
    total_hidden: usize,
    empty: bool,
    desktop_driver_missing: bool,
    in_context: usize,
    over_budget: usize,
    dropped: usize,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if !ready {
        parts.push("Some MCP servers are still connecting. Results may be incomplete.".into());
    }
    if empty {
        if total_hidden == 0 {
            parts.push(
                "No on-demand tools are available in this session. Enable MCP servers in /mcps."
                    .into(),
            );
        } else if ready {
            parts.push(format!(
                "No tool matched. Tool names and descriptions are English — retry with the server \
                 or capability name (e.g. \"linear issue\", \"scheduler\"); {total_hidden} \
                 on-demand tools are registered."
            ));
        }
    }
    if empty && desktop_driver_missing {
        parts.push(
            "Controlling this machine's desktop (click / type / screenshot) needs the cua-driver \
             MCP server, which is not connected. Tell the user to open /computer and press i to \
             install it — Dock does not ship the driver."
                .into(),
        );
    }
    if in_context > 0 {
        parts.push(format!(
            "{in_context} result(s) already have their input_schema earlier in this conversation \
             (schema_in_context) — call them with use_tool directly instead of re-reading the \
             schema. Search the exact tool name if you must see it again."
        ));
    }
    if over_budget > 0 {
        parts.push(format!(
            "{over_budget} result(s) dropped their input_schema to stay inside the output budget \
             (schema_omitted) — narrow the query or lower limit, then search again."
        ));
    }
    if dropped > 0 {
        parts.push(format!(
            "{dropped} further match(es) were left out entirely: the output budget is spent. \
             Narrow the query or lower limit."
        ));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// 隐藏目录（MCP extras + 按需本地工具），检索与计数都以它为准。
fn hidden_index(tools: &Tools) -> Vec<IndexedTool> {
    tools
        .specs()
        .iter()
        .filter(|s| tools.is_hidden(&s.name))
        .map(|s| tool_index::index_tool(s, catalog_group(&s.name)))
        .collect()
}

/// 模型上下文里已经有 schema 的工具名。真相源是发给采样器的那份历史
/// （`Sessions::model_history`）：压缩把旧的工具结果换成摘要 / 桩之后解析不出
/// 工具名，schema 自动重新变成"没见过"，不需要任何失效逻辑。
pub fn schemas_in_context(history: &[LogEvent]) -> HashSet<String> {
    let mut out = HashSet::new();
    for event in history {
        let LogEvent::ToolExecute { name, content, .. } = event else {
            continue;
        };
        if name != SEARCH_TOOL_NAME {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(content) else {
            continue;
        };
        let Some(groups) = value.get("results").and_then(|r| r.as_array()) else {
            continue;
        };
        for group in groups {
            let Some(rows) = group.get("tools").and_then(|t| t.as_array()) else {
                continue;
            };
            for row in rows {
                // 只有真的带了 schema 的那次命中才算数：降级行（schema_in_context /
                // schema_omitted）说明 schema 来自更早的某一行，那一行会自己计入。
                if row.get("input_schema").is_none() {
                    continue;
                }
                if let Some(name) = row.get("tool_name").and_then(|n| n.as_str()) {
                    out.insert(name.to_string());
                }
            }
        }
    }
    out
}

pub async fn run_use_tool(tools: &Tools, exec: &cordis::Context, call: ToolCall) -> ToolResult {
    let (tool_name, tool_input) = parse_use_args(&call.arguments);
    if tool_name.is_empty() {
        return tool_result(
            call,
            "tool_name is required. Use search_tool to discover on-demand tools.",
        );
    }
    let resolved = resolve_hidden_name(tools, &tool_name);
    if tools.is_hidden(&resolved) {
        let inner = ToolCall {
            id: call.id.clone(),
            name: resolved,
            arguments: tool_input,
        };
        let mut result = tools.execute_on(exec, inner).await;
        result.name = USE_TOOL_NAME.into();
        result.content = cap_use_tool_output(&result.call_id, result.content).await;
        return result;
    }
    if is_first_class_tool(tools, &tool_name) {
        return tool_result(
            call,
            format!(
                "`{tool_name}` is a first-class tool, not an on-demand tool. \
                 Call `{tool_name}` directly as its own tool call instead of \
                 routing it through `use_tool`."
            ),
        );
    }
    tool_result(
        call,
        format!(
            "'{tool_name}' is not an enabled on-demand tool. \
             Use `{SEARCH_TOOL_NAME}` to discover available tools and copy \
             tool_name from the result. Never guess parameter names."
        ),
    )
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

fn catalog_group(name: &str) -> String {
    if let Some((server, _)) = split_mcp_public_name(name) {
        return server.to_string();
    }
    match name {
        n if n.starts_with("cordis_") => "cordis".into(),
        n if n.starts_with("scheduler_") => "scheduler".into(),
        n if n.starts_with("memory_") => "memory".into(),
        n if n.starts_with("browser_") => "browser".into(),
        "monitor" => "monitor".into(),
        "update_goal" => "goal".into(),
        "lsp" => "lsp".into(),
        "skill" => "skills".into(),
        "workflow" => "workflow".into(),
        _ => "dock".into(),
    }
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
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .min(MAX_SEARCH_LIMIT)
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

fn resolve_hidden_name(tools: &Tools, name: &str) -> String {
    if tools.is_hidden(name) || is_mcp_public_name(name) {
        return name.to_string();
    }
    if let Some((server, tool)) = name.split_once("__") {
        let public = public_tool_name(server, tool);
        if tools.is_hidden(&public) {
            return public;
        }
    }
    name.to_string()
}

fn is_first_class_tool(tools: &Tools, name: &str) -> bool {
    if is_meta(name) {
        return false;
    }
    workspace::handles(name)
        || tools
            .specs()
            .iter()
            .any(|s| s.name == name && !tools.is_hidden(name))
}

fn is_meta(name: &str) -> bool {
    name == SEARCH_TOOL_NAME || name == USE_TOOL_NAME
}

/// 超帽的输出先落盘再截断，模型能按路径把剩下的捞回来（Grok `mcp_truncate`
/// 同款）。写盘失败退回纯截断，不要因为磁盘问题让工具调用失败。
/// 字节兜底帽 + 溢出落盘。实现搬去了 `cordis_base::tool_output`，内置工具与
/// `use_tool` 共用同一套语义和同一个落盘目录（`$DOCK_HOME/tool-output/`）。
async fn cap_use_tool_output(call_id: &str, content: String) -> String {
    let budget = cordis_base::tool_output::Budget::opaque(USE_TOOL_MAX_OUTPUT_BYTES);
    cordis_base::tool_output::cap_bytes(call_id, content, &budget).await
}

/// 预算按**实际发出去的**缩进 JSON 计（紧凑长度会低估近一倍）。算的是命中行
/// 本身，分组外壳与嵌套缩进不计——所以整份输出会比预算略大一点，但拦得住
/// "一次 `limit: 255` 把目录灌进历史"这件事。
fn row_bytes(row: &Value) -> usize {
    serde_json::to_string_pretty(row)
        .map(|s| s.len())
        .unwrap_or_else(|_| row.to_string().len())
}

fn pretty(v: Value) -> String {
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string())
}

fn truncate_description(s: &str) -> String {
    truncate_chars(s, MAX_DESC_CHARS)
}

fn truncate_chars(s: &str, max: usize) -> String {
    const SUFFIX: &str = "\u{2026} [truncated]";
    if s.chars().count() <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(SUFFIX.chars().count());
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
    use crate::tools::registry::tool_result;
    use std::sync::Arc;

    fn stub_body() -> crate::tools::registry::ToolBody {
        Arc::new(|call| Box::pin(async move { tool_result(call, "pong") }))
    }

    fn spec(name: &str, desc: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: desc.into(),
            parameters_json: r#"{"type":"object"}"#.into(),
        }
    }

    /// 干净上下文：没有任何 schema 发过。
    fn seen() -> HashSet<String> {
        HashSet::new()
    }

    /// 把一次 search 的输出当成历史里的工具结果。
    fn search_row(content: &str) -> LogEvent {
        LogEvent::ToolExecute {
            id: "call-1".into(),
            name: SEARCH_TOOL_NAME.into(),
            arguments: r#"{"query":"linear issue"}"#.into(),
            content: content.into(),
            images: Vec::new(),
        }
    }

    /// 没装 driver 时，模型搜桌面工具只会得到空结果；note 得把下一步说出来，
    /// 否则它只能回一句「我没有这个能力」。
    #[test]
    fn empty_desktop_search_points_at_the_computer_cockpit() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx);
        let _a = tools
            .register_mcp(spec("mcp_linear__save_issue", "save an issue"), stub_body())
            .unwrap();

        let out = run_search(&tools, None, &seen(), r#"{"query":"click desktop window"}"#);
        assert!(out.contains("/computer"), "{out}");
        assert!(out.contains("cua-driver"), "{out}");

        // 非桌面类查询不该被这句噪音污染。
        let other = run_search(&tools, None, &seen(), r#"{"query":"zzzz nothing"}"#);
        assert!(!other.contains("/computer"), "{other}");
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
        let exact = run_search(
            &tools,
            None,
            &seen(),
            r#"{"query":"mcp_linear__save_issue"}"#,
        );
        assert!(exact.contains("mcp_linear__save_issue"), "{exact}");
        assert!(!exact.contains("mcp_github__create_issue"), "{exact}");
        let ranked = run_search(&tools, None, &seen(), r#"{"query":"linear issue"}"#);
        assert!(ranked.contains("\"server\": \"linear\""), "{ranked}");
        assert!(ranked.contains("total_hidden_tools"), "{ranked}");
    }

    /// 两颗带胖 schema 的 MCP 工具，够看出去掉 schema 后的体积差。
    fn fat_tools(tools: &Tools) -> Vec<cordis::Disposable> {
        let schema = |field: &str| {
            format!(
                r#"{{"type":"object","properties":{{"{field}":{{"type":"string","description":"{}"}}}},"required":["{field}"]}}"#,
                "d".repeat(400)
            )
        };
        vec![
            tools
                .register_mcp(
                    ToolSpec {
                        name: "mcp_linear__save_issue".into(),
                        description: "Save an issue".into(),
                        parameters_json: schema("issue_id"),
                    },
                    stub_body(),
                )
                .unwrap(),
            tools
                .register_mcp(
                    ToolSpec {
                        name: "mcp_linear__list_issues".into(),
                        description: "List issues".into(),
                        parameters_json: schema("team_id"),
                    },
                    stub_body(),
                )
                .unwrap(),
        ]
    }

    #[test]
    fn search_omits_schema_when_already_in_context() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx);
        let _fat = fat_tools(&tools);
        let args = r#"{"query":"issue"}"#;

        let first = run_search(&tools, None, &seen(), args);
        assert!(first.contains("input_schema"), "{first}");

        let seen_after = schemas_in_context(&[search_row(&first)]);
        assert!(seen_after.contains("mcp_linear__save_issue"), "{first}");

        let second = run_search(&tools, None, &seen_after, args);
        let rows = |out: &str| serde_json::from_str::<Value>(out).unwrap()["results"].to_string();
        assert!(!rows(&second).contains("input_schema"), "{second}");
        assert!(second.contains("\"schema_in_context\": true"), "{second}");
        // 名字还在，模型照样能 use_tool。
        assert!(second.contains("mcp_linear__save_issue"), "{second}");
        assert!(
            second.contains("already have their input_schema"),
            "{second}"
        );
        // 省下来的是 schema 那一坨，跟 note 的长短无关。
        let rows_len = |out: &str| rows(out).len();
        assert!(
            rows_len(&second) * 3 < rows_len(&first),
            "重复搜索应显著变小：{} → {}",
            rows_len(&first),
            rows_len(&second)
        );
    }

    #[test]
    fn exact_name_search_always_returns_schema() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx);
        let _fat = fat_tools(&tools);
        let mut already = HashSet::new();
        already.insert("mcp_linear__save_issue".to_string());
        let exact = run_search(
            &tools,
            None,
            &already,
            r#"{"query":"mcp_linear__save_issue"}"#,
        );
        assert!(exact.contains("input_schema"), "{exact}");
        assert!(!exact.contains("schema_in_context"), "{exact}");
        // 去掉 server 前缀的本名同样是逃生舱。
        let raw = run_search(&tools, None, &already, r#"{"query":"save_issue"}"#);
        assert!(raw.contains("input_schema"), "{raw}");
    }

    #[test]
    fn compacted_history_restores_full_schema() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx);
        let _fat = fat_tools(&tools);
        let args = r#"{"query":"issue"}"#;
        let first = run_search(&tools, None, &seen(), args);

        // 压缩把工具结果换成桩文本：解析不出工具名，schema 重新算"没见过"。
        let stub = search_row("Tool call omitted for brevity");
        assert!(schemas_in_context(&[stub]).is_empty());

        // 降级行本身不算"发过 schema"。
        let second = run_search(
            &tools,
            None,
            &schemas_in_context(&[search_row(&first)]),
            args,
        );
        assert!(
            schemas_in_context(&[search_row(&second)]).is_empty(),
            "{second}"
        );
    }

    #[test]
    fn search_output_respects_byte_budget() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx);
        let big = format!(
            r#"{{"type":"object","properties":{{"payload":{{"type":"string","description":"{}"}}}}}}"#,
            "p".repeat(1500)
        );
        let _keeps: Vec<_> = (0..60)
            .map(|i| {
                tools
                    .register_mcp(
                        ToolSpec {
                            name: format!("mcp_probe__job_{i}"),
                            description: "probe catalog job".into(),
                            parameters_json: big.clone(),
                        },
                        stub_body(),
                    )
                    .unwrap()
            })
            .collect();
        let out = run_search(
            &tools,
            None,
            &seen(),
            r#"{"query":"probe job","limit":255}"#,
        );
        let parsed: Value = serde_json::from_str(&out).expect("JSON 仍然合法");
        // 不封顶的话 60 × 1.5KB schema 就是 90KB+；降级行也计进预算，总量咬住 20KB。
        assert!(
            out.len() < SEARCH_MAX_OUTPUT_BYTES + 2_000,
            "预算应挡住整份目录：{}",
            out.len()
        );
        assert!(out.contains("\"schema_omitted\": \"budget\""), "{out}");
        assert!(out.contains("narrow the query"), "{out}");
        // 降级的只是 schema，工具名一条不少。
        let rows: usize = parsed["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["tools"].as_array().unwrap().len())
            .sum();
        assert_eq!(rows, 60, "{out}");
    }

    #[test]
    fn budget_drops_tail_matches_and_says_so() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx);
        let _keeps: Vec<_> = (0..255)
            .map(|i| {
                tools
                    .register_mcp(
                        spec(&format!("mcp_probe__job_{i}"), "probe catalog job"),
                        stub_body(),
                    )
                    .unwrap()
            })
            .collect();
        let out = run_search(
            &tools,
            None,
            &seen(),
            r#"{"query":"probe job","limit":255}"#,
        );
        let parsed: Value = serde_json::from_str(&out).expect("JSON 仍然合法");
        assert!(out.len() < SEARCH_MAX_OUTPUT_BYTES * 3 / 2, "{}", out.len());
        let rows: usize = parsed["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["tools"].as_array().unwrap().len())
            .sum();
        assert!(rows > 0 && rows < 255, "整批发不下，应只发一部分：{rows}");
        assert!(out.contains("were left out entirely"), "{out}");
    }

    #[test]
    fn zero_hits_note_points_at_the_next_move() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx);
        let _fat = fat_tools(&tools);
        // 中文查询：英文描述的目录打不中，note 要给出下一步。
        let miss = run_search(&tools, None, &seen(), r#"{"query":"定时任务"}"#);
        assert!(miss.contains("\"results\": []"), "{miss}");
        assert!(miss.contains("No tool matched"), "{miss}");
        assert!(miss.contains("2 on-demand tools are registered"), "{miss}");
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
        assert!(native.content.contains("first-class tool"), "{native:?}");
        assert!(native.content.contains("bash"), "{native:?}");
    }

    #[test]
    fn search_includes_deferred_local_and_caps_limit() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx);
        let _s = tools
            .register_deferred(
                spec("scheduler_create", "create a scheduled task"),
                stub_body(),
            )
            .unwrap();
        let _m = tools
            .register_mcp(spec("mcp_linear__save_issue", "save an issue"), stub_body())
            .unwrap();
        let found = run_search(&tools, None, &seen(), r#"{"query":"scheduler"}"#);
        assert!(found.contains("scheduler_create"), "{found}");
        assert!(found.contains("\"server\": \"scheduler\""), "{found}");
        assert!(found.contains("\"total_hidden_tools\": 2"), "{found}");
        assert_eq!(parse_limit(r#"{"query":"x","limit":999}"#), 255);
        assert_eq!(parse_limit(r#"{"query":"x"}"#), 5);
        assert_eq!(parse_limit(r#"{"query":"x","limit":0}"#), 5);
    }

    #[test]
    fn search_limit_truncates_hits() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx);
        let _keeps: Vec<_> = (0..8)
            .map(|i| {
                tools
                    .register_deferred(
                        spec(&format!("probe_tool_{i}"), "probe catalog item"),
                        stub_body(),
                    )
                    .unwrap()
            })
            .collect();
        let limited = run_search(&tools, None, &seen(), r#"{"query":"probe","limit":3}"#);
        let n = limited.matches("probe_tool_").count();
        assert_eq!(n, 3, "{limited}");
        assert!(limited.contains("\"total_hidden_tools\": 8"), "{limited}");
    }

    #[tokio::test]
    async fn use_tool_dispatches_deferred_and_caps_output() {
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx.clone());
        let _d = tools
            .register_deferred(spec("scheduler_create", "sched"), stub_body())
            .unwrap();
        let ok = run_use_tool(
            &tools,
            &ctx,
            ToolCall {
                id: "1".into(),
                name: USE_TOOL_NAME.into(),
                arguments: r#"{"tool_name":"scheduler_create","tool_input":{}}"#.into(),
            },
        )
        .await;
        assert_eq!(ok.content, "pong");

        let fat: crate::tools::registry::ToolBody = Arc::new(|call| {
            Box::pin(async move { tool_result(call, "x".repeat(USE_TOOL_MAX_OUTPUT_BYTES + 50)) })
        });
        let _mcp = tools
            .register_mcp(
                ToolSpec {
                    name: "mcp_probe__blob".into(),
                    description: "blob".into(),
                    parameters_json: r#"{"type":"object"}"#.into(),
                },
                fat,
            )
            .unwrap();
        let _home = cordis_base::test_env::scoped().home();
        let capped = run_use_tool(
            &tools,
            &ctx,
            ToolCall {
                id: "2".into(),
                name: USE_TOOL_NAME.into(),
                arguments: r#"{"tool_name":"mcp_probe__blob","tool_input":{}}"#.into(),
            },
        )
        .await;
        assert!(capped.content.contains("output truncated"), "{capped:?}");
        assert!(capped.content.len() < USE_TOOL_MAX_OUTPUT_BYTES + 400);

        // 截断掉的部分落盘，模型能按路径捞回全文。
        let path = cordis_base::tool_output::spill_dir().join(format!(
            "{}.txt",
            cordis_base::tool_output::offload_stem("2")
        ));
        assert!(
            capped.content.contains(&path.to_string_lossy().to_string()),
            "{capped:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap().len(),
            USE_TOOL_MAX_OUTPUT_BYTES + 50
        );
    }

    #[tokio::test]
    async fn use_tool_offload_stem_cannot_escape_the_folder() {
        let _home = cordis_base::test_env::scoped().home();
        let ctx = cordis::Context::new();
        let tools = Tools::echo(ctx.clone());
        let fat: crate::tools::registry::ToolBody = Arc::new(|call| {
            Box::pin(async move { tool_result(call, "y".repeat(USE_TOOL_MAX_OUTPUT_BYTES + 10)) })
        });
        let _mcp = tools
            .register_mcp(spec("mcp_probe__blob", "blob"), fat)
            .unwrap();
        let out = run_use_tool(
            &tools,
            &ctx,
            ToolCall {
                id: "../../escape".into(),
                name: USE_TOOL_NAME.into(),
                arguments: r#"{"tool_name":"mcp_probe__blob","tool_input":{}}"#.into(),
            },
        )
        .await;
        let dir = cordis_base::config::dock_home().join("tool-output");
        assert!(out.content.contains("output truncated"), "{out:?}");
        assert!(
            dir.join(format!(
                "{}.txt",
                cordis_base::tool_output::offload_stem("../../escape")
            ))
            .exists(),
            "{out:?}"
        );
    }
}
