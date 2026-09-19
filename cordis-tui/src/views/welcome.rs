//! Welcome screen: Grok hero box (logo left, version + menu right).
//! Shimmer is the Grok sweep. Menu rows keep hit-rects so a left click
//! matches the shortcut.

use std::sync::Mutex;
use std::time::Instant;

use cordis::Context;
use cordis_spine::{Sessions, SESSIONS};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::grok::color::blend_color;
use crate::theme::Theme;

/// Welcome menu row — same order as Grok's post-auth stacked menu (new / resume / quit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WelcomeHit {
    NewSession,
    Resume,
    Quit,
}

const MENU: [(&str, &str, WelcomeHit); 3] = [
    ("ctrl+w", "新会话", WelcomeHit::NewSession),
    ("f3", "恢复会话", WelcomeHit::Resume),
    ("ctrl+q", "退出", WelcomeHit::Quit),
];

const LOGO: &str = include_str!("../../assets/logo/logo16.txt");
const LOGO_MID: &str = include_str!("../../assets/logo/logo12.txt");
const LOGO_SMALL: &str = include_str!("../../assets/logo/logo07.txt");
const TITLE: &str = "Dock";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const SUBTITLE: &str = "Cordis 插件树 · 反馈用 /help";
const TIP: &str = "Shift+Tab 切换询问 / 始终允许";

/// Side-by-side hero box (Grok `HERO_BOX_MIN_WIDTH` is 90; Dock is a bit tighter).
const HERO_MIN_WIDTH: u16 = 64;
const HERO_MIN_HEIGHT: u16 = 12;
const V_PAD: u16 = 1;
const LOGO_H_PAD: u16 = 3;
const H_INSET: u16 = 2;

/// Thresholds are the welcome pane height (already minus status/prompt/hints).
const SMALL_LOGO_MIN_HEIGHT: u16 = 12;
const MID_LOGO_MIN_HEIGHT: u16 = 18;
const FULL_LOGO_MIN_HEIGHT: u16 = 24;

pub struct Welcome {
    ctx: Context,
    start: Instant,
    menu_rects: Mutex<Vec<(Rect, WelcomeHit)>>,
    hover: Mutex<Option<usize>>,
}

impl Welcome {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            start: Instant::now(),
            menu_rects: Mutex::new(Vec::new()),
            hover: Mutex::new(None),
        }
    }

    pub fn empty_session(&self) -> bool {
        self.ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| s.events().is_empty())
            .unwrap_or(true)
    }

    pub fn hit(&self, column: u16, row: u16) -> Option<WelcomeHit> {
        hit_rects(&self.menu_rects.lock().unwrap(), column, row)
    }

    pub fn set_mouse(&self, column: u16, row: u16) {
        let next = self
            .menu_rects
            .lock()
            .unwrap()
            .iter()
            .position(|(rect, _)| rect.contains(Position { x: column, y: row }));
        *self.hover.lock().unwrap() = next;
    }
}

fn hit_rects(rects: &[(Rect, WelcomeHit)], column: u16, row: u16) -> Option<WelcomeHit> {
    let pos = Position { x: column, y: row };
    rects
        .iter()
        .find(|(rect, _)| rect.contains(pos))
        .map(|(_, hit)| *hit)
}

impl Widget for &Welcome {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let theme = Theme::current();
        buf.set_style(area, Style::default().bg(theme.bg_base));
        if area.height < 8 {
            self.menu_rects.lock().unwrap().clear();
            return;
        }
        let hover = *self.hover.lock().unwrap();
        let rects = render_welcome(area, buf, &theme, self.start.elapsed().as_secs_f32(), hover);
        *self.menu_rects.lock().unwrap() = rects;
    }
}

fn render_welcome(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    secs: f32,
    hover: Option<usize>,
) -> Vec<(Rect, WelcomeHit)> {
    if area.width >= HERO_MIN_WIDTH && area.height >= HERO_MIN_HEIGHT {
        render_hero_box(area, buf, theme, secs, hover)
    } else {
        render_stacked_box(area, buf, theme, secs, hover)
    }
}

