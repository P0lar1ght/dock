//! Ask-user overlay chrome, same radio layout as `permission_view`.
//! Option labels come from Grok `ask_user_question`; "Other" is appended
//! (Grok always offers a freeform choice). Display uses 其他.

use cordis_spine::{AskPrompt, Question};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget};
use unicode_width::UnicodeWidthChar;

use crate::grok::glyphs;
use crate::grok::picker::PickerHits;
use crate::theme::Theme;

const OTHER: &str = "其他";

pub fn labels(q: &Question) -> Vec<String> {
    let mut out: Vec<String> = q.options.iter().map(|o| o.label.clone()).collect();
    let has_other = out.iter().any(|l| {
        let t = l.trim();
        t.eq_ignore_ascii_case("other") || t == OTHER
    });
    if !has_other {
        out.push(OTHER.into());
    }
    out
}

pub fn chrome_height(prompt: &AskPrompt, width: u16) -> u16 {
    let Some(q) = prompt.questions.get(prompt.index) else {
        return 8;
    };
    let content_w = width.saturating_sub(5).max(8) as usize;
    let q_rows = wrap_line(&q.question, content_w).len().min(3) as u16;
    let n = labels(q).len() as u16;
    (2 + q_rows + 1 + n + 1).clamp(8, 16)
}

pub fn render(
    buf: &mut Buffer,
    area: Rect,
    prompt: &AskPrompt,
    selected: usize,
    picked: &[bool],
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
    let step = format!("问题 {}/{}", prompt.index + 1, prompt.questions.len().max(1));
    buf.set_line(
        content_x,
        y,
        &Line::from(Span::styled(step, title_style)),
        content_w,
    );
    y = y.saturating_add(1);

    let Some(q) = prompt.questions.get(prompt.index) else {
        return PickerHits::default();
    };
    let summary_style = Style::default().fg(theme.gray).bg(theme.bg_light);
    for line in wrap_line(&q.question, content_w as usize).into_iter().take(3) {
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

    let opts = labels(q);
    let multi = q.multi_select.unwrap_or(false);
    let sel = selected.min(opts.len().saturating_sub(1));
    let mut hits = PickerHits::default();
    for (i, label) in opts.iter().enumerate() {
        if y >= area.y + area.height {
            break;
        }
        let selected = i == sel;
        let on = picked.get(i).copied().unwrap_or(false) || (!multi && selected);
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
        let marker = if on {
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
    use cordis_spine::{Question, QuestionOption};

    #[test]
    fn appends_other_when_missing() {
        let q = Question {
            question: "Which?".into(),
            options: vec![QuestionOption {
                label: "A".into(),
                description: String::new(),
                preview: None,
                id: None,
            }],
            multi_select: None,
            id: None,
        };
        let labs = labels(&q);
        assert_eq!(labs.last().map(String::as_str), Some(OTHER));
    }
}
