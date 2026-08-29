//! Goal tool cards — set / progress / done / blocked (Plan card chrome).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

pub fn is_goal_tool(name: &str) -> bool {
    matches!(name, "update_goal" | "UpdateGoal" | "Goal")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Set,
    Progress,
    Done,
    Blocked,
    Rejected,
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
    let _ = name;
    let parsed = parse_args(arguments);
    let kind = classify(&parsed, content);
    let failed = kind == Kind::Rejected;
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;
    let accent = kind_accent(kind, theme);
    let label = kind_label(kind);
    let title = parsed
        .objective
        .clone()
        .or_else(|| parsed.message.clone())
        .or_else(|| summary_from_content(content))
        .unwrap_or_default();
    let mut header = header_line(label, &title, muted, failed, accent, theme, width);
    prepend_diamond(&mut header, accent, failed, theme);
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
    if let Some(obj) = parsed.objective.as_deref().filter(|s| !s.is_empty()) {
        out.push(Line::from(vec![
            Span::styled("  目标  ", theme.muted()),
            Span::styled(obj.to_string(), Style::default().fg(theme.text_primary)),
        ]));
    }
    if let Some(msg) = parsed.message.as_deref().filter(|s| !s.is_empty()) {
        out.push(Line::from(vec![
            Span::styled("  进度  ", theme.muted()),
            Span::styled(msg.to_string(), Style::default().fg(theme.text_secondary)),
        ]));
    }
    if let Some(reason) = parsed.blocked.as_deref().filter(|s| !s.is_empty()) {
        out.push(Line::from(vec![
            Span::styled("  受阻  ", theme.muted()),
            Span::styled(reason.to_string(), Style::default().fg(theme.warning)),
        ]));
    }
    if let Some(sum) = summary_from_content(content).filter(|s| !s.is_empty()) {
        if parsed.message.as_deref() != Some(sum.as_str())
            && parsed.objective.as_deref() != Some(sum.as_str())
        {
            out.push(Line::from(Span::styled(format!("  {sum}"), theme.muted())));
        }
    }
    if out.len() == 2 {
        out.push(Line::from(Span::styled(
            "  目标已更新".to_string(),
            theme.muted(),
        )));
    }
    out
}

struct Args {
    objective: Option<String>,
    message: Option<String>,
    completed: bool,
    blocked: Option<String>,
}

fn parse_args(arguments: &str) -> Args {
    let v = serde_json::from_str::<serde_json::Value>(arguments).unwrap_or(serde_json::Value::Null);
    Args {
        objective: v
            .get("objective")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        message: v
            .get("message")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        completed: v.get("completed").and_then(|x| x.as_bool()) == Some(true),
        blocked: v
            .get("blocked_reason")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    }
}

fn classify(args: &Args, content: &str) -> Kind {
    let failed = content.starts_with("Error")
        || content.contains("goal_update_")
        || content.contains("未挂载")
        || content.contains("HarnessDisabled")
        || content.contains("no /goal");
    if failed {
        return Kind::Rejected;
    }
    if args.completed {
        return Kind::Done;
    }
    if args.blocked.is_some() {
        return Kind::Blocked;
    }
    if args.objective.is_some() {
        return Kind::Set;
    }
    Kind::Progress
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Set => "Goal: 设定",
        Kind::Progress => "Goal: 进展",
        Kind::Done => "Goal: 完成",
        Kind::Blocked => "Goal: 受阻",
        Kind::Rejected => "Goal: 失败",
    }
}

fn kind_accent(kind: Kind, theme: &Theme) -> Color {
    match kind {
        Kind::Set => theme.accent_plan,
        Kind::Progress => theme.running,
        Kind::Done => theme.accent_success,
        Kind::Blocked => theme.warning,
        Kind::Rejected => theme.accent_error,
    }
}

fn summary_from_content(content: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(s) = v.get("summary").and_then(|x| x.as_str()) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    let t = content.trim();
    if t.is_empty() {
        None
    } else {
        t.lines().next().map(str::to_string)
    }
}

fn header_line(
    label: &str,
    title: &str,
    muted: bool,
    failed: bool,
    accent: Color,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let text = if muted {
        theme.muted()
    } else if failed {
        Style::default().fg(theme.accent_error)
    } else {
        Style::default().fg(accent)
    };
    let bold = text.add_modifier(Modifier::BOLD);
    let title_style = if muted {
        theme.muted()
    } else {
        Style::default().fg(theme.text_primary)
    };
    let mut spans = vec![Span::styled(format!("{label} "), bold)];
    if !title.is_empty() {
        spans.push(Span::styled(title.to_string(), title_style));
    }
    let line = Line::from(spans);
    if width == 0 {
        line
    } else {
        truncate_line(line, width.saturating_sub(2).max(8))
    }
}

fn prepend_diamond(line: &mut Line<'static>, accent: Color, failed: bool, theme: &Theme) {
    let fg = if failed { theme.accent_error } else { accent };
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

    fn plain(lines: &[Line<'_>]) -> String {
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
    fn set_card_shows_objective() {
        let theme = Theme::current();
        let collapsed = lines(
            "update_goal",
            r#"{"objective":"理解并分析 TUI"}"#,
            r#"{"success":true,"summary":"Goal set: 理解并分析 TUI."}"#,
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&collapsed);
        assert!(text.contains("Goal: 设定"), "{text}");
        assert!(text.contains("理解并分析 TUI"), "{text}");
        let open = lines(
            "update_goal",
            r#"{"objective":"理解并分析 TUI"}"#,
            r#"{"success":true,"summary":"Goal set: 理解并分析 TUI."}"#,
            &theme,
            80,
            ToolMode::Expanded,
            false,
        );
        let body = plain(&open);
        assert!(body.contains("目标"), "{body}");
    }

    #[test]
    fn done_and_blocked_labels() {
        let theme = Theme::current();
        let done = plain(&lines(
            "update_goal",
            r#"{"completed":true,"message":"shipped"}"#,
            r#"{"success":true,"summary":"Goal marked complete."}"#,
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        ));
        assert!(done.contains("Goal: 完成"), "{done}");
        let blocked = plain(&lines(
            "update_goal",
            r#"{"blocked_reason":"no sdk"}"#,
            r#"{"success":true,"summary":"Goal blocked: no sdk."}"#,
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        ));
        assert!(blocked.contains("Goal: 受阻"), "{blocked}");
    }
}
