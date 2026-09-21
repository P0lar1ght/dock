//! `memory_search` card — port of Grok `MemorySearchToolCallBlock`.
//!
//! Collapsed: `◆ Memory Search <query> (N results)`. Expanded: numbered
//! path:range + score/source, then up to 3 non-empty snippet lines.
//! Fold is Collapsed ↔ Expanded only (Truncated paints like Expanded).
//! Parses `dock-memory::format_search_results` text; unrecognized output
//! falls back to the generic tool card. Does **not** change the string
//! returned to the model.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::scrollback::card;
use crate::scrollback::live;
use crate::scrollback::tool::{self, ToolMode};
use crate::theme::Theme;

pub fn is_memory_search(name: &str) -> bool {
    name == "memory_search"
}

/// Binary fold: Collapsed ↔ Expanded (skip Truncated).
pub fn next_mode(current: ToolMode) -> Option<ToolMode> {
    match current {
        ToolMode::Collapsed => Some(ToolMode::Expanded),
        _ => None,
    }
}

/// Render the specialized card, or [`None`] when `content` is not
/// `format_search_results` / a memory error (caller falls back to generic).
pub fn try_lines(
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Option<Vec<Line<'static>>> {
    let query = parse_query(arguments);
    let failed = looks_failed(content);
    let parsed = if failed || (running && content.is_empty()) {
        Some(Vec::new())
    } else {
        try_parse_results(content)
    }?;

    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;
    let count = parsed.len();
    let mut header = header_line(&query, count, muted, theme, Some(card::header_width(width)));
    prepend_diamond(&mut header, theme, failed);
    if running && !open {
        live::mark_running(&mut header, theme);
    }
    // Truncated body matches Expanded; fold mark still reflects `mode` so the
    // shared card-shell tests see three distinct chevrons. Click cycle skips
    // Truncated via `next_mode`.
    let header = card::finish_header(header, width, mode, theme);
    if !open {
        return Some(vec![header]);
    }

    let mut out = vec![header];
    if running && content.is_empty() {
        out.push(Line::from(""));
        out.push(card::indent(Line::from(live::running_body(theme))));
        return Some(out);
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
        return Some(out);
    }
    if parsed.is_empty() {
        out.push(Line::from(""));
        out.push(card::indent(Line::from(Span::styled(
            "(no results)".to_string(),
            theme.muted(),
        ))));
        return Some(out);
    }

    out.push(Line::from(""));
    for (i, r) in parsed.iter().enumerate() {
        let path_display = shorten_path(&r.path);
        let path_span = Span::styled(
            format!("{path_display}:{}-{}", r.start_line, r.end_line),
            theme.primary().add_modifier(Modifier::BOLD),
        );
        let meta_span = Span::styled(
            format!("  (score: {:.2}, {})", r.score, r.source),
            theme.dim(),
        );
        out.push(card::indent(Line::from(vec![
            Span::styled(format!("{}. ", i + 1), theme.muted()),
            path_span,
            meta_span,
        ])));

        let snippet_lines: Vec<&str> = r
            .snippet
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(3)
            .collect();
        for sl in snippet_lines {
            let trimmed = sl.trim();
            let display = if width == 0 {
                trimmed.to_string()
            } else {
                // indent (2) + snippet pad (2) + right margin.
                let budget = width.saturating_sub(card::BODY_INDENT + 2 + 1).max(8);
                crate::grok::line_utils::truncate_str(trimmed, budget)
            };
            out.push(card::indent(Line::from(Span::styled(
                format!("  {display}"),
                theme.muted(),
            ))));
        }
    }
    Some(out)
}

pub fn lines(
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    try_lines(arguments, content, theme, width, mode, running).unwrap_or_else(|| {
        tool::lines(
            "memory_search",
            arguments,
            content,
            theme,
            width,
            mode,
            running,
        )
    })
}

#[derive(Debug, Clone)]
pub struct MemoryResult {
    pub score: f64,
    pub source: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub snippet: String,
}

