//! MCP `search_tool` card — Grok `IntegrationSearchToolCallBlock`.
//! Header: **Search Tools** `query` `(N results)`. Expanded: numbered
//! action + ghosted server, not raw JSON.

use cordis_spine::SEARCH_TOOL_NAME;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::scrollback::card;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

pub fn is_search_tool(name: &str) -> bool {
    name == SEARCH_TOOL_NAME
}

pub fn lines(
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let query = parse_query(arguments);
    let results = parse_results(content);
    let failed = looks_failed(content);
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;
    let mut header = header_line(
        &query,
        results.len(),
        muted,
        theme,
        Some(card::header_width(width)),
    );
    prepend_diamond(&mut header, theme, failed);
    if running && !open {
        live::mark_running(&mut header, theme);
    }
    let header = card::finish_header(header, width, mode, theme);
    if !open {
        return vec![header];
    }

    let mut out = vec![header];
    if running && content.is_empty() {
        out.push(Line::from(""));
        out.push(card::indent(Line::from(live::running_body(theme))));
        return out;
    }
    if failed {
        out.push(Line::from(""));
        let rows: Vec<Line<'static>> = content
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(theme.accent_error),
                ))
            })
            .collect();
        out.extend(card::body(rows, width));
        return out;
    }
    if results.is_empty() {
        out.push(Line::from(""));
        out.push(card::indent(Line::from(Span::styled(
            "（无结果）".to_string(),
            theme.muted(),
        ))));
        return out;
    }

    out.push(Line::from(""));
    for (i, tool) in results.iter().enumerate() {
        let action = titleize(discovered_tool_action(&tool.name, &tool.server));
        let server = titleize(&tool.server);
        let mut spans = vec![
            Span::styled(format!("{}. ", i + 1), theme.muted()),
            Span::styled(action, theme.primary().add_modifier(Modifier::BOLD)),
        ];
        if !server.is_empty() {
            spans.push(Span::styled(format!("  {server}"), theme.dim()));
        }
        out.push(card::indent(Line::from(spans)));
    }
    out
}

struct DiscoveredTool {
    name: String,
    server: String,
}

fn parse_query(arguments: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    v.get("query")
        .and_then(|q| q.as_str())
        .unwrap_or(arguments.trim())
        .trim()
        .to_string()
}

/// Grok `parse_search_tool_results`: grouped `{ results: [{ server, tools }] }`.
fn parse_results(content: &str) -> Vec<DiscoveredTool> {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(content) else {
        return Vec::new();
    };
    let Some(groups) = val.get("results").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut results = Vec::new();
    for group in groups {
        let server = group
            .get("server")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let Some(tools) = group.get("tools").and_then(|v| v.as_array()) else {
            continue;
        };
        for r in tools {
            let Some(name) = r.get("tool_name").and_then(|v| v.as_str()) else {
                continue;
            };
            results.push(DiscoveredTool {
                name: name.to_string(),
                server: server.clone(),
            });
        }
    }
    results
}

/// Grok `discovered_tool_action`, plus Dock's `mcp_` public-name prefix.
fn discovered_tool_action<'a>(name: &'a str, server: &str) -> &'a str {
    let rest = name.strip_prefix("mcp_").unwrap_or(name);
    rest.strip_prefix(server)
        .and_then(|r| r.strip_prefix("__"))
        .unwrap_or(rest)
}

fn titleize(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_failed(content: &str) -> bool {
    let t = content.trim_start();
    t.starts_with("Error") || t.starts_with("error")
}

fn header_line(
    query: &str,
    count: usize,
    muted: bool,
    theme: &Theme,
    max_width: Option<usize>,
) -> Line<'static> {
    let text_style = if muted {
        theme.muted()
    } else {
        theme.primary()
    };
    let bold = text_style.add_modifier(Modifier::BOLD);
    let query_style = if muted {
        theme.muted()
    } else {
        theme.fg(theme.command)
    };
    let s = if count == 1 { "" } else { "s" };
    let suffix = format!(" ({count} result{s})");
    let line = Line::from(vec![
        Span::styled("Search Tools ".to_string(), bold),
        Span::styled(query.to_string(), query_style),
        Span::styled(suffix, theme.dim()),
    ]);
    match max_width {
        Some(w) => truncate_line(line, w),
        None => line,
    }
}

fn prepend_diamond(line: &mut Line<'static>, theme: &Theme, failed: bool) {
    let fg = if failed {
        theme.accent_error
    } else {
        theme.accent_tool
    };
    line.spans.insert(
        0,
        Span::styled(
            format!("{} ", glyphs::diamond_filled()),
            Style::default().fg(fg),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn sample_json() -> String {
        serde_json::json!({
            "results": [
                {
                    "server": "linear",
                    "tools": [
                        {
                            "tool_name": "mcp_linear__save_issue",
                            "description": "save",
                            "score": 1.0,
                            "input_schema": {"type": "object"}
                        },
                        {
                            "tool_name": "mcp_linear__list_issues",
                            "description": "list",
                            "score": 0.5,
                            "input_schema": {"type": "object"}
                        }
                    ]
                }
            ],
            "total_hidden_tools": 2,
            "status": "ready"
        })
        .to_string()
    }

    #[test]
    fn collapsed_header_is_search_tools_not_raw_name() {
        let theme = Theme::current();
        let text = plain(&lines(
            r#"{"query":"linear issue"}"#,
            &sample_json(),
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        ));
        assert!(text.contains("Search Tools "), "{text}");
        assert!(text.contains("linear issue"), "{text}");
        assert!(text.contains("2 results"), "{text}");
        assert!(!text.contains("search_tool"), "{text}");
        assert!(!text.contains("mcp_linear"), "{text}");
        assert!(!text.contains("Save Issue"), "{text}");
    }

    #[test]
    fn expanded_lists_action_and_server() {
        let theme = Theme::current();
        let text = plain(&lines(
            r#"{"query":"linear"}"#,
            &sample_json(),
            &theme,
            80,
            ToolMode::Expanded,
            false,
        ));
        assert!(text.contains("1. "), "{text}");
        assert!(text.contains("Save Issue"), "{text}");
        assert!(text.contains("Linear"), "{text}");
        assert!(text.contains("List Issues"), "{text}");
        assert!(!text.contains("mcp_linear__save_issue"), "{text}");
        assert!(!text.contains("input_schema"), "{text}");
    }

    #[test]
    fn action_name_strips_mcp_prefix() {
        assert_eq!(
            discovered_tool_action("mcp_linear__save_issue", "linear"),
            "save_issue"
        );
        assert_eq!(
            discovered_tool_action("linear__save_issue", "linear"),
            "save_issue"
        );
        assert_eq!(
            discovered_tool_action("google_calendar_search", "Google Calendar"),
            "google_calendar_search"
        );
    }

    #[test]
    fn empty_results_copy() {
        let theme = Theme::current();
        let text = plain(&lines(
            r#"{"query":"nothing"}"#,
            r#"{"results":[],"total_hidden_tools":0,"status":"ready"}"#,
            &theme,
            80,
            ToolMode::Expanded,
            false,
        ));
        assert!(text.contains("（无结果）"), "{text}");
    }

    #[test]
    fn truncated_lists_all_like_grok() {
        let theme = Theme::current();
        let text = plain(&lines(
            r#"{"query":"linear"}"#,
            &sample_json(),
            &theme,
            80,
            ToolMode::Truncated,
            false,
        ));
        assert!(text.contains("Save Issue"), "{text}");
        assert!(text.contains("List Issues"), "{text}");
        assert!(!text.contains("more"), "{text}");
    }
}
