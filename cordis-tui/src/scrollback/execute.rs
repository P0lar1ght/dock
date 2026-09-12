//! Execute / bash card — grok-shaped `$ command` + head/tail truncation,
//! with exit-error accent.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::scrollback::card;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

const FIRST_LINES: usize = 2;
const LAST_LINES: usize = 3;

pub fn is_execute_tool(name: &str) -> bool {
    matches!(name, "bash" | "run_terminal_cmd" | "execute" | "shell")
}

pub fn lines(
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let command = command_from_args(arguments).unwrap_or_else(|| "\u{2026}".into());
    let failed =
        content.starts_with("exit ") || content.starts_with("Error") || content.contains("\nexit ");
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;

    let mut header = shell_header(&command, theme, muted);
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
        out.push(Line::from(live::running_body(theme)));
        return out;
    }
    if content.is_empty() || content == "(no output)" {
        out.push(Line::from(""));
        out.push(card::indent(Line::from(Span::styled(
            "（无输出）".to_string(),
            theme.muted(),
        ))));
        return out;
    }

    out.push(Line::from(""));
    let body_style = if failed {
        Style::default().fg(theme.accent_error)
    } else {
        theme.muted()
    };
    let styled: Vec<Line<'static>> = content
        .lines()
        .map(|line| Line::from(Span::styled(line.to_string(), body_style)))
        .collect();
    let rows = card::body(styled, width);
    if mode == ToolMode::Truncated {
        out.extend(card::head_tail(rows, FIRST_LINES, LAST_LINES, "行", theme));
    } else {
        out.extend(rows);
    }
    out
}

fn shell_header(command: &str, theme: &Theme, muted: bool) -> Line<'static> {
    let cmd_style = if muted {
        theme.muted()
    } else {
        theme.primary().add_modifier(Modifier::BOLD)
    };
    let cmd = command.replace('\n', " ");
    Line::from(vec![
        Span::styled("$ ".to_string(), theme.dim()),
        Span::styled(cmd, cmd_style),
    ])
}

fn command_from_args(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    v.get("command")
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
    fn truncated_hides_middle() {
        let theme = Theme::current();
        let body: String = (1..=12).map(|i| format!("L{i:02}\n")).collect();
        let lines = lines(
            r#"{"command":"seq"}"#,
            &body,
            &theme,
            80,
            ToolMode::Truncated,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("$ "), "{text}");
        assert!(text.contains("\u{2026} 还有 7 行"), "{text}");
        assert!(!text.contains("L05"), "{text}");
        // 正文按公共外壳缩进，和标题文字对齐。
        assert!(text.contains("\n  L01"), "{text}");
        // 截断态的折叠符说明「还有更多」。
        assert!(
            lines[0].spans.last().unwrap().content.contains('\u{2304}'),
            "{text}"
        );
    }
}
