//! Picker chrome copied from grok-build/.../xai-grok-pager/src/views/picker.rs
//! (`render_floating_frame`, `render_fullscreen_frame`, `render_search_bar`,
//! `render_picker_row`, `render_close_button`). LineEditor / nucleo / embed
//! mode / fold fields are omitted.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::grok::color::dim_area;
use crate::grok::glyphs;
use crate::grok::line_utils::truncate_str;
use crate::theme::Theme;

const SEARCH_BAR_LABEL: &str = " search: ";

pub struct PickerFrame {
    pub content: Rect,
    pub close_button: Rect,
}

#[derive(Default)]
pub struct PickerHits {
    pub close_button: Rect,
    /// `(data_index, row_rect)` for visible rows.
    pub rows: Vec<(usize, Rect)>,
    /// `(kill_rect, scheduled task_id)` for `/loop` rows in the tasks pane.
    pub kill_buttons: Vec<(Rect, String)>,
}

/// A selectable leaf row (Grok non-expandable `PickerRow`).
pub struct PickerRow<'a> {
    pub label: &'a str,
    pub right_label: &'a str,
    pub selected: bool,
}

pub fn render_close_button(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    theme: &Theme,
    hovered: bool,
    bg: Option<ratatui::style::Color>,
) -> Rect {
    let text = glyphs::ballot_x_button();
    let w: u16 = 3;
    let bx = x + width.saturating_sub(w);
    let mut style = if hovered {
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.gray)
    };
    if let Some(c) = bg {
        style = style.bg(c);
    }
    buf.set_span(bx, y, &Span::styled(text, style), w);
    Rect::new(bx, y, w, 1)
}

pub fn render_floating_frame(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    close_hovered: bool,
) -> Option<PickerFrame> {
    if area.height < 8 || area.width < 30 {
        return None;
    }
    dim_area(buf, area, theme.bg_base, 0.5);
    let popup_w = ((area.width as f32 * 0.65) as u16).max(44).min(area.width);
    let popup_h = (4 + 20).min(area.height.saturating_sub(2));
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 3;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);
    Clear.render(popup_area, buf);
    let base_style = Style::default().fg(theme.text_primary).bg(theme.bg_base);
    buf.set_style(popup_area, base_style);
    let border = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.gray_dim))
        .style(base_style);
    let inner = border.inner(popup_area);
    border.render(popup_area, buf);
    if inner.height < 3 || inner.width < 10 {
        return None;
    }
    let close_button = render_close_button(
        buf,
        inner.x,
        inner.y,
        inner.width,
        theme,
        close_hovered,
        Some(theme.bg_base),
    );
    Some(PickerFrame {
        content: inner,
        close_button,
    })
}

pub struct BorderedFrame {
    pub title_row: Rect,
    pub content: Rect,
}

pub fn render_bordered_frame(
    buf: &mut Buffer,
    area: Rect,
    border_color: ratatui::style::Color,
    bg: ratatui::style::Color,
) -> Option<BorderedFrame> {
    if area.height < 5 || area.width < 10 {
        return None;
    }
    let base_style = Style::default().fg(border_color).bg(bg);
    Clear.render(area, buf);
    buf.set_style(area, Style::default().bg(bg));
    let header_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 2,
    };
    Block::default()
        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
        .border_style(base_style)
        .render(header_area, buf);
    let title_row = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    let content_area = Rect {
        x: area.x,
        y: area.y + 2,
        width: area.width,
        height: area.height.saturating_sub(2),
    };
    let content_block = Block::default()
        .borders(Borders::ALL)
        .border_style(base_style);
    let content = content_block.inner(content_area);
    content_block.render(content_area, buf);
    if let Some(cell) = buf.cell_mut((area.x, content_area.y)) {
        cell.set_char('\u{251c}');
        cell.set_style(base_style);
    }
    if let Some(cell) = buf.cell_mut((area.x + area.width.saturating_sub(1), content_area.y)) {
        cell.set_char('\u{2524}');
        cell.set_style(base_style);
    }
    if content.width == 0 || content.height == 0 {
        return None;
    }
    Some(BorderedFrame { title_row, content })
}

