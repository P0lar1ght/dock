//! MCP tool card — Grok `UseToolCallBlock`: **Server** `Action`, kv args, output.

use cordis_spine::{is_mcp_public_name, USE_TOOL_NAME};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::grok::wrapping::word_wrap_lines;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

const TRUNCATED_INLINE: usize = 3;

pub fn is_mcp_tool(name: &str) -> bool {
    name == USE_TOOL_NAME || is_mcp_public_name(name)
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
    let failed = looks_failed(content);
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;
    let mut header = header_line(
        &card_name(name, arguments),
        theme,
        muted,
        Some(width.saturating_sub(2)),
    );
    prepend_diamond(&mut header, theme, failed);
    if running && !open {
        live::mark_running(&mut header, theme);
        return vec![header];
    }
    if !open {
        return vec![header];
    }

    let mut out = vec![header];
    let args = display_args(name, arguments);
    if !args.is_empty() {
        out.push(Line::from(""));
        for (key, val) in &args {
            out.push(Line::from(vec![
                Span::styled(format!("  {key}: "), theme.muted()),
                Span::styled(val.clone(), theme.primary()),
            ]));
        }
    }
    if running && content.is_empty() {
        out.push(Line::from(""));
        out.push(Line::from(live::running_body(theme)));
        return finish(out, width);
    }
    if !content.is_empty() {
        out.push(Line::from(""));
        let failed_style = if failed {
            theme.fg(theme.accent_error)
        } else {
            theme.primary()
        };
        let raw: Vec<&str> = content.lines().collect();
        let (shown, extra) = if mode == ToolMode::Truncated && raw.len() > TRUNCATED_INLINE {
            (&raw[..TRUNCATED_INLINE], Some(raw.len() - TRUNCATED_INLINE))
        } else {
            (raw.as_slice(), None)
        };
        for line in shown {
            out.push(Line::from(Span::styled(format!("  {line}"), failed_style)));
        }
        if let Some(remaining) = extra {
            out.push(Line::from(Span::styled(
                format!("  … 还有 {remaining} 行"),
                theme.dim(),
            )));
        }
    }
    finish(out, width)
}

fn finish(out: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        out
    } else {
        word_wrap_lines(out, width)
    }
}

fn looks_failed(content: &str) -> bool {
    let t = content.trim_start();
    t.starts_with("Error") || t.starts_with("error") || t.starts_with("{\"error")
}

/// Grok `mcp_titleize_segment`: split on `_`, title-case each word.
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

fn split_name(name: &str) -> (String, String) {
    let rest = name.strip_prefix("mcp_").unwrap_or(name);
    match rest.split_once("__") {
        Some((server, action)) => (titleize(server), titleize(action)),
        None => (String::new(), titleize(rest)),
    }
}

fn header_line(name: &str, theme: &Theme, muted: bool, max_width: Option<usize>) -> Line<'static> {
    let text_style = if muted {
        theme.muted()
    } else {
        theme.primary()
    };
    let bold_style = text_style.add_modifier(Modifier::BOLD);
    let action_style = if muted {
        theme.muted()
    } else {
        theme.fg(theme.command)
    };
    let (server, action) = split_name(name);
    let line = if server.is_empty() {
        Line::from(vec![Span::styled(action, bold_style)])
    } else {
        Line::from(vec![
            Span::styled(format!("{server} "), bold_style),
            Span::styled(action, action_style),
        ])
    };
    match max_width {
        Some(w) => truncate_line(line, w),
        None => line,
    }
}

fn card_name(name: &str, arguments: &str) -> String {
    if name == USE_TOOL_NAME {
        if let Some(inner) = inner_tool_name(arguments) {
            return inner;
        }
    }
    name.to_string()
}

