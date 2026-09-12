//! Subagent card in the parent scrollback — Grok `SubagentBlock` (always
//! collapsed). Click opens the child conversation overlay.

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::grok::line_utils::truncate_line;
use crate::scrollback::card;
use crate::scrollback::live;
use crate::theme::Theme;
use cordis_spine::{LogEvent, SubagentSnap};

pub const HEADER_PREFIX: &str = "sub:";

/// The one spawn tool: its card opens the child conversation overlay.
pub fn opens_child_overlay(name: &str) -> bool {
    name == "task"
}

pub fn parse_id(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix("subagent_id:") {
            let id = rest.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

pub fn resolve_id(content: &str, arguments: &str, agents: &[SubagentSnap]) -> Option<String> {
    if let Some(id) = parse_id(content) {
        return Some(id);
    }
    let desc = json_field(arguments, "description")?;
    let hits: Vec<_> = agents.iter().filter(|a| a.description == desc).collect();
    if hits.len() == 1 {
        return Some(hits[0].id.clone());
    }
    hits.iter()
        .find(|a| a.running())
        .or_else(|| hits.first())
        .map(|a| a.id.clone())
}

pub fn header_id(
    tool_call_id: &str,
    name: &str,
    content: &str,
    arguments: &str,
    agents: &[SubagentSnap],
) -> String {
    if opens_child_overlay(name) {
        if let Some(id) = resolve_id(content, arguments, agents) {
            return format!("{HEADER_PREFIX}{id}");
        }
    }
    tool_call_id.to_string()
}

pub fn open_id(header_id: &str) -> Option<&str> {
    header_id.strip_prefix(HEADER_PREFIX)
}

pub struct CardLive<'a> {
    pub snap: Option<&'a SubagentSnap>,
    pub events: &'a [LogEvent],
    /// Preset `agents/<type>.yml` display name (e.g. 岑).
    pub role: Option<String>,
}

pub fn lines(
    arguments: &str,
    content: &str,
    live: Option<CardLive<'_>>,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let desc = live
        .as_ref()
        .and_then(|l| l.snap)
        .map(|s| s.description.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| json_field(arguments, "description"))
        .or_else(|| quoted_from_notice(content))
        .unwrap_or_else(|| "子代理".into());
    let typ = live
        .as_ref()
        .and_then(|l| l.snap)
        .map(|s| s.subagent_type.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| json_field(arguments, "subagent_type"))
        .or_else(|| typed_from_notice(content))
        .unwrap_or_default();
    let role = live
        .as_ref()
        .and_then(|l| l.role.as_deref())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let (verb, running) = status_verb(content, live.as_ref().and_then(|l| l.snap));
    let activity = live
        .as_ref()
        .filter(|_| running)
        .and_then(|l| live_activity(l.events));
    let label_style = if running {
        theme.primary().add_modifier(Modifier::BOLD)
    } else {
        theme.muted().add_modifier(Modifier::BOLD)
    };
    let mut spans = vec![Span::styled(verb.to_string(), label_style)];
    if let Some(role) = role.filter(|r| r != &typ) {
        spans.push(Span::styled(
            format!(" {role}"),
            if running {
                theme.primary().add_modifier(Modifier::BOLD)
            } else {
                theme.muted().add_modifier(Modifier::BOLD)
            },
        ));
    }
    if !typ.is_empty() {
        spans.push(Span::styled(format!(" {typ}"), theme.muted()));
    }
    spans.push(Span::styled(
        format!(" \u{201C}{desc}\u{201D}"),
        if running {
            theme.primary()
        } else {
            theme.muted()
        },
    ));
    if let Some(act) = activity {
        spans.push(Span::styled(format!(" \u{00b7} {act}"), theme.muted()));
    }
    // No elapsed clock here: this line is layout-cached, so a baked duration
    // would freeze at whatever it read when the frame was built.
    // `Scrollback::paint_live_chrome` right-aligns a fresh one every paint.
    spans.push(Span::styled("  （点击查看）".to_string(), theme.dim()));
    let mut header = Line::from(spans);
    header.spans.insert(
        0,
        live::diamond(if running {
            theme.accent_running
        } else {
            theme.accent_thinking
        }),
    );
    if width > 0 {
        header = truncate_line(header, width);
    }
    let mut out = vec![header];
    if !running {
        if let Some(preview) = live
            .as_ref()
            .and_then(|l| last_assistant_text(l.events))
            .or_else(|| {
                live.as_ref()
                    .and_then(|l| l.snap)
                    .map(|s| s.output.as_str())
                    .filter(|s| !s.trim().is_empty())
            })
        {
            let one = preview
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or(preview)
                .trim();
            if !one.is_empty() {
                let mut preview_line =
                    card::indent(Line::from(Span::styled(one.to_string(), theme.dim())));
                if width > 0 {
                    preview_line = truncate_line(preview_line, width);
                }
                out.push(preview_line);
            }
        }
    }
    out
}

pub fn last_assistant_text(events: &[LogEvent]) -> Option<&str> {
    events.iter().rev().find_map(|e| match e {
        LogEvent::LlmStream(llm) if !llm.text.trim().is_empty() => Some(llm.text.as_str()),
        _ => None,
    })
}

/// Whether this card is still running. `build_frame` needs it to register the
/// row as live without re-deriving the header.
pub(crate) fn is_running(content: &str, snap: Option<&SubagentSnap>) -> bool {
    status_verb(content, snap).1
}

fn status_verb(content: &str, snap: Option<&SubagentSnap>) -> (&'static str, bool) {
    if let Some(s) = snap {
        if s.cancelled {
            return ("子代理已取消", false);
        }
        if s.running() {
            return ("子代理运行中", true);
        }
        if s.idle {
            return ("子代理空闲", false);
        }
        if s.done {
            return ("子代理完成", false);
        }
    }
    let c = content.trim();
    if c.contains("cancelled") || c.contains("canceled") {
        ("子代理已取消", false)
    } else if c.contains("Subagent started") || c.contains("still running") {
        ("子代理运行中", true)
    } else if c.contains("Subagent completed") || c.contains("subagent_result") {
        ("子代理完成", false)
    } else if c.is_empty() {
        ("子代理运行中", true)
    } else {
        ("子代理", false)
    }
}

pub(crate) fn live_activity(events: &[LogEvent]) -> Option<String> {
    activity_label(events)
}

fn activity_label(events: &[LogEvent]) -> Option<String> {
    for e in events.iter().rev() {
        match e {
            LogEvent::ToolExecute { name, .. } => return Some(name.clone()),
            LogEvent::LlmStream(llm) if !llm.tool_calls.is_empty() => {
                return llm.tool_calls.last().map(|c| c.name.clone());
            }
            LogEvent::LlmStream(llm) if !llm.reasoning.trim().is_empty() => {
                return Some("思考中".to_string());
            }
            LogEvent::LlmStream(llm) if !llm.text.trim().is_empty() => {
                return Some("回复中".to_string());
            }
            _ => {}
        }
    }
    Some("运行中".into())
}

fn json_field(arguments: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    v.get(key)?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn quoted_from_notice(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix("description:") {
            let t = rest.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn typed_from_notice(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix("type:") {
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
    fn parses_background_notice() {
        let text = "Subagent started in background.\n         subagent_id: abc-1\n         type: 观\n         description: 观 观察\n";
        assert_eq!(parse_id(text).as_deref(), Some("abc-1"));
        assert_eq!(quoted_from_notice(text).as_deref(), Some("观 观察"));
        assert_eq!(typed_from_notice(text).as_deref(), Some("观"));
    }

    #[test]
    fn header_id_uses_sub_prefix() {
        let id = header_id("call-9", "task", "subagent_id: kid\n", "{}", &[]);
        assert_eq!(id, "sub:kid");
        assert_eq!(open_id(&id), Some("kid"));
        assert!(open_id("call-9").is_none());
        assert!(opens_child_overlay("task"));
        assert!(!opens_child_overlay("bash"));
    }

    #[test]
    fn subagent_idle_preview_is_plain_one_line() {
        let theme = Theme::current();
        let snap = SubagentSnap {
            id: "k".into(),
            description: "观察".into(),
            subagent_type: "guan".into(),
            done: false,
            idle: true,
            cancelled: false,
            output: "## 结论\n\n第一行预览".into(),
            started_at: std::time::Instant::now(),
        };
        let live = CardLive {
            snap: Some(&snap),
            events: &[],
            role: Some("观".into()),
        };
        let lines = lines("{}", "Subagent completed.\n", Some(live), &theme, 80);
        assert!(lines.len() >= 2, "{lines:?}");
        let preview: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            preview.contains("结论") || preview.contains("第一行预览"),
            "{preview}"
        );
    }

    #[test]
    fn card_shows_roster_role_and_click_hint() {
        let theme = Theme::current();
        let live = CardLive {
            snap: None,
            events: &[],
            role: Some("观".into()),
        };
        let lines = lines(
            r#"{"prompt":"x","description":"观 观察","subagent_type":"观"}"#,
            "Subagent started in background.\n         subagent_id: abc\n         type: 观\n         description: 观 观察\n",
            Some(live),
            &theme,
            80,
        );
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(text.contains("观"), "{text}");
        assert!(text.contains("点击查看"), "{text}");
    }
}
