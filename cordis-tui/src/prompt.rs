//! Prompt chrome copied from grok-build/.../prompt_widget/mod.rs `draw`
//! (`╭──────────╮` / `│` / `╰─ model · flags ─╯`).
//!
//! Composer grows with content (Grok `desired_height`). Cursor editing,
//! Shift+Enter newline, bracketed paste, and `@` file search sit on the
//! pager's core input path.

use std::ops::Range;
use std::sync::{Arc, Mutex};

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::widgets::Widget;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme::Theme;
use crate::clipboard::ClipboardImage;
use crate::file_search::{self, FileSearchSnapshot};

const IMAGE_CAP: usize = 10;
const PASTE_CHIP_LINES: usize = 4;

#[derive(Clone, Debug)]
pub struct PastedImage {
    pub n: u32,
    pub mime: String,
    pub data: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

impl PastedImage {
    pub fn from_clipboard(n: u32, img: ClipboardImage) -> Self {
        Self {
            n,
            mime: img.mime,
            data: img.data,
            width: img.width,
            height: img.height,
        }
    }

    pub fn chip(&self) -> String {
        format!("[Image #{}]", self.n)
    }

    pub fn format_name(&self) -> &str {
        match self.mime.as_str() {
            "image/png" => "PNG",
            "image/jpeg" => "JPEG",
            "image/tiff" => "TIFF",
            "image/gif" => "GIF",
            "image/webp" => "WEBP",
            other => other,
        }
    }

    pub fn size_label(&self) -> String {
        format_bytes(self.data.len())
    }
}

pub struct TakenPrompt {
    pub text: String,
    pub images: Vec<PastedImage>,
}

#[derive(Default)]
struct State {
    input: String,
    /// Byte index into `input`.
    cursor: usize,
    history: Vec<String>,
    /// `None` = live buffer; `Some(i)` = recalling `history[i]`.
    history_idx: Option<usize>,
    stash: String,
    slash_selected: usize,
    file_selected: usize,
    file_dismissed: bool,
    last_at_query: String,
    chrome_info: String,
    images: Vec<PastedImage>,
    image_counter: u32,
    paste_bodies: Vec<(String, String)>,
}

impl State {
    fn clamp_cursor(&mut self) {
        if self.cursor > self.input.len() {
            self.cursor = self.input.len();
        } else if !self.input.is_char_boundary(self.cursor) {
            self.cursor = prev_boundary(&self.input, self.cursor);
        }
    }

    fn set_cursor(&mut self, i: usize) {
        self.cursor = i.min(self.input.len());
        self.clamp_cursor();
        self.file_dismissed = false;
    }
}

#[derive(Default)]
pub struct PromptWidget {
    state: Mutex<State>,
}

impl PromptWidget {
    pub fn text(&self) -> String {
        self.state.lock().unwrap().input.clone()
    }

    pub fn can_send(&self) -> bool {
        let state = self.state.lock().unwrap();
        !state.input.trim().is_empty() || !state.images.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.state.lock().unwrap().cursor
    }

    pub fn push(&self, c: char) {
        self.insert_str(&c.to_string());
    }

    pub fn insert_str(&self, s: &str) {
        let mut state = self.state.lock().unwrap();
        insert_at_cursor(&mut state, s);
    }

    pub fn set_info(&self, info: impl Into<String>) {
        self.state.lock().unwrap().chrome_info = info.into();
    }

    pub fn handle_paste(&self, text: &str) {
        let text = crate::clipboard::normalize_cr(text);
        if text.is_empty() {
            return;
        }
        let lines = text.lines().count();
        if lines >= PASTE_CHIP_LINES || text.len() > 4000 {
            let mut state = self.state.lock().unwrap();
            let input = state.input.clone();
            state.paste_bodies.retain(|(chip, _)| input.contains(chip));
            let n = lines.max(1);
            let chip = if text.len() > 4000 {
                format!("[Pasted: {}]", format_bytes(text.len()))
            } else {
                format!("[Pasted: {n} lines]")
            };
            insert_at_cursor(&mut state, &chip);
            state.paste_bodies.push((chip, text));
        } else {
            self.insert_str(&text);
        }
    }

