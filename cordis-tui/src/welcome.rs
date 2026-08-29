//! Welcome screen copied from grok-build/.../xai-grok-pager/src/views/welcome
//! (braille logo + `label … shortcut` menu). Shimmer is the Grok sweep.
//! Menu rows keep Grok hit-rects so a left click matches the shortcut.

use std::sync::Mutex;
use std::time::Instant;

use cordis::Context;
use cordis_spine::{Sessions, SESSIONS};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use super::theme::Theme;

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

const LOGO: &str = include_str!("../assets/logo/logo16.txt");
const LOGO_MID: &str = include_str!("../assets/logo/logo12.txt");
const LOGO_SMALL: &str = include_str!("../assets/logo/logo07.txt");
const TITLE: &str = "Dock";

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
    let pos = Position {
        x: column,
        y: row,
    };
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

        let logo = pick_logo(area.height);
        let logo_h = logo.map(count_lines).unwrap_or(0);
        let title_h = 1u16;
        let menu_h = MENU.len() as u16;
        let gap = 1u16;
        let block_h = logo_h
            .saturating_add(u16::from(logo_h > 0))
            .saturating_add(title_h)
            .saturating_add(gap)
            .saturating_add(menu_h);
        let top = area.height.saturating_sub(block_h) / 2;

        let chunks = Layout::vertical([
            Constraint::Length(top),
            Constraint::Length(logo_h),
            Constraint::Length(u16::from(logo_h > 0)),
            Constraint::Length(title_h),
            Constraint::Length(gap),
            Constraint::Length(menu_h),
            Constraint::Min(0),
        ])
        .split(area);

        if let Some(logo) = logo {
            render_logo(
                chunks[1],
                buf,
                &theme,
                logo,
                self.start.elapsed().as_secs_f32(),
            );
        }
        Paragraph::new(Line::from(Span::styled(
            TITLE,
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Center)
        .render(chunks[3], buf);
        let hover = *self.hover.lock().unwrap();
        let rects = render_menu(chunks[5], buf, &theme, hover);
        *self.menu_rects.lock().unwrap() = rects;
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

fn render_logo(area: Rect, buf: &mut Buffer, theme: &Theme, logo: &str, secs: f32) {
    let lines: Vec<&str> = non_empty_lines(logo).collect();
    let rows = lines.len().max(1) as f32;
    let cols = lines
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
                let diag = (col as f32 + (rows - 1.0 - row as f32)) / (cols + rows);
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
            Line::from(spans).alignment(Alignment::Center)
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
    let label_w = MENU.iter().map(|(_, label, _)| cols(label)).max().unwrap_or(0);
    let key_w = MENU.iter().map(|(key, _, _)| cols(key)).max().unwrap_or(0);
    let menu_width = label_w.saturating_add(2).saturating_add(key_w).min(area.width);
    let [_, menu_centered, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(menu_width),
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .areas(area);

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
        let logo = pick_logo(area.height);
        let logo_h = logo.map(count_lines).unwrap_or(0);
        let title_h = 1u16;
        let menu_h = MENU.len() as u16;
        let gap = 1u16;
        let block_h = logo_h
            .saturating_add(u16::from(logo_h > 0))
            .saturating_add(title_h)
            .saturating_add(gap)
            .saturating_add(menu_h);
        let top = area.height.saturating_sub(block_h) / 2;
        let chunks = Layout::vertical([
            Constraint::Length(top),
            Constraint::Length(logo_h),
            Constraint::Length(u16::from(logo_h > 0)),
            Constraint::Length(title_h),
            Constraint::Length(gap),
            Constraint::Length(menu_h),
            Constraint::Min(0),
        ])
        .split(area);
        Paragraph::new(Line::from(Span::styled(
            TITLE,
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD),
        )))
        .alignment(Alignment::Center)
        .render(chunks[3], &mut buf);
        let rects = render_menu(chunks[5], &mut buf, &theme, hover);
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
}
