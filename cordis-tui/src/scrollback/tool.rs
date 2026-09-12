//! Generic tool card — grok pager `OtherToolCallBlock` chrome + Execute-style
//! head/tail truncation for large output.
//!
//! Fold cycle (click header): Collapsed → Truncated → Expanded → Collapsed.
//! Truncated matches grok Execute defaults (`first_lines=2`, `last_lines=3`).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::scrollback::card;
use crate::scrollback::live;
use crate::theme::Theme;

/// Scrollback fold for a tool card (grok `DisplayMode` subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMode {
    Collapsed,
    Truncated,
    Expanded,
}

impl ToolMode {
    /// Click on the header: Collapsed → Truncated → Expanded → Collapsed.
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Collapsed => Some(Self::Truncated),
            Self::Truncated => Some(Self::Expanded),
            Self::Expanded => None,
        }
    }
}

/// Grok ExecuteConfig defaults for truncated shell/tool stdout.
const FIRST_LINES: usize = 2;
const LAST_LINES: usize = 3;

pub fn lines(
    name: &str,
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let open = mode != ToolMode::Collapsed;
    let muted = !open && !running;
    let summary = argument_summary(name, arguments);
    let mut header = if is_shell(name) {
        shell_header(&summary, theme, muted)
    } else {
        collapsed_line(
            name,
            &summary,
            theme,
            muted,
            Some(card::header_width(width)),
        )
    };
    prepend_diamond(&mut header, theme);
    if running && !open {
        live::mark_running(&mut header, theme);
    }
    let header = card::finish_header(header, width, mode, theme);
    if !open {
        return vec![header];
    }

    let mut out = vec![header];
    out.push(Line::from(""));
    if !is_shell(name) && !arguments.trim().is_empty() {
        out.push(card::indent(Line::from(Span::styled(
            "输入".to_string(),
            theme.dim(),
        ))));
        let input = pretty_args(arguments);
        let styled: Vec<Line<'static>> = input
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), theme.muted())))
            .collect();
        out.extend(card::body(styled, width));
        out.push(Line::from(""));
    }
    if running && content.is_empty() {
        out.push(card::indent(Line::from(live::running_body(theme))));
        return out;
    }
    if content.is_empty() {
        return out;
    }
    out.push(card::indent(Line::from(Span::styled(
        "输出".to_string(),
        theme.dim(),
    ))));
    let styled: Vec<Line<'static>> = content
        .lines()
        .map(|line| Line::from(Span::styled(line.to_string(), theme.muted())))
        .collect();
    let rows = card::body(styled, width);
    if mode == ToolMode::Truncated {
        out.extend(card::head_tail(rows, FIRST_LINES, LAST_LINES, "行", theme));
    } else {
        out.extend(rows);
    }
    out
}

fn is_shell(name: &str) -> bool {
    matches!(name, "bash" | "run_terminal_cmd" | "execute")
}

fn shell_header(command: &str, theme: &Theme, muted: bool) -> Line<'static> {
    let cmd_style = if muted {
        theme.muted()
    } else {
        theme.primary()
    };
    let cmd = if command.trim().is_empty() {
        "\u{2026}".to_string()
    } else {
        command.replace('\n', " ")
    };
    Line::from(vec![
        Span::styled("$ ".to_string(), theme.dim()),
        Span::styled(cmd, cmd_style),
    ])
}

fn pretty_args(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| raw.to_string())
}

fn json_str<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()))
}

fn argument_summary(name: &str, arguments: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let picked = match name {
        "bash" | "run_terminal_cmd" | "execute" => json_str(&v, &["command"]),
        "read_file" | "write_file" => json_str(&v, &["target_file", "path"]),
        "list_dir" => json_str(&v, &["target_directory", "path"]),
        "grep" => json_str(&v, &["pattern"]),
        "glob" => json_str(&v, &["glob_pattern", "pattern"]),
        "search_replace" => json_str(&v, &["file_path", "path"]),
        _ => None,
    };
    if let Some(s) = picked {
        return s.to_string();
    }
    if let Some(obj) = v.as_object() {
        if let Some((_, serde_json::Value::String(s))) = obj.iter().next() {
            return s.clone();
        }
    }
    let flat = arguments.replace('\n', " ");
    if flat.chars().count() > 60 {
        format!("{}…", flat.chars().take(59).collect::<String>())
    } else {
        flat
    }
}

