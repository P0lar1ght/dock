//! Permission overlay chrome copied from grok pager `views/permission_view.rs`
//! (`┃` accent, numbered radio options). ACP kinds; no MvpAgent.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget};
use unicode_width::UnicodeWidthChar;

use crate::grok::glyphs;
use crate::grok::picker::PickerHits;
use crate::theme::Theme;
use cordis_spine::{PermissionOptionKind, PermissionPrompt};

pub const OPTIONS: &[(PermissionOptionKind, &str)] = &[
    (PermissionOptionKind::AllowOnce, "允许一次"),
    (PermissionOptionKind::AllowAlways, "始终允许"),
    (PermissionOptionKind::RejectOnce, "拒绝一次"),
    (PermissionOptionKind::RejectAlways, "始终拒绝"),
];

pub fn chrome_height(prompt: &PermissionPrompt, width: u16) -> u16 {
    let content_w = width.saturating_sub(5).max(8) as usize;
    let summary_rows = wrap_line(&prompt.summary, content_w).len().min(3) as u16;
    (2 + summary_rows + 1 + OPTIONS.len() as u16 + 1).clamp(8, 16)
}

pub fn render(
    buf: &mut Buffer,
    area: Rect,
    prompt: &PermissionPrompt,
    selected: usize,
) -> PickerHits {
    if area.height == 0 || area.width == 0 {
        return PickerHits::default();
    }
    let theme = Theme::current();
    let bg = Style::default().fg(theme.text_primary).bg(theme.bg_light);
    Clear.render(area, buf);
    fill_rect(buf, area, bg);

    let accent = Style::default().fg(theme.accent_user).bg(theme.bg_light);
    for row in area.y..area.y + area.height {
        if let Some(cell) = buf.cell_mut((area.x, row)) {
            cell.set_symbol(glyphs::accent_bar());
            cell.set_style(accent);
        }
    }

    let content_x = area.x.saturating_add(3);
    let content_w = area.width.saturating_sub(5);
    let mut y = area.y.saturating_add(1);
    let title_style = Style::default()
        .fg(theme.text_primary)
        .bg(theme.bg_light)
        .add_modifier(Modifier::BOLD);
    buf.set_line(
        content_x,
        y,
        &Line::from(Span::styled(
            format!("允许使用 {}？", prompt.tool),
            title_style,
        )),
        content_w,
    );
    y = y.saturating_add(1);

    let summary_style = Style::default().fg(theme.gray).bg(theme.bg_light);
    for line in wrap_line(&prompt.summary, content_w as usize)
        .into_iter()
        .take(3)
    {
        if y >= area.y + area.height {
            break;
        }
        buf.set_line(
            content_x,
            y,
            &Line::from(Span::styled(line, summary_style)),
            content_w,
        );
        y = y.saturating_add(1);
    }
    y = y.saturating_add(1);

    let sel = selected.min(OPTIONS.len().saturating_sub(1));
    let mut hits = PickerHits::default();
    for (i, (_, label)) in OPTIONS.iter().enumerate() {
        if y >= area.y + area.height {
            break;
        }
        let selected = i == sel;
        let row_bg = if selected {
            theme.bg_visual
        } else {
            theme.bg_light
        };
        let row_rect = Rect {
            x: area.x.saturating_add(1),
            y,
            width: area.width.saturating_sub(1),
            height: 1,
        };
        fill_rect(buf, row_rect, Style::default().bg(row_bg));
        if let Some(cell) = buf.cell_mut((area.x, y)) {
            cell.set_symbol(glyphs::accent_bar());
            cell.set_style(Style::default().fg(theme.accent_user).bg(row_bg));
        }
        let num_style = Style::default().fg(theme.accent_user).bg(row_bg);
        let marker = if selected {
            glyphs::filled_dot()
        } else {
            glyphs::hollow_dot()
        };
        let text_style = if selected {
            Style::default()
                .fg(theme.text_primary)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary).bg(row_bg)
        };
        buf.set_line(
            content_x,
            y,
            &Line::from(vec![
                Span::styled(format!("{} ", i + 1), num_style),
                Span::styled(format!("({marker}) "), text_style),
                Span::styled(*label, text_style),
            ]),
            content_w,
        );
        hits.rows.push((
            i,
            Rect {
                x: content_x,
                y,
                width: content_w,
                height: 1,
            },
        ));
        y = y.saturating_add(1);
    }
    hits
}

pub fn kind_at(selected: usize) -> PermissionOptionKind {
    OPTIONS
        .get(selected.min(OPTIONS.len() - 1))
        .map(|(k, _)| *k)
        .unwrap_or(PermissionOptionKind::RejectOnce)
}

fn fill_rect(buf: &mut Buffer, area: Rect, style: Style) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_symbol(" ");
                cell.set_style(style);
            }
        }
    }
}

fn wrap_line(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;
    for ch in text.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(1);
        if current_w + w > width && !current.is_empty() {
            lines.push(current);
            current = String::new();
            current_w = 0;
        }
        current.push(ch);
        current_w += w;
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;

    #[test]
    fn options_are_chinese() {
        assert_eq!(OPTIONS[0].1, "允许一次");
        assert_eq!(OPTIONS[3].1, "始终拒绝");
    }

    #[test]
    fn fill_clears_leftover_glyphs() {
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        for y in 0..12 {
            buf.set_string(0, y, "resolver = \"2\" edition workspace", Style::default());
        }
        let prompt = PermissionPrompt {
            tool: "search_replace".into(),
            summary: "old_string / new_string".into(),
        };
        render(&mut buf, area, &prompt, 0);
        let mut painted = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                painted.push_str(buf[(x, y)].symbol());
            }
        }
        let compact: String = painted
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '┃')
            .collect();
        assert!(!compact.contains("oncene"), "{compact}");
        assert!(!compact.contains("alwaysaid"), "{compact}");
        assert!(compact.contains("允许一次"), "{compact}");
        assert!(compact.contains("始终拒绝"), "{compact}");
    }
}
