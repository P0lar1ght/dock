//! Welcome screen copied from grok-build/.../xai-grok-pager/src/views/welcome
//! (braille logo + `label … shortcut` menu). Shimmer is the Grok sweep.

use std::time::Instant;

use cordis::Context;
use cordis_spine::{Sessions, SESSIONS};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use super::theme::Theme;

const LOGO: &str = include_str!("../assets/logo/logo07.txt");
const LOGO_SMALL: &str = include_str!("../assets/logo/logo05.txt");

const SMALL_LOGO_MIN_HEIGHT: u16 = 22;
const FULL_LOGO_MIN_HEIGHT: u16 = 26;

pub struct Welcome {
    ctx: Context,
    start: Instant,
}

impl Welcome {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            start: Instant::now(),
        }
    }

    pub fn empty_session(&self) -> bool {
        self.ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| s.events().is_empty())
            .unwrap_or(true)
    }
}

impl Widget for &Welcome {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let theme = Theme::current();
        buf.set_style(area, Style::default().bg(theme.bg_base));
        if area.height < 8 {
            return;
        }

        let logo = pick_logo(area.height);
        let logo_h = logo.map(count_lines).unwrap_or(0);
        let menu_h = 4u16;
        let gap = 1u16;
        let block_h = logo_h.saturating_add(gap).saturating_add(menu_h);
        let top = area.height.saturating_sub(block_h) / 3;

        let chunks = Layout::vertical([
            Constraint::Length(top),
            Constraint::Length(logo_h),
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
        render_menu(chunks[3], buf, &theme);
    }
}

fn pick_logo(window_height: u16) -> Option<&'static str> {
    if window_height < SMALL_LOGO_MIN_HEIGHT {
        None
    } else if window_height < FULL_LOGO_MIN_HEIGHT {
        Some(LOGO_SMALL)
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

fn render_menu(area: Rect, buf: &mut Buffer, theme: &Theme) {
    let items: [(&str, &str); 3] = [
        ("ctrl+w", "New session"),
        ("f3", "Resume session"),
        ("ctrl+q", "Quit"),
    ];
    let label_style = Style::default()
        .fg(theme.text_primary)
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(theme.gray_bright);
    let content_min: u16 = items
        .iter()
        .map(|(key, label)| cols(key) + cols(label) + 4)
        .max()
        .unwrap_or(0);
    let menu_width = content_min.max(30);
    let [_, menu_centered, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(menu_width.min(area.width)),
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .areas(area);

    let mut y = menu_centered.y;
    for (key, label) in items {
        if y >= menu_centered.y + menu_centered.height {
            break;
        }
        let key_width = cols(key);
        let label_len = cols(label);
        buf.set_stringn(menu_centered.x, y, label, label_len as usize, label_style);
        let key_x = menu_centered
            .x
            .saturating_add(menu_centered.width.saturating_sub(key_width));
        buf.set_stringn(key_x, y, key, key_width as usize, key_style);
        y = y.saturating_add(1);
    }
}
