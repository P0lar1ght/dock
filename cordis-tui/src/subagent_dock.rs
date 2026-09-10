//! Sticky live-subagent portraits above the composer.
//!
//! Each unfinished child is one square: the first grapheme of that agent's
//! display name. Nothing is keyed off a fixed roster. Clicking opens the
//! Grok-style fullscreen child transcript. Running squares breathe and
//! carry a bead around the rim (80ms shimmer).

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use cordis::Context;
use cordis_spine::{AgentPresets, SubagentSnap, Subagents, AGENT_PRESETS, SUBAGENTS};

use crate::grok::color::{blend_color, pulse_brightness, pulse_fg, shimmer_tick};
use crate::theme::Theme;

/// Square: top, face, bottom.
pub const TILE_ROWS: u16 = 3;
const GAP: u16 = 2;
const INDENT: u16 = 1;
/// Match `color.rs` `SHIMMER_PULSE_SPEED` so the bead and ◆ stay in step.
const BREATHE_SPEED: f32 = 0.192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockChip {
    pub id: String,
    pub name: String,
    pub face: String,
    pub running: bool,
    pub cancelled: bool,
}

pub fn live_chips(ctx: &Context) -> Vec<DockChip> {
    let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
        return Vec::new();
    };
    let presets = ctx.get::<AgentPresets>(AGENT_PRESETS);
    chips_from_snaps(&sub.list(), |ty| {
        presets.as_ref().and_then(|p| p.role_label(ty))
    })
}

pub fn chips_from_snaps(
    snaps: &[SubagentSnap],
    mut role: impl FnMut(&str) -> Option<String>,
) -> Vec<DockChip> {
    snaps
        .iter()
        .filter(|s| !s.done)
        .map(|s| {
            let name = display_name(s, role(&s.subagent_type).as_deref());
            let face = face_glyph(&name);
            DockChip {
                id: s.id.clone(),
                name,
                face,
                running: s.running(),
                cancelled: s.cancelled,
            }
        })
        .collect()
}

