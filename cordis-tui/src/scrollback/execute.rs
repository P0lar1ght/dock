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
    let failed = is_failure(content);
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

/// `Bash <command>`，与 Read / Glob / List / Search / Edit 同构：**粗体工具名 +
/// 常规参数**。
///
/// 原先只有一个 dim 的 `$` 前缀、命令本身粗体，于是滚动区里只有 bash 这一行
/// 没有工具名，扫一眼分不出它是什么卡。保留 `$` 是因为它仍是「这是一条 shell
/// 命令」最直接的提示，但降级成标签之后的分隔符，粗体让给标签。
fn shell_header(command: &str, theme: &Theme, muted: bool) -> Line<'static> {
    let text_style = if muted {
        theme.muted()
    } else {
        theme.primary()
    };
    let label_style = text_style.add_modifier(Modifier::BOLD);
    let cmd = command.replace('\n', " ");
    Line::from(vec![
        Span::styled("Bash ".to_string(), label_style),
        Span::styled("$ ".to_string(), theme.dim()),
        Span::styled(cmd, text_style),
    ])
}

fn command_from_args(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    v.get("command")
        .and_then(|x| x.as_str())
        .map(str::to_string)
}

/// 一次已完成的 bash 调用，供 [`group_lines`] 合并渲染。
pub struct Call<'a> {
    pub arguments: &'a str,
    pub content: &'a str,
}

/// 连续多次 bash 调用合成一张卡。
///
/// 滚动区里一串 `$ cargo fmt` / `$ cargo clippy` / `$ cargo test` 会占掉三张卡
/// 的篇幅，而它们在语义上是同一件事（「跑一遍门禁」）。合并之后折叠态只占一行，
/// 展开仍能逐条看命令与各自的输出。
///
/// **只合并相邻且都已完成的前台 bash**：后台 bash 归 `bg_task` 卡（它有自己的
/// 任务 id 与实时输出），正在跑的调用要单独挂时钟，都不参与合并。
pub fn group_lines(
    calls: &[Call<'_>],
    theme: &Theme,
    width: usize,
    mode: ToolMode,
) -> Vec<Line<'static>> {
    let failed = calls.iter().any(|c| is_failure(c.content));
    let open = mode != ToolMode::Collapsed;
    let muted = !open || failed;
    let text_style = if muted {
        theme.muted()
    } else {
        theme.primary()
    };

    let mut header = Line::from(vec![
        Span::styled("Bash ".to_string(), text_style.add_modifier(Modifier::BOLD)),
        Span::styled(format!("{} 条命令", calls.len()), text_style),
    ]);
    prepend_diamond(&mut header, theme, failed);
    let header = card::finish_header(header, width, mode, theme);
    if !open {
        return vec![header];
    }

    let mut out = vec![header];
    for call in calls {
        out.push(Line::from(""));
        let command = command_from_args(call.arguments).unwrap_or_else(|| "\u{2026}".into());
        out.push(card::indent(Line::from(vec![
            Span::styled("$ ".to_string(), theme.dim()),
            Span::styled(command.replace('\n', " "), theme.primary()),
        ])));
        out.extend(call_body(call.content, theme, width, mode));
    }
    out
}

/// 一条命令的输出块（合并卡里用）。空输出也要留一行，否则读不出这条跑过没有。
fn call_body(content: &str, theme: &Theme, width: usize, mode: ToolMode) -> Vec<Line<'static>> {
    if content.is_empty() || content == "(no output)" {
        return vec![card::indent(Line::from(Span::styled(
            "（无输出）".to_string(),
            theme.muted(),
        )))];
    }
    let body_style = if is_failure(content) {
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
        card::head_tail(rows, FIRST_LINES, LAST_LINES, "行", theme)
    } else {
        rows
    }
}

fn is_failure(content: &str) -> bool {
    content.starts_with("exit ") || content.starts_with("Error") || content.contains("\nexit ")
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
        // 工具名要在，否则滚动区里只有 bash 这一行认不出是什么卡。
        assert!(text.contains("Bash $ seq"), "{text}");
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
