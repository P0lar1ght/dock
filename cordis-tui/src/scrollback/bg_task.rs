//! Background bash / monitor card — Grok `BgTaskBlock` (always collapsed).
//! Click opens the job output overlay.

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::grok::line_utils::truncate_line;
use crate::scrollback::live;
use crate::theme::Theme;
use cordis_spine::JobSnapshot;

pub const HEADER_PREFIX: &str = "job:";

pub fn is_bg_tool(name: &str) -> bool {
    matches!(
        name,
        "bash" | "run_terminal_cmd" | "execute" | "shell" | "monitor"
    )
}

pub fn parse_id(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix("task_id:") {
            let id = rest.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

pub fn is_bg_notice(content: &str) -> bool {
    content.contains("moved to background")
        || content.contains("Monitor started in background")
        || content.contains("still running in the background")
}

pub fn header_id(
    name: &str,
    content: &str,
    arguments: &str,
    jobs: &[JobSnapshot],
) -> Option<String> {
    if !is_bg_tool(name) {
        return None;
    }
    let id = parse_id(content).or_else(|| match_job(arguments, jobs))?;
    if is_bg_notice(content) || name == "monitor" || jobs.iter().any(|j| j.id == id) {
        Some(format!("{HEADER_PREFIX}{id}"))
    } else {
        None
    }
}

pub fn open_id(header_id: &str) -> Option<&str> {
    header_id.strip_prefix(HEADER_PREFIX)
}

/// Whether this card is still running. `build_frame` needs it to register the
/// row as live without re-deriving the header.
pub(crate) fn is_running(content: &str, snap: Option<&JobSnapshot>) -> bool {
    snap.is_some_and(|s| !s.done) || (snap.is_none() && is_bg_notice(content))
}

fn match_job(arguments: &str, jobs: &[JobSnapshot]) -> Option<String> {
    let cmd = json_field(arguments, "command")?;
    let hits: Vec<_> = jobs.iter().filter(|j| j.command == cmd).collect();
    if hits.len() == 1 {
        return Some(hits[0].id.clone());
    }
    hits.iter()
        .find(|j| !j.done)
        .or_else(|| hits.first())
        .map(|j| j.id.clone())
}

pub fn lines(
    name: &str,
    arguments: &str,
    content: &str,
    snap: Option<&JobSnapshot>,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let monitor = name == "monitor" || snap.is_some_and(|s| s.is_monitor);
    let desc = snap
        .and_then(|s| s.description.as_deref())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| json_field(arguments, "description"))
        .or_else(|| field_from_notice(content, "description:"))
        .or_else(|| json_field(arguments, "command"))
        .or_else(|| snap.map(|s| s.command.clone()))
        .unwrap_or_else(|| "任务".into());
    let failed =
        snap.is_some_and(|s| s.done && looks_failed(&s.output)) || content.starts_with("Error");
    let running = is_running(content, snap);
    let (verb, color) = if failed {
        ("任务失败", theme.accent_error)
    } else if running {
        (
            if monitor {
                "监视运行中"
            } else {
                "任务运行中"
            },
            theme.accent_running,
        )
    } else {
        (
            if monitor {
                "监视完成"
            } else {
                "任务完成"
            },
            theme.accent_thinking,
        )
    };
    let label_style = if running {
        theme.primary().add_modifier(Modifier::BOLD)
    } else {
        theme.muted().add_modifier(Modifier::BOLD)
    };
    let mut spans = vec![Span::styled(verb.to_string(), label_style)];
    spans.push(Span::styled(
        format!(" \u{201C}{desc}\u{201D}"),
        if running {
            theme.primary()
        } else {
            theme.muted()
        },
    ));
    if running {
        if let Some(act) = snap.and_then(|s| last_output_line(&s.output)) {
            spans.push(Span::styled(format!(" \u{00b7} {act}"), theme.muted()));
        }
        // Elapsed is painted, not built — see `subagent::lines`.
    }
    spans.push(Span::styled("  （点击查看）".to_string(), theme.dim()));
    let mut line = Line::from(spans);
    line.spans.insert(0, live::diamond(color));
    if width > 0 {
        line = truncate_line(line, width);
    }
    vec![line]
}

fn looks_failed(output: &str) -> bool {
    let t = output.trim();
    t.starts_with("exit ") || t.starts_with("Error") || t.contains("\nexit ")
}

fn last_output_line(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(|s| {
            let mut t = s.to_string();
            if t.chars().count() > 40 {
                t = t.chars().take(39).collect::<String>();
                t.push('\u{2026}');
            }
            t
        })
}

fn json_field(arguments: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    v.get(key)?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn field_from_notice(content: &str, prefix: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix(prefix) {
            let t = rest.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bash_background_notice() {
        let text =
            "[Command moved to background]\n\ntask_id: job-3\n\nThe command is still running";
        assert_eq!(parse_id(text).as_deref(), Some("job-3"));
        assert!(is_bg_notice(text));
        let hid = header_id("bash", text, r#"{"command":"sleep 9"}"#, &[]).unwrap();
        assert_eq!(open_id(&hid), Some("job-3"));
    }

    #[test]
    fn foreground_bash_is_not_a_job_card() {
        assert!(header_id("bash", "hello\n", r#"{"command":"echo hello"}"#, &[]).is_none());
    }

    #[test]
    fn running_card_shows_last_output_and_click_hint() {
        let snap = JobSnapshot {
            id: "job-3".into(),
            command: "seq 3".into(),
            done: false,
            output: "1\n2\n3\n".into(),
            description: Some("count".into()),
            is_monitor: false,
            start_time: std::time::SystemTime::now(),
        };
        let theme = Theme::current();
        let notice = "[Command moved to background]\n\ntask_id: job-3\n";
        let text: String = lines(
            "bash",
            r#"{"command":"seq 3"}"#,
            notice,
            Some(&snap),
            &theme,
            80,
        )
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect();
        assert!(text.contains("任务运行中"), "{text}");
        assert!(text.contains("count"), "{text}");
        assert!(text.contains("3"), "{text}");
        assert!(text.contains("点击查看"), "{text}");
    }
}