    pub fn insert_image(&self, img: ClipboardImage) -> Result<PastedImage, String> {
        let mut state = self.state.lock().unwrap();
        if state.images.len() >= IMAGE_CAP {
            return Err(format!("最多 {IMAGE_CAP} 张图片"));
        }
        if img.width > 0 && img.height > 0 && (img.width < 8 || img.height < 8) {
            return Err(format!(
                "图片太小（{}×{}），至少 8×8",
                img.width, img.height
            ));
        }
        state.image_counter += 1;
        let pasted = PastedImage::from_clipboard(state.image_counter, img);
        let chip = format!("{} ", pasted.chip());
        insert_at_cursor(&mut state, &chip);
        state.images.push(pasted.clone());
        Ok(pasted)
    }

    pub fn preview_image(&self) -> Option<PastedImage> {
        let state = self.state.lock().unwrap();
        let cursor = state.cursor;
        for img in state.images.iter().rev() {
            let chip = img.chip();
            if let Some(at) = state.input.find(&chip) {
                if cursor >= at && cursor <= at + chip.len() + 1 {
                    return Some(img.clone());
                }
            }
        }
        state.images.last().cloned()
    }

    pub fn backspace(&self) {
        let mut state = self.state.lock().unwrap();
        if state.cursor == 0 {
            return;
        }
        if let Some(range) = chip_touching(&state.input, state.cursor, true) {
            drop_chip_at(&mut state, range);
            return;
        }
        let cursor = state.cursor;
        let from = prev_boundary(&state.input, cursor);
        state.input.replace_range(from..cursor, "");
        state.cursor = from;
        state.slash_selected = 0;
        state.file_dismissed = false;
        sync_chips(&mut state);
    }

    pub fn delete(&self) {
        let mut state = self.state.lock().unwrap();
        if state.cursor >= state.input.len() {
            return;
        }
        if let Some(range) = chip_touching(&state.input, state.cursor, false) {
            drop_chip_at(&mut state, range);
            return;
        }
        let cursor = state.cursor;
        let to = next_boundary(&state.input, cursor);
        state.input.replace_range(cursor..to, "");
        state.slash_selected = 0;
        state.file_dismissed = false;
        sync_chips(&mut state);
    }

    pub fn move_left(&self) {
        let mut state = self.state.lock().unwrap();
        let next = prev_boundary(&state.input, state.cursor);
        state.set_cursor(next);
    }

    pub fn move_right(&self) {
        let mut state = self.state.lock().unwrap();
        let next = next_boundary(&state.input, state.cursor);
        state.set_cursor(next);
    }

    pub fn move_home(&self) {
        let mut state = self.state.lock().unwrap();
        let start = line_start(&state.input, state.cursor);
        state.set_cursor(start);
    }

    pub fn move_end(&self) {
        let mut state = self.state.lock().unwrap();
        let end = line_end(&state.input, state.cursor);
        state.set_cursor(end);
    }

    pub fn move_buffer_start(&self) {
        let mut state = self.state.lock().unwrap();
        state.set_cursor(0);
    }

    pub fn move_buffer_end(&self) {
        let mut state = self.state.lock().unwrap();
        let end = state.input.len();
        state.set_cursor(end);
    }

    pub fn replace_range(&self, range: std::ops::Range<usize>, text: &str) {
        let mut state = self.state.lock().unwrap();
        let start = range.start.min(state.input.len());
        let end = range.end.min(state.input.len());
        state.input.replace_range(start..end, text);
        state.cursor = start + text.len();
        state.slash_selected = 0;
        state.file_dismissed = false;
    }

    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap();
        state.input.clear();
        state.cursor = 0;
        state.slash_selected = 0;
        state.file_selected = 0;
        state.file_dismissed = false;
        state.last_at_query.clear();
        state.images.clear();
        state.image_counter = 0;
        state.paste_bodies.clear();
    }

