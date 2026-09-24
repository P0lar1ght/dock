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
    // Dock 落下的 `is_error`（非零退出、被拒、被中断），不按输出猜。
    failed: bool,
) -> Vec<Line<'static>> {
    let command = command_from_args(arguments).unwrap_or_else(|| "\u{2026}".into());
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
    /// 该次调用的 tool call id——同时是它**自己那一段**的折叠键。
    pub id: &'a str,
    pub arguments: &'a str,
    pub content: &'a str,
    /// 这条调用失败了（`ToolExecute::is_error`）。
    pub failed: bool,
    /// 这一段的折叠态。缺省继承组的折叠态，用户单独点过才有自己的值。
    pub mode: ToolMode,
}

/// 合并卡的渲染结果：行，加上「第几行归哪个折叠键管」。
///
/// 组头归组键（折叠整张卡），每条命令的 `$ cmd` 与正文归它自己的 id——
/// grok 的做法是每条 execute 调用各是一个 `ExecuteToolCallBlock`、各带
/// `DisplayMode`，这里在「一张卡」的外观下保住同一件事：**三条长输出可以
/// 只收掉其中一条**。
pub struct GroupCard {
    pub lines: Vec<Line<'static>>,
    /// `(相对 lines 的行号, 折叠键)`
    pub row_ids: Vec<(usize, String)>,
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
    group_key: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
) -> GroupCard {
    let failed = calls.iter().any(|c| c.failed);
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
    let mut row_ids = vec![(0usize, group_key.to_string())];
    if !open {
        return GroupCard {
            lines: vec![header],
            row_ids,
        };
    }

    let mut out = vec![header];
    for call in calls {
        // 空行归组：点它切整组，不至于误伤某一条命令。
        row_ids.push((out.len(), group_key.to_string()));
        out.push(Line::from(""));
        let start = out.len();
        let command = command_from_args(call.arguments).unwrap_or_else(|| "\u{2026}".into());
        out.push(card::indent(Line::from(vec![
            Span::styled("$ ".to_string(), theme.dim()),
            Span::styled(command.replace('\n', " "), theme.primary()),
        ])));
        if call.mode != ToolMode::Collapsed {
            out.extend(call_body(
                call.content,
                call.failed,
                theme,
                width,
                call.mode,
            ));
        }
        // 这条命令的 `$ cmd` 与正文都归它自己的 id——点它只折它。
        for row in start..out.len() {
            row_ids.push((row, call.id.to_string()));
        }
    }
    GroupCard {
        lines: out,
        row_ids,
    }
}

/// 一条命令的输出块（合并卡里用）。空输出也要留一行，否则读不出这条跑过没有。
fn call_body(
    content: &str,
    failed: bool,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
) -> Vec<Line<'static>> {
    if content.is_empty() || content == "(no output)" {
        return vec![card::indent(Line::from(Span::styled(
            "（无输出）".to_string(),
            theme.muted(),
        )))];
    }
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
        card::head_tail(rows, FIRST_LINES, LAST_LINES, "行", theme)
    } else {
        rows
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

    /// 回归：失败以前按输出猜（`Error` / `exit` 开头），打印了 `Error` 字样但成功的
    /// 命令会被标红，真失败但输出正常的反而不标。现在只看 Dock 落下的 `is_error`。
    #[test]
    fn failure_color_follows_the_flag_not_the_output() {
        let theme = Theme::current();
        let diamond_fg = |content: &str, failed: bool| {
            lines(
                r#"{"command":"x"}"#,
                content,
                &theme,
                80,
                ToolMode::Collapsed,
                false,
                failed,
            )
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains(glyphs::diamond_filled()))
            .and_then(|s| s.style.fg)
        };
        assert_eq!(
            diamond_fg("Error: 只是打印出来的字", false),
            Some(theme.accent_tool)
        );
        assert_eq!(diamond_fg("看着正常", true), Some(theme.accent_error));
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