fn parse_query(arguments: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    v.get("query")
        .and_then(|q| q.as_str())
        .unwrap_or(arguments.trim())
        .trim()
        .to_string()
}

fn looks_failed(content: &str) -> bool {
    let t = content.trim_start();
    t.starts_with("Error") || t.starts_with("error")
}

/// `Some` when `content` is `format_search_results` output (including empty).
/// `None` → fall back to the generic tool card.
pub fn try_parse_results(content: &str) -> Option<Vec<MemoryResult>> {
    let t = content.trim();
    if t.is_empty() {
        return None;
    }
    if t.starts_with("No memory results found") {
        return Some(Vec::new());
    }
    if t.starts_with("Found ") || t.contains("### Result ") {
        return Some(parse_memory_results(content));
    }
    None
}

/// Parse `dock-memory::format_search_results` text (### Result / **File:** / fence).
pub fn parse_memory_results(output: &str) -> Vec<MemoryResult> {
    let mut results = Vec::new();

    for section in output.split("### Result ") {
        if !section.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }

        let mut score = 0.0;
        let mut source = String::new();
        let mut path = String::new();
        let mut start_line = 0;
        let mut end_line = 0;
        let mut snippet = String::new();

        let lines: Vec<&str> = section.lines().collect();

        if let Some(first) = lines.first() {
            if let Some(score_start) = first.find("score: ") {
                if let Some(after) = first.get(score_start + 7..) {
                    if let Some(end) = after.find(',') {
                        if let Some(score_str) = after.get(..end) {
                            score = score_str.parse().unwrap_or(0.0);
                        }
                    }
                }
            }
            if let Some(src_start) = first.find("source: ") {
                if let Some(after) = first.get(src_start + 8..) {
                    let end = after.find(')').unwrap_or(after.len());
                    if let Some(src) = after.get(..end) {
                        source = src.to_string();
                    }
                }
            }
        }

        for line in lines.iter().skip(1) {
            if let Some(rest) = line.strip_prefix("**File:** ") {
                if let Some(paren) = rest.find(" (lines ") {
                    if let Some(path_str) = rest.get(..paren) {
                        path = path_str.to_string();
                        if let Some(range_str) = rest.get(paren + 8..) {
                            let range_str = range_str.trim_end_matches(')');
                            if let Some((s, e)) = range_str.split_once('-') {
                                start_line = s.parse().unwrap_or(0);
                                end_line = e.parse().unwrap_or(0);
                            }
                        }
                    }
                } else {
                    path = rest.to_string();
                }
            }
        }

        if let Some(code_start) = section.find("```\n") {
            if let Some(after_start) = section.get(code_start + 4..) {
                if let Some(code_end) = after_start.find("\n```") {
                    if let Some(body) = after_start.get(..code_end) {
                        snippet = body.to_string();
                    }
                }
            }
        }

        if !path.is_empty() || !snippet.is_empty() {
            results.push(MemoryResult {
                score,
                source,
                path,
                start_line,
                end_line,
                snippet,
            });
        }
    }

    results
}

