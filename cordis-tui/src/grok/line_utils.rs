//! Copied from grok-build/.../xai-grok-pager-render/src/render/line_utils.rs
//! (`truncate_line`, `fit_line_to_width`, `line_to_static`, `push_owned_lines`).

use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Clone a borrowed ratatui `Line` into an owned `'static` line.
pub fn line_to_static(line: &Line<'_>) -> Line<'static> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .iter()
            .map(|s| Span {
                style: s.style,
                content: std::borrow::Cow::Owned(s.content.to_string()),
            })
            .collect(),
    }
}

/// Append owned copies of borrowed lines to `out`.
pub fn push_owned_lines(src: &[Line<'_>], out: &mut Vec<Line<'static>>) {
    for l in src {
        out.push(line_to_static(l));
    }
}

/// Walks spans left-to-right, consuming width budget. When the budget is
/// exhausted mid-span, that span is truncated and `…` is appended.
pub fn truncate_line(line: Line<'static>, max_width: usize) -> Line<'static> {
    if max_width == 0 {
        return Line::from(vec![]);
    }

    let total: usize = line.spans.iter().map(|s| s.content.width()).sum();
    if total <= max_width {
        return line;
    }

    let budget = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut out: Vec<Span<'static>> = Vec::new();

    for span in line.spans {
        let sw = span.content.width();
        if used + sw <= budget {
            used += sw;
            out.push(span);
        } else {
            let remaining = budget - used;
            if remaining > 0 {
                let truncated = take_width(&span.content, remaining);
                out.push(Span::styled(truncated, span.style));
            }
            let ellipsis_style = out.last().map(|s| s.style).unwrap_or_default();
            out.push(Span::styled("\u{2026}", ellipsis_style));
            return Line::from(out);
        }
    }

    Line::from(out)
}

/// Clip or pad a styled `Line` to exactly `width` display columns.
pub fn fit_line_to_width<'a>(line: Line<'a>, width: usize) -> Line<'a> {
    let total: usize = line.spans.iter().map(|s| s.content.width()).sum();
    if total == width {
        return line;
    }

    let Line {
        style,
        alignment,
        mut spans,
    } = line;

    if total < width {
        spans.push(Span::raw(" ".repeat(width - total)));
        return Line {
            style,
            alignment,
            spans,
        };
    }

    let mut out: Vec<Span<'a>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let sw = span.content.width();
        if used + sw <= width {
            used += sw;
            out.push(span);
            if used == width {
                break;
            }
            continue;
        }
        let remaining = width - used;
        let mut taken = String::new();
        let mut taken_width = 0usize;
        for g in span.content.graphemes(true) {
            let gw = g.width();
            if taken_width + gw > remaining {
                break;
            }
            taken_width += gw;
            taken.push_str(g);
        }
        if !taken.is_empty() {
            out.push(Span::styled(taken, span.style));
            used += taken_width;
        }
        if used < width {
            out.push(Span::raw(" ".repeat(width - used)));
        }
        break;
    }

    Line {
        style,
        alignment,
        spans: out,
    }
}

/// Truncate `s` to `max_width` display columns, appending `…` when clipped.
pub fn truncate_str(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        return s.to_string();
    }
    let budget = max_width.saturating_sub(1);
    let mut out = take_width(s, budget);
    out.push('\u{2026}');
    out
}

fn take_width(s: &str, n: usize) -> String {
    s[..byte_offset_at_width(s, n)].to_string()
}

fn byte_offset_at_width(s: &str, max_width: usize) -> usize {
    let mut width = 0;
    for (i, ch) in s.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > max_width {
            return i;
        }
        width += cw;
    }
    s.len()
}
