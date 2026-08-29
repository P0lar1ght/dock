//! Plan mode tool cards — enter / exit (grok "Plan: Enter" / "Plan: Exit").
//!
//! Exit opens markdown-rendered plan body (from tool result after `## 计划：`
//! / `## Plan:`). Truncated: head 8 / tail 4 wrapped lines.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::scrollback::assistant;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

const FIRST_LINES: usize = 8;
const LAST_LINES: usize = 4;

const EMPTY_PLAN_PLACEHOLDER: &str = "\
# 尚未写入计划

代理退出计划模式时计划文件为空。

- 可继续实现
- 或再次 `/plan` 让代理重写计划
";

pub fn is_plan_tool(name: &str) -> bool {
    matches!(
        name,
        "enter_plan_mode"
            | "exit_plan_mode"
            | "EnterPlanMode"
            | "ExitPlanMode"
            | "Plan: Enter"
            | "Plan: Exit"
    )
}

pub fn lines(
    name: &str,
    _arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let exiting = is_exit(name);
    if exiting {
        exit_lines(content, theme, width, mode, running)
    } else {
        enter_lines(content, theme, width, mode, running)
    }
}

fn is_exit(name: &str) -> bool {
    matches!(name, "exit_plan_mode" | "ExitPlanMode" | "Plan: Exit")
}

fn enter_lines(
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let failed = content.starts_with("Error") || content.contains("未挂载");
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;
    let path = extract_plan_path(content).unwrap_or_else(|| ".dock/plan.md".into());

    let mut header = header_line("Plan: Enter", &path, None, muted, failed, theme, width);
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
    out.push(Line::from(Span::styled(
        "  只读探索 · 写入计划文件 · 完成后 exit_plan_mode".to_string(),
        theme.muted(),
    )));
    out
}

fn exit_lines(
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let failed = content.starts_with("Error")
        || content.contains("未挂载")
        || content.contains("当前不在计划模式");
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;
    let path = extract_plan_path(content).unwrap_or_else(|| ".dock/plan.md".into());
    let plan_md = extract_plan_markdown(content);
    let empty = plan_md.as_ref().is_none_or(|s| s.trim().is_empty());
    let line_count = plan_md
        .as_ref()
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    let suffix = if failed {
        None
    } else if empty {
        Some("(empty)".into())
    } else {
        Some(format!("({line_count} lines)"))
    };

    let mut header = header_line(
        "Plan: Exit",
        &path,
        suffix.as_deref(),
        muted,
        failed,
        theme,
        width,
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

    let body_src = if empty {
        EMPTY_PLAN_PLACEHOLDER
    } else {
        plan_md.as_deref().unwrap_or(EMPTY_PLAN_PLACEHOLDER)
    };
    let rendered = assistant::render(body_src, theme, width.saturating_sub(2).max(20));
    let body = apply_truncation(rendered.lines, mode, theme);
    out.extend(body);
    out
}

fn header_line(
    label: &str,
    path: &str,
    suffix: Option<&str>,
    muted: bool,
    failed: bool,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let text = if muted {
        theme.muted()
    } else if failed {
        Style::default().fg(theme.accent_error)
    } else {
        Style::default().fg(theme.accent_plan)
    };
    let bold = text.add_modifier(Modifier::BOLD);
    let path_style = if muted {
        theme.muted()
    } else {
        theme.fg(theme.path)
    };
    let mut spans = vec![
        Span::styled(format!("{label} "), bold),
        Span::styled(path.to_string(), path_style),
    ];
    if let Some(s) = suffix {
        spans.push(Span::styled(format!(" {s}"), theme.dim()));
    }
    let line = Line::from(spans);
    if width == 0 {
        line
    } else {
        truncate_line(line, width.saturating_sub(2).max(8))
    }
}

fn prepend_diamond(line: &mut Line<'static>, theme: &Theme, failed: bool) {
    let fg = if failed {
        theme.accent_error
    } else {
        theme.accent_plan
    };
    line.spans.insert(
        0,
        Span::styled(
            format!("{} ", glyphs::diamond_filled()),
            Style::default().fg(fg),
        ),
    );
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

/// Prefer relative `.dock/plan.md`; fall back to any path mentioned after 保存在 / saved at.
fn extract_plan_path(content: &str) -> Option<String> {
    for line in content.lines() {
        let t = line.trim();
        if t.contains(".dock/plan.md") {
            if let Some(idx) = t.find(".dock/plan.md") {
                return Some(t[idx..].split_whitespace().next()?.to_string());
            }
        }
        for marker in [
            "保存在：",
            "保存在:",
            "saved at:",
            "saved at：",
            "计划路径：",
            "计划路径:",
        ] {
            if let Some(rest) = t.split_once(marker) {
                let p = rest.1.trim();
                if !p.is_empty() {
                    return Some(p.to_string());
                }
            }
        }
    }
    None
}

/// Body after `## 计划：` / `## Plan:` (gork ExitPlanModeOutput prompt shape).
fn extract_plan_markdown(content: &str) -> Option<String> {
    const MARKERS: &[&str] = &["## 计划：", "## 计划:", "## Plan:", "## Plan："];
    for marker in MARKERS {
        if let Some(idx) = content.find(marker) {
            let after = content[idx + marker.len()..].trim();
            return Some(after.to_string());
        }
    }
    // Empty-exit messages have no marker.
    if content.contains("计划文件是空的")
        || (content.contains("empty") && content.to_lowercase().contains("plan"))
    {
        return Some(String::new());
    }
    None
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
    fn enter_collapsed_shows_plan_enter() {
        let theme = Theme::current();
        let lines = lines(
            "enter_plan_mode",
            "{}",
            "已进入计划模式\n\n把计划写到 /tmp/x/.dock/plan.md。文件已存在且为空。",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Plan: Enter"), "{text}");
        assert!(text.contains(".dock/plan.md"), "{text}");
        assert!(!text.contains("只读探索"), "{text}");
    }

    #[test]
    fn exit_collapsed_counts_lines() {
        let theme = Theme::current();
        let content = "已退出计划模式。现在可以改文件、跑 shell。\n\n\
            计划已保存在：.dock/plan.md\n\n\
            ## 计划：\n\
            # Auth\n\n\
            1. step one\n\
            2. step two\n";
        let lines = lines(
            "exit_plan_mode",
            "{}",
            content,
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Plan: Exit"), "{text}");
        assert!(text.contains(".dock/plan.md"), "{text}");
        assert!(text.contains("lines"), "{text}");
        assert!(!text.contains("step one"), "{text}");
    }

    #[test]
    fn exit_truncated_renders_markdown_heading() {
        let theme = Theme::current();
        let content = "已退出计划模式。\n\n计划已保存在：.dock/plan.md\n\n## 计划：\n# Auth Plan\n\nDo the thing.\n";
        let lines = lines(
            "exit_plan_mode",
            "{}",
            content,
            &theme,
            80,
            ToolMode::Truncated,
            false,
        );
        let text = plain(&lines);
        assert!(
            text.contains("Auth Plan") || text.contains("Auth"),
            "{text}"
        );
        assert!(!text.contains("## 计划"), "{text}");
    }

    #[test]
    fn exit_empty_shows_placeholder_when_open() {
        let theme = Theme::current();
        let lines = lines(
            "exit_plan_mode",
            "{}",
            "已退出计划模式，但计划文件是空的。现在可以改文件、跑 shell。",
            &theme,
            80,
            ToolMode::Truncated,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Plan: Exit"), "{text}");
        assert!(text.contains("(empty)"), "{text}");
        assert!(text.contains("尚未写入") || text.contains("计划"), "{text}");
    }
}
