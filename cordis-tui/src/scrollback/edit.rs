//! Edit / write tool card — simplified grok `EditToolCallBlock`.
//!
//! Dock `search_replace` only returns `updated path`, so the diff is rebuilt
//! from `old_string` / `new_string` in the arguments. `write_file` shows a
//! Creating header plus `+` lines from `contents`.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::grok::wrapping::word_wrap_lines;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

const FIRST_LINES: usize = 8;
const LAST_LINES: usize = 4;

pub fn is_edit_tool(name: &str) -> bool {
    matches!(
        name,
        "search_replace" | "edit" | "strreplace" | "write_file" | "write"
    )
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
    let creating = matches!(name, "write_file" | "write")
        || (name == "search_replace"
            && serde_json::from_str::<serde_json::Value>(arguments)
                .ok()
                .and_then(|v| {
                    v.get("old_string")
                        .and_then(|s| s.as_str())
                        .map(|s| s.is_empty())
                })
                .unwrap_or(false));
    let path = path_from_args(arguments).unwrap_or_else(|| "…".into());
    let failed = content.starts_with("Error");
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;

    let diff = if creating {
        DiffBody::from_create(contents_from_args(arguments).as_deref().unwrap_or(""))
    } else {
        DiffBody::from_replace(
            old_from_args(arguments).as_deref().unwrap_or(""),
            new_from_args(arguments).as_deref().unwrap_or(""),
        )
    };

    let mut header = header_line(
        creating,
        &path,
        diff.inserts,
        diff.deletes,
        muted,
        theme,
        width.saturating_sub(2).max(8),
    );
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
        for line in content.lines() {
            out.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(theme.accent_error),
            )));
        }
        return out;
    }
    if running && diff.rows.is_empty() {
        out.push(Line::from(live::running_body(theme)));
        return out;
    }
    let body = render_diff_rows(&diff.rows, theme, width);
    out.extend(apply_truncation(body, mode, theme));
    out
}

struct DiffBody {
    rows: Vec<DiffRow>,
    inserts: usize,
    deletes: usize,
}

enum DiffRow {
    Del(String),
    Ins(String),
    #[allow(dead_code)]
    Context(String),
}

impl DiffBody {
    fn from_replace(old: &str, new: &str) -> Self {
        let old_lines: Vec<&str> = if old.is_empty() {
            Vec::new()
        } else {
            old.lines().collect()
        };
        let new_lines: Vec<&str> = if new.is_empty() {
            Vec::new()
        } else {
            new.lines().collect()
        };
        // Simple display: all deletions then all insertions (no LCS). Enough
        // for search_replace's single-hunk shape.
        let mut rows = Vec::new();
        for l in &old_lines {
            rows.push(DiffRow::Del((*l).to_string()));
        }
        for l in &new_lines {
            rows.push(DiffRow::Ins((*l).to_string()));
        }
        Self {
            inserts: new_lines.len(),
            deletes: old_lines.len(),
            rows,
        }
    }

    fn from_create(contents: &str) -> Self {
        let lines: Vec<&str> = contents.lines().collect();
        let rows = lines
            .iter()
            .map(|l| DiffRow::Ins((*l).to_string()))
            .collect();
        Self {
            inserts: lines.len(),
            deletes: 0,
            rows,
        }
    }
}

fn header_line(
    creating: bool,
    path: &str,
    inserts: usize,
    deletes: usize,
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
    let prefix = if creating { "Creating " } else { "Edit " };
    let mut spans = vec![
        Span::styled(prefix.to_string(), bold),
        Span::styled(path.to_string(), path_style),
    ];
    // Diffstat stays visible when collapsed (muted label); only skip on empty.
    if inserts > 0 || deletes > 0 {
        spans.push(Span::styled(" ".to_string(), theme.muted()));
        if inserts > 0 {
            spans.push(Span::styled(
                format!("+{inserts}"),
                if muted {
                    theme.muted()
                } else {
                    Style::default().fg(theme.diff_insert_fg)
                },
            ));
        }
        if inserts > 0 && deletes > 0 {
            spans.push(Span::styled("/".to_string(), theme.dim()));
        }
        if deletes > 0 {
            spans.push(Span::styled(
                format!("-{deletes}"),
                if muted {
                    theme.muted()
                } else {
                    Style::default().fg(theme.diff_delete_fg)
                },
            ));
        }
    }
    let line = Line::from(spans);
    if width == 0 {
        line
    } else {
        truncate_line(line, width)
    }
}