fn shorten_path(path: &str) -> &str {
    if let Some(idx) = path.find("/memory/") {
        let rest = &path[idx + "/memory/".len()..];
        if let Some(slash) = rest.find('/') {
            return rest.get(slash + 1..).unwrap_or(rest);
        }
        return rest;
    }
    path.rsplit('/').next().unwrap_or(path)
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
        Span::styled("Memory Search ".to_string(), bold),
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

    fn sample_format_search_results() -> String {
        r#"Found 2 memory result(s):

### Result 1 (score: 0.85, source: workspace)
**File:** /home/u/.dock/memory/workspace-foo/topics/api.md (lines 1-5)
```
## API
REST endpoints use bearer tokens
```

### Result 2 (score: 0.42, source: global)
**File:** /home/u/.dock/memory/global/MEMORY.md (lines 10-20)
```
session content
```
"#
        .to_string()
    }

    #[test]
    fn parse_sample_format_search_results() {
        let results = parse_memory_results(&sample_format_search_results());
        assert_eq!(results.len(), 2);
        let r0 = &results[0];
        assert!((r0.score - 0.85).abs() < 0.01);
        assert_eq!(r0.source, "workspace");
        assert_eq!(r0.path, "/home/u/.dock/memory/workspace-foo/topics/api.md");
        assert_eq!(r0.start_line, 1);
        assert_eq!(r0.end_line, 5);
        assert!(r0.snippet.contains("bearer"));
        let r1 = &results[1];
        assert!((r1.score - 0.42).abs() < 0.01);
        assert_eq!(r1.source, "global");
    }

    #[test]
    fn parse_empty_no_results() {
        let results = try_parse_results("No memory results found for query.").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn bad_text_falls_back_to_generic() {
        assert!(try_parse_results("hello world, not memory output").is_none());
        let theme = Theme::current();
        let text = plain(&lines(
            r#"{"query":"q"}"#,
            "hello world, not memory output",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        ));
        // Generic tool card titles with the tool name, not "Memory Search".
        assert!(
            text.contains("memory_search") || text.contains("Memory_search") || text.contains("q"),
            "expected generic fallback chrome: {text}"
        );
        assert!(
            !text.contains("Memory Search "),
            "specialized header must not appear on parse fail: {text}"
        );
    }

    #[test]
    fn collapsed_header_is_memory_search() {
        let theme = Theme::current();
        let text = plain(&lines(
            r#"{"query":"bearer tokens"}"#,
            &sample_format_search_results(),
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        ));
        assert!(text.contains("Memory Search "), "{text}");
        assert!(text.contains("bearer tokens"), "{text}");
        assert!(text.contains("2 results"), "{text}");
        assert!(!text.contains("topics/api"), "{text}");
    }

    #[test]
    fn expanded_lists_path_score_and_snippet() {
        let theme = Theme::current();
        let text = plain(&lines(
            r#"{"query":"bearer"}"#,
            &sample_format_search_results(),
            &theme,
            80,
            ToolMode::Expanded,
            false,
        ));
        assert!(text.contains("1. "), "{text}");
        assert!(text.contains("topics/api.md:1-5"), "{text}");
        assert!(text.contains("score: 0.85"), "{text}");
        assert!(text.contains("workspace"), "{text}");
        assert!(text.contains("bearer tokens"), "{text}");
        assert!(!text.contains("(no results)"), "{text}");
    }

    #[test]
    fn empty_shows_no_results() {
        let theme = Theme::current();
        let text = plain(&lines(
            r#"{"query":"zzz"}"#,
            "No memory results found for query.",
            &theme,
            80,
            ToolMode::Expanded,
            false,
        ));
        assert!(text.contains("(no results)"), "{text}");
        assert!(text.contains("0 results"), "{text}");
    }

    #[test]
    fn error_renders_in_red_card() {
        let theme = Theme::current();
        let out = lines(
            r#"{"query":"q"}"#,
            "Error: index locked",
            &theme,
            80,
            ToolMode::Expanded,
            false,
        );
        let text = plain(&out);
        assert!(text.contains("Memory Search "), "{text}");
        assert!(text.contains("Error: index locked"), "{text}");
        let err_span = out
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("Error: index locked"))
            .expect("error span");
        assert_eq!(err_span.style.fg, Some(theme.accent_error));
    }

    #[test]
    fn next_mode_is_binary() {
        assert_eq!(next_mode(ToolMode::Collapsed), Some(ToolMode::Expanded));
        assert_eq!(next_mode(ToolMode::Truncated), None);
        assert_eq!(next_mode(ToolMode::Expanded), None);
    }

    #[test]
    fn shorten_memory_path() {
        assert_eq!(
            shorten_path("/home/u/.dock/memory/workspace-foo/topics/api.md"),
            "topics/api.md"
        );
        assert_eq!(
            shorten_path("/home/u/.dock/memory/global/MEMORY.md"),
            "MEMORY.md"
        );
        assert_eq!(shorten_path("/some/other/path.md"), "path.md");
    }
}
