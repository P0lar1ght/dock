//! Status line values. The widget is the copied Grok `StatusBar`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use cordis::Context;
use cordis_spine::{
    snapshot_context, ContextBook, ContextSnapshot, LlmOutput, LogEvent, Sessions, TokenUsage,
    CONTEXT, SESSIONS,
};
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use crate::names::SESSION_PORT;
use crate::session::SessionRef;
use crate::status_bar;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::text::{Line, Span};

const FLASH_TTL: Duration = Duration::from_millis(2500);

struct SnapMemo {
    rev: u64,
    usage: TokenUsage,
    snap: ContextSnapshot,
}

pub struct StatusLine {
    ctx: Context,
    notice: Mutex<Option<(String, Instant)>>,
    right_hit: Mutex<Option<Rect>>,
    snap_memo: Mutex<Option<SnapMemo>>,
}

impl StatusLine {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            notice: Mutex::new(None),
            right_hit: Mutex::new(None),
            snap_memo: Mutex::new(None),
        }
    }

    pub fn flash(&self, msg: impl Into<String>) {
        *self.notice.lock().unwrap() = Some((msg.into(), Instant::now()));
    }

    pub fn clear_notice(&self) {
        *self.notice.lock().unwrap() = None;
    }

    pub fn left(&self) -> String {
        cwd_label()
    }

    pub fn center(&self) -> Option<String> {
        let sessions = self.ctx.require::<Sessions>(SESSIONS).ok()?;
        let turns = sessions
            .events()
            .iter()
            .filter(|e| matches!(e, LogEvent::User(_)))
            .count();
        (turns > 0).then(|| format!("第 {turns} 轮"))
    }

    pub fn right(&self) -> Option<String> {
        {
            let mut notice = self.notice.lock().unwrap();
            if let Some((msg, at)) = notice.as_ref() {
                if at.elapsed() < FLASH_TTL {
                    return Some(msg.clone());
                }
            }
            *notice = None;
        }
        Some(occupancy_label(&self.live_snapshot()))
    }

    pub fn right_is_flash(&self) -> bool {
        self.notice
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() < FLASH_TTL)
    }

    pub fn occupancy_style(&self) -> Style {
        let theme = Theme::current();
        occupancy_style(self.live_snapshot().usage_pct, &theme)
    }

    fn live_snapshot(&self) -> ContextSnapshot {
        let (rev, usage) = self
            .ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| s.occupancy_stamp())
            .unwrap_or((0, TokenUsage::default()));
        let mut memo = self.snap_memo.lock().unwrap();
        if let Some(cached) = memo.as_ref() {
            if cached.rev == rev && cached.usage == usage {
                return cached.snap.clone();
            }
        }
        let snap = self
            .ctx
            .get::<ContextBook>(CONTEXT)
            .map(|b| b.window())
            .unwrap_or_else(|| snapshot_context(&self.ctx));
        *memo = Some(SnapMemo {
            rev,
            usage,
            snap: snap.clone(),
        });
        snap
    }

    pub fn remember_right_hit(&self, area: Rect, text: &str) {
        *self.right_hit.lock().unwrap() = Some(status_bar::right_rect(area, text));
    }

    pub fn hit_right(&self, column: u16, row: u16) -> bool {
        self.right_hit
            .lock()
            .unwrap()
            .is_some_and(|r| r.contains(Position { x: column, y: row }))
    }
}

pub fn occupancy_label(snap: &ContextSnapshot) -> String {
    format!(
        "上下文 {}/{}",
        format_tokens(snap.used),
        format_tokens(snap.total)
    )
}

fn occupancy_style(pct: u8, theme: &Theme) -> Style {
    let fg = if pct >= 85 {
        theme.warning
    } else if pct >= 70 {
        theme.accent_user
    } else {
        theme.gray
    };
    Style::default().fg(fg).bg(theme.bg_base)
}

fn cwd_label() -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if let Some(home) = home {
        if let Ok(rel) = cwd.strip_prefix(&home) {
            let rel = rel.to_string_lossy();
            return if rel.is_empty() {
                "~".into()
            } else {
                format!("~/{rel}")
            };
        }
    }
    cwd.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".into())
}