    pub fn take(&self) -> String {
        self.take_prompt().text
    }

    pub fn take_prompt(&self) -> TakenPrompt {
        let mut state = self.state.lock().unwrap();
        let mut text = std::mem::take(&mut state.input);
        for (chip, body) in state.paste_bodies.drain(..) {
            text = text.replace(&chip, &body);
        }
        if !text.trim().is_empty() {
            state.history.push(text.clone());
        }
        let images = std::mem::take(&mut state.images);
        state.history_idx = None;
        state.stash.clear();
        state.cursor = 0;
        state.slash_selected = 0;
        state.file_selected = 0;
        state.file_dismissed = false;
        state.last_at_query.clear();
        state.image_counter = 0;
        TakenPrompt { text, images }
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
        state.cursor = state.input.len();
        state.slash_selected = 0;
    }

    pub fn file_search_snapshot(&self) -> FileSearchSnapshot {
        let mut state = self.state.lock().unwrap();
        if let Some(ctx) = file_search::detect(&state.input, state.cursor) {
            if ctx.query != state.last_at_query {
                state.last_at_query = ctx.query;
                state.file_selected = 0;
            }
        } else {
            state.last_at_query.clear();
            state.file_dismissed = false;
        }
        file_search::snapshot(
            &state.input,
            state.cursor,
            state.file_selected,
            state.file_dismissed,
        )
    }

    pub fn file_search_move(&self, delta: i16) {
        let mut state = self.state.lock().unwrap();
        let snap = file_search::snapshot(
            &state.input,
            state.cursor,
            state.file_selected,
            state.file_dismissed,
        );
        if !snap.open || snap.matches.is_empty() {
            return;
        }
        let n = snap.matches.len() as i32;
        state.file_selected = (state.file_selected as i32 + delta as i32).rem_euclid(n) as usize;
    }

    pub fn file_search_dismiss(&self) {
        self.state.lock().unwrap().file_dismissed = true;
    }

    /// Insert the highlighted `@` path. Directories in dir-mode keep the
    /// trailing `/` so the popup stays open (Grok drill-down).
    pub fn accept_file_search(&self) -> bool {
        let snap = self.file_search_snapshot();
        let Some(hit) = snap.current().cloned() else {
            return false;
        };
        let text = self.text();
        let cursor = self.cursor();
        let Some(ctx) = file_search::detect(&text, cursor) else {
            return false;
        };
        let path = file_search::normalize_display_path(&hit.path).to_string();
        let insert = if hit.is_dir && ctx.is_dir_mode() {
            format!("{path}/")
        } else if hit.is_dir {
            format!("{path}/")
        } else {
            format!("{path} ")
        };
        self.replace_range(ctx.path_range(), &insert);
        true
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
        state.cursor = state.input.len();
        state.file_dismissed = false;
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
        state.cursor = state.input.len();
        state.file_dismissed = false;
    }

    /// Chrome (2) + content rows, capped at `max` (Grok: prompt ≤ half screen).
    pub fn desired_height(&self, width: u16, max: u16) -> u16 {
        let inner = width.saturating_sub(4).max(1) as usize;
        let text = self.text();
        let rows = visual_rows(&text, inner).len().max(1) as u16;
        (rows.saturating_add(2)).clamp(3, max.max(3))
    }

