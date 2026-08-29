//! Queued follow-ups above the composer.
//!
//! Copied layout from grok-build/.../xai-grok-pager/src/views/queue_pane.rs
//! (`#N` prefix, first line, `[Send now]` on the right). Dock shows `[发送]`
//! on every row while a turn is running; click sends that row now. Esc with
//! an empty composer takes the newest row back into the prompt.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::grok::line_utils::truncate_str;
use crate::session::QueuedItem;
use crate::theme::Theme;

/// Grok `MAX_QUEUE_HEIGHT`.
const MAX_QUEUE_HEIGHT: u16 = 3;
const SEND_LABEL: &str = "[发送]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueHit {
    Send(String),
    Edit(String),
}

pub fn desired_height(n: usize) -> u16 {
    (n as u16).min(MAX_QUEUE_HEIGHT)
}

pub fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

pub fn hit(hits: &[(Rect, QueueHit)], column: u16, row: u16) -> Option<QueueHit> {
    let pos = Position { x: column, y: row };
    hits.iter()
        .find(|(r, _)| r.contains(pos))
        .map(|(_, h)| h.clone())
}

pub fn paint(
    buf: &mut Buffer,
    area: Rect,
    items: &[QueuedItem],
    working: bool,
    mouse: (u16, u16),
) -> Vec<(Rect, QueueHit)> {
    let mut hits = Vec::new();
    if area.height == 0 || area.width == 0 || items.is_empty() {
        return hits;
    }
    let theme = Theme::current();
    let shown = items.len().min(MAX_QUEUE_HEIGHT as usize);
    let send_w = SEND_LABEL.width() as u16;
    for (i, item) in items.iter().take(shown).enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        let row = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };
        let style = Style::default().bg(theme.bg_base);
        for x in row.x..row.x + row.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_style(style);
            }
        }
        let prefix = format!("#{} ", i + 1);
        let prefix_w = prefix.width();
        let extra_lines = item.text.lines().count().saturating_sub(1);
        let suffix = if extra_lines == 1 {
            " (+1 line)".to_string()
        } else if extra_lines > 1 {
            format!(" (+{extra_lines} lines)")
        } else {
            String::new()
        };
        let suffix_w = suffix.width();
        let btn_gap = 1u16;
        let show_send = working && area.width > prefix_w as u16 + send_w + btn_gap;
        let reserved = if show_send { send_w + btn_gap } else { 0 };
        let text_budget = area
            .width
            .saturating_sub(prefix_w as u16)
            .saturating_sub(reserved)
            .saturating_sub(suffix_w as u16) as usize;
        let body = truncate_str(&first_line(&item.text), text_budget);
        let mut spans = vec![
            Span::styled(prefix, Style::default().fg(theme.gray)),
            Span::styled(body, Style::default().fg(theme.accent_user)),
        ];
        if extra_lines > 0 {
            spans.push(Span::styled(suffix, Style::default().fg(theme.gray)));
        }
        buf.set_line(
            area.x,
            y,
            &Line::from(spans),
            area.width.saturating_sub(reserved),
        );
        if show_send {
            let send_x = area.x + area.width.saturating_sub(send_w);
            let hovered = mouse.1 == y && mouse.0 >= send_x && mouse.0 < send_x + send_w;
            let send_style = if hovered {
                Style::default().fg(theme.text_primary)
            } else {
                Style::default().fg(theme.gray)
            };
            buf.set_line(
                send_x,
                y,
                &Line::from(Span::styled(SEND_LABEL, send_style)),
                send_w,
            );
            hits.push((
                Rect {
                    x: send_x,
                    y,
                    width: send_w,
                    height: 1,
                },
                QueueHit::Send(item.id.clone()),
            ));
            let edit_w = send_x.saturating_sub(area.x);
            if edit_w > 0 {
                hits.push((
                    Rect {
                        x: area.x,
                        y,
                        width: edit_w,
                        height: 1,
                    },
                    QueueHit::Edit(item.id.clone()),
                ));
            }
        } else {
            hits.push((row, QueueHit::Edit(item.id.clone())));
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;

    fn item(id: &str, text: &str) -> QueuedItem {
        QueuedItem {
            id: id.into(),
            text: text.into(),
        }
    }

    #[test]
    fn first_line_skips_blank() {
        assert_eq!(first_line("\n  hello  \nmore"), "hello");
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn height_caps_at_three() {
        assert_eq!(desired_height(0), 0);
        assert_eq!(desired_height(1), 1);
        assert_eq!(desired_height(8), 3);
    }

    #[test]
    fn send_hit_is_on_the_right() {
        let area = Rect::new(0, 10, 40, 1);
        let mut buf = Buffer::empty(area);
        let hits = paint(
            &mut buf,
            area,
            &[item("p1", "follow up please")],
            true,
            (0, 0),
        );
        let send = hits.iter().find_map(|(r, h)| match h {
            QueueHit::Send(id) if id == "p1" => Some(*r),
            _ => None,
        });
        let send = send.expect("send button");
        assert_eq!(send.y, 10);
        assert!(send.x + send.width == 40, "button flush right, {send:?}");
        assert_eq!(
            hit(&hits, send.x, send.y),
            Some(QueueHit::Send("p1".into()))
        );
        assert_eq!(hit(&hits, 0, 10), Some(QueueHit::Edit("p1".into())));
    }
}
