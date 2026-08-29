//! In-app mouse text selection for scrollback.
//!
//! Adapted from grok-build usage_modal drag-select (`PendingPress` → `TextDrag`
//! after a 1-cell move, copy on mouse-up) plus grapheme column helpers from
//! `scrollback/types.rs`. Full scrollback `ResolvedSelectionModel` is too heavy
//! for cordis's plain `Vec<Line>` frame; this matches the painted cells.

use std::ops::Range;

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

/// Left-button press before the movement threshold promotes a drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingPress {
    pub start_col: u16,
    pub start_row: u16,
    pub endpoint: TextEndpoint,
}

/// Display-column endpoints into plain lines; copy happens on mouse-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextDrag {
    pub anchor: TextEndpoint,
    pub head: TextEndpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextEndpoint {
    pub line_idx: usize,
    pub col: u16,
}

impl TextDrag {
    pub fn ordered(self) -> (TextEndpoint, TextEndpoint) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn is_non_empty(self) -> bool {
        let (s, e) = self.ordered();
        s != e
    }
}

/// Same 1-cell threshold as grok scrollback `drag_threshold_exceeded`.
pub fn drag_threshold_exceeded(pending: &PendingPress, col: u16, row: u16) -> bool {
    pending.start_col.abs_diff(col) >= 1 || pending.start_row.abs_diff(row) >= 1
}

pub fn apply_selection_highlight(theme: &Theme, cell: &mut Cell) {
    let band = theme.text_primary;
    if band == Color::Reset || theme.bg_base == Color::Reset {
        cell.modifier.insert(Modifier::REVERSED);
        return;
    }
    cell.modifier.remove(Modifier::REVERSED);
    cell.set_fg(theme.bg_base);
    cell.set_bg(band);
}

/// Map screen `(column, row)` into a line/col endpoint for `plain_lines`.
pub fn endpoint_at(
    plain_lines: &[String],
    content: Rect,
    scroll: u16,
    column: u16,
    row: u16,
) -> Option<TextEndpoint> {
    if content.width == 0 || content.height == 0 || plain_lines.is_empty() {
        return None;
    }
    if row < content.y || row >= content.y.saturating_add(content.height) {
        return None;
    }
    if column < content.x || column >= content.x.saturating_add(content.width) {
        // Still allow a hit on the row; clamp col below.
    }
    let visible_row = row.saturating_sub(content.y) as usize;
    let line_idx = (scroll as usize).saturating_add(visible_row);
    if line_idx >= plain_lines.len() {
        return None;
    }
    let text = &plain_lines[line_idx];
    let line_w = str_display_cells(text).min(u16::MAX as usize) as u16;
    let col = column.saturating_sub(content.x).min(line_w);
    Some(TextEndpoint { line_idx, col })
}

/// Clamp pointer into `content` then resolve an endpoint (for drag head tracking
/// when the cursor leaves the pane).
pub fn endpoint_clamped(
    plain_lines: &[String],
    content: Rect,
    scroll: u16,
    column: u16,
    row: u16,
) -> Option<TextEndpoint> {
    if content.width == 0 || content.height == 0 {
        return None;
    }
    let max_c = content.x.saturating_add(content.width.saturating_sub(1));
    let max_r = content.y.saturating_add(content.height.saturating_sub(1));
    let cc = column.clamp(content.x, max_c);
    let rr = row.clamp(content.y, max_r);
    endpoint_at(plain_lines, content, scroll, cc, rr)
}