/// Roster `name:` if present, otherwise the type id. Never a baked-in label.
pub fn display_name(snap: &SubagentSnap, role: Option<&str>) -> String {
    let from_role = role.map(str::trim).filter(|s| !s.is_empty());
    let from_type = snap.subagent_type.trim();
    from_role
        .map(str::to_string)
        .or_else(|| {
            if from_type.is_empty() {
                None
            } else {
                Some(from_type.to_string())
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".into())
}

/// First grapheme of the name. ASCII letters become uppercase so latin role
/// ids still read as a portrait initial; CJK / emoji stay as-is.
pub fn face_glyph(name: &str) -> String {
    let trimmed = name.trim();
    let Some(g) = trimmed.graphemes(true).next() else {
        return "?".into();
    };
    let mut chars = g.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() && chars.next().is_none() => {
            c.to_ascii_uppercase().to_string()
        }
        Some(_) => g.to_string(),
        None => "?".into(),
    }
}

pub fn desired_height(ctx: &Context, width: u16) -> u16 {
    height_for(&live_chips(ctx), width)
}

pub fn height_for(chips: &[DockChip], width: u16) -> u16 {
    if chips.is_empty() || width == 0 {
        0
    } else {
        TILE_ROWS
    }
}

pub fn paint(
    ctx: &Context,
    buf: &mut Buffer,
    area: Rect,
    selected: Option<&str>,
) -> Vec<(Rect, String)> {
    paint_chips(buf, area, &live_chips(ctx), selected)
}

pub fn paint_chips(
    buf: &mut Buffer,
    area: Rect,
    chips: &[DockChip],
    selected: Option<&str>,
) -> Vec<(Rect, String)> {
    if area.height == 0 || area.width == 0 || chips.is_empty() {
        return Vec::new();
    }
    let theme = Theme::current();
    let avail = area.width.saturating_sub(INDENT);
    let origin_x = area.x.saturating_add(INDENT);
    let (shown, overflow) = fit_tiles(chips, avail);
    let mut hits = Vec::new();
    let mut x = origin_x;
    for idx in shown {
        let chip = &chips[idx];
        let tw = tile_width(chip);
        if x.saturating_sub(origin_x) + tw > avail {
            break;
        }
        let tile = Rect {
            x,
            y: area.y,
            width: tw.min(area.x.saturating_add(area.width).saturating_sub(x)),
            height: area.height.min(TILE_ROWS),
        };
        let is_sel = selected == Some(chip.id.as_str());
        paint_tile(buf, tile, chip, is_sel, &theme, shimmer_tick());
        hits.push((tile, chip.id.clone()));
        x = x.saturating_add(tw).saturating_add(GAP);
    }
    if overflow > 0 {
        let label = format!("+{overflow}");
        let tw = overflow_width(overflow);
        if x.saturating_sub(origin_x) + tw <= avail {
            let tile = Rect {
                x,
                y: area.y,
                width: tw,
                height: area.height.min(TILE_ROWS),
            };
            paint_overflow(buf, tile, &label, &theme);
        }
    }
    hits
}

pub fn hit(hits: &[(Rect, String)], column: u16, row: u16) -> Option<String> {
    let pos = Position { x: column, y: row };
    hits.iter()
        .find(|(r, _)| r.contains(pos))
        .map(|(_, id)| id.clone())
}

pub fn tile_width(chip: &DockChip) -> u16 {
    circle_width(&chip.face)
}

fn circle_width(face: &str) -> u16 {
    (UnicodeWidthStr::width(face) as u16)
        .saturating_add(2)
        .max(3)
}

fn overflow_width(_n: usize) -> u16 {
    circle_width("+")
}

/// How many portraits fit, plus how many were clipped.
pub fn fit_tiles(chips: &[DockChip], width: u16) -> (Vec<usize>, usize) {
    if chips.is_empty() || width == 0 {
        return (Vec::new(), chips.len());
    }
    let mut used = 0u16;
    let mut shown = Vec::new();
    for (i, chip) in chips.iter().enumerate() {
        let tw = tile_width(chip);
        let rest = chips.len() - i - 1;
        let gap = if shown.is_empty() { 0 } else { GAP };
        let next = used.saturating_add(gap).saturating_add(tw);
        let overflow_need = if rest > 0 {
            GAP.saturating_add(overflow_width(rest))
        } else {
            0
        };
        if next.saturating_add(overflow_need) > width && !shown.is_empty() {
            return (shown, chips.len() - i);
        }
        if next > width {
            if shown.is_empty() {
                shown.push(i);
                return (shown, chips.len().saturating_sub(1));
            }
            return (shown, chips.len() - i);
        }
        used = next;
        shown.push(i);
    }
    (shown, 0)
}

fn paint_tile(
    buf: &mut Buffer,
    area: Rect,
    chip: &DockChip,
    selected: bool,
    theme: &Theme,
    tick: u64,
) {
    let base = avatar_color(&chip.name, theme);
    let accent = if chip.cancelled {
        theme.accent_error
    } else if chip.running {
        pulse_fg(theme.bg_base, base)
    } else {
        blend_color(theme.bg_base, base, 0.7).unwrap_or(base)
    };
    let phase = id_phase(&chip.id);
    let pulse = if chip.running {
        pulse_brightness(tick.wrapping_add(phase), BREATHE_SPEED)
    } else {
        0.0
    };
    let mut fill_op = if chip.running {
        0.14 + pulse * 0.24
    } else {
        0.08
    };
    if selected {
        fill_op += 0.12;
    }
    let fill = blend_color(theme.bg_base, base, fill_op).unwrap_or(theme.bg_light);
    paint_square(
        buf,
        area,
        &chip.face,
        accent,
        base,
        fill,
        theme.bg_base,
        chip.running,
        selected,
        tick,
        phase,
        theme,
    );
}

fn paint_overflow(buf: &mut Buffer, area: Rect, _label: &str, theme: &Theme) {
    let accent = theme.gray;
    paint_square(
        buf,
        area,
        "+",
        accent,
        accent,
        theme.bg_base,
        theme.bg_base,
        false,
        false,
        0,
        0,
        theme,
    );
}

#[allow(clippy::too_many_arguments)]
// TUI 绘制/布局函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到所有调用点，故意保留。
fn paint_square(
    buf: &mut Buffer,
    area: Rect,
    face: &str,
    accent: Color,
    bead: Color,
    fill: Color,
    outside: Color,
    running: bool,
    selected: bool,
    tick: u64,
    phase: u64,
    theme: &Theme,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    fill_rect(buf, area, outside);
    let w = circle_width(face).min(area.width);
    if w < 3 || area.height < 3 {
        let face_style = Style::default()
            .fg(theme.text_primary)
            .bg(fill)
            .add_modifier(Modifier::BOLD);
        center_text(buf, area.x, area.y, area.width, face, face_style);
        return;
    }
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y;
    let rim = rim_cells(w);
    let hot = if running {
        Some(hot_index(tick, phase, rim.len()))
    } else {
        None
    };
    let inner = w.saturating_sub(2);
    let inner_x = x.saturating_add(1);
    let face_y = y.saturating_add(1);
    for dx in 0..inner {
        put_char(
            buf,
            inner_x.saturating_add(dx),
            face_y,
            ' ',
            Style::default().fg(accent).bg(fill),
        );
    }
    let face_style = Style::default()
        .fg(if running {
            theme.text_primary
        } else {
            theme.text_secondary
        })
        .bg(fill)
        .add_modifier(Modifier::BOLD);
    center_text(buf, inner_x, face_y, inner, face, face_style);

    let dim = blend_color(outside, accent, if selected { 0.95 } else { 0.85 }).unwrap_or(accent);
    for (i, (dx, dy, idle, hot_ch)) in rim.iter().enumerate() {
        let is_hot = hot == Some(i);
        let ch = if is_hot { *hot_ch } else { *idle };
        let fg = if is_hot { bead } else { dim };
        put_char(
            buf,
            x.saturating_add(*dx),
            y.saturating_add(*dy),
            ch,
            Style::default()
                .fg(fg)
                .bg(if *dy == 1 { fill } else { outside }),
        );
    }
}

/// Clockwise rim of the 3-row square. `(dx, dy, idle, bead)`.
fn rim_cells(w: u16) -> Vec<(u16, u16, char, char)> {
    let mut v = Vec::new();
    v.push((0, 0, '┌', '┌'));
    for dx in 1..w.saturating_sub(1) {
        v.push((dx, 0, '─', '━'));
    }
    v.push((w.saturating_sub(1), 0, '┐', '┐'));
    v.push((w.saturating_sub(1), 1, '│', '┃'));
    v.push((w.saturating_sub(1), 2, '┘', '┘'));
    for dx in (1..w.saturating_sub(1)).rev() {
        v.push((dx, 2, '─', '━'));
    }
    v.push((0, 2, '└', '└'));
    v.push((0, 1, '│', '┃'));
    v
}

fn hot_index(tick: u64, phase: u64, n: usize) -> usize {
    if n == 0 {
        0
    } else {
        ((tick / 2).wrapping_add(phase) % n as u64) as usize
    }
}

fn id_phase(id: &str) -> u64 {
    let mut h = 0u64;
    for b in id.as_bytes() {
        h = h.wrapping_mul(16777619).wrapping_add(*b as u64);
    }
    h
}

/// Stable palette from the display name — same name → same color, any roster.
fn avatar_color(name: &str, theme: &Theme) -> Color {
    let mut h = 0u32;
    for b in name.as_bytes() {
        h = h.wrapping_mul(16777619).wrapping_add(*b as u32);
    }
    let palette = [
        theme.accent_user,
        theme.accent_assistant,
        theme.accent_tool,
        theme.accent_skill,
        theme.accent_plan,
        theme.accent_verify,
        theme.accent_remember,
        theme.command,
        theme.path,
        theme.running,
    ];
    palette[(h as usize) % palette.len()]
}

fn fill_rect(buf: &mut Buffer, area: Rect, bg: Color) {
    let style = Style::default().bg(bg);
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(' ');
                cell.set_style(style);
            }
        }
    }
}

