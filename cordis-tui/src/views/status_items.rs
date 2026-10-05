//! 插件状态项（`"status.items"`）在终端里的样子：快捷键条那一行的右侧，
//! 一项一个「● 文字」，色调上色。插件给的文字只截断、不改写。

use cordis_base::view::Tone;
use cordis_spine::StatusItem;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::grok::line_utils::truncate_str;
use crate::theme::Theme;

/// 单项最多占几列（超出省略号）。
const MAX_ITEM: usize = 20;
/// 项与项之间。
const GAP: &str = "  ";

fn color(tone: Tone, theme: &Theme) -> ratatui::style::Color {
    match tone {
        Tone::Default => theme.text_secondary,
        Tone::Muted => theme.gray,
        Tone::Accent => theme.running,
        Tone::Success => theme.accent_success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.accent_error,
    }
}

/// 画成的一行（不含位置）。最多占 `max_width` 列：放不下的项整项丢掉，
/// 末尾补「+N」。
pub fn line(items: &[StatusItem], theme: &Theme, max_width: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for (i, item) in items.iter().enumerate() {
        let text = truncate_str(&item.text, MAX_ITEM);
        let piece = 2 + text.width() + if spans.is_empty() { 0 } else { GAP.len() };
        let rest = items.len() - i - 1;
        // 后面还有就给「  +N」留位置（N 两位数时多一列）。
        let more = if rest > 0 {
            format!("{GAP}+{rest}").width()
        } else {
            0
        };
        if used + piece + more > max_width {
            let left = items.len() - i;
            let tag = format!("{}+{left}", if spans.is_empty() { "" } else { GAP });
            if used + tag.width() <= max_width {
                spans.push(Span::styled(tag, Style::default().fg(theme.gray)));
            }
            break;
        }
        if !spans.is_empty() {
            spans.push(Span::raw(GAP));
        }
        let fg = color(item.tone, theme);
        spans.push(Span::styled("● ", Style::default().fg(fg)));
        spans.push(Span::styled(text, Style::default().fg(fg)));
        used += piece;
    }
    Line::from(spans)
}

/// 右对齐画进 `area` 的第一行，回实际占用的列数（给左边的快捷键条让位用）。
pub fn render(buf: &mut Buffer, area: Rect, items: &[StatusItem], theme: &Theme) -> u16 {
    if items.is_empty() || area.height == 0 {
        return 0;
    }
    let line = line(items, theme, (area.width as usize) / 2);
    let width = line.width() as u16;
    if width == 0 {
        return 0;
    }
    let x = area.x + area.width.saturating_sub(width);
    buf.set_line(x, area.y, &line, width);
    width
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn items_truncate_and_overflow_into_a_count() {
        let theme = Theme::groknight();
        let items = vec![
            StatusItem::new("a", "部署中 60%"),
            StatusItem::new("b", "CI 通过"),
            StatusItem::new("c", "一个特别特别特别长的状态文字写不下"),
        ];
        assert_eq!(
            text(&line(&items[..2], &theme, 80)),
            "● 部署中 60%  ● CI 通过"
        );
        let long = text(&line(&items[2..], &theme, 80));
        assert!(long.ends_with('…'), "{long}");
        assert_eq!(text(&line(&items, &theme, 20)), "● 部署中 60%  +2");
    }
}