/// Copied from grok `OtherToolCallBlock::collapsed_line`.
fn collapsed_line(
    name: &str,
    summary: &str,
    theme: &Theme,
    muted: bool,
    width: Option<usize>,
) -> Line<'static> {
    let text_style = if muted {
        theme.muted()
    } else {
        theme.primary()
    };
    let bold_style = text_style.add_modifier(Modifier::BOLD);

    let mut spans = if let Some((label, content)) = name.split_once(": ") {
        vec![
            Span::styled(format!("{label} "), bold_style),
            Span::styled(content.to_string(), text_style),
        ]
    } else {
        vec![Span::styled(name.to_string(), bold_style)]
    };

    if !summary.is_empty() {
        if let Some(w) = width {
            let used: usize = spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let summary = format!("  {summary}");
            // 显示宽度，不是字节数：中文摘要 3 字节/字符只占 2 列。
            if used + unicode_width::UnicodeWidthStr::width(summary.as_str()) <= w {
                spans.push(Span::styled(summary, theme.muted()));
            }
        } else {
            spans.push(Span::styled(format!("  {summary}"), theme.muted()));
        }
    }

    let line = Line::from(spans);
    match width {
        Some(w) => truncate_line(line, w),
        None => line,
    }
}

/// Copied from grok `prepend_bullet`: insert `{glyph} ` at the start of the line.
fn prepend_diamond(line: &mut Line<'static>, theme: &Theme) {
    let bullet_span = Span::styled(
        format!("{} ", glyphs::diamond_filled()),
        Style::default().fg(theme.accent_tool),
    );
    line.spans.insert(0, bullet_span);
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
    fn truncated_hides_middle_lines() {
        let theme = Theme::current();
        let body: String = (1..=12).map(|i| format!("L{i:02}\n")).collect();
        let lines = lines(
            "bash",
            r#"{"command":"seq 12"}"#,
            &body,
            &theme,
            80,
            ToolMode::Truncated,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("L01"), "{text}");
        assert!(text.contains("L02"), "{text}");
        assert!(text.contains("\u{2026} 还有 7 行"), "{text}");
        assert!(text.contains("L11"), "{text}");
        assert!(text.contains("L12"), "{text}");
        assert!(!text.contains("L05"), "{text}");
    }

    #[test]
    fn expanded_keeps_middle_lines() {
        let theme = Theme::current();
        let body: String = (1..=12).map(|i| format!("L{i:02}\n")).collect();
        let lines = lines(
            "bash",
            r#"{"command":"seq 12"}"#,
            &body,
            &theme,
            80,
            ToolMode::Expanded,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("L05"), "{text}");
        assert!(!text.contains("还有"), "{text}");
    }

    #[test]
    fn mode_cycles_three_ways() {
        assert_eq!(ToolMode::Collapsed.next(), Some(ToolMode::Truncated));
        assert_eq!(ToolMode::Truncated.next(), Some(ToolMode::Expanded));
        assert_eq!(ToolMode::Expanded.next(), None);
    }

    /// 中文摘要 3 字节/字符只占 2 列：按字节数比宽度会把放得下的摘要整条
    /// 丢掉（旧实现 `used + summary.len()` 的笔误）。
    #[test]
    fn chinese_summary_fits_by_display_width_not_bytes() {
        let theme = Theme::current();
        // "Edit" 4 列 + "  中文参数" 10 列 = 14 列，正好放得下；
        // 按字节算是 4 + 14 = 18 > 14，旧实现会丢。
        let line = collapsed_line("Edit", "中文参数", &theme, false, Some(14));
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("中文参数"), "{text}");

        // 真放不下（宽 12）时仍应被省掉，由 truncate_line 兜底头行。
        let line = collapsed_line("Edit", "中文参数", &theme, false, Some(12));
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!text.contains("中文参数"), "{text}");
    }
}
