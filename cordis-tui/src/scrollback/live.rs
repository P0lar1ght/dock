//! Live chrome for running cards: a travelling shine across the header row,
//! plus a right-aligned elapsed clock.
//!
//! **Nothing here animates by itself.** The main transcript is layout-cached
//! ([`crate::scrollback::Scrollback::ensure_frame`]), so a frame built now can
//! be painted for minutes — anything time-varying baked into a `Line` freezes
//! there. Rebuilding on the shimmer tick is not an option either: a 450-event
//! session takes ~276ms to rebuild, which at 12.5Hz is several cores. So the
//! motion is painted, never built: [`shine_at`] recolours cells that are
//! already on screen, and the built `Line` carries no moving parts at all.
//!
//! ## Why a shine and not a spinner
//!
//! Dock already has a motion vocabulary — the welcome logo sweeps a diagonal
//! highlight (`welcome::shine_opacity`) and running rows breathe through
//! `grok::color::pulse_fg`. A running card is the
//! same idea reduced to one row: a head travels left to right with a short
//! trailing glow, tinting the text toward the card's own accent as it passes.
//! That keeps the card's mark instead of parking a foreign spinner glyph in
//! front of it, changes no widths, and reads as *alive* rather than *blinking*.
//!
//! The sweep tints toward the accent, not toward `text_primary`: a running
//! card's header already *is* `text_primary`, so brightening it is a no-op.
//! Tinting toward the accent also says which state the card is in — cyan for a
//! tool, the thinking violet for reasoning — instead of just "something moved".
//!
//! The previous `sin²` colour pulse is gone: at 12.5fps it read as flicker, and
//! blending one accent toward the background fights every palette it runs on.

use std::time::{Instant, SystemTime};

use ratatui::style::{Color, Style};
use ratatui::text::Span;

use crate::grok::glyphs;
use crate::status::format_duration_short;
use crate::theme::Theme;

/// Columns of trailing glow behind the shine head.
const TAIL: u64 = 10;
/// Columns the head advances per 80ms shimmer tick (~37 columns/second).
const COLS_PER_TICK: u64 = 3;

/// How hot column `col` of a `width`-wide running row is at `tick`, in `[0, 1]`.
///
/// `0.0` everywhere ahead of the head and past the tail, so a caller can skip
/// most cells. The head runs off the right edge and re-enters from the left.
pub fn shine_at(col: usize, width: usize, tick: u64) -> f32 {
    if width == 0 {
        return 0.0;
    }
    // Modulo in integer space: `tick` is milliseconds-since-epoch / 80, far past
    // where f32 can still count by ones, so `tick as f32` would stutter.
    let span = width as u64 + TAIL * 2;
    let head = (tick.saturating_mul(COLS_PER_TICK) % span) as i64 - TAIL as i64;
    let behind = head - col as i64;
    if behind < 0 || behind > TAIL as i64 {
        return 0.0;
    }
    // Linear falloff. A squared one looked right on paper but the far tail
    // rounded to the same RGB as the unlit cell, so the sweep read as a short
    // bright dash instead of a trail.
    1.0 - behind as f32 / TAIL as f32
}

/// Card bullet. State rides on `accent`; motion is painted, not baked.
pub fn diamond(accent: Color) -> Span<'static> {
    Span::styled(
        format!("{} ", glyphs::diamond_filled()),
        Style::default().fg(accent),
    )
}

pub fn running_hint(theme: &Theme) -> Span<'static> {
    Span::styled("  运行中".to_string(), theme.muted())
}

pub fn running_body(theme: &Theme) -> Span<'static> {
    Span::styled("运行中".to_string(), theme.muted())
}

/// Recolour an already-prepended bullet to the running accent and append the
/// steady label. Both are static — the shine does the moving.
pub fn mark_running(header: &mut ratatui::text::Line<'static>, theme: &Theme) {
    if let Some(span) = header.spans.first_mut() {
        span.style.fg = Some(theme.accent_running);
    }
    header.spans.push(running_hint(theme));
}

/// Wall-clock start for a monotonic one, so every live row hands the painter the
/// same type. `Instant` does not survive the build/paint split as a value.
pub fn wall_start(started: Instant) -> Option<SystemTime> {
    SystemTime::now().checked_sub(started.elapsed())
}