    pub fn cursor_position(&self, area: Rect) -> Option<Position> {
        if area.height < 3 || area.width < 4 {
            return None;
        }
        let inner = area.width.saturating_sub(4).max(1) as usize;
        let state = self.state.lock().unwrap();
        let rows = visual_rows(&state.input, inner);
        let body_h = area.height.saturating_sub(2).max(1) as usize;
        let cursor_row = row_for_cursor(&rows, state.cursor);
        let start = (cursor_row + 1).saturating_sub(body_h);
        let vis = cursor_row.saturating_sub(start);
        let row = rows.get(cursor_row)?;
        let col_text = &state.input[row.byte_start..state.cursor.min(row.byte_end).max(row.byte_start)];
        let col = UnicodeWidthStr::width(row.prefix) as u16 + UnicodeWidthStr::width(col_text) as u16;
        Some(Position {
            x: area.x.saturating_add(2).saturating_add(col.min(area.width.saturating_sub(4))),
            y: area.y.saturating_add(1).saturating_add(vis as u16),
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

        let content = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
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

        let body = chunks[1];
        for y in body.y..body.y + body.height {
            if let Some(cell) = buf.cell_mut((left_x, y)) {
                cell.set_char('\u{2502}');
                cell.set_style(div_style);
            }
            if let Some(cell) = buf.cell_mut((right_x, y)) {
                cell.set_char('\u{2502}');
                cell.set_style(div_style);
            }
        }

        let state = self.state.lock().unwrap();
        paint_info_line(buf, chunks[2], area, &state.chrome_info, bg, &theme, div_style);

        let inner = area.width.saturating_sub(4).max(1) as usize;
        let rows = visual_rows(&state.input, inner);
        let style = Style::default().fg(theme.text_primary).bg(bg);
        let chip_style = Style::default().fg(theme.text_primary).bg(theme.paste_bg);
        let body_h = body.height as usize;
        let cursor_row = row_for_cursor(&rows, state.cursor);
        let start = (cursor_row + 1).saturating_sub(body_h.max(1));
        let text_x = area.x.saturating_add(2);
        for vis in 0..body_h {
            let idx = start + vis;
            let y = body.y + vis as u16;
            if y >= body.y + body.height {
                break;
            }
            let Some(row) = rows.get(idx) else {
                if vis == 0 && state.input.is_empty() {
                    buf.set_stringn(text_x, y, "> ", body.width.saturating_sub(2) as usize, style);
                }
                break;
            };
            let slice = &state.input[row.byte_start..row.byte_end];
            paint_prompt_row(
                buf,
                text_x,
                y,
                body.width.saturating_sub(2),
                row.prefix,
                slice,
                style,
                chip_style,
            );
        }
    }
}

fn paint_info_line(
    buf: &mut Buffer,
    row: Rect,
    area: Rect,
    info: &str,
    bg: ratatui::style::Color,
    theme: &Theme,
    div_style: Style,
) {
    let info = info.trim();
    if info.is_empty() {
        return;
    }
    let max_w = area.width.saturating_sub(4);
    if max_w < 4 {
        return;
    }
    let label = format!(" {info} ");
    let trunc = truncate_width(&label, max_w as usize);
    let w = UnicodeWidthStr::width(trunc.as_str()) as u16;
    let x = area.x.saturating_add(1);
    let fg = crate::grok::color::blend_color(bg, theme.text_secondary, 0.6).unwrap_or(theme.gray);
    let style = Style::default().fg(fg).bg(bg);
    buf.set_stringn(x, row.y, &trunc, w as usize, style);
    let _ = div_style;
}

fn paint_prompt_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    prefix: &str,
    slice: &str,
    style: Style,
    chip_style: Style,
) {
    let line = format!("{prefix}{slice}");
    let chips = chip_ranges(&line);
    let mut col = x;
    let mut byte = 0usize;
    let end_x = x.saturating_add(width);
    for ch in line.chars() {
        if col >= end_x {
            break;
        }
        let next = byte + ch.len_utf8();
        let in_chip = chips.iter().any(|r| byte >= r.start && byte < r.end);
        let w = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        if w == 0 {
            byte = next;
            continue;
        }
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(ch);
            cell.set_style(if in_chip { chip_style } else { style });
        }
        col = col.saturating_add(w);
        byte = next;
    }
}

