//! Loop / scheduler tool cards — set / list / close (Goal card chrome).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::scrollback::card;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

pub fn is_scheduler_tool(name: &str) -> bool {
    matches!(
        name,
        "scheduler_create" | "scheduler_list" | "scheduler_delete"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Set,
    List,
    Close,
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
    let parsed = parse_args(arguments);
    let kind = classify(name, content);
    let failed = kind == Kind::Rejected;
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;
    let accent = kind_accent(kind, theme);
    let label = kind_label(kind);
    let title = parsed
        .prompt
        .clone()
        .or_else(|| parsed.id.clone())
        .or_else(|| summary_from_content(content))
        .unwrap_or_default();
    let mut header = header_line(label, &title, muted, failed, accent, theme, width);
    prepend_diamond(&mut header, accent, failed, theme);
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
    if let Some(id) = parsed.id.as_deref().filter(|s| !s.is_empty()) {
        out.push(Line::from(vec![
            Span::styled("  id    ", theme.muted()),
            Span::styled(id.to_string(), Style::default().fg(theme.text_primary)),
        ]));
    }
    if let Some(interval) = parsed.interval.as_deref().filter(|s| !s.is_empty()) {
        out.push(Line::from(vec![
            Span::styled("  间隔  ", theme.muted()),
            Span::styled(
                interval.to_string(),
                Style::default().fg(theme.text_secondary),
            ),
        ]));
    }
    if let Some(prompt) = parsed.prompt.as_deref().filter(|s| !s.is_empty()) {
        out.push(Line::from(vec![
            Span::styled("  提问  ", theme.muted()),
            Span::styled(prompt.to_string(), Style::default().fg(theme.text_primary)),
        ]));
    }
    if let Some(sum) = summary_from_content(content).filter(|s| !s.is_empty()) {
        if parsed.prompt.as_deref() != Some(sum.as_str()) {
            out.push(card::indent(Line::from(Span::styled(sum, theme.muted()))));
        }
    }
    if out.len() == 2 {
        out.push(Line::from(Span::styled(
            "  定时任务已更新".to_string(),
            theme.muted(),
        )));
    }
    out
}

struct Args {
    interval: Option<String>,
    prompt: Option<String>,
    id: Option<String>,
}

fn parse_args(arguments: &str) -> Args {
    let v = serde_json::from_str::<serde_json::Value>(arguments).unwrap_or(serde_json::Value::Null);
    let id = v
        .get("task_id")
        .or_else(|| v.get("id"))
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Args {
        interval: v
            .get("interval")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        prompt: v
            .get("prompt")
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        id,
    }
}

fn classify(name: &str, content: &str) -> Kind {
    let failed = content.starts_with("Error")
        || content.contains("unknown task_id")
        || content.contains("cron is not mounted")
        || content.contains("invalid interval")
        || content.contains("maximum ")
        || content.contains("未挂载");
    if failed {
        return Kind::Rejected;
    }
    match name {
        "scheduler_list" => Kind::List,
        "scheduler_delete" => Kind::Close,
        _ => Kind::Set,
    }
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Set => "Loop: 设定",
        Kind::List => "Loop: 列表",
        Kind::Close => "Loop: 关闭",
        Kind::Rejected => "Loop: 失败",
    }
}

fn kind_accent(kind: Kind, theme: &Theme) -> Color {
    match kind {
        Kind::Set => theme.accent_plan,
        Kind::List => theme.running,
        Kind::Close => theme.accent_success,
        Kind::Rejected => theme.accent_error,
    }
}

fn summary_from_content(content: &str) -> Option<String> {
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
        truncate_line(line, card::header_width(width))
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
    fn create_card_hides_json() {
        let theme = Theme::current();
        let collapsed = lines(
            "scheduler_create",
            r#"{"interval":"5m","prompt":"检查部署","fire_immediately":true}"#,
            "已设定 cron-1（every 5 minutes）",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&collapsed);
        assert!(text.contains("Loop: 设定"), "{text}");
        assert!(text.contains("检查部署"), "{text}");
        assert!(!text.contains("fire_immediately"), "{text}");
        assert!(!text.contains('{'), "{text}");
        let open = lines(
            "scheduler_create",
            r#"{"interval":"5m","prompt":"检查部署"}"#,
            "已设定 cron-1（every 5 minutes）",
            &theme,
            80,
            ToolMode::Expanded,
            false,
        );
        let body = plain(&open);
        assert!(body.contains("间隔"), "{body}");
        assert!(body.contains("5m"), "{body}");
    }

    #[test]
    fn list_delete_and_error_labels() {
        let theme = Theme::current();
        let list = plain(&lines(
            "scheduler_list",
            "{}",
            "cron-1：every 5 minutes — check",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        ));
        assert!(list.contains("Loop: 列表"), "{list}");
        let close = plain(&lines(
            "scheduler_delete",
            r#"{"id":"cron-1"}"#,
            "已关闭 cron-1",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        ));
        assert!(close.contains("Loop: 关闭"), "{close}");
        assert!(close.contains("cron-1"), "{close}");
        let fail = plain(&lines(
            "scheduler_create",
            r#"{"interval":"5m","prompt":"x"}"#,
            "Error: maximum 50 scheduled tasks",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        ));
        assert!(fail.contains("Loop: 失败"), "{fail}");
    }
}
