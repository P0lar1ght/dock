//! Local `memory_search` / `memory_get`. Files under `~/.dock/memory` and `.dock/memory`.
//! Line numbering copied from Grok `memory/get_tool.rs` `format_with_line_numbers`.

use std::path::{Path, PathBuf};

use cordis::{plugin, Inject, Plugin};

use crate::config::dock_home;
use crate::names::{MEMORY, TOOLS};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

const SEARCH_PARAMS: &str = r#"{"type":"object","properties":{"query":{"type":"string","description":"Search query. Prefer specific technical terms."},"max_results":{"type":"integer"},"min_score":{"type":"number"}},"required":["query"]}"#;
const GET_PARAMS: &str = r#"{"type":"object","properties":{"path":{"type":"string","description":"Memory file path from memory_search."},"from":{"type":"integer","description":"1-based start line."},"lines":{"type":"integer","description":"Max lines to return."}},"required":["path"]}"#;

/// Named `"memory"` so TUI / tests can live-lookup roots.
pub struct Memory;

pub fn memory_roots() -> Vec<PathBuf> {
    vec![
        dock_home().join("memory"),
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".dock")
            .join("memory"),
    ]
}

pub fn tool_memory() -> Plugin {
    plugin("tool-memory", Inject::from([TOOLS]), |ctx, _: &()| {
        ctx.provide(MEMORY, Memory)?;
        let tools = ctx.require::<Tools>(TOOLS)?;
        let search: ToolBody =
            std::sync::Arc::new(|call| Box::pin(async move { memory_search(call) }));
        let get: ToolBody = std::sync::Arc::new(|call| Box::pin(async move { memory_get(call) }));
        own_registered(
            ctx,
            vec![
                tools.register(
                    ToolSpec {
                        name: "memory_search".into(),
                        description: "Search cross-session local memory for relevant knowledge chunks. Returns ranked results from ~/.dock/memory and .dock/memory.\n\nUse this proactively when a question references prior work, decisions, or conventions you do not have in the current transcript.\n\nMemory is historical context, not automatically the current plan. Verify recalled facts against live sources before relying on them.".into(),
                        parameters_json: SEARCH_PARAMS.into(),
                    },
                    search,
                )?,
                tools.register(
                    ToolSpec {
                        name: "memory_get".into(),
                        description: "Read a memory file by path. Returns the file content with line numbers, optionally limited to a range of lines.\n\nUse after memory_search returns a relevant result. Line numbers are 1-based and match the from parameter.".into(),
                        parameters_json: GET_PARAMS.into(),
                    },
                    get,
                )?,
            ],
        )?;
        Ok(None)
    })
}

fn memory_search(call: ToolCall) -> ToolResult {
    let v: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or_default();
    let query = v.get("query").and_then(|x| x.as_str()).unwrap_or("").trim();
    if query.is_empty() {
        return tool_result(call, "Error: query is required");
    }
    let max = v
        .get("max_results")
        .and_then(|x| x.as_u64())
        .unwrap_or(6)
        .clamp(1, 20) as usize;
    let min_score = v.get("min_score").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(|w| w.to_ascii_lowercase())
        .collect();
    if terms.is_empty() {
        return tool_result(call, "No memory results found for query.");
    }
    let mut hits: Vec<(f64, PathBuf, usize, usize, String)> = Vec::new();
    for root in memory_roots() {
        walk_md(&root, &root, &terms, &mut hits);
    }
    hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    hits.retain(|h| h.0 >= min_score);
    hits.truncate(max);
    if hits.is_empty() {
        return tool_result(call, "No memory results found for query.");
    }
    let mut out = format!("Found {} memory result(s):\n", hits.len());
    for (i, (score, path, start, end, snippet)) in hits.iter().enumerate() {
        out.push_str(&format!(
            "\n### Result {} (score: {:.2}, source: local)\n**File:** {} (lines {}-{})\n```\n{}\n```\n",
            i + 1,
            score,
            path.display(),
            start,
            end,
            snippet
        ));
    }
    tool_result(call, out)
}