fn render_diff_rows(rows: &[DiffRow], theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for row in rows {
        let (mark, text, style) = match row {
            DiffRow::Del(t) => (
                "- ",
                t.as_str(),
                Style::default()
                    .fg(theme.diff_delete_fg)
                    .bg(theme.diff_delete_bg),
            ),
            DiffRow::Ins(t) => (
                "+ ",
                t.as_str(),
                Style::default()
                    .fg(theme.diff_insert_fg)
                    .bg(theme.diff_insert_bg),
            ),
            DiffRow::Context(t) => ("  ", t.as_str(), Style::default().fg(theme.diff_equal_fg)),
        };
        let line = Line::from(vec![
            Span::styled(mark.to_string(), style),
            Span::styled(text.to_string(), style),
        ]);
        out.push(line);
    }
    if width == 0 {
        out
    } else {
        word_wrap_lines(out, width.saturating_sub(2).max(20))
    }
}

fn apply_truncation(
    wrapped: Vec<Line<'static>>,
    mode: ToolMode,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if mode != ToolMode::Truncated {
        return wrapped;
    }
    let total = wrapped.len();
    let threshold = FIRST_LINES + LAST_LINES;
    if total <= threshold {
        return wrapped;
    }
    let hidden = total - threshold;
    let mut out: Vec<_> = wrapped.iter().take(FIRST_LINES).cloned().collect();
    out.push(Line::from(Span::styled(
        format!("\u{2026} +{hidden} lines"),
        theme.muted(),
    )));
    out.extend(wrapped.into_iter().skip(total - LAST_LINES));
    out
}

fn path_from_args(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    ["file_path", "target_file", "path", "filePath"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(str::to_string))
}

fn old_from_args(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    v.get("old_string")
        .and_then(|x| x.as_str())
        .map(str::to_string)
}

fn new_from_args(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    v.get("new_string")
        .and_then(|x| x.as_str())
        .map(str::to_string)
}

fn contents_from_args(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    v.get("contents")
        .and_then(|x| x.as_str())
        .map(str::to_string)
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
    fn collapsed_edit_shows_diffstat() {
        let theme = Theme::current();
        let lines = lines(
            "search_replace",
            r#"{"file_path":"a.rs","old_string":"a\nb","new_string":"a\nc\nd"}"#,
            "updated a.rs",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Edit "), "{text}");
        assert!(text.contains("a.rs"), "{text}");
        // Naive all-del-then-all-ins: old 2 lines, new 3 lines.
        assert!(text.contains("+3"), "{text}");
        assert!(text.contains("-2"), "{text}");
        assert!(!text.contains("+ a"), "{text}");
    }

    #[test]
    fn expanded_shows_minus_plus() {
        let theme = Theme::current();
        let lines = lines(
            "search_replace",
            r#"{"file_path":"a.rs","old_string":"old","new_string":"new"}"#,
            "updated a.rs",
            &theme,
            80,
            ToolMode::Expanded,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("- old") || text.contains("-old"), "{text}");
        assert!(text.contains("+ new") || text.contains("+new"), "{text}");
    }

    #[test]
    fn write_file_creating_prefix() {
        let theme = Theme::current();
        let lines = lines(
            "write_file",
            r#"{"target_file":"n.rs","contents":"fn main() {}"}"#,
            "wrote n.rs",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Creating "), "{text}");
        assert!(text.contains("n.rs"), "{text}");
    }
}