fn center_text(buf: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    let w = UnicodeWidthStr::width(text) as u16;
    let pad = width.saturating_sub(w) / 2;
    put_str(buf, x.saturating_add(pad), y, text, style);
}

fn put_str(buf: &mut Buffer, x: u16, y: u16, s: &str, style: Style) {
    let mut col = x;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if w == 0 {
            continue;
        }
        put_char(buf, col, y, ch, style);
        col = col.saturating_add(w);
    }
}

fn put_char(buf: &mut Buffer, x: u16, y: u16, ch: char, style: Style) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(ch);
        cell.set_style(style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn snap(id: &str, ty: &str, done: bool) -> SubagentSnap {
        SubagentSnap {
            id: id.into(),
            description: "ignored".into(),
            subagent_type: ty.into(),
            done,
            idle: !done,
            cancelled: false,
            output: String::new(),
            started_at: Instant::now(),
        }
    }

    #[test]
    fn face_comes_from_the_name_not_a_type_map() {
        assert_eq!(face_glyph("观"), "观");
        assert_eq!(face_glyph("锁"), "锁");
        assert_eq!(face_glyph("突击"), "突");
        assert_eq!(face_glyph("explore"), "E");
        assert_eq!(face_glyph("general-purpose"), "G");
        assert_eq!(face_glyph("  plan"), "P");
        assert_eq!(face_glyph(""), "?");
    }

    #[test]
    fn display_name_prefers_roster_then_type_id() {
        let sa = snap("a", "jia", false);
        assert_eq!(display_name(&sa, Some("甲")), "甲");
        assert_eq!(display_name(&sa, Some("  ")), "jia");
        assert_eq!(display_name(&sa, None), "jia");
        let empty = snap("b", "", false);
        assert_eq!(display_name(&empty, None), "?");
    }

    #[test]
    fn done_agents_are_not_chips() {
        let snaps = vec![snap("live", "观", false), snap("gone", "锁", true)];
        let chips = chips_from_snaps(&snaps, |ty| Some(ty.to_string()));
        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].name, "观");
        assert_eq!(chips[0].face, "观");
    }

    #[test]
    fn portraits_lay_out_horizontally_and_clip_with_count() {
        let chips: Vec<_> = ["观", "锁", "甲", "乙", "丙"]
            .into_iter()
            .enumerate()
            .map(|(i, n)| DockChip {
                id: format!("id{i}"),
                name: n.into(),
                face: face_glyph(n),
                running: false,
                cancelled: false,
            })
            .collect();
        // 观 box is 4 cols; five tiles + gaps need more than 12.
        let (shown, overflow) = fit_tiles(&chips, 12);
        assert!(!shown.is_empty());
        assert!(overflow > 0);
        assert_eq!(shown.len() + overflow, chips.len());
    }

    #[test]
    fn empty_strip_takes_no_rows() {
        assert_eq!(height_for(&[], 80), 0);
    }

    #[test]
    fn click_hits_the_tile_not_the_gap() {
        let chips = vec![DockChip {
            id: "abc".into(),
            name: "观".into(),
            face: "观".into(),
            running: true,
            cancelled: false,
        }];
        let area = Rect::new(0, 10, 40, 3);
        let mut buf = Buffer::empty(area);
        let hits = paint_chips(&mut buf, area, &chips, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hit(&hits, hits[0].0.x, hits[0].0.y).as_deref(), Some("abc"));
        let past = hits[0]
            .0
            .x
            .saturating_add(hits[0].0.width)
            .saturating_add(1);
        assert_eq!(hit(&hits, past, hits[0].0.y), None);
        let tile = hits[0].0;
        let mut saw_corner = false;
        for x in tile.x..tile.x + tile.width {
            if buf[(x, tile.y)].symbol().contains('┌') {
                saw_corner = true;
            }
        }
        assert!(saw_corner, "avatar is a square box, not a circle blob");
    }
}
