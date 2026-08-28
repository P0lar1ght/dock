//! Shortcuts bar copied from grok pager `views/shortcuts_bar.rs`.
//! Keys are already-formatted strings (no `KeyShortcut` / action registry).

use std::borrow::Cow;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct HintItem {
    pub key: Cow<'static, str>,
    pub label: Cow<'static, str>,
}

impl HintItem {
    pub fn new(key: impl Into<Cow<'static, str>>, label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
        }
    }
}

pub struct ShortcutsBar<'a> {
    hints: &'a [HintItem],
}

impl<'a> ShortcutsBar<'a> {
    pub fn new(hints: &'a [HintItem]) -> Self {
        Self { hints }
    }
}

impl Widget for ShortcutsBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }
        let theme = Theme::current();
        let bg_style = Style::default()
            .bg(theme.bg_base)
            .fg(theme.gray)
            .remove_modifier(Modifier::all());
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, area.y)) {
                cell.reset();
                cell.set_style(bg_style);
            }
        }
        let key_style = Style::default()
            .fg(theme.text_secondary)
            .bg(theme.bg_base)
            .add_modifier(Modifier::BOLD);
        let action_style = Style::default()
            .fg(theme.gray)
            .bg(theme.bg_base)
            .remove_modifier(Modifier::BOLD | Modifier::DIM);
        let sep_style = Style::default()
            .fg(theme.gray)
            .bg(theme.bg_base)
            .add_modifier(Modifier::DIM)
            .remove_modifier(Modifier::BOLD);
        let mut x = area.x;
        for (i, hint) in self.hints.iter().enumerate() {
            if i > 0 {
                let sep = Span::styled("  │  ", sep_style);
                let w = 5u16;
                if x + w > area.x + area.width {
                    break;
                }
                buf.set_span(x, area.y, &sep, w);
                x += w;
            }
            let key_w = hint.key.width() as u16;
            if x + key_w + 1 + hint.label.width() as u16 > area.x + area.width {
                break;
            }
            let key_span = Span::styled(hint.key.as_ref(), key_style);
            buf.set_span(x, area.y, &key_span, key_w);
            x += key_w;
            let colon = Span::styled(":", action_style);
            buf.set_span(x, area.y, &colon, 1);
            x += 1;
            let label = Span::styled(format!(" {}", hint.label), action_style);
            let lw = hint.label.width() as u16 + 1;
            buf.set_span(x, area.y, &label, lw);
            x += lw;
        }
    }
}