pub fn paint_image_card(buf: &mut Buffer, area: Rect, image: &PastedImage, theme: &Theme) {
    if area.height < 5 || area.width < 16 {
        return;
    }
    let bg = theme.paste_bg;
    let fg = theme.text_primary;
    let dim = theme.gray;
    let border = Style::default().fg(theme.prompt_border_active).bg(bg);
    let title = format!(
        " Image #{} — {} · {}x{} · {} ",
        image.n,
        image.format_name(),
        image.width,
        image.height,
        image.size_label()
    );
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_symbol(" ");
                cell.set_style(Style::default().bg(bg));
            }
        }
    }
    let left = area.x;
    let right = area.x + area.width.saturating_sub(1);
    let top = area.y;
    let bot = area.y + area.height.saturating_sub(1);
    for x in left..=right {
        if let Some(cell) = buf.cell_mut((x, top)) {
            cell.set_char(if x == left {
                '\u{250c}'
            } else if x == right {
                '\u{2510}'
            } else {
                '\u{2500}'
            });
            cell.set_style(border);
        }
        if let Some(cell) = buf.cell_mut((x, bot)) {
            cell.set_char(if x == left {
                '\u{2514}'
            } else if x == right {
                '\u{2518}'
            } else {
                '\u{2500}'
            });
            cell.set_style(border);
        }
    }
    for y in top + 1..bot {
        if let Some(cell) = buf.cell_mut((left, y)) {
            cell.set_char('\u{2502}');
            cell.set_style(border);
        }
        if let Some(cell) = buf.cell_mut((right, y)) {
            cell.set_char('\u{2502}');
            cell.set_style(border);
        }
    }
    buf.set_stringn(
        left.saturating_add(1),
        top,
        &title,
        area.width.saturating_sub(2) as usize,
        Style::default().fg(fg).bg(bg),
    );
    let rows = [
        format!(" Format: {}", image.format_name()),
        format!(" Dimensions: {} x {}", image.width, image.height),
        format!(" Size: {}", image.size_label()),
    ];
    for (i, line) in rows.iter().enumerate() {
        let y = top + 1 + i as u16;
        if y >= bot {
            break;
        }
        buf.set_stringn(
            left.saturating_add(1),
            y,
            line,
            area.width.saturating_sub(2) as usize,
            Style::default().fg(dim).bg(bg),
        );
    }
}

fn insert_at_cursor(state: &mut State, s: &str) {
    let i = state.cursor.min(state.input.len());
    state.input.insert_str(i, s);
    state.cursor = i + s.len();
    state.slash_selected = 0;
    state.file_dismissed = false;
}

