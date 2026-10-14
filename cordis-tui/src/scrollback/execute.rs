//! Execute / bash card — grok-shaped `$ command` + head/tail truncation,
//! with exit-error accent.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::scrollback::card;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;
use unicode_width::UnicodeWidthStr;

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
    let description = description_from_args(arguments);
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;

    // 有描述：标题是「描述 · 程序」，完整命令展开后在正文第一行。
    let mut header = match &description {
        Some(desc) => described_header(desc, program_label(&command).as_deref(), theme, muted),
        None => shell_header(&command, theme, muted),
    };
    prepend_diamond(&mut header, theme, failed);
    if running && !open {
        live::mark_running(&mut header, theme);
    }
    let header = card::finish_header(header, width, mode, theme);
    if !open {
        return vec![header];
    }

    let mut out = vec![header];
    if description.is_some() {
        out.push(Line::from(""));
        out.extend(command_rows(&command, theme, width));
    }
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

/// 有描述时的标题：`<描述> · <程序>`。
///
/// 描述是给人看的一句话；整条命令放进标题只会截出一串 `cd … && find src src-…`，
/// 所以标题只带主程序名（[`program_label`]）认个类型，原文展开后在正文第一行。
/// 不再写「Bash」：前面的 ◆ 和程序名已经说明这是一条命令。
fn described_header(
    description: &str,
    program: Option<&str>,
    theme: &Theme,
    muted: bool,
) -> Line<'static> {
    let text_style = if muted {
        theme.muted()
    } else {
        theme.primary()
    };
    let mut spans = vec![Span::styled(description.to_string(), text_style)];
    if let Some(program) = program {
        spans.push(Span::styled(format!(" · {program}"), theme.dim()));
    }
    Line::from(spans)
}

/// 命令的主程序，给标题认类型用：`cd dock-gui && find src … | wc -l` → `find`，
/// `RUST_LOG=debug cargo test -p x` → `cargo test`，`git -C repo status` → `git status`。
///
/// 跳过开头的 `cd` / `export` 等铺垫段、环境变量赋值和 `sudo` / `time` 之类前缀；
/// 管道只看第一段；带子命令的程序（cargo、git、npm…）连子命令一起。认不出就 `None`。
/// GUI 的 `dock-gui/src/dock/programLabel.ts` 是同一套规则。
fn program_label(command: &str) -> Option<String> {
    const SETUP: &[&str] = &[
        "cd", "pushd", "popd", "export", "unset", "set", "source", ".",
    ];
    const PREFIX: &[&str] = &["sudo", "time", "env", "nohup", "exec", "command"];
    const SUBCOMMANDS: &[&str] = &[
        "cargo", "git", "npm", "pnpm", "yarn", "bun", "npx", "go", "docker", "kubectl", "gh",
        "brew", "uv", "pip", "rustup", "deno", "make",
    ];
    let flat = command
        .replace("&&", ";")
        .replace("||", ";")
        .replace('\n', ";");
    for segment in flat.split(';') {
        let first = segment.split('|').next().unwrap_or("");
        let mut words = first
            .split_whitespace()
            .map(|w| w.trim_matches(|c| c == '\'' || c == '"'))
            .filter(|w| !w.is_empty());
        let Some(program) = words
            .by_ref()
            .find(|w| !is_env_assignment(w) && !PREFIX.contains(w))
        else {
            continue;
        };
        if SETUP.contains(&program) {
            continue;
        }
        let name = program.rsplit('/').next().unwrap_or(program);
        if name.is_empty() {
            continue;
        }
        if SUBCOMMANDS.contains(&name) {
            let mut skip_value = false;
            for w in words {
                if skip_value {
                    skip_value = false;
                    continue;
                }
                // `git -C <目录>` / `git -c k=v`：选项带值，值不是子命令。
                if name == "git" && (w == "-C" || w == "-c") {
                    skip_value = true;
                    continue;
                }
                if w.starts_with('-') || w.starts_with('+') {
                    continue;
                }
                if w.contains('/') || w.contains('=') {
                    break;
                }
                return Some(format!("{name} {w}"));
            }
        }
        return Some(name.to_string());
    }
    None
}

