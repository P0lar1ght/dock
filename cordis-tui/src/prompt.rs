//! Prompt chrome copied from grok-build/.../prompt_widget/mod.rs `draw`
//! (`accent ┃` + `╭──────────╮` / `╰──────────╯`).
//!
//! Composer grows with content (Grok `desired_height`). Bracketed paste and
//! history recall are the pager's core input path.

use std::sync::Mutex;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use super::theme::Theme;

#[derive(Default)]
struct State {
    input: String,
    history: Vec<String>,
    /// `None` = live buffer; `Some(i)` = recalling `history[i]`.
    history_idx: Option<usize>,
    stash: String,
    slash_selected: usize,
}

#[derive(Default)]
pub struct PromptWidget {
    state: Mutex<State>,
}

impl PromptWidget {
    pub fn text(&self) -> String {
        self.state.lock().unwrap().input.clone()
    }

    pub fn push(&self, c: char) {
        let mut state = self.state.lock().unwrap();
        state.input.push(c);
        state.slash_selected = 0;
    }

    pub fn insert_str(&self, s: &str) {
        let mut state = self.state.lock().unwrap();
        state.input.push_str(s);
        state.slash_selected = 0;
    }

    pub fn backspace(&self) {
        let mut state = self.state.lock().unwrap();
        state.input.pop();
        state.slash_selected = 0;
    }

    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap();
        state.input.clear();
        state.slash_selected = 0;
    }

    pub fn take(&self) -> String {
        let mut state = self.state.lock().unwrap();
        let text = std::mem::take(&mut state.input);
        if !text.trim().is_empty() {
            state.history.push(text.clone());
        }
        state.history_idx = None;
        state.stash.clear();
        state.slash_selected = 0;
        text
    }

    pub fn slash_snapshot(&self) -> crate::slash::SlashSnapshot {
        let state = self.state.lock().unwrap();
        crate::slash::snapshot(&state.input, state.slash_selected)
    }

    pub fn slash_move(&self, delta: i16) {
        let mut state = self.state.lock().unwrap();
        let snap = crate::slash::snapshot(&state.input, state.slash_selected);
        if !snap.open || snap.matches.is_empty() {
            return;
        }
        let n = snap.matches.len() as i32;
        let next = (state.slash_selected as i32 + delta as i32).rem_euclid(n) as usize;
        state.slash_selected = next;
    }

    pub fn apply_slash_insert(&self, display: &str) {
        let mut state = self.state.lock().unwrap();
        state.input = display.to_string();
        state.slash_selected = 0;
    }

    pub fn history(&self) -> Vec<String> {
        self.state.lock().unwrap().history.clone()
    }

    pub fn apply_text(&self, text: &str) {
        self.apply_slash_insert(text);
    }

    pub fn history_prev(&self) {
        let mut state = self.state.lock().unwrap();
        if state.history.is_empty() {
            return;
        }
        match state.history_idx {
            None => {
                state.stash = state.input.clone();
                let i = state.history.len() - 1;
                state.history_idx = Some(i);
                state.input = state.history[i].clone();
            }
            Some(0) => {}
            Some(i) => {
                let i = i - 1;
                state.history_idx = Some(i);
                state.input = state.history[i].clone();
            }
        }
    }

    pub fn history_next(&self) {
        let mut state = self.state.lock().unwrap();
        let Some(i) = state.history_idx else {
            return;
        };
        if i + 1 >= state.history.len() {
            state.history_idx = None;
            state.input = std::mem::take(&mut state.stash);
        } else {
            state.history_idx = Some(i + 1);
            state.input = state.history[i + 1].clone();
        }
    }

    /// Chrome (2) + content rows, capped at `max` (Grok: prompt ≤ half screen).
    pub fn desired_height(&self, width: u16, max: u16) -> u16 {
        let inner = width.saturating_sub(4).max(1) as usize;
        let text = self.text();
        let rows = if text.is_empty() {
            1u16
        } else {
            text.split('\n')
                .map(|line| {
                    let w = UnicodeWidthStr::width(line).max(1);
                    ((w + inner - 1) / inner).max(1) as u16
                })
                .fold(0u16, u16::saturating_add)
        };
        (rows.saturating_add(2)).clamp(3, max.max(3))
    }

    /// Cursor after `> ` on the last content row.
    pub fn cursor_position(&self, area: Rect) -> Option<Position> {
        if area.height < 3 || area.width < 4 {
            return None;
        }
        let text = self.text();
        let last = text.split('\n').next_back().unwrap_or("");
        let col = 2 + UnicodeWidthStr::width(last) as u16;
        Some(Position {
            x: area
                .x
                .saturating_add(2)
                .saturating_add(col.min(area.width.saturating_sub(4))),
            y: area.y.saturating_add(area.height.saturating_sub(2)),
        })
    }
}

impl Widget for &PromptWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 4 {
            return;
        }
        let theme = Theme::current();
        let bg = theme.bg_base;
        let border_color = theme.prompt_border_active;
        buf.set_style(area, Style::default().fg(theme.text_primary).bg(bg));

        let accent_color = theme.accent_user;
        for y in area.y..area.y + area.height {
            if let Some(cell) = buf.cell_mut((area.x, y)) {
                cell.set_symbol("┃");
                cell.set_style(Style::default().fg(accent_color).bg(bg));
            }
        }

        let content = Rect {
            x: area.x + 2,
            y: area.y,
            width: area.width.saturating_sub(3),
            height: area.height,
        };
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(content);

        let div_style = Style::default().fg(border_color).bg(bg);
        let left_x = area.x;
        let right_x = area.x + area.width.saturating_sub(1);
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, chunks[0].y)) {
                let ch = if x == left_x {
                    '\u{256d}'
                } else if x == right_x {
                    '\u{256e}'
                } else {
                    '\u{2500}'
                };
                cell.set_char(ch);
                cell.set_style(div_style);
            }
            if let Some(cell) = buf.cell_mut((x, chunks[2].y)) {
                let ch = if x == left_x {
                    '\u{2570}'
                } else if x == right_x {
                    '\u{256f}'
                } else {
                    '\u{2500}'
                };
                cell.set_char(ch);
                cell.set_style(div_style);
            }
        }

        let text = self.text();
        let body = chunks[1];
        let style = Style::default().fg(theme.text_primary).bg(bg);
        if text.is_empty() {
            buf.set_stringn(body.x, body.y, "> ", body.width as usize, style);
            return;
        }
        let mut y = body.y;
        for (i, line) in text.split('\n').enumerate() {
            if y >= body.y + body.height {
                break;
            }
            let drawn = if i == 0 {
                format!("> {line}")
            } else {
                format!("  {line}")
            };
            buf.set_stringn(body.x, y, drawn, body.width as usize, style);
            y += 1;
        }
    }
}
