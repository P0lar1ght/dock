//! Copied from grok-build/.../xai-grok-pager-render/src/render/color.rs
//! (`blend_channel`, `blend_color`, `blend_area`, `dim_area`).
//! `pulse_brightness` is Grok `tokyonight.rs`. Indexed→256 omitted; GrokNight is RGB.

use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

/// Dock shimmer is 80ms (~12.5 fps). Grok `USER_WAITING_PULSE_SPEED` is 0.08 at
/// ~30 fps; scale so a `sin²` cycle stays about 1.3s.
const SHIMMER_PULSE_SPEED: f32 = 0.192;
/// Grok appearance `dim_accent` default for collapsed running bullets.
const DIM_ACCENT: f32 = 0.5;

fn color_to_rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

/// - `opacity = 0.0`: fully `base`
/// - `opacity = 1.0`: fully `original`
pub fn blend_channel(base: u8, original: u8, opacity: f32) -> u8 {
    let result = base as f32 * (1.0 - opacity) + original as f32 * opacity;
    result.round() as u8
}

pub fn blend_color(base: Color, original: Color, opacity: f32) -> Option<Color> {
    let (base_r, base_g, base_b) = color_to_rgb(base)?;
    let (orig_r, orig_g, orig_b) = color_to_rgb(original)?;
    Some(Color::Rgb(
        blend_channel(base_r, orig_r, opacity),
        blend_channel(base_g, orig_g, opacity),
        blend_channel(base_b, orig_b, opacity),
    ))
}

/// Grok `pulse_brightness`: `sin²(tick * speed)` in `[0, 1]`.
pub fn pulse_brightness(tick: u64, speed: f32) -> f32 {
    let sin_val = (tick as f32 * speed).sin();
    sin_val * sin_val
}

pub fn shimmer_tick() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64 / 80)
        .unwrap_or(0)
}

/// Running ◆: pre-dim like Grok collapsed bullets, then pulse toward `bg`.
pub fn pulse_fg(bg: Color, accent: Color) -> Color {
    let dimmed = blend_color(bg, accent, DIM_ACCENT).unwrap_or(accent);
    let brightness = pulse_brightness(shimmer_tick(), SHIMMER_PULSE_SPEED);
    blend_color(bg, dimmed, 0.3 + brightness * 0.7).unwrap_or(dimmed)
}

/// Each parameter is `Option<(target, opacity)>`. Named ANSI colors are left
/// unchanged (same as Grok).
pub fn blend_area(
    buf: &mut Buffer,
    area: Rect,
    fg: Option<(Color, f32)>,
    bg: Option<(Color, f32)>,
) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            let Some(cell) = buf.cell_mut((x, y)) else {
                continue;
            };
            if let Some((target, opacity)) = fg {
                if let Some(blended) = blend_color(target, cell.fg, opacity) {
                    cell.set_fg(blended);
                }
            }
            if let Some((target, opacity)) = bg {
                if let Some(blended) = blend_color(target, cell.bg, opacity) {
                    cell.set_bg(blended);
                }
            }
        }
    }
}

/// Dim a screen area: reset modifiers then blend fg toward `blend_bg`.
pub fn dim_area(buf: &mut Buffer, area: Rect, blend_bg: Color, blend_factor: f32) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.modifier = Modifier::empty();
            }
        }
    }
    blend_area(buf, area, Some((blend_bg, blend_factor)), None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_channel_extremes() {
        assert_eq!(blend_channel(0, 255, 0.0), 0);
        assert_eq!(blend_channel(0, 255, 1.0), 255);
        assert_eq!(blend_channel(0, 100, 0.5), 50);
    }

    #[test]
    fn pulse_brightness_range() {
        assert!((pulse_brightness(0, 0.08) - 0.0).abs() < f32::EPSILON);
        for tick in 0..200 {
            let b = pulse_brightness(tick, 0.08);
            assert!((0.0..=1.0).contains(&b), "tick={tick} b={b}");
        }
    }

    #[test]
    fn dim_area_strips_bold() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf.cell_mut((0, 0)).unwrap().set_style(
            ratatui::style::Style::default()
                .fg(Color::Rgb(200, 200, 200))
                .add_modifier(Modifier::BOLD),
        );
        dim_area(&mut buf, Rect::new(0, 0, 2, 1), Color::Rgb(20, 20, 20), 0.5);
        let cell = buf.cell((0, 0)).unwrap();
        assert!(!cell.modifier.contains(Modifier::BOLD));
        assert_eq!(cell.fg, Color::Rgb(110, 110, 110));
    }
}
