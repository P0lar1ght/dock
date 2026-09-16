//! Sticky goal chrome above the composer.
//!
//! Two rows while a goal is present: title + pause / edit / close on the
//! first row; last `update_goal` note plus a travelling multi-color wave on
//! the second. The 80ms shimmer tick drives both the hue sweep and the
//! block-wave. Paused goals freeze and dim.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::grok::color::{blend_color, pulse_brightness};
use crate::grok::line_utils::truncate_str;
use crate::theme::Theme;

pub const PANE_ROWS: u16 = 2;
const PAUSE: &str = "[暂停]";
const RESUME: &str = "[继续]";
const EDIT: &str = "[修改]";
const CLOSE: &str = "[关闭]";
const BTN_GAP: u16 = 1;
const WAVE: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalHit {
    Pause,
    Edit,
    Close,
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalChrome {
    pub title: String,
    pub status: String,
    pub paused: bool,
}

pub fn desired_height(present: bool) -> u16 {
    if present {
        PANE_ROWS
    } else {
        0
    }
}

pub fn hit(hits: &[(Rect, GoalHit)], column: u16, row: u16) -> Option<GoalHit> {
    let pos = Position { x: column, y: row };
    hits.iter().find(|(r, _)| r.contains(pos)).map(|(_, h)| *h)
}

pub fn paint(
    buf: &mut Buffer,
    area: Rect,
    chrome: &GoalChrome,
    mouse: (u16, u16),
    tick: u64,
) -> Vec<(Rect, GoalHit)> {
    let mut hits = Vec::new();
    if area.height == 0 || area.width == 0 {
        return hits;
    }
    let theme = Theme::current();
    let tick = if chrome.paused { 0 } else { tick };
    fill_motion(buf, area, &theme, tick, chrome.paused);

    let pause_label = if chrome.paused { RESUME } else { PAUSE };
    let buttons = button_layout(area, pause_label);
    paint_title_row(buf, area, chrome, &theme, tick, &buttons);
    if area.height >= 2 {
        paint_wave_row(buf, area, chrome, &theme, tick, buttons.left);
    }
    push_button_hits(&mut hits, &buttons);
    hits.push((area, GoalHit::Open));
    apply_button_hover(buf, &buttons, mouse, &theme, chrome.paused);
    hits
}

struct Buttons {
    pause: Rect,
    edit: Rect,
    close: Rect,
    left: u16,
}

fn button_layout(area: Rect, pause_label: &str) -> Buttons {
    let pause_w = pause_label.width() as u16;
    let edit_w = EDIT.width() as u16;
    let close_w = CLOSE.width() as u16;
    let need = pause_w
        .saturating_add(BTN_GAP)
        .saturating_add(edit_w)
        .saturating_add(BTN_GAP)
        .saturating_add(close_w);
    let y = area.y;
    if area.width < need {
        return Buttons {
            pause: Rect {
                x: area.x,
                y,
                width: 0,
                height: 0,
            },
            edit: Rect {
                x: area.x,
                y,
                width: 0,
                height: 0,
            },
            close: Rect {
                x: area.x,
                y,
                width: 0,
                height: 0,
            },
            left: area.x + area.width,
        };
    }
    let close_x = area.x + area.width.saturating_sub(close_w);
    let edit_x = close_x.saturating_sub(BTN_GAP).saturating_sub(edit_w);
    let pause_x = edit_x.saturating_sub(BTN_GAP).saturating_sub(pause_w);
    Buttons {
        pause: Rect {
            x: pause_x,
            y,
            width: pause_w,
            height: 1,
        },
        edit: Rect {
            x: edit_x,
            y,
            width: edit_w,
            height: 1,
        },
        close: Rect {
            x: close_x,
            y,
            width: close_w,
            height: 1,
        },
        left: pause_x,
    }
}

fn push_button_hits(hits: &mut Vec<(Rect, GoalHit)>, buttons: &Buttons) {
    if buttons.pause.width > 0 {
        hits.push((buttons.pause, GoalHit::Pause));
    }
    if buttons.edit.width > 0 {
        hits.push((buttons.edit, GoalHit::Edit));
    }
    if buttons.close.width > 0 {
        hits.push((buttons.close, GoalHit::Close));
    }
}

fn paint_title_row(
    buf: &mut Buffer,
    area: Rect,
    chrome: &GoalChrome,
    theme: &Theme,
    tick: u64,
    buttons: &Buttons,
) {
    let pal = motion_palette(theme);
    let diamond = if chrome.paused { "◇" } else { "◆" };
    let diamond_fg = if chrome.paused {
        theme.warning
    } else {
        pal[((tick / 2) as usize) % pal.len()]
    };
    let tag = if chrome.paused {
        "已暂停"
    } else {
        "进行中"
    };
    let tag_fg = if chrome.paused {
        theme.warning
    } else {
        pal[((tick / 2) as usize + 2) % pal.len()]
    };
    let title = chrome.title.trim();
    let title = if title.is_empty() { "目标" } else { title };
    let prefix = format!("{diamond} {tag}  ");
    let prefix_w = prefix.width() as u16;
    let title_budget = buttons
        .left
        .saturating_sub(area.x)
        .saturating_sub(prefix_w)
        .saturating_sub(1) as usize;
    let shown = truncate_str(title, title_budget);
    let title_fg = if chrome.paused {
        theme.text_secondary
    } else {
        theme.text_primary
    };
    buf.set_line(
        area.x,
        area.y,
        &Line::from(vec![
            Span::styled(
                diamond.to_string(),
                Style::default().fg(diamond_fg).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                tag,
                Style::default().fg(tag_fg).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(shown, Style::default().fg(title_fg)),
        ]),
        buttons.left.saturating_sub(area.x),
    );

    let pause_label = if chrome.paused { RESUME } else { PAUSE };
    paint_button(buf, buttons.pause, pause_label, theme.running, false, theme);
    paint_button(buf, buttons.edit, EDIT, theme.accent_plan, false, theme);
    paint_button(buf, buttons.close, CLOSE, theme.accent_error, false, theme);
}

fn paint_button(
    buf: &mut Buffer,
    rect: Rect,
    label: &str,
    fg: Color,
    hovered: bool,
    theme: &Theme,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let style = if hovered {
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(fg)
    };
    buf.set_line(
        rect.x,
        rect.y,
        &Line::from(Span::styled(label, style)),
        rect.width,
    );
}

fn apply_button_hover(
    buf: &mut Buffer,
    buttons: &Buttons,
    mouse: (u16, u16),
    theme: &Theme,
    paused: bool,
) {
    let pause_label = if paused { RESUME } else { PAUSE };
    let over = |r: Rect| {
        r.width > 0
            && r.contains(Position {
                x: mouse.0,
                y: mouse.1,
            })
    };
    if over(buttons.pause) {
        paint_button(buf, buttons.pause, pause_label, theme.running, true, theme);
    }
    if over(buttons.edit) {
        paint_button(buf, buttons.edit, EDIT, theme.accent_plan, true, theme);
    }
    if over(buttons.close) {
        paint_button(buf, buttons.close, CLOSE, theme.accent_error, true, theme);
    }
}

fn paint_wave_row(
    buf: &mut Buffer,
    area: Rect,
    chrome: &GoalChrome,
    theme: &Theme,
    tick: u64,
    buttons_left: u16,
) {
    let y = area.y.saturating_add(1);
    if y >= area.y + area.height {
        return;
    }
    let pal = motion_palette(theme);
    let status = chrome.status.trim();
    let status_w = if status.is_empty() {
        0
    } else {
        status.width() as u16 + 1
    };
    let wave_end = if status_w == 0 {
        area.x + area.width
    } else {
        buttons_left.min(area.x + area.width)
    };
    let wave_budget = wave_end.saturating_sub(area.x);
    let wave_w = if status_w == 0 {
        wave_budget
    } else {
        wave_budget.saturating_sub(status_w).clamp(8, 18)
    };
    for i in 0..wave_w {
        let x = area.x + i;
        let (ch, fg) = if chrome.paused {
            (
                '─',
                blend_color(theme.bg_base, theme.gray, 0.7).unwrap_or(theme.gray),
            )
        } else {
            let phase = (i as u64).wrapping_add(tick);
            let tri = wave_height(phase);
            let hot = pulse_brightness(tick.wrapping_add(i as u64), 0.28);
            let base = pal[(phase as usize) % pal.len()];
            let fg = blend_color(theme.bg_base, base, 0.45 + hot * 0.55).unwrap_or(base);
            (WAVE[tri], fg)
        };
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(&ch.to_string());
            cell.set_fg(fg);
        }
    }
    if status_w == 0 {
        return;
    }
    let text_x = area.x.saturating_add(wave_w).saturating_add(1);
    let budget = (area.x + area.width).saturating_sub(text_x) as usize;
    if budget == 0 {
        return;
    }
    let shown = truncate_str(status, budget);
    let fg = if chrome.paused {
        theme.gray
    } else {
        theme.text_secondary
    };
    buf.set_line(
        text_x,
        y,
        &Line::from(Span::styled(shown, Style::default().fg(fg))),
        budget as u16,
    );
}

fn wave_height(phase: u64) -> usize {
    let p = (phase % 16) as usize;
    if p < 8 { p } else { 16 - p }.min(7)
}

fn fill_motion(buf: &mut Buffer, area: Rect, theme: &Theme, tick: u64, paused: bool) {
    let pal = motion_palette(theme);
    for dy in 0..area.height {
        for dx in 0..area.width {
            let x = area.x + dx;
            let y = area.y + dy;
            let Some(cell) = buf.cell_mut((x, y)) else {
                continue;
            };
            cell.reset();
            let bg = if paused {
                theme.bg_base
            } else {
                let c = pal[((dx as u64).wrapping_add(tick / 2)) as usize % pal.len()];
                let wash = 0.06 + 0.06 * pulse_brightness(tick.wrapping_add(dx as u64), 0.22);
                blend_color(theme.bg_base, c, wash).unwrap_or(theme.bg_base)
            };
            cell.set_style(Style::default().bg(bg));
        }
    }
}

fn motion_palette(theme: &Theme) -> [Color; 8] {
    [
        theme.running,
        theme.accent_system,
        theme.accent_running,
        theme.accent_verify,
        theme.path,
        theme.command,
        theme.accent_success,
        theme.accent_error,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use std::collections::HashSet;
    use unicode_width::UnicodeWidthStr;

    fn chrome(paused: bool, status: &str) -> GoalChrome {
        GoalChrome {
            title: "理解并分析 TUI".into(),
            status: status.into(),
            paused,
        }
    }

    #[test]
    fn height_is_two_when_present() {
        assert_eq!(desired_height(false), 0);
        assert_eq!(desired_height(true), 2);
    }

    fn visible_row(buf: &Buffer, y: u16, width: u16) -> String {
        let mut s = String::new();
        let mut skip = 0u16;
        for x in 0..width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let sym = buf.cell((x, y)).unwrap().symbol();
            s.push_str(sym);
            let w = UnicodeWidthStr::width(sym) as u16;
            if w > 1 {
                skip = w - 1;
            }
        }
        s
    }

    #[test]
    fn buttons_hit_on_the_right() {
        let area = Rect::new(0, 8, 64, 2);
        let mut buf = Buffer::empty(area);
        let hits = paint(&mut buf, area, &chrome(false, ""), (0, 0), 3);
        assert_eq!(hit(&hits, 0, 8), Some(GoalHit::Open));
        let close = hits
            .iter()
            .find(|(_, h)| *h == GoalHit::Close)
            .map(|(r, _)| *r)
            .expect("close");
        assert_eq!(close.y, 8);
        assert_eq!(close.x + close.width, 64);
        assert_eq!(hit(&hits, close.x, close.y), Some(GoalHit::Close));
        let pause = hits
            .iter()
            .find(|(_, h)| *h == GoalHit::Pause)
            .map(|(r, _)| *r)
            .expect("pause");
        assert_eq!(hit(&hits, pause.x, pause.y), Some(GoalHit::Pause));
        let edit = hits
            .iter()
            .find(|(_, h)| *h == GoalHit::Edit)
            .map(|(r, _)| *r)
            .expect("edit");
        assert_eq!(hit(&hits, edit.x, edit.y), Some(GoalHit::Edit));
        let row = visible_row(&buf, 8, 64);
        assert!(row.contains("进行中"), "{row}");
        assert!(row.contains("[暂停]"), "{row}");
        assert!(row.contains("[修改]"), "{row}");
        assert!(row.contains("[关闭]"), "{row}");
    }

    #[test]
    fn paused_flips_resume_and_freezes_wave() {
        let area = Rect::new(0, 0, 64, 2);
        let mut buf = Buffer::empty(area);
        let hits = paint(&mut buf, area, &chrome(true, "停住"), (0, 0), 99);
        let row = visible_row(&buf, 0, 64);
        assert!(row.contains("已暂停"), "{row}");
        assert!(row.contains("[继续]"), "{row}");
        let wave = visible_row(&buf, 1, 18);
        assert!(wave.contains('─'), "{wave}");
        assert!(!wave.contains('█'), "{wave}");
        assert_eq!(hit(&hits, 0, 0), Some(GoalHit::Open));
    }

    #[test]
    fn running_wave_uses_several_colors_and_shifts() {
        let area = Rect::new(0, 0, 48, 2);
        let mut a = Buffer::empty(area);
        let mut b = Buffer::empty(area);
        paint(&mut a, area, &chrome(false, ""), (0, 0), 0);
        paint(&mut b, area, &chrome(false, ""), (0, 0), 4);
        let mut colors = HashSet::new();
        for x in 0..18 {
            colors.insert(format!("{:?}", a.cell((x, 1)).unwrap().fg));
        }
        assert!(
            colors.len() >= 4,
            "expected a multi-color wave, got {colors:?}"
        );
        let shifted = (0..18).any(|x| a.cell((x, 1)).unwrap().fg != b.cell((x, 1)).unwrap().fg);
        assert!(shifted, "wave should travel with tick");
    }
}
