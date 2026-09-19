//! Ask-user overlay chrome: radio dots for single-select, checkbox squares for multi.
//! Option labels come from Grok `ask_user_question`; "Other" is appended
//! (Grok always offers a freeform choice). Display uses 其他.
//! Selecting 其他 requires typed notes; the wire label stays `"Other"`.

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
const OTHER_WIRE: &str = "Other";
pub const MAX_ASK_DRAFT: usize = 4096;

pub fn labels(q: &Question) -> Vec<String> {
    let mut out: Vec<String> = q.options.iter().map(|o| o.label.clone()).collect();
    let has_other = out.iter().any(|l| is_other_label(l));
    if !has_other {
        out.push(OTHER.into());
    }
    out
}

pub fn is_other_label(label: &str) -> bool {
    let t = label.trim();
    t.eq_ignore_ascii_case(OTHER_WIRE) || t == OTHER
}

pub fn wire_label(display: &str) -> String {
    if is_other_label(display) {
        OTHER_WIRE.into()
    } else {
        display.to_string()
    }
}

pub fn other_active(labels: &[String], selected: usize, picked: &[bool], multi: bool) -> bool {
    labels.get(selected).is_some_and(|l| is_other_label(l))
        || (multi
            && labels
                .iter()
                .enumerate()
                .any(|(i, l)| picked.get(i).copied().unwrap_or(false) && is_other_label(l)))
}

pub fn push_draft(draft: &mut String, cursor: &mut usize, text: &str) {
    for c in text.chars() {
        push_draft_char(draft, cursor, c);
    }
}

pub fn push_draft_char(draft: &mut String, cursor: &mut usize, c: char) {
    if c == '\n' || c == '\r' || c.is_control() {
        return;
    }
    if draft.chars().count() >= MAX_ASK_DRAFT {
        return;
    }
    let byte = char_byte_index(draft, *cursor);
    draft.insert(byte, c);
    *cursor = cursor.saturating_add(1);
}

pub fn backspace_draft(draft: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let end = char_byte_index(draft, *cursor);
    *cursor -= 1;
    let start = char_byte_index(draft, *cursor);
    draft.replace_range(start..end, "");
}

pub fn move_draft_cursor(draft: &str, cursor: &mut usize, delta: i16) {
    let len = draft.chars().count();
    let next = (*cursor as i32 + delta as i32).clamp(0, len as i32) as usize;
    *cursor = next;
}

pub fn clamp_draft_cursor(draft: &str, cursor: &mut usize) {
    *cursor = (*cursor).min(draft.chars().count());
}

fn char_byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

pub fn chrome_height(prompt: &AskPrompt, width: u16, selected: usize, picked: &[bool]) -> u16 {
    let Some(q) = prompt.questions.get(prompt.index) else {
        return 8;
    };
    let content_w = width.saturating_sub(5).max(8) as usize;
    let q_rows = wrap_line(&q.question, content_w).len().min(3) as u16;
    let opts = labels(q);
    let n = opts.len() as u16;
    let multi = q.multi_select.unwrap_or(false);
    let extra = if other_active(&opts, selected, picked, multi) {
        2
    } else {
        0
    };
    (2 + q_rows + 1 + n + 1 + extra).clamp(8, 18)
}

/// Freeform 「其他」field state passed into [`render`].
pub struct AskDraft<'a> {
    pub text: &'a str,
    pub cursor: usize,
    pub focused: bool,
}