fn walk_md(
    root: &Path,
    dir: &Path,
    terms: &[String],
    hits: &mut Vec<(f64, PathBuf, usize, usize, String)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_md(root, &path, terms, hits);
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "md" && ext != "txt" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lines: Vec<&str> = text.split('\n').collect();
        let mut i = 0;
        while i < lines.len() {
            let end = (i + 20).min(lines.len());
            let chunk = lines[i..end].join("\n");
            let lower = chunk.to_ascii_lowercase();
            let mut hit = 0usize;
            for t in terms {
                if lower.contains(t) {
                    hit += 1;
                }
            }
            if hit > 0 {
                let score = hit as f64 / terms.len() as f64;
                hits.push((score, path.clone(), i + 1, end, chunk));
            }
            i += 20;
        }
    }
}

fn memory_get(call: ToolCall) -> ToolResult {
    let v: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or_default();
    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("").trim();
    if path.is_empty() {
        return tool_result(call, "Error: path is required");
    }
    let from = v.get("from").and_then(|x| x.as_u64()).map(|n| n as usize);
    let lines = v.get("lines").and_then(|x| x.as_u64()).map(|n| n as usize);
    match read_memory(path, from, lines) {
        Ok(body) => tool_result(call, body),
        Err(e) => tool_result(call, format!("Error: {e}")),
    }
}

fn read_memory(path: &str, from: Option<usize>, lines: Option<usize>) -> Result<String, String> {
    let requested = PathBuf::from(path);
    let canonical = if requested.is_absolute() {
        requested.canonicalize().map_err(|e| e.to_string())?
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(&requested)
            .canonicalize()
            .map_err(|e| e.to_string())?
    };
    let allowed = memory_roots().iter().any(|root| {
        canonical.starts_with(root)
            || root
                .canonicalize()
                .ok()
                .is_some_and(|r| canonical.starts_with(r))
    });
    if !allowed {
        return Err("path is outside memory directories (~/.dock/memory, .dock/memory)".into());
    }
    let text = std::fs::read_to_string(&canonical).map_err(|e| e.to_string())?;
    let all: Vec<&str> = text.split('\n').collect();
    let start = from.unwrap_or(1).max(1);
    let start_idx = start.saturating_sub(1).min(all.len());
    let take = lines.unwrap_or(all.len());
    let slice = &all[start_idx..(start_idx + take).min(all.len())];
    let content = slice.join("\n");
    let numbered = format_with_line_numbers(&content, start);
    Ok(format!(
        "**File:** {}\n**Lines:** {} (from: {}, limit: {})\n\n{}",
        canonical.display(),
        slice.len(),
        from.map_or_else(|| "start".into(), |f| f.to_string()),
        lines.map_or_else(|| "all".into(), |l| l.to_string()),
        numbered
    ))
}

/// Copied from Grok `format_with_line_numbers`. `split('\n')` keeps a trailing blank.
pub(crate) fn format_with_line_numbers(content: &str, first_line_num: usize) -> String {
    if content.is_empty() {
        return String::new();
    }
    content
        .split('\n')
        .enumerate()
        .map(|(i, line)| format!("{}→{}", first_line_num + i, line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::format_with_line_numbers;

    #[test]
    fn test_format_basic_line_numbers() {
        let out = format_with_line_numbers("alpha\nbeta\ngamma", 1);
        assert_eq!(out, "1→alpha\n2→beta\n3→gamma");
    }

    #[test]
    fn test_format_offset_adjusts_line_numbers() {
        let out = format_with_line_numbers("line five\nline six", 5);
        assert!(out.starts_with("5→line five"), "got: {out}");
        assert!(out.ends_with("6→line six"), "got: {out}");
    }
}
