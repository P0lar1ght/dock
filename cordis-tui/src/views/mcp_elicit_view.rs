//! MCP elicitation overlay. Same radio chrome as ask/permission.
//! Select fields append 「其他」 in spine; typing that row submits the draft.

use cordis_spine::ElicitPrompt;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget};
use unicode_width::UnicodeWidthChar;

use crate::grok::glyphs;
use crate::grok::picker::PickerHits;
use crate::theme::Theme;

pub fn other_active(prompt: &ElicitPrompt, selected: usize, picked: &[bool]) -> bool {
    prompt.other_index == Some(selected)
        || (prompt.multi
            && prompt
                .other_index
                .is_some_and(|i| picked.get(i).copied().unwrap_or(false)))
}

pub fn needs_draft(prompt: &ElicitPrompt, selected: usize, picked: &[bool]) -> bool {
    prompt.typing || other_active(prompt, selected, picked)
}

pub fn chrome_height(prompt: &ElicitPrompt, width: u16, selected: usize, picked: &[bool]) -> u16 {
    let content_w = width.saturating_sub(5).max(8) as usize;
    let msg_rows = wrap_line(&prompt.message, content_w).len().min(3) as u16;
    let n = prompt.options.len() as u16;
    let extra = if needs_draft(prompt, selected, picked) {
        2
    } else {
        0
    };
    (2 + msg_rows + 1 + 1 + n + 1 + extra).clamp(8, 20)
}

pub fn render(
    buf: &mut Buffer,
    area: Rect,
    prompt: &ElicitPrompt,
    selected: usize,
    picked: &[bool],
    draft: &str,
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
            format!("MCP · {}", prompt.server),
            title_style,
        )),
        content_w,
    );
    y = y.saturating_add(1);

    let summary_style = Style::default().fg(theme.gray).bg(theme.bg_light);
    for line in wrap_line(&prompt.message, content_w as usize)
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
    if !prompt.heading.is_empty() && y < area.y + area.height {
        buf.set_line(
            content_x,
            y,
            &Line::from(Span::styled(
                prompt.heading.clone(),
                Style::default().fg(theme.text_primary).bg(theme.bg_light),
            )),
            content_w,
        );
        y = y.saturating_add(1);
    }
    y = y.saturating_add(1);

    let sel = selected.min(prompt.options.len().saturating_sub(1));
    let mut hits = PickerHits::default();
    for (i, label) in prompt.options.iter().enumerate() {
        if y >= area.y + area.height {
            break;
        }
        let selected_row = i == sel;
        let on = picked.get(i).copied().unwrap_or(false) || (!prompt.multi && selected_row);
        let row_bg = if selected_row {
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
        let marker = if on {
            glyphs::filled_dot()
        } else {
            glyphs::hollow_dot()
        };
        let text_style = if selected_row {
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
                Span::styled(label.clone(), text_style),
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

    if needs_draft(prompt, sel, picked) && y.saturating_add(1) < area.y + area.height {
        y = y.saturating_add(1);
        let row_bg = theme.bg_visual;
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
        let label_style = Style::default().fg(theme.gray).bg(row_bg);
        let text_style = Style::default()
            .fg(theme.text_primary)
            .bg(row_bg)
            .add_modifier(Modifier::BOLD);
        let cursor_style = Style::default().fg(theme.accent_user).bg(row_bg);
        let mut spans = vec![Span::styled("输入 ", label_style)];
        if draft.is_empty() {
            spans.push(Span::styled("具体内容", label_style));
            spans.push(Span::styled("█", cursor_style));
        } else {
            spans.push(Span::styled(draft.to_string(), text_style));
            spans.push(Span::styled("█", cursor_style));
        }
        buf.set_line(content_x, y, &Line::from(spans), content_w);
    }
    hits
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

    fn prompt() -> ElicitPrompt {
        ElicitPrompt {
            server: "local".into(),
            message: "选环境".into(),
            heading: "环境（1/1）".into(),
            options: vec!["开发".into(), "其他".into()],
            typing: false,
            multi: false,
            draft: String::new(),
            other_index: Some(1),
            selected: 0,
            picked: Vec::new(),
        }
    }

    #[test]
    fn other_row_needs_draft() {
        let p = prompt();
        assert!(!needs_draft(&p, 0, &[]));
        assert!(needs_draft(&p, 1, &[]));
    }
}