fn chip_ranges(s: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            if let Some(end) = s[i..].find(']') {
                let token = &s[i..i + end + 1];
                if token.starts_with("[Image #") || token.starts_with("[Pasted:") {
                    out.push(i..i + end + 1);
                    i += end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn chip_touching(input: &str, cursor: usize, deleting_left: bool) -> Option<Range<usize>> {
    for range in chip_ranges(input) {
        if deleting_left {
            if cursor == range.end || (cursor > range.start && cursor <= range.end) {
                return Some(range);
            }
        } else if cursor >= range.start && cursor < range.end {
            return Some(range);
        }
    }
    None
}

fn drop_chip_at(state: &mut State, range: Range<usize>) {
    let chip = state.input.get(range.clone()).unwrap_or("").to_string();
    state.input.replace_range(range.clone(), "");
    state.cursor = range.start;
    state.slash_selected = 0;
    state.file_dismissed = false;
    state.images.retain(|img| img.chip() != chip);
    state.paste_bodies.retain(|(c, _)| c != &chip);
}

fn sync_chips(state: &mut State) {
    let input = state.input.clone();
    state.images.retain(|img| input.contains(&img.chip()));
    state.paste_bodies.retain(|(chip, _)| input.contains(chip));
}

fn format_bytes(n: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if n < 1024 {
        format!("{n} B")
    } else if (n as f64) < MB {
        format!("{:.1} KB", n as f64 / KB)
    } else {
        format!("{:.1} MB", n as f64 / MB)
    }
}

fn truncate_width(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

struct VisualRow {
    prefix: &'static str,
    byte_start: usize,
    byte_end: usize,
}

fn visual_rows(text: &str, content_w: usize) -> Vec<VisualRow> {
    let wrap_w = content_w.saturating_sub(2).max(1);
    let mut rows = Vec::new();
    let mut offset = 0usize;
    let parts: Vec<&str> = text.split('\n').collect();
    for (li, line) in parts.iter().enumerate() {
        let prefix = if li == 0 { "> " } else { "  " };
        for (start, end) in wrap_byte_ranges(line, wrap_w) {
            rows.push(VisualRow {
                prefix,
                byte_start: offset + start,
                byte_end: offset + end,
            });
        }
        offset += line.len();
        if li + 1 < parts.len() {
            offset += 1;
        }
    }
    if rows.is_empty() {
        rows.push(VisualRow {
            prefix: "> ",
            byte_start: 0,
            byte_end: 0,
        });
    }
    rows
}

fn wrap_byte_ranges(s: &str, width: usize) -> Vec<(usize, usize)> {
    if s.is_empty() {
        return vec![(0, 0)];
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut w = 0usize;
    for (i, ch) in s.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w > 0 && w + cw > width {
            out.push((start, i));
            start = i;
            w = cw;
        } else {
            w += cw;
        }
    }
    out.push((start, s.len()));
    out
}

fn row_for_cursor(rows: &[VisualRow], cursor: usize) -> usize {
    rows.iter()
        .rposition(|r| r.byte_start <= cursor)
        .unwrap_or(0)
}

fn prev_boundary(s: &str, i: usize) -> usize {
    if i == 0 {
        return 0;
    }
    let mut i = i - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    i + s[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(0)
}

fn line_start(s: &str, cursor: usize) -> usize {
    s[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

fn line_end(s: &str, cursor: usize) -> usize {
    s[cursor..]
        .find('\n')
        .map(|i| cursor + i)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_at_cursor_not_only_at_end() {
        let prompt = PromptWidget::default();
        prompt.insert_str("ac");
        prompt.move_left();
        prompt.push('b');
        assert_eq!(prompt.text(), "abc");
        assert_eq!(prompt.cursor(), 2);
    }

    #[test]
    fn backspace_deletes_before_cursor() {
        let prompt = PromptWidget::default();
        prompt.insert_str("ab");
        prompt.backspace();
        assert_eq!(prompt.text(), "a");
        assert_eq!(prompt.cursor(), 1);
    }

    #[test]
    fn paste_short_text_is_inline() {
        let prompt = PromptWidget::default();
        prompt.handle_paste("hello");
        assert_eq!(prompt.text(), "hello");
    }

    #[test]
    fn paste_four_lines_becomes_chip() {
        let prompt = PromptWidget::default();
        prompt.handle_paste("a\nb\nc\nd");
        assert_eq!(prompt.text(), "[Pasted: 4 lines]");
        let taken = prompt.take_prompt();
        assert_eq!(taken.text, "a\nb\nc\nd");
    }

    #[test]
    fn image_chip_and_backspace_drops_it() {
        let prompt = PromptWidget::default();
        let img = ClipboardImage::from_bytes(
            {
                let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
                png.extend_from_slice(&[0, 0, 0, 13]);
                png.extend_from_slice(b"IHDR");
                png.extend_from_slice(&32u32.to_be_bytes());
                png.extend_from_slice(&32u32.to_be_bytes());
                png.extend_from_slice(&[8, 2, 0, 0, 0]);
                png
            },
            Some("image/png"),
        )
        .unwrap();
        prompt.insert_image(img).unwrap();
        assert!(prompt.text().starts_with("[Image #1]"));
        prompt.backspace();
        prompt.backspace();
        assert!(prompt.text().is_empty());
    }
}