fn render_hero_box(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    secs: f32,
    hover: Option<usize>,
) -> Vec<(Rect, WelcomeHit)> {
    let box_w = area.width.saturating_sub(4).clamp(40, 108);
    let inner_w = box_w.saturating_sub(2);
    let logo = pick_hero_logo(inner_w);
    let logo_h = logo.map(count_lines).unwrap_or(0);
    let logo_w = logo.map(visual_width).unwrap_or(0);
    let left_w = if logo_w == 0 {
        H_INSET
    } else {
        logo_w + LOGO_H_PAD.saturating_sub(1) + LOGO_H_PAD
    }
    .min(inner_w.saturating_sub(20));
    let right_h = 1 + 1 + 1 + 1 + MENU.len() as u16;
    let inner_h = logo_h.max(right_h).max(1);
    let box_h = (2 + V_PAD * 2 + inner_h).min(area.height);

    let top = area.height.saturating_sub(box_h) / 3;
    let [_, slot, _] = Layout::vertical([
        Constraint::Length(top),
        Constraint::Length(box_h),
        Constraint::Min(0),
    ])
    .areas(area);
    let [_, hero, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(box_w.min(slot.width)),
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .areas(slot);

    paint_box(hero, buf, theme);

    let inner = Rect {
        x: hero.x.saturating_add(1),
        y: hero.y.saturating_add(1 + V_PAD),
        width: hero.width.saturating_sub(2),
        height: hero.height.saturating_sub(2 + V_PAD * 2),
    };
    if inner.width < 8 || inner.height == 0 {
        return Vec::new();
    }

    let left_w = left_w.min(inner.width.saturating_sub(16));
    if let Some(logo) = logo {
        let logo_left = LOGO_H_PAD.saturating_sub(1);
        let logo_area = Rect {
            x: inner.x.saturating_add(logo_left),
            y: inner.y,
            width: logo_w.min(inner.width.saturating_sub(logo_left)),
            height: logo_h.min(inner.height),
        };
        render_logo(logo_area, buf, theme, logo, secs, Alignment::Left);
    }

    let right = Rect {
        x: inner.x.saturating_add(left_w),
        y: inner.y,
        width: inner.width.saturating_sub(left_w),
        height: inner.height,
    };
    paint_right_column(right, buf, theme, hover)
}

fn render_stacked_box(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    secs: f32,
    hover: Option<usize>,
) -> Vec<(Rect, WelcomeHit)> {
    let logo = pick_logo(area.height);
    let logo_h = logo.map(count_lines).unwrap_or(0);
    let copy_h = 1 + 1 + 1 + 1 + MENU.len() as u16;
    let inner_h = logo_h
        .saturating_add(u16::from(logo_h > 0))
        .saturating_add(copy_h);
    let box_h = (2 + V_PAD * 2 + inner_h).min(area.height);
    let box_w = area.width.saturating_sub(2).min(72).max(24.min(area.width));

    let top = area.height.saturating_sub(box_h) / 3;
    let [_, slot, _] = Layout::vertical([
        Constraint::Length(top),
        Constraint::Length(box_h),
        Constraint::Min(0),
    ])
    .areas(area);
    let [_, hero, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(box_w.min(slot.width)),
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .areas(slot);

    paint_box(hero, buf, theme);
    let inner = Rect {
        x: hero.x.saturating_add(1),
        y: hero.y.saturating_add(1 + V_PAD),
        width: hero.width.saturating_sub(2),
        height: hero.height.saturating_sub(2 + V_PAD * 2),
    };
    if inner.height < 4 {
        return render_menu(inner, buf, theme, hover, false);
    }

    let chunks = Layout::vertical([
        Constraint::Length(logo_h),
        Constraint::Length(u16::from(logo_h > 0)),
        Constraint::Min(copy_h),
    ])
    .split(inner);
    if let Some(logo) = logo {
        render_logo(chunks[0], buf, theme, logo, secs, Alignment::Center);
    }
    paint_right_column(chunks[2], buf, theme, hover)
}

fn paint_box(area: Rect, buf: &mut Buffer, theme: &Theme) {
    let border = blend_color(theme.bg_base, theme.gray_dim, 0.45).unwrap_or(theme.gray_dim);
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border).bg(theme.bg_base))
        .style(Style::default().bg(theme.bg_base))
        .render(area, buf);
}

fn paint_right_column(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    hover: Option<usize>,
) -> Vec<(Rect, WelcomeHit)> {
    if area.width < 8 || area.height == 0 {
        return Vec::new();
    }
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(MENU.len() as u16),
    ])
    .split(area);

    let title = Span::styled(
        format!("{TITLE}  "),
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    let version = Span::styled(VERSION, Style::default().fg(theme.gray));
    Paragraph::new(Line::from(vec![title, version])).render(rows[0], buf);

    if rows[1].height > 0 {
        buf.set_stringn(
            rows[1].x,
            rows[1].y,
            SUBTITLE,
            rows[1].width as usize,
            Style::default().fg(theme.gray),
        );
    }
    if rows[2].height > 0 {
        buf.set_stringn(
            rows[2].x,
            rows[2].y,
            TIP,
            rows[2].width as usize,
            Style::default().fg(theme.path),
        );
    }
    render_menu(rows[4], buf, theme, hover, true)
}

