//! StatusBar widget - displays context info at the top.

//! Copied from grok-build/crates/codegen/xai-grok-pager/src/views/status_bar.rs

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use super::theme::Theme;

/// Status bar showing context information.
///
/// Displays: token count, current turn, view mode, etc.
/// Respects layout: first 3 cols and last 2 cols are empty.
pub struct StatusBar<'a> {
    /// Left-aligned content (e.g., "Context: 5.2k tokens")
    pub left: &'a str,
    /// Center content (e.g., "Turn 2/3")
    pub center: Option<&'a str>,
    /// Right-aligned content (e.g., view mode indicator)
    pub right: Option<&'a str>,
}

impl<'a> StatusBar<'a> {
    /// Create a new status bar with left content.
    pub fn new(left: &'a str) -> Self {
        Self {
            left,
            center: None,
            right: None,
        }
    }

    /// Add center content.
    pub fn center(mut self, text: &'a str) -> Self {
        self.center = Some(text);
        self
    }

    /// Add right content.
    pub fn right(mut self, text: &'a str) -> Self {
        self.right = Some(text);
        self
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let theme = Theme::current();

        // Layout: outer block already has 2-char horizontal padding
        // No additional margins needed
        let left_margin = 0u16;
        let right_margin = 0u16;
        let content_x = area.x + left_margin;
        let content_width = area.width.saturating_sub(left_margin + right_margin);

        if content_width < 10 {
            return;
        }

        let style = Style::default().fg(theme.gray).bg(theme.bg_base);

        // Fill background (the whole row)
        buf.set_style(area, Style::default().bg(theme.bg_base));

        // Left content
        let left_w = UnicodeWidthStr::width(self.left) as u16;
        let left_span = Span::styled(self.left, style);
        buf.set_span(content_x, area.y, &left_span, content_width);

        if let Some(center) = self.center {
            let center_width = UnicodeWidthStr::width(center) as u16;
            let center_x = content_x + (content_width.saturating_sub(center_width)) / 2;
            if center_x > content_x + left_w + 2 {
                let center_span = Span::styled(center, style);
                buf.set_span(center_x, area.y, &center_span, center_width);
            }
        }

        if let Some(right) = self.right {
            let right_width = UnicodeWidthStr::width(right) as u16;
            let right_x = content_x + content_width.saturating_sub(right_width);
            let right_span = Span::styled(right, style);
            buf.set_span(right_x, area.y, &right_span, right_width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    #[test]
    fn chinese_right_aligns_by_display_width() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        StatusBar::new("cwd").right("空闲").render(area, &mut buf);
        // Display width 4, so the pair starts at x=16 — not byte-len 6 at x=14.
        assert_eq!(buf[(16, 0)].symbol(), "空");
        assert_eq!(buf[(18, 0)].symbol(), "闲");
        assert_eq!(UnicodeWidthStr::width("空闲"), 4);
        assert_eq!("空闲".len(), 6);
    }
}