pub fn format_tokens(n: u64) -> String {
    if n < 1000 {
        format!("{n}")
    } else if n < 10_000 {
        format!("{:.2}k", n as f64 / 1000.0)
    } else if n < 100_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else if n < 1_000_000 {
        format!("{}k", n / 1000)
    } else if n < 10_000_000 {
        format!("{:.2}m", n as f64 / 1_000_000.0)
    } else {
        format!("{:.1}m", n as f64 / 1_000_000.0)
    }
}

pub fn format_duration_short(d: std::time::Duration) -> String {
    let secs = d.as_secs_f32();
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else if secs < 60.0 {
        format!("{}s", secs.round() as u32)
    } else {
        let m = (secs / 60.0).floor() as u32;
        let s = (secs % 60.0).round() as u32;
        format!("{m}m{s:02}s")
    }
}

/// Show each braille spinner frame for this many shimmer ticks (~7.5fps at 30Hz).
const SPINNER_DIVISOR: u64 = 4;

/// Only while a turn is running or a permission prompt is open.
/// Idle "就绪" is not a Grok chrome row — it sat on top of the user block.
pub fn turn_status_height(ctx: &Context, waiting_permission: bool) -> u16 {
    if waiting_permission {
        return 1;
    }
    u16::from(
        ctx.get::<SessionRef>(SESSION_PORT)
            .is_some_and(|h| h.working()),
    )
}

/// Grok turn-status row: `⠧ Waiting…` … `↓15.7k [stop]`.
/// Elapsed generation time lives on the prompt chrome bottom border.
pub fn render_turn_status(buf: &mut Buffer, area: Rect, ctx: &Context, waiting_permission: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let theme = Theme::current();
    let base = Style::default().fg(theme.gray).bg(theme.bg_base);
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.reset();
            cell.set_style(base);
        }
    }
    let working = ctx
        .get::<SessionRef>(SESSION_PORT)
        .is_some_and(|h| h.working());
    if !waiting_permission && !working {
        return;
    }
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return;
    };
    let (label, label_style) =
        sessions.with_log(|events, _| turn_activity_label(events, waiting_permission, &theme));
    let queued = ctx
        .get::<SessionRef>(SESSION_PORT)
        .map(|s| s.queued_prompts().len())
        .unwrap_or(0);
    let can_send = ctx
        .get::<crate::prompt::PromptWidget>(crate::names::TUI_PROMPT)
        .is_some_and(|p| p.can_send());
    let queue_hint = if queued == 0 {
        String::new()
    } else if !can_send {
        format!(" · {queued} queued, Enter to send now")
    } else {
        format!(" · {queued} queued")
    };

    let frames = crate::grok::glyphs::braille_spinner_frames();
    let tick = crate::grok::color::shimmer_tick() / SPINNER_DIVISOR;
    let spinner = frames[(tick as usize) % frames.len()];
    let spinner_style = if waiting_permission {
        Style::default().fg(theme.warning).bg(theme.bg_base)
    } else {
        label_style
    };

    let usage = sessions.usage();
    let down = crate::grok::glyphs::token_down();
    let tokens = format!("{down}{}", format_tokens(usage.completion));
    let stop = "[stop]";
    let right_w = tokens.width() + 1 + stop.width();
    let budget = area.width as usize;
    let gap = 2usize;
    let right_budget = if right_w + gap < budget {
        budget - right_w - gap
    } else {
        budget
    };

    let spin = format!("{spinner} ");
    let mut label = label;
    let mut hint = queue_hint;
    let mut left_w = spin.width() + label.width() + hint.width();
    if left_w > right_budget {
        hint.clear();
        left_w = spin.width() + label.width();
    }
    if left_w > right_budget {
        label = short_activity_label(waiting_permission);
    }
    let mut left_spans = vec![
        Span::styled(spin, spinner_style),
        Span::styled(label, label_style),
    ];
    if !hint.is_empty() {
        left_spans.push(Span::styled(hint, base));
    }

    buf.set_line(area.x, area.y, &Line::from(left_spans), area.width);

    if right_w < budget {
        let x = area.x + area.width.saturating_sub(right_w as u16);
        buf.set_line(
            x,
            area.y,
            &Line::from(vec![
                Span::styled(tokens, base),
                Span::styled(" ", base),
                Span::styled(
                    stop,
                    Style::default().fg(theme.accent_error).bg(theme.bg_base),
                ),
            ]),
            right_w as u16,
        );
    }
}

fn short_activity_label(waiting_permission: bool) -> String {
    if waiting_permission {
        "等待授权".into()
    } else {
        "生成中…".into()
    }
}