fn pick_hero_logo(inner_w: u16) -> Option<&'static str> {
    if inner_w < 16 {
        None
    } else {
        Some(LOGO_SMALL)
    }
}

fn pick_logo(window_height: u16) -> Option<&'static str> {
    if window_height < SMALL_LOGO_MIN_HEIGHT {
        None
    } else if window_height < MID_LOGO_MIN_HEIGHT {
        Some(LOGO_SMALL)
    } else if window_height < FULL_LOGO_MIN_HEIGHT {
        Some(LOGO_MID)
    } else {
        Some(LOGO)
    }
}

fn non_empty_lines(logo: &str) -> impl Iterator<Item = &str> {
    logo.lines().filter(|l| !l.is_empty())
}

fn count_lines(logo: &str) -> u16 {
    non_empty_lines(logo).count() as u16
}

fn visual_width(logo: &str) -> u16 {
    non_empty_lines(logo).map(cols).max().unwrap_or(0)
}

fn shine_opacity(diag: f32, secs: f32) -> f32 {
    const BAND: f32 = 0.38;
    const CYCLE: f32 = 4.0;
    const SWEEP_FRAC: f32 = 0.32;
    const SHINE: f32 = 0.33;
    const PULSE: f32 = 0.06;
    const PULSE_SECS: f32 = 5.0;

    let p = (secs % CYCLE) / CYCLE;
    let q = (p / SWEEP_FRAC).min(1.0);
    let band_pos = -BAND + q * (1.0 + 2.0 * BAND);
    let pulse = PULSE * (0.5 - 0.5 * (std::f32::consts::TAU * secs / PULSE_SECS).cos());
    let d = (diag - band_pos).abs();
    let shine = if d < BAND {
        0.5 * (1.0 + (std::f32::consts::PI * d / BAND).cos())
    } else {
        0.0
    };
    (pulse + SHINE * shine).clamp(0.0, 1.0)
}

fn blend(a: Color, b: Color, t: f32) -> Color {
    let Color::Rgb(ar, ag, ab) = a else {
        return b;
    };
    let Color::Rgb(br, bg, bb) = b else {
        return b;
    };
    let t = t.clamp(0.0, 1.0);
    Color::Rgb(
        (ar as f32 + (br as f32 - ar as f32) * t).round() as u8,
        (ag as f32 + (bg as f32 - ag as f32) * t).round() as u8,
        (ab as f32 + (bb as f32 - ab as f32) * t).round() as u8,
    )
}

fn render_logo(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    logo: &str,
    secs: f32,
    align: Alignment,
) {
    let lines: Vec<&str> = non_empty_lines(logo).collect();
    let rows = lines.len().max(1) as f32;
    let cols_n = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let base = theme.gray;
    let hilite = theme.text_primary;
    let logo_lines: Vec<Line> = lines
        .iter()
        .enumerate()
        .map(|(row, line)| {
            let mut spans: Vec<Span> = Vec::new();
            let mut run = String::new();
            let mut run_color: Option<Color> = None;
            for (col, ch) in line.chars().enumerate() {
                let diag = (col as f32 + (rows - 1.0 - row as f32)) / (cols_n + rows);
                let color = blend(base, hilite, shine_opacity(diag, secs));
                if run_color != Some(color) {
                    if let Some(prev) = run_color {
                        spans.push(Span::styled(
                            std::mem::take(&mut run),
                            Style::default().fg(prev),
                        ));
                    }
                    run_color = Some(color);
                }
                run.push(ch);
            }
            if let Some(prev) = run_color {
                spans.push(Span::styled(run, Style::default().fg(prev)));
            }
            Line::from(spans).alignment(align)
        })
        .collect();
    Paragraph::new(logo_lines).render(area, buf);
}

fn cols(text: &str) -> u16 {
    UnicodeWidthStr::width(text) as u16
}

