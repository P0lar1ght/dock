//! Dropdown chrome copied from grok pager `views/slash_dropdown.rs`
//! (aligned label column, `❯` on the selected row, `bg_visual` highlight).
//! Tags, provenance badges, nucleo indices, and SafeBuf are omitted.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::grok::glyphs;
use crate::theme::Theme;

use super::{SlashPick, SlashSnapshot, MAX_VISIBLE_SUGGESTIONS};

const LABEL_CAP: usize = 40;
const LABEL_DESC_GAP: usize = 2;
const PREFIX_W: usize = 2;

#[derive(Debug, Clone)]
pub struct SuggestionRow {
    pub display: String,
    pub description: String,
    pub pick: SlashPick,
}

pub fn desired_item_rows(items: &[SuggestionRow], _items_width: u16) -> u16 {
    (items.len().min(MAX_VISIBLE_SUGGESTIONS)) as u16
}

fn compute_label_column_w(items: &[SuggestionRow], content_w: usize) -> usize {
    let budget = (content_w * 3 / 5).min(LABEL_CAP);
    let max_display_w = items
        .iter()
        .map(|r| r.display.width())
        .filter(|w| *w <= LABEL_CAP)
        .max()
        .unwrap_or(0);
    max_display_w.min(budget)
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(1);
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > budget {
            break;
        }
        w += cw;
        out.push(ch);
    }
    out.push('\u{2026}');
    out
}

fn build_item_lines(
    item: &SuggestionRow,
    is_selected: bool,
    label_col_w: usize,
    theme: &Theme,
) -> Line<'static> {
    let bold = if is_selected {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    let row_bg = if is_selected {
        theme.bg_visual
    } else {
        theme.bg_light
    };
    let normal_style = Style::default()
        .fg(theme.text_primary)
        .bg(row_bg)
        .add_modifier(bold);
    let desc_style = Style::default().fg(theme.gray).bg(row_bg);
    let bg_style = Style::default().bg(row_bg);
    let prefix = if is_selected {
        glyphs::prompt_arrow()
    } else {
        "  "
    };
    let label = truncate_str(&item.display, label_col_w);
    let pad = " ".repeat(label_col_w.saturating_sub(label.width()));
    let gap = " ".repeat(LABEL_DESC_GAP);
    Line::from(vec![
        Span::styled(
            prefix.to_string(),
            if is_selected { normal_style } else { bg_style },
        ),
        Span::styled(format!("{label}{pad}{gap}"), normal_style),
        Span::styled(item.description.clone(), desc_style),
    ])
    .style(Style::default().bg(row_bg))
}

/// Render slash rows into `area` (no chrome). Copied layout from Grok `render_dropdown`.
pub fn render_dropdown(buf: &mut Buffer, area: Rect, snap: &SlashSnapshot, theme: &Theme) {
    if area.height == 0 || area.width < 4 || !snap.open {
        return;
    }
    let items = &snap.matches;
    let selected = snap.selected.min(items.len().saturating_sub(1));
    let row_w = area.width as usize;
    let label_col_w = compute_label_column_w(items, row_w.saturating_sub(PREFIX_W));
    let visible = area.height as usize;
    let start = selected.saturating_sub(visible.saturating_sub(1) / 2);
    let start = start.min(items.len().saturating_sub(visible));
    buf.set_style(area, Style::default().bg(theme.bg_light));
    for vis in 0..visible {
        let idx = start + vis;
        if idx >= items.len() {
            break;
        }
        let line = build_item_lines(&items[idx], idx == selected, label_col_w, theme);
        let y = area.y + vis as u16;
        let row = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        buf.set_style(row, line.style);
        line.render(row, buf);
    }
}