pub fn text_for_drag(drag: TextDrag, lines: &[String], panel_width: u16) -> Option<String> {
    let (start, end) = drag.ordered();
    if start.line_idx >= lines.len() {
        return None;
    }
    let mut out = String::new();
    let mut wrote_any = false;
    let last = end.line_idx.min(lines.len().saturating_sub(1));
    for (idx, text) in lines.iter().enumerate().take(last + 1).skip(start.line_idx) {
        let Some((lo, hi)) = selection_cols(drag, idx, text, panel_width) else {
            continue;
        };
        let slice = slice_display_cols(text, lo, hi);
        if wrote_any {
            out.push('\n');
        }
        out.push_str(&slice);
        wrote_any = true;
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

pub fn paint_text_drag(
    drag: TextDrag,
    plain_lines: &[String],
    content: Rect,
    scroll: u16,
    buf: &mut Buffer,
    theme: &Theme,
) {
    if !drag.is_non_empty() || content.width == 0 || content.height == 0 {
        return;
    }
    let (start, end) = drag.ordered();
    let scroll = scroll as usize;
    for idx in start.line_idx..=end.line_idx {
        let Some(text) = plain_lines.get(idx) else {
            break;
        };
        let Some(visible_row) = idx.checked_sub(scroll) else {
            continue;
        };
        if visible_row >= content.height as usize {
            break;
        }
        let Some((lo, hi)) = selection_cols(drag, idx, text, content.width) else {
            continue;
        };
        let screen_y = content.y + visible_row as u16;
        let x_lo = (content.x + lo).min(content.x + content.width);
        let x_hi = (content.x + hi).min(content.x + content.width);
        for x in x_lo..x_hi {
            if let Some(cell) = buf.cell_mut((x, screen_y)) {
                apply_selection_highlight(theme, cell);
            }
        }
    }
}

fn col_at_char_start(text: &str, col: u16) -> u16 {
    grapheme_cells_at(text, col).map_or(col, |cells| cells.start)
}

/// Display-column range `[lo, hi)` for one line of a drag, clamped to the panel
/// width so copy and highlight cover the same characters.
fn selection_cols(
    drag: TextDrag,
    line_idx: usize,
    text: &str,
    panel_width: u16,
) -> Option<(u16, u16)> {
    let (start, end) = drag.ordered();
    if line_idx < start.line_idx || line_idx > end.line_idx {
        return None;
    }
    let line_w = (str_display_cells(text).min(u16::MAX as usize) as u16).min(panel_width);
    let (raw_lo, raw_hi) = if start.line_idx == end.line_idx {
        (
            col_at_char_start(text, start.col),
            col_past_grapheme(text, end.col),
        )
    } else if line_idx == start.line_idx {
        (col_at_char_start(text, start.col), line_w)
    } else if line_idx == end.line_idx {
        (0, col_past_grapheme(text, end.col))
    } else {
        (0, line_w)
    };
    let lo = raw_lo.min(line_w);
    let hi = raw_hi.min(line_w);
    if hi < lo {
        return None;
    }
    // Blank lines yield hi == lo == 0; keep them so multi-line copy preserves newlines.
    if hi == lo && line_w > 0 {
        return None;
    }
    Some((lo, hi))
}

/// Slice `text` to the graphemes overlapping the display-column range `[start, end)`.
pub fn slice_display_cols(text: &str, start: u16, end: u16) -> String {
    if start >= end || text.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let mut col = 0u16;

    for grapheme in text.graphemes(true) {
        let width = grapheme_width(grapheme) as u16;
        let next_col = col.saturating_add(width);

        if width == 0 {
            if col >= start && col < end {
                out.push_str(grapheme);
            }
            continue;
        }
        if next_col <= start {
            col = next_col;
            continue;
        }
        if col >= end {
            break;
        }
        out.push_str(grapheme);
        col = next_col;
    }

    out
}

/// Display-cell range of the grapheme covering `col`, or `None` past the last grapheme.
pub fn grapheme_cells_at(text: &str, col: u16) -> Option<Range<u16>> {
    let mut c = 0u16;

    for grapheme in text.graphemes(true) {
        let width = grapheme_width(grapheme) as u16;
        if width == 0 {
            continue;
        }

        let next = c.saturating_add(width);
        if next > col {
            return Some(c..next);
        }
        c = next;
    }

    None
}

/// Exclusive display column just past the grapheme occupying `col`.
pub fn col_past_grapheme(text: &str, col: u16) -> u16 {
    match grapheme_cells_at(text, col) {
        Some(cells) => cells.end,
        None => u16::try_from(str_display_cells(text)).unwrap_or(u16::MAX),
    }
}

fn grapheme_width(grapheme: &str) -> usize {
    UnicodeWidthStr::width(grapheme)
}

fn str_display_cells(s: &str) -> usize {
    s.graphemes(true).map(grapheme_width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_slice_and_order() {
        let lines = vec!["hello world".to_string()];
        // head.col is the last selected cell; col_past_grapheme includes that grapheme.
        let drag = TextDrag {
            anchor: TextEndpoint {
                line_idx: 0,
                col: 0,
            },
            head: TextEndpoint {
                line_idx: 0,
                col: 4,
            },
        };
        assert_eq!(text_for_drag(drag, &lines, 80).as_deref(), Some("hello"));
    }

    #[test]
    fn blank_line_keeps_newline() {
        let lines = vec!["".to_string(), "Title:".to_string()];
        let drag = TextDrag {
            anchor: TextEndpoint {
                line_idx: 0,
                col: 0,
            },
            head: TextEndpoint {
                line_idx: 1,
                col: 5,
            },
        };
        assert_eq!(text_for_drag(drag, &lines, 80).as_deref(), Some("\nTitle:"));
    }

    #[test]
    fn reverse_drag_orders() {
        let lines = vec!["abcdef".to_string()];
        let drag = TextDrag {
            anchor: TextEndpoint {
                line_idx: 0,
                col: 4,
            },
            head: TextEndpoint {
                line_idx: 0,
                col: 1,
            },
        };
        assert_eq!(text_for_drag(drag, &lines, 80).as_deref(), Some("bcde"));
    }
}
