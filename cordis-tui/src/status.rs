//! Status line values. The widget is the copied Grok `StatusBar`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use cordis::Context;
use cordis_spine::{LogEvent, Sessions, SESSIONS};

use crate::names::SESSION_PORT;
use crate::session::SessionRef;
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

const FLASH_TTL: Duration = Duration::from_millis(2500);

pub struct StatusLine {
    ctx: Context,
    notice: Mutex<Option<(String, Instant)>>,
}

impl StatusLine {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            notice: Mutex::new(None),
        }
    }

    pub fn flash(&self, msg: impl Into<String>) {
        *self.notice.lock().unwrap() = Some((msg.into(), Instant::now()));
    }

    pub fn clear_notice(&self) {
        *self.notice.lock().unwrap() = None;
    }

    pub fn left(&self) -> String {
        let cwd = cwd_label();
        let Some(sessions) = self.ctx.get::<Sessions>(SESSIONS) else {
            return cwd;
        };
        if !has_turns(&sessions) {
            return cwd;
        }
        let u = sessions.usage();
        let window = if u.window == 0 { 128_000 } else { u.window };
        format!(
            "{cwd}  上下文 {}/{}",
            format_tokens(u.context_used()),
            format_tokens(window)
        )
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
        let sessions = self.ctx.get::<Sessions>(SESSIONS)?;
        if !has_turns(&sessions) {
            return None;
        }
        let working = self
            .ctx
            .get::<SessionRef>(SESSION_PORT)
            .is_some_and(|h| h.working());
        Some(if working {
            "工作中".into()
        } else {
            "空闲".into()
        })
    }
}

fn has_turns(sessions: &Sessions) -> bool {
    sessions
        .events()
        .iter()
        .any(|e| matches!(e, LogEvent::User(_)))
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

pub fn render_turn_status(buf: &mut Buffer, area: Rect, ctx: &Context, waiting_permission: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let theme = Theme::current();
    let style = Style::default().fg(theme.gray).bg(theme.bg_base);
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.reset();
            cell.set_style(style);
        }
    }
    let working = ctx
        .get::<SessionRef>(SESSION_PORT)
        .is_some_and(|h| h.working());
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return;
    };
    let usage = sessions.usage();
    let mut left = if waiting_permission {
        "等待授权".to_string()
    } else if working {
        let timer = sessions
            .turn_elapsed()
            .map(format_duration_short)
            .unwrap_or_else(|| "0.0s".into());
        let queued = ctx
            .get::<SessionRef>(SESSION_PORT)
            .map(|s| s.queued_prompts().len())
            .unwrap_or(0);
        let can_send = ctx
            .get::<crate::prompt::PromptWidget>(crate::names::TUI_PROMPT)
            .is_some_and(|p| p.can_send());
        let mut line = format!("生成中  {timer}");
        if queued > 0 && !can_send {
            line.push_str(&format!("  ·  {queued} queued, Enter to send now"));
        } else if queued > 0 {
            line.push_str(&format!("  ·  {queued} queued"));
        }
        line
    } else {
        return;
    };
    let up = crate::grok::glyphs::token_up();
    let down = crate::grok::glyphs::token_down();
    let right = format!(
        "上传 {}{}  下载 {}{}",
        up,
        format_tokens(usage.prompt),
        down,
        format_tokens(usage.completion)
    );
    let left_w = left.width() as u16;
    let right_w = right.width() as u16;
    if left_w + right_w + 2 > area.width {
        left = if waiting_permission {
            "等待授权".into()
        } else {
            "生成中".into()
        };
    }
    buf.set_line(
        area.x,
        area.y,
        &Line::from(Span::styled(left, style)),
        area.width,
    );
    if right_w < area.width {
        let x = area.x + area.width.saturating_sub(right_w);
        buf.set_line(x, area.y, &Line::from(Span::styled(right, style)), right_w);
    }
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
}
