//! Read-only title + body overlay (usage and extra slash overlays).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::grok::picker::{render_floating_frame, render_header, PickerHits};
use crate::theme::Theme;

pub fn render(buf: &mut Buffer, area: Rect, title: &str, body: &str, scroll: usize) -> PickerHits {
    let theme = Theme::current();
    let Some(frame) = render_floating_frame(buf, area, &theme, false) else {
        return PickerHits::default();
    };
    let inner = frame.content;
    if inner.height < 2 || inner.width < 10 {
        return PickerHits {
            close_button: frame.close_button,
            ..Default::default()
        };
    }
    render_header(buf, inner.x, inner.y, inner.width, &theme, title);
    let body_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };
    let lines: Vec<Line<'static>> = body
        .lines()
        .enumerate()
        .map(|(i, row)| {
            if i == 0 {
                Line::styled(
                    row.to_string(),
                    Style::default()
                        .fg(theme.text_primary)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Line::styled(row.to_string(), Style::default().fg(theme.text_primary))
            }
        })
        .collect();
    let visible: Vec<Line<'static>> = lines
        .into_iter()
        .skip(scroll)
        .take(body_area.height as usize)
        .collect();
    Paragraph::new(visible).render(body_area, buf);
    PickerHits {
        close_button: frame.close_button,
        ..Default::default()
    }
}

pub fn max_scroll(body: &str, viewport: usize) -> usize {
    let n = body.lines().count();
    n.saturating_sub(viewport.max(1))
}
