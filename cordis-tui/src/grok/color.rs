//! Copied from grok-build/.../xai-grok-pager-render/src/render/color.rs
//! (`blend_channel`, `blend_color`, `blend_area`, `dim_area`).
//! Indexed→256 quantization omitted; GrokNight is RGB.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

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