fn render_menu(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    hover: Option<usize>,
    left_align: bool,
) -> Vec<(Rect, WelcomeHit)> {
    let label_style = Style::default()
        .fg(theme.text_primary)
        .add_modifier(Modifier::BOLD);
    let label_hover_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.bg_highlight)
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(theme.gray_bright);
    let key_hover_style = Style::default()
        .fg(theme.gray_bright)
        .bg(theme.bg_highlight);
    let label_w = MENU
        .iter()
        .map(|(_, label, _)| cols(label))
        .max()
        .unwrap_or(0);
    let key_w = MENU.iter().map(|(key, _, _)| cols(key)).max().unwrap_or(0);
    let menu_width = label_w
        .saturating_add(2)
        .saturating_add(key_w)
        .min(area.width);
    let menu_centered = if left_align {
        Rect {
            x: area.x,
            y: area.y,
            width: menu_width,
            height: area.height,
        }
    } else {
        let [_, mid, _] = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(menu_width),
            Constraint::Min(0),
        ])
        .flex(Flex::Center)
        .areas(area);
        mid
    };

    let mut rects = Vec::with_capacity(MENU.len());
    let mut y = menu_centered.y;
    for (i, (key, label, hit)) in MENU.iter().enumerate() {
        if y >= menu_centered.y + menu_centered.height {
            break;
        }
        let hovered = hover == Some(i);
        let row_rect = Rect {
            x: menu_centered.x,
            y,
            width: menu_centered.width,
            height: 1,
        };
        rects.push((row_rect, *hit));
        if hovered {
            let hover_bg = Style::default().bg(theme.bg_highlight);
            for x in menu_centered.x..menu_centered.x + menu_centered.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(hover_bg);
                }
            }
        }
        let key_width = cols(key);
        let label_len = cols(label);
        buf.set_stringn(
            menu_centered.x,
            y,
            label,
            label_len as usize,
            if hovered {
                label_hover_style
            } else {
                label_style
            },
        );
        let key_x = menu_centered
            .x
            .saturating_add(menu_centered.width.saturating_sub(key_width));
        buf.set_stringn(
            key_x,
            y,
            key,
            key_width as usize,
            if hovered { key_hover_style } else { key_style },
        );
        y = y.saturating_add(1);
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint(width: u16, height: u16, hover: Option<usize>) -> (Buffer, Vec<(Rect, WelcomeHit)>) {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        let theme = Theme::current();
        buf.set_style(area, Style::default().bg(theme.bg_base));
        let rects = render_welcome(area, &mut buf, &theme, 0.0, hover);
        (buf, rects)
    }

    fn compact(buf: &Buffer) -> String {
        let area = buf.area();
        let mut painted = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                painted.push_str(buf[(x, y)].symbol());
            }
        }
        painted.chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn menu_labels_are_chinese() {
        let (buf, _) = paint(80, 30, None);
        let text = compact(&buf);
        assert!(text.contains("新会话"), "{text}");
        assert!(text.contains("恢复会话"), "{text}");
        assert!(text.contains("退出"), "{text}");
    }

    #[test]
    fn click_each_row_maps_to_action() {
        let (_, rects) = paint(80, 30, None);
        assert_eq!(rects.len(), 3);
        let (new_r, _) = rects[0];
        let (resume_r, _) = rects[1];
        let (quit_r, _) = rects[2];
        assert_eq!(
            hit_rects(&rects, new_r.x, new_r.y),
            Some(WelcomeHit::NewSession)
        );
        assert_eq!(
            hit_rects(&rects, resume_r.x + resume_r.width / 2, resume_r.y),
            Some(WelcomeHit::Resume)
        );
        assert_eq!(
            hit_rects(&rects, quit_r.x + quit_r.width.saturating_sub(1), quit_r.y),
            Some(WelcomeHit::Quit)
        );
        assert_eq!(hit_rects(&rects, 0, 0), None);
    }

    #[test]
    fn logo_tiers_grow_with_pane() {
        assert_eq!(pick_logo(11), None);
        assert_eq!(pick_logo(14).map(count_lines), Some(7));
        assert_eq!(pick_logo(18).map(count_lines), Some(12));
        assert_eq!(pick_logo(24).map(count_lines), Some(16));
    }

    #[test]
    fn menu_is_tight_not_thirty_cols() {
        let (_, rects) = paint(80, 24, None);
        assert!(rects[0].0.width < 30, "width {}", rects[0].0.width);
        assert!(rects[0].0.width >= 10);
    }

    #[test]
    fn title_is_dock_under_logo() {
        let (buf, _) = paint(80, 24, None);
        let text = compact(&buf);
        assert!(text.contains("Dock"), "{text}");
    }

    #[test]
    fn hero_box_shows_version_and_rounded_border() {
        let (buf, rects) = paint(80, 24, None);
        let text = compact(&buf);
        assert!(text.contains('\u{256d}') || text.contains('╭'), "{text}");
        assert!(text.contains('\u{256f}') || text.contains('╯'), "{text}");
        assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
        assert!(text.contains("Cordis"), "{text}");
        assert!(
            rects[0].0.x > 20,
            "menu should sit in the right column {}",
            rects[0].0.x
        );
    }
}