fn is_env_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(key, _)| {
        key.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// 展开后正文里的完整命令：`$ ` 暗色 + 命令，按宽度折行、与正文同缩进。
fn command_rows(command: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    card::body(
        vec![Line::from(vec![
            Span::styled("$ ".to_string(), theme.dim()),
            Span::styled(command.replace('\n', " "), theme.primary()),
        ])],
        width,
    )
}

/// 模型给的 `description`（bash 参数里的一句话说明），折掉换行与多余空白；空的当没有。
fn description_from_args(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let desc = v.get("description")?.as_str()?;
    let desc = desc.split_whitespace().collect::<Vec<_>>().join(" ");
    (!desc.is_empty()).then_some(desc)
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

    // 有描述的条目按列对齐：描述补齐到同宽，程序名排成一列。
    let desc_col = calls
        .iter()
        .filter_map(|c| description_from_args(c.arguments))
        .map(|d| UnicodeWidthStr::width(d.as_str()))
        .max()
        .unwrap_or(0)
        .min(card::body_width(width).saturating_sub(16).max(10));

    let mut out = vec![header];
    for call in calls {
        // 空行归组：点它切整组，不至于误伤某一条命令。
        row_ids.push((out.len(), group_key.to_string()));
        out.push(Line::from(""));
        let start = out.len();
        let command = command_from_args(call.arguments).unwrap_or_else(|| "\u{2026}".into());
        let call_open = call.mode != ToolMode::Collapsed;
        match description_from_args(call.arguments) {
            // 有描述：条目是「描述  程序」（列对齐）；展开时完整命令另起一行。
            Some(desc) => {
                let mut spans = vec![Span::styled(desc.clone(), theme.primary())];
                if let Some(program) = program_label(&command) {
                    let pad = desc_col.saturating_sub(UnicodeWidthStr::width(desc.as_str()));
                    spans.push(Span::styled(
                        format!("{}   {program}", " ".repeat(pad)),
                        theme.dim(),
                    ));
                }
                let entry = Line::from(spans);
                let entry = if width == 0 {
                    entry
                } else {
                    crate::grok::line_utils::truncate_line(entry, card::body_width(width))
                };
                out.push(card::indent(entry));
                if call_open {
                    out.extend(command_rows(&command, theme, width));
                }
            }
            None => out.push(card::indent(Line::from(vec![
                Span::styled("$ ".to_string(), theme.dim()),
                Span::styled(command.replace('\n', " "), theme.primary()),
            ]))),
        }
        if call_open {
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

    const DESCRIBED: &str = r#"{"command":"cd dock-gui && find src -type f | wc -l","description":"Count source files\n excluding build output"}"#;

    /// 有描述：标题是「描述 · 程序」（不再截一串命令头），展开后完整命令是正文第一行。
    #[test]
    fn description_leads_with_program_and_full_command_inside() {
        let theme = Theme::current();
        let collapsed = lines(
            DESCRIBED,
            "42",
            &theme,
            120,
            ToolMode::Collapsed,
            false,
            false,
        );
        assert_eq!(collapsed.len(), 1);
        let head = plain(&collapsed);
        assert!(
            head.contains("\u{25c6} Count source files excluding build output · find"),
            "{head}"
        );
        assert!(!head.contains("cd dock-gui"), "标题不放命令原文：{head}");
        assert!(!head.contains("Bash"), "{head}");
        let program = collapsed[0]
            .spans
            .iter()
            .find(|s| s.content.contains("· find"))
            .expect("程序名那一段");
        assert_eq!(program.style, theme.dim(), "程序名是暗色");

        let open = lines(
            DESCRIBED,
            "42",
            &theme,
            120,
            ToolMode::Expanded,
            false,
            false,
        );
        let text = plain(&open);
        let rows: Vec<&str> = text.lines().collect();
        assert!(
            rows[0].contains("Count source files excluding build output · find"),
            "{text}"
        );
        assert_eq!(
            rows[2].trim(),
            "$ cd dock-gui && find src -type f | wc -l",
            "{text}"
        );
        assert_eq!(rows[4].trim(), "42", "{text}");
    }

    /// 合并卡：有描述的条目是「描述  程序」且程序名对成一列；展开的那条完整命令另起一行。
    #[test]
    fn group_entries_align_descriptions_and_programs() {
        let theme = Theme::current();
        let calls = [
            Call {
                id: "a",
                arguments: DESCRIBED,
                content: "42",
                failed: false,
                mode: ToolMode::Collapsed,
            },
            Call {
                id: "b",
                arguments: r#"{"command":"git status","description":"Show working tree status"}"#,
                content: "clean",
                failed: false,
                mode: ToolMode::Expanded,
            },
            Call {
                id: "c",
                arguments: r#"{"command":"ls"}"#,
                content: "",
                failed: false,
                mode: ToolMode::Collapsed,
            },
        ];
        let card = group_lines(&calls, "g", &theme, 120, ToolMode::Expanded);
        let text = plain(&card.lines);
        let row = |needle: &str| {
            text.lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle}：{text}"))
                .to_string()
        };
        let a = row("Count source files");
        let b = row("Show working tree status");
        assert_eq!(
            a.find("find"),
            b.find("git status"),
            "程序名对齐：\n{a}\n{b}"
        );
        assert!(text.contains("Show working tree status"), "{text}");
        assert!(
            text.contains("\n  $ git status\n  clean"),
            "展开的那条另起一行：{text}"
        );
        assert!(!a.contains("cd dock-gui"), "收起的那条不放命令原文：{a}");
        // 没描述的照旧。
        assert!(text.contains("  $ ls"), "{text}");
    }

    #[test]
    fn program_label_finds_the_real_program() {
        let cases = [
            ("cd dock-gui && find src -type f | wc -l", Some("find")),
            (
                "RUST_LOG=debug cargo +nightly test -p cordis-tui",
                Some("cargo test"),
            ),
            ("git -C ../repo status -sb", Some("git status")),
            ("sudo time npm run build", Some("npm run")),
            ("export A=1; pnpm install", Some("pnpm install")),
            ("/usr/bin/python3 -m http.server", Some("python3")),
            ("cargo --version", Some("cargo")),
            ("'ls' -la", Some("ls")),
            ("cd /tmp", None),
            ("", None),
        ];
        for (command, want) in cases {
            assert_eq!(program_label(command).as_deref(), want, "{command}");
        }
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
