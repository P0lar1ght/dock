//! ListDir card — simplified grok `ListDirToolCallBlock`.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::scrollback::card;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

const FIRST_LINES: usize = 12;
const LAST_LINES: usize = 4;

pub fn is_list_dir_tool(name: &str) -> bool {
    matches!(name, "list_dir" | "ls")
}

pub fn lines(
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let path = path_from_args(arguments).unwrap_or_else(|| "…".into());
    let failed = content.starts_with("Error");
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;

    let entries = parse_entries(content);
    let mut header = header_line(
        &path,
        entries.len(),
        failed || entries.is_empty() && content.contains("(empty)"),
        muted,
        theme,
        card::header_width(width),
    );
    prepend_diamond(&mut header, theme, failed);
    if running && !open {
        live::mark_running(&mut header, theme);
    }
    let header = card::finish_header(header, width, mode, theme);
    if !open {
        return vec![header];
    }

    let mut out = vec![header, Line::from("")];
    if failed {
        for line in content.lines() {
            out.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(theme.accent_error),
            )));
        }
        return out;
    }
    if entries.is_empty() {
        out.push(card::indent(Line::from(Span::styled(
            "（空目录）".to_string(),
            theme.muted(),
        ))));
        return out;
    }
    let body: Vec<Line<'static>> = entries
        .into_iter()
        .map(|e| {
            let style = if e.ends_with('/') {
                theme.fg(theme.path)
            } else {
                theme.primary()
            };
            Line::from(Span::styled(e, style))
        })
        .collect();
    let rows = card::body(body, width);
    if mode == ToolMode::Truncated {
        out.extend(card::head_tail(rows, FIRST_LINES, LAST_LINES, "项", theme));
    } else {
        out.extend(rows);
    }
    out
}

fn parse_entries(content: &str) -> Vec<String> {
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    if lines.is_empty() {
        return lines;
    }
    // workspace list_dir: first line is the directory path itself.
    if lines.len() == 1 && lines[0].contains("(empty)") {
        return Vec::new();
    }
    if lines.len() > 1 {
        lines.remove(0);
    }
    lines.retain(|l| !l.trim().is_empty());
    lines
}

fn header_line(
    path: &str,
    count: usize,
    empty: bool,
    muted: bool,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let text = if muted {
        theme.muted()
    } else {
        theme.primary()
    };
    let bold = text.add_modifier(Modifier::BOLD);
    let path_style = if muted {
        theme.muted()
    } else {
        theme.fg(theme.path)
    };
    let mut spans = vec![
        Span::styled("List ".to_string(), bold),
        Span::styled(path.to_string(), path_style),
    ];
    let suffix = if empty {
        " (empty)".to_string()
    } else if count == 1 {
        " (1 entry)".to_string()
    } else if count > 0 {
        format!(" ({count} entries)")
    } else {
        String::new()
    };
    if !suffix.is_empty() {
        spans.push(Span::styled(suffix, theme.dim()));
    }
    let line = Line::from(spans);
    if width == 0 {
        line
    } else {
        truncate_line(line, width)
    }
}

fn path_from_args(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    ["target_directory", "path"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(str::to_string))
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
    fn collapsed_lists_count() {
        let theme = Theme::current();
        let content = "/tmp/x\na.txt\nb/\nc.txt\n";
        let lines = lines(
            r#"{"target_directory":"/tmp/x"}"#,
            content,
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("List "), "{text}");
        assert!(text.contains("3 entries"), "{text}");
        assert!(!text.contains("a.txt"), "{text}");
    }
}