pub fn render_fullscreen_frame(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    title: Option<&str>,
    close_hovered: bool,
) -> Option<PickerFrame> {
    let frame = render_bordered_frame(buf, area, theme.gray_dim, theme.bg_base)?;
    if let Some(title_text) = title {
        let title_line = Line::from(vec![
            Span::styled("  ", Style::default().bg(theme.bg_base)),
            Span::styled(
                title_text,
                Style::default()
                    .fg(theme.text_primary)
                    .bg(theme.bg_base)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        buf.set_line(
            frame.title_row.x,
            frame.title_row.y,
            &title_line,
            frame.title_row.width,
        );
        let close_button = render_close_button(
            buf,
            frame.title_row.x,
            frame.title_row.y,
            frame.title_row.width,
            theme,
            close_hovered,
            Some(theme.bg_base),
        );
        Some(PickerFrame {
            content: frame.content,
            close_button,
        })
    } else {
        let extra = frame.content.y.saturating_sub(frame.title_row.y);
        let content = Rect {
            x: frame.content.x + 1,
            y: frame.title_row.y,
            width: frame.content.width.saturating_sub(2),
            height: frame.content.height + extra,
        };
        Some(PickerFrame {
            content,
            close_button: Rect::default(),
        })
    }
}

pub fn render_search_bar(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    theme: &Theme,
    query: &str,
    active: bool,
    bg: Option<ratatui::style::Color>,
) {
    let bg_style = |style: Style| -> Style {
        if let Some(c) = bg {
            style.bg(c)
        } else {
            style
        }
    };
    let label = SEARCH_BAR_LABEL;
    buf.set_line(
        x,
        y,
        &Line::from(Span::styled(
            label,
            bg_style(Style::default().fg(theme.gray)),
        )),
        width,
    );
    let label_w = label.len() as u16;
    let input_x = x + label_w;
    let input_width = width.saturating_sub(label_w) as usize;
    let cursor_limit = input_width.saturating_sub(1);
    let query_cursor = query.len();
    let prefix_width = query.width();
    let (start_byte, cursor_col) = if prefix_width <= cursor_limit {
        (0, prefix_width)
    } else {
        let mut skipped_width = 0usize;
        let mut start_byte = 0usize;
        for (index, character) in query.char_indices() {
            if index >= query_cursor || prefix_width - skipped_width <= cursor_limit {
                break;
            }
            skipped_width += character.width().unwrap_or(0);
            start_byte = index + character.len_utf8();
        }
        (start_byte, prefix_width - skipped_width)
    };
    if !query.is_empty() {
        let displayed = truncate_str(&query[start_byte..], cursor_limit);
        buf.set_span(
            input_x,
            y,
            &Span::styled(
                displayed.as_str(),
                bg_style(Style::default().fg(theme.text_primary)),
            ),
            displayed.width() as u16,
        );
    }
    if active {
        let cursor_x = input_x + (cursor_col as u16).min(cursor_limit as u16);
        if cursor_x < x + width {
            if let Some(cell) = buf.cell_mut((cursor_x, y)) {
                let cursor_fg = bg.unwrap_or(theme.bg_base);
                cell.set_style(Style::default().fg(cursor_fg).bg(theme.text_primary));
            }
        }
    }
}

pub fn render_divider(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    theme: &Theme,
    bg: Option<ratatui::style::Color>,
) {
    let style = if let Some(c) = bg {
        Style::default().fg(theme.gray_dim).bg(c)
    } else {
        Style::default().fg(theme.gray_dim)
    };
    for cx in x..x + width {
        if let Some(cell) = buf.cell_mut((cx, y)) {
            cell.set_char('\u{2500}');
            cell.set_style(style);
        }
    }
}

#[allow(dead_code)]
pub fn render_header(buf: &mut Buffer, x: u16, y: u16, width: u16, theme: &Theme, label: &str) {
    let header_style = Style::default()
        .fg(theme.gray)
        .bg(theme.bg_base)
        .add_modifier(Modifier::BOLD);
    let sep_style = Style::default().fg(theme.gray_dim).bg(theme.bg_base);
    let title = format!(" {label} ");
    let title_w = title.width();
    let remaining = (width as usize).saturating_sub(title_w);
    let sep: String = std::iter::repeat_n('\u{2500}', remaining).collect();
    let line = Line::from(vec![
        Span::styled(title, header_style),
        Span::styled(sep, sep_style),
    ]);
    buf.set_line(x, y, &line, width);
}

/// Leaf picker row: `◆` + label + right-aligned meta. Selected uses `bg_visual`.
pub fn render_picker_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    theme: &Theme,
    row: &PickerRow,
) {
    let row_bg = if row.selected {
        theme.bg_visual
    } else {
        theme.bg_base
    };
    let row_rect = Rect {
        x,
        y,
        width,
        height: 1,
    };
    buf.set_style(row_rect, Style::default().bg(row_bg));
    let label_style = if row.selected {
        Style::default()
            .fg(theme.text_primary)
            .bg(row_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_primary).bg(row_bg)
    };
    let meta_fg = theme.gray;
    let fold_width: u16 = 2;
    let trailing_pad = 1u16;
    let content_width = width.saturating_sub(fold_width + trailing_pad);
    let (max_label_width, truncated_right) = if row.right_label.is_empty() {
        (content_width as usize, String::new())
    } else {
        let gap = 2u16;
        let usable = content_width.saturating_sub(gap);
        let max_right = (usable / 2) as usize;
        let right = truncate_str(row.right_label, max_right);
        let right_cols = right.width() as u16;
        let max_label = usable.saturating_sub(right_cols) as usize;
        (max_label, right)
    };
    let truncated_label = truncate_str(row.label, max_label_width);
    buf.set_span(
        x,
        y,
        &Span::styled(
            format!("{} ", glyphs::diamond_filled()),
            Style::default().fg(theme.gray_dim).bg(row_bg),
        ),
        fold_width,
    );
    buf.set_span(
        x + fold_width,
        y,
        &Span::styled(truncated_label.as_str(), label_style),
        truncated_label.width() as u16,
    );
    if !truncated_right.is_empty() {
        let right_width = truncated_right.width() as u16;
        let right_x = x + width.saturating_sub(right_width + trailing_pad);
        buf.set_span(
            right_x,
            y,
            &Span::styled(
                truncated_right.as_str(),
                Style::default().fg(meta_fg).bg(row_bg),
            ),
            right_width,
        );
    }
}

/// Search + divider + leaf rows inside an already-drawn frame `content`.
pub fn render_picker_list(
    buf: &mut Buffer,
    content: Rect,
    theme: &Theme,
    query: &str,
    rows: &[PickerRow<'_>],
    skip_top: u16,
) -> Vec<(usize, Rect)> {
    if content.height <= skip_top + 2 || content.width < 8 {
        return Vec::new();
    }
    let search_y = content.y + skip_top;
    render_search_bar(
        buf,
        content.x,
        search_y,
        content.width,
        theme,
        query,
        true,
        Some(theme.bg_base),
    );
    render_divider(
        buf,
        content.x,
        search_y + 1,
        content.width,
        theme,
        Some(theme.bg_base),
    );
    let list_y = search_y + 2;
    let list_h = content
        .y
        .saturating_add(content.height)
        .saturating_sub(list_y);
    let mut hits = Vec::new();
    let selected = rows.iter().position(|r| r.selected).unwrap_or(0);
    let visible = list_h as usize;
    let start = selected.saturating_sub(visible.saturating_sub(1) / 2);
    let start = start.min(rows.len().saturating_sub(visible));
    for vis in 0..visible {
        let idx = start + vis;
        if idx >= rows.len() {
            break;
        }
        let y = list_y + vis as u16;
        render_picker_row(buf, content.x, y, content.width, theme, &rows[idx]);
        hits.push((
            idx,
            Rect {
                x: content.x,
                y,
                width: content.width,
                height: 1,
            },
        ));
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_bar_paints_query() {
        let theme = Theme::groknight();
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        render_search_bar(
            &mut buf,
            0,
            0,
            40,
            &theme,
            "hello",
            true,
            Some(theme.bg_base),
        );
        let line: String = (0..40)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect();
        assert!(line.contains("search:"), "{line}");
        assert!(line.contains("hello"), "{line}");
    }
}