pub fn render(
    buf: &mut Buffer,
    area: Rect,
    prompt: &AskPrompt,
    selected: usize,
    picked: &[bool],
    draft: AskDraft<'_>,
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
    let step = if prompt.questions.len() > 1 {
        format!(
            "问题 {}/{}  ← → 切换",
            prompt.index + 1,
            prompt.questions.len()
        )
    } else {
        format!(
            "问题 {}/{}",
            prompt.index + 1,
            prompt.questions.len().max(1)
        )
    };
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
    for line in wrap_line(&q.question, content_w as usize)
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
        let marker = if multi {
            if on {
                glyphs::checked_square()
            } else {
                glyphs::hollow_square()
            }
        } else if on {
            glyphs::filled_dot()
        } else {
            glyphs::hollow_dot()
        };
        let marker_span = if multi {
            format!("{marker} ")
        } else {
            format!("({marker}) ")
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
                Span::styled(marker_span, text_style),
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

    if other_active(&opts, sel, picked, multi) && y.saturating_add(1) < area.y + area.height {
        y = y.saturating_add(1);
        let row_bg = if draft.focused {
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
        let label_style = Style::default().fg(theme.gray).bg(row_bg);
        let text_style = if draft.focused {
            Style::default()
                .fg(theme.text_primary)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.gray).bg(row_bg)
        };
        let placeholder_style = Style::default().fg(theme.gray).bg(row_bg);
        let cursor_style = Style::default()
            .fg(theme.bg_light)
            .bg(theme.accent_user)
            .add_modifier(Modifier::BOLD);
        let mut spans = vec![
            Span::styled("› ", Style::default().fg(theme.accent_user).bg(row_bg)),
            Span::styled("其他 ", label_style),
        ];
        let cursor = draft.cursor.min(draft.text.chars().count());
        if draft.text.is_empty() {
            if draft.focused {
                spans.push(Span::styled("█", cursor_style));
            }
            spans.push(Span::styled("输入具体内容…", placeholder_style));
        } else if draft.focused {
            let before: String = draft.text.chars().take(cursor).collect();
            let at = draft.text.chars().nth(cursor);
            let after: String = draft.text.chars().skip(cursor.saturating_add(1)).collect();
            if !before.is_empty() {
                spans.push(Span::styled(before, text_style));
            }
            match at {
                Some(ch) => spans.push(Span::styled(ch.to_string(), cursor_style)),
                None => spans.push(Span::styled("█", cursor_style)),
            }
            if !after.is_empty() {
                spans.push(Span::styled(after, text_style));
            }
        } else {
            spans.push(Span::styled(draft.text.to_string(), text_style));
        }
        buf.set_line(content_x, y, &Line::from(spans), content_w);
        hits.draft_input = Rect {
            x: content_x,
            y,
            width: content_w,
            height: 1,
        };
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
    use cordis_spine::{AskPrompt, Question, QuestionOption};
    use ratatui::buffer::Buffer;

    fn sample(label: &str) -> Question {
        Question {
            question: "Which?".into(),
            options: vec![QuestionOption {
                label: label.into(),
                description: String::new(),
                preview: None,
                id: None,
            }],
            multi_select: None,
            id: None,
        }
    }

    #[test]
    fn appends_other_when_missing() {
        let labs = labels(&sample("A"));
        assert_eq!(labs.last().map(String::as_str), Some(OTHER));
        assert!(other_active(&labs, labs.len() - 1, &[], false));
        assert!(!other_active(&labs, 0, &[], false));
    }

    #[test]
    fn wire_label_other_is_english() {
        assert_eq!(wire_label("其他"), OTHER_WIRE);
        assert_eq!(wire_label("Other"), OTHER_WIRE);
        assert_eq!(wire_label("A"), "A");
    }

    #[test]
    fn does_not_append_when_other_present() {
        let labs = labels(&sample("Other"));
        assert_eq!(labs, vec!["Other".to_string()]);
    }

    #[test]
    fn push_draft_skips_newlines_and_caps() {
        let mut draft = String::new();
        let mut cursor = 0;
        push_draft(&mut draft, &mut cursor, "ab\ncd");
        assert_eq!(draft, "abcd");
        assert_eq!(cursor, 4);
        let mut long = "x".repeat(MAX_ASK_DRAFT);
        let mut cur = long.chars().count();
        push_draft_char(&mut long, &mut cur, 'y');
        assert_eq!(long.chars().count(), MAX_ASK_DRAFT);
    }

    #[test]
    fn draft_cursor_insert_and_backspace() {
        let mut draft = String::from("abc");
        let mut cursor = 1;
        push_draft_char(&mut draft, &mut cursor, 'X');
        assert_eq!(draft, "aXbc");
        assert_eq!(cursor, 2);
        backspace_draft(&mut draft, &mut cursor);
        assert_eq!(draft, "abc");
        assert_eq!(cursor, 1);
        move_draft_cursor(&draft, &mut cursor, -1);
        assert_eq!(cursor, 0);
        move_draft_cursor(&draft, &mut cursor, 99);
        assert_eq!(cursor, 3);
    }

    fn sample_prompt(multi: bool) -> AskPrompt {
        AskPrompt {
            questions: vec![Question {
                question: "Which?".into(),
                options: vec![
                    QuestionOption {
                        label: "Alpha".into(),
                        description: String::new(),
                        preview: None,
                        id: None,
                    },
                    QuestionOption {
                        label: "Beta".into(),
                        description: String::new(),
                        preview: None,
                        id: None,
                    },
                ],
                multi_select: Some(multi),
                id: None,
            }],
            index: 0,
            current_labels: Vec::new(),
            current_notes: None,
            max_index: 0,
        }
    }

    fn paint(
        prompt: &AskPrompt,
        selected: usize,
        picked: &[bool],
        draft: &str,
        focused: bool,
    ) -> (String, PickerHits) {
        let area = Rect::new(0, 0, 48, 14);
        let mut buf = Buffer::empty(area);
        let hits = render(
            &mut buf,
            area,
            prompt,
            selected,
            picked,
            AskDraft {
                text: draft,
                cursor: draft.chars().count(),
                focused,
            },
        );
        let mut painted = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                painted.push_str(buf[(x, y)].symbol());
            }
        }
        // Wide CJK glyphs leave blank continuation cells in the buffer.
        let compact: String = painted
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '┃')
            .collect();
        (compact, hits)
    }

    #[test]
    fn single_select_uses_radio_dots() {
        let prompt = sample_prompt(false);
        let (painted, _) = paint(&prompt, 0, &[true, false, false], "", false);
        assert!(
            painted.contains(glyphs::filled_dot()) && painted.contains(glyphs::hollow_dot()),
            "{painted}"
        );
        assert!(
            !painted.contains(glyphs::hollow_square())
                && !painted.contains(glyphs::checked_square()),
            "{painted}"
        );
    }

    #[test]
    fn multi_select_uses_checkbox_squares() {
        let prompt = sample_prompt(true);
        let (painted, _) = paint(&prompt, 0, &[true, false, false], "", false);
        assert!(
            painted.contains(glyphs::checked_square()) && painted.contains(glyphs::hollow_square()),
            "{painted}"
        );
        assert!(
            !painted.contains(glyphs::filled_dot()) && !painted.contains(glyphs::hollow_dot()),
            "{painted}"
        );
    }

    #[test]
    fn draft_caret_only_when_focused() {
        let prompt = sample_prompt(false);
        let other = labels(prompt.questions.first().unwrap()).len() - 1;
        let (focused, hits_f) = paint(&prompt, other, &vec![false; other + 1], "", true);
        let (blurred, hits_b) = paint(&prompt, other, &vec![false; other + 1], "", false);
        assert!(focused.contains('█'), "{focused}");
        assert!(!blurred.contains('█'), "{blurred}");
        assert!(blurred.contains("输入具体内容"), "{blurred}");
        assert!(hits_f.draft_input.width > 0 && hits_f.draft_input.height > 0);
        assert!(hits_b.draft_input.width > 0 && hits_b.draft_input.height > 0);
    }

    #[test]
    fn draft_hit_rect_empty_when_other_inactive() {
        let prompt = sample_prompt(false);
        let (_, hits) = paint(&prompt, 0, &[false, false, false], "", false);
        assert_eq!(hits.draft_input.width, 0);
    }
}