fn inner_tool_name(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    v.get("tool_name")
        .and_then(|n| n.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn display_args(name: &str, arguments: &str) -> Vec<(String, String)> {
    if name != USE_TOOL_NAME {
        return input_args(arguments);
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return input_args(arguments);
    };
    match v.get("tool_input") {
        Some(input) => flatten_value(input),
        None => input_args(arguments)
            .into_iter()
            .filter(|(k, _)| k != "tool_name")
            .collect(),
    }
}

fn flatten_value(v: &serde_json::Value) -> Vec<(String, String)> {
    match v {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, val)| {
                let s = match val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (k.clone(), s)
            })
            .collect(),
        serde_json::Value::Null => Vec::new(),
        other => vec![("args".into(), other.to_string())],
    }
}

fn input_args(arguments: &str) -> Vec<(String, String)> {
    let trimmed = arguments.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return vec![("args".into(), trimmed.to_string())];
    };
    match v {
        serde_json::Value::Object(map) => map
            .into_iter()
            .map(|(k, val)| {
                let s = match val {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                (k, s)
            })
            .collect(),
        serde_json::Value::Null => Vec::new(),
        other => vec![("args".into(), other.to_string())],
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
    fn use_tool_header_uses_inner_name() {
        let theme = Theme::current();
        let text = plain(&lines(
            "use_tool",
            r#"{"tool_name":"mcp_local__echo","tool_input":{"text":"hi"}}"#,
            "pong",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        ));
        assert!(text.contains("Local"), "{text}");
        assert!(text.contains("Echo"), "{text}");
        assert!(!text.contains("use_tool"), "{text}");
        assert!(!text.contains("mcp_local__echo"), "{text}");
    }

    #[test]
    fn header_strips_prefix_and_titleizes() {
        assert_eq!(titleize("list_issues"), "List Issues");
        assert_eq!(titleize("local"), "Local");
        assert_eq!(
            split_name("mcp_local__sungods_search"),
            ("Local".into(), "Sungods Search".into())
        );
        let theme = Theme::current();
        let text = plain(&lines(
            "mcp_local__echo",
            r#"{"text":"hi"}"#,
            "pong",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        ));
        assert!(text.contains("Local"), "{text}");
        assert!(text.contains("Echo"), "{text}");
        assert!(!text.contains("mcp_local__echo"), "{text}");
        assert!(!text.contains("Mcp "), "{text}");
    }

    #[test]
    fn use_tool_expanded_shows_inner_args() {
        let theme = Theme::current();
        let text = plain(&lines(
            "use_tool",
            r#"{"tool_name":"mcp_local__echo","tool_input":{"text":"hi","n":2}}"#,
            "pong",
            &theme,
            80,
            ToolMode::Expanded,
            false,
        ));
        assert!(text.contains("Local"), "{text}");
        assert!(text.contains("text: "), "{text}");
        assert!(text.contains("hi"), "{text}");
        assert!(!text.contains("tool_name"), "{text}");
        assert!(!text.contains("tool_input"), "{text}");
    }

    #[test]
    fn expanded_shows_kv_args_and_output() {
        let theme = Theme::current();
        let text = plain(&lines(
            "mcp_local__echo",
            r#"{"text":"hi","n":2}"#,
            "pong",
            &theme,
            80,
            ToolMode::Expanded,
            false,
        ));
        assert!(text.contains("text: "), "{text}");
        assert!(text.contains("hi"), "{text}");
        assert!(text.contains("pong"), "{text}");
    }

    #[test]
    fn truncated_caps_inline_output() {
        let theme = Theme::current();
        let body: String = (1..=8).map(|i| format!("L{i:02}\n")).collect();
        let text = plain(&lines(
            "mcp_local__echo",
            "{}",
            &body,
            &theme,
            80,
            ToolMode::Truncated,
            false,
        ));
        assert!(text.contains("L01"), "{text}");
        assert!(text.contains("L03"), "{text}");
        assert!(!text.contains("L04"), "{text}");
        assert!(text.contains("还有 5 行"), "{text}");
    }
}