/// `12.3s` — the painter's right-aligned clock. No leading separator; the
/// painter owns the gap.
pub fn elapsed_label(started: SystemTime) -> Option<String> {
    started.elapsed().ok().map(format_duration_short)
}

/// ` · 12.3s` — inline form, for the overlays that rebuild on every paint.
pub fn elapsed_instant(started: Instant) -> String {
    format!(" · {}", format_duration_short(started.elapsed()))
}

/// ` · 12.3s` — inline form for a wall-clock start.
pub fn elapsed_system(started: SystemTime) -> String {
    started
        .elapsed()
        .ok()
        .map(|d| format!(" · {}", format_duration_short(d)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(width: usize, tick: u64) -> Vec<f32> {
        (0..width).map(|c| shine_at(c, width, tick)).collect()
    }

    #[test]
    fn the_shine_is_a_head_with_a_trailing_glow() {
        // Head at column 20: hottest there, fading backwards, dark ahead.
        let tick = (20 + TAIL) / COLS_PER_TICK;
        let heat = row(60, tick);
        let hottest = heat
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        assert!(heat[hottest] > 0.5, "{heat:?}");
        assert!(
            heat[hottest + 1..].iter().all(|h| *h == 0.0),
            "nothing ahead of the head: {heat:?}"
        );
        let tail_start = hottest.saturating_sub(TAIL as usize - 1);
        assert!(
            heat[tail_start..hottest].windows(2).all(|w| w[0] <= w[1]),
            "tail must rise monotonically toward the head: {heat:?}"
        );
        assert!(heat[..tail_start].iter().all(|h| *h == 0.0), "{heat:?}");
    }

    #[test]
    fn the_shine_moves_between_ticks_and_loops() {
        let a = row(60, 100);
        let b = row(60, 101);
        assert_ne!(a, b, "the sweep must advance every tick");
        // `(t + span) * COLS_PER_TICK ≡ t * COLS_PER_TICK (mod span)`, so one
        // span of ticks lands back on the same phase.
        let span = 60 + TAIL * 2;
        assert_eq!(row(60, 100), row(60, 100 + span), "the sweep must loop");
    }

    /// `shimmer_tick()` is ~2e10; f32 cannot count by ones anywhere near that,
    /// so the phase has to be computed in integer space or the sweep freezes.
    /// Sampled over a whole cycle: the head is legitimately off-screen for the
    /// short pause between passes, so two adjacent ticks can match.
    #[test]
    fn the_shine_still_moves_at_a_real_epoch_tick() {
        let now = crate::grok::color::shimmer_tick();
        assert!(now > 1_000_000_000, "sanity: epoch tick is huge, {now}");
        let cycle = (60 + TAIL * 2) / COLS_PER_TICK + 1;
        let seen: Vec<Vec<f32>> = (0..cycle).map(|i| row(60, now + i)).collect();
        let changed = seen.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(
            changed >= cycle as usize - 6,
            "frozen at a real clock: only {changed} of {} ticks moved",
            cycle - 1
        );
    }

    /// The pause between passes is short — a breath, not a stall.
    #[test]
    fn the_gap_between_passes_is_a_fifth_of_the_cycle_at_most() {
        let cycle = (33 + TAIL * 2) / COLS_PER_TICK + 1;
        let dark = (0..cycle)
            .filter(|i| row(33, *i).iter().all(|h| *h == 0.0))
            .count();
        assert!(
            dark * 4 <= cycle as usize,
            "{dark} dark ticks out of {cycle} is a stall, not a breath"
        );
    }

    #[test]
    fn a_zero_width_row_is_never_hot() {
        assert_eq!(shine_at(0, 0, 12345), 0.0);
    }

    /// The whole point of the rewrite: a card must not carry a width that
    /// changes on its own, or a cached frame goes stale mid-word.
    #[test]
    fn the_running_label_has_no_moving_parts() {
        let theme = Theme::current();
        assert_eq!(running_hint(&theme).content, "  运行中");
        assert_eq!(running_body(&theme).content, "运行中");
        assert_eq!(
            diamond(Color::Red).content,
            format!("{} ", glyphs::diamond_filled())
        );
    }
}
