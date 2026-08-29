//! Live chrome for running cards: pulsing ◆, cycling dots, ticking elapsed.
//! Rebuilds on the 80ms shimmer tick (`event_loop::live_redraw`).

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::grok::color::pulse_fg;
use crate::grok::glyphs;
use crate::status::format_duration_short;
use crate::theme::Theme;

pub fn diamond(theme: &Theme, accent: Color, pulse: bool) -> Span<'static> {
    let fg = if pulse {
        pulse_fg(theme.bg_base, accent)
    } else {
        accent
    };
    Span::styled(
        format!("{} ", glyphs::diamond_filled()),
        Style::default().fg(fg),
    )
}

pub fn pulse_diamond(line: &mut Line<'static>, theme: &Theme) {
    pulse_diamond_color(line, theme, theme.accent_running);
}

pub fn pulse_diamond_color(line: &mut Line<'static>, theme: &Theme, accent: Color) {
    if let Some(span) = line.spans.first_mut() {
        span.style.fg = Some(pulse_fg(theme.bg_base, accent));
    }
}

pub fn running_dots() -> &'static str {
    const FRAMES: [&str; 3] = [".", "..", "..."];
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    FRAMES[((ms / 400) % 3) as usize]
}

pub fn running_hint(theme: &Theme) -> Span<'static> {
    Span::styled(format!("  运行中{}", running_dots()), theme.muted())
}

pub fn running_body(theme: &Theme) -> Span<'static> {
    Span::styled(format!("运行中{}", running_dots()), theme.muted())
}

pub fn mark_running(header: &mut Line<'static>, theme: &Theme) {
    pulse_diamond(header, theme);
    header.spans.push(running_hint(theme));
}

pub fn elapsed_instant(started: Instant) -> String {
    format!(" · {}", format_duration_short(started.elapsed()))
}

pub fn elapsed_system(started: SystemTime) -> String {
    started
        .elapsed()
        .ok()
        .map(|d| format!(" · {}", format_duration_short(d)))
        .unwrap_or_default()
}
