//! WebSearch / WebFetch cards — simplified grok web tool blocks.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::grok::wrapping::word_wrap_lines;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

const TRUNCATED_INLINE: usize = 3;
const EXPANDED_INLINE: usize = 10;
const MAX_SOURCES: usize = 3;

pub fn is_web_tool(name: &str) -> bool {
    matches!(name, "web_search" | "web_fetch" | "fetch")
}

pub fn lines(
    name: &str,
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    if matches!(name, "web_fetch" | "fetch") {
        fetch_lines(arguments, content, theme, width, mode, running)
    } else {
        search_lines(arguments, content, theme, width, mode, running)
    }
}

fn search_lines(
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let query = str_arg(arguments, &["query", "q"]).unwrap_or_default();
    let failed = content.starts_with("Error");
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;
    let urls = extract_result_urls(content);

    let mut header = {
        let text = if muted {
            theme.muted()
        } else {
            theme.primary()
        };
        let bold = text.add_modifier(Modifier::BOLD);
        let mut spans = vec![
            Span::styled("Web Search ".to_string(), bold),
            Span::styled(
                query.clone(),
                if muted {
                    theme.muted()
                } else {
                    theme.fg(theme.command)
                },
            ),
        ];
        if !urls.is_empty() {
            let n = urls.len();
            let label = if n == 1 {
                " (1 site)".to_string()
            } else {
                format!(" ({n} sites)")
            };
            spans.push(Span::styled(label, theme.dim()));
        }
        let line = Line::from(spans);
        if width == 0 {
            line
        } else {
            truncate_line(line, width.saturating_sub(2).max(8))
        }
    };
    prepend_diamond(&mut header, theme, failed);
    if running && !open {
        live::mark_running(&mut header, theme);
        return vec![header];
    }
    if !open {
        return vec![header];
    }

    let mut out = vec![header, Line::from("")];
    if failed {
        push_error(&mut out, content, theme);
        return out;
    }
    let cap = if mode == ToolMode::Truncated {
        TRUNCATED_INLINE
    } else {
        EXPANDED_INLINE
    };
    let body_lines: Vec<&str> = content.lines().collect();
    let show = body_lines.iter().take(cap);
    for line in show {
        out.push(Line::from(Span::styled(format!("  {line}"), theme.muted())));
    }
    if body_lines.len() > cap {
        let rest = body_lines.len() - cap;
        out.push(Line::from(Span::styled(
            format!("  \u{2026} ({rest} more lines)"),
            theme.dim(),
        )));
    }
    if !urls.is_empty() {
        out.push(Line::from(""));
        out.push(sources_line(&urls, theme));
    }
    if width > 0 {
        out = word_wrap_lines(out, width);
    }
    out
}

fn fetch_lines(
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let url = str_arg(arguments, &["url", "target_url"]).unwrap_or_else(|| {
        content
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("HTTP "))
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("…")
            .to_string()
    });
    let failed = content.starts_with("Error");
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;

    let mut header = {
        let text = if muted {
            theme.muted()
        } else {
            theme.primary()
        };
        let bold = text.add_modifier(Modifier::BOLD);
        let line = Line::from(vec![
            Span::styled("Fetch ".to_string(), bold),
            Span::styled(
                url,
                if muted {
                    theme.muted()
                } else {
                    theme.fg(theme.path)
                },
            ),
        ]);
        if width == 0 {
            line
        } else {
            truncate_line(line, width.saturating_sub(2).max(8))
        }
    };
    prepend_diamond(&mut header, theme, failed);
    if running && !open {
        live::mark_running(&mut header, theme);
        return vec![header];
    }
    if !open {
        return vec![header];
    }

    let mut out = vec![header, Line::from("")];
    if failed {
        push_error(&mut out, content, theme);
        return out;
    }
    // First line is often `HTTP 200 url` — show as meta.
    let mut body = content.lines().peekable();
    if let Some(first) = body.peek() {
        if first.starts_with("HTTP ") {
            out.push(Line::from(Span::styled(format!("  {first}"), theme.dim())));
            body.next();
            if body.peek().is_some_and(|l| l.is_empty()) {
                body.next();
            }
            out.push(Line::from(""));
        }
    }
    let rest: Vec<&str> = body.collect();
    let cap = if mode == ToolMode::Truncated {
        TRUNCATED_INLINE
    } else {
        EXPANDED_INLINE
    };
    for line in rest.iter().take(cap) {
        out.push(Line::from(Span::styled(format!("  {line}"), theme.muted())));
    }
    if rest.len() > cap {
        let n = rest.len() - cap;
        out.push(Line::from(Span::styled(
            format!("  \u{2026} ({n} more lines)"),
            theme.dim(),
        )));
    } else if rest.is_empty() {
        out.push(Line::from(Span::styled(
            "  (no content)".to_string(),
            theme.muted(),
        )));
    }
    if width > 0 {
        out = word_wrap_lines(out, width);
    }
    out
}

fn extract_result_urls(content: &str) -> Vec<String> {
    // Dock format: `1. https://...`
    content
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            let rest = t
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .trim_start_matches(['.', ')', ' ']);
            if rest.starts_with("http://") || rest.starts_with("https://") {
                Some(rest.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn sources_line(urls: &[String], theme: &Theme) -> Line<'static> {
    let domains: Vec<String> = urls
        .iter()
        .filter_map(|u| extract_domain(u))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let shown: Vec<&str> = domains
        .iter()
        .take(MAX_SOURCES)
        .map(|s| s.as_str())
        .collect();
    let extra = domains.len().saturating_sub(MAX_SOURCES);
    let mut text = format!("  Sources: {}", shown.join(", "));
    if extra > 0 {
        text.push_str(&format!(" (+{extra} more)"));
    }
    Line::from(Span::styled(text, theme.dim()))
}

fn extract_domain(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = rest.split('/').next()?;
    Some(host.trim_start_matches("www.").to_string())
}

fn str_arg(arguments: &str, keys: &[&str]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    keys.iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(str::to_string))
}

fn push_error(out: &mut Vec<Line<'static>>, content: &str, theme: &Theme) {
    for line in content.lines() {
        out.push(Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.accent_error),
        )));
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

    #[test]
    fn search_collapsed_counts_sites() {
        let theme = Theme::current();
        let content =
            "Search results for \"rust\":\n1. https://www.rust-lang.org/\n2. https://docs.rs/\n";
        let lines = lines(
            "web_search",
            r#"{"query":"rust"}"#,
            content,
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Web Search "), "{text}");
        assert!(text.contains("2 sites"), "{text}");
    }

    #[test]
    fn fetch_collapsed_shows_url() {
        let theme = Theme::current();
        let lines = lines(
            "web_fetch",
            r#"{"url":"https://example.com/page"}"#,
            "HTTP 200 https://example.com/page\n\nHello",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Fetch "), "{text}");
        assert!(text.contains("example.com"), "{text}");
        assert!(!text.contains("Hello"), "{text}");
    }
}