fn turn_activity_label(
    events: &[LogEvent],
    waiting_permission: bool,
    theme: &Theme,
) -> (String, Style) {
    let secondary = Style::default().fg(theme.text_secondary).bg(theme.bg_base);
    let tool = Style::default().fg(theme.accent_success).bg(theme.bg_base);
    let warn = Style::default().fg(theme.warning).bg(theme.bg_base);

    if waiting_permission {
        return ("等待授权".into(), warn);
    }

    match events.last() {
        Some(LogEvent::LlmStream(out)) => {
            if let Some(name) = pending_tool_name(events, out) {
                return (format!("运行 {name}…"), tool);
            }
            if !out.text.trim().is_empty() {
                return ("生成中…".into(), secondary);
            }
            if !out.reasoning.trim().is_empty() {
                return ("思考中…".into(), secondary);
            }
            ("等待响应…".into(), secondary)
        }
        Some(LogEvent::ToolExecute { name, .. }) => {
            // Between tool result and the next sample.
            let _ = name;
            ("等待响应…".into(), secondary)
        }
        _ => ("等待响应…".into(), secondary),
    }
}

fn pending_tool_name(events: &[LogEvent], out: &LlmOutput) -> Option<String> {
    for call in out.tool_calls.iter().rev() {
        let done = events
            .iter()
            .any(|e| matches!(e, LogEvent::ToolExecute { id, .. } if id == &call.id));
        if !done {
            return Some(call.name.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_compact() {
        assert_eq!(format_tokens(42), "42");
        assert_eq!(format_tokens(1230), "1.23k");
        assert_eq!(format_tokens(12_300), "12.3k");
    }

    #[test]
    fn cwd_uses_tilde_when_under_home() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let Ok(cwd) = std::env::current_dir() else {
            return;
        };
        if cwd.starts_with(&home) {
            assert!(cwd_label().starts_with('~'), "cwd_label={}", cwd_label());
        }
    }

    #[test]
    fn left_is_cwd_only() {
        let line = StatusLine::new(Context::new());
        assert!(!line.left().contains("上下文"), "left={}", line.left());
    }

    #[test]
    fn right_shows_occupancy_without_flash() {
        let line = StatusLine::new(Context::new());
        let right = line.right().expect("occupancy");
        assert!(right.starts_with("上下文 "), "right={right}");
        assert!(right.contains('/'), "right={right}");
    }

    #[test]
    fn occupancy_label_formats_window() {
        let snap = ContextSnapshot {
            used: 20_000,
            total: 204_800,
            model: String::new(),
            system_prompt_tokens: 0,
            message_tokens: 0,
            tool_definitions_tokens: 0,
            tool_definitions_count: 0,
            free_tokens: 184_800,
            usage_pct: 10,
            auto_compact_threshold_percent: 85,
            turn_count: 0,
            tool_call_count: 0,
            compaction_count: 0,
            categories: Vec::new(),
        };
        assert_eq!(occupancy_label(&snap), "上下文 20.0k/204k");
    }

    #[test]
    fn activity_waiting_before_stream() {
        let theme = Theme::current();
        let (label, _) = turn_activity_label(&[LogEvent::User("hi".into())], false, &theme);
        assert_eq!(label, "等待响应…");
    }

    #[test]
    fn activity_thinking_and_responding() {
        let theme = Theme::current();
        let (think, _) = turn_activity_label(
            &[LogEvent::LlmStream(LlmOutput {
                reasoning: "hmm".into(),
                ..LlmOutput::default()
            })],
            false,
            &theme,
        );
        assert_eq!(think, "思考中…");
        let (gen, _) = turn_activity_label(
            &[LogEvent::LlmStream(LlmOutput {
                text: "hello".into(),
                ..LlmOutput::default()
            })],
            false,
            &theme,
        );
        assert_eq!(gen, "生成中…");
    }

    #[test]
    fn activity_pending_tool() {
        let theme = Theme::current();
        let (label, _) = turn_activity_label(
            &[LogEvent::LlmStream(LlmOutput {
                tool_calls: vec![cordis_spine::ToolCall {
                    id: "1".into(),
                    name: "bash".into(),
                    arguments: "{}".into(),
                }],
                ..LlmOutput::default()
            })],
            false,
            &theme,
        );
        assert_eq!(label, "运行 bash…");
    }
}
