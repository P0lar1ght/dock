//! Ask-user overlay chrome: radio dots for single-select, checkbox squares for multi.
//! Option labels come from Grok `ask_user_question`; "Other" is appended
//! (Grok always offers a freeform choice). Display uses 其他.
//! Selecting 其他 requires typed notes; the wire label stays `"Other"`.

use cordis_spine::{AskPrompt, Question};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::grok::glyphs;
use crate::grok::picker::PickerHits;
use crate::theme::Theme;

const OTHER: &str = "其他";
const OTHER_WIRE: &str = "Other";
pub const MAX_ASK_DRAFT: usize = 4096;
/// 「其他」输入框占的行数：上边框 + 输入行 + 下边框。
const DRAFT_BOX_ROWS: u16 = 3;

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
    // 「其他」字段是个三行圆角框（上边框 / 输入行 / 下边框），前面再留一行空隙。
    let extra = if other_active(&opts, selected, picked, multi) {
        DRAFT_BOX_ROWS + 1
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
        // 单选的实心点**只**跟高亮走。掺进 `picked` 会让按过空格的行都留着一个
        // 点，屏幕上像是多选了，而 `accept_ask` 单选时只认 `selected`。
        let on = if multi {
            picked.get(i).copied().unwrap_or(false)
        } else {
            selected
        };
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

    if other_active(&opts, sel, picked, multi) {
        let rows_left = (area.y + area.height).saturating_sub(y.saturating_add(1));
        if rows_left >= DRAFT_BOX_ROWS {
            y = y.saturating_add(1);
            draw_draft_box(
                buf,
                Rect {
                    x: area.x.saturating_add(1),
                    y,
                    width: area.width.saturating_sub(2),
                    height: DRAFT_BOX_ROWS,
                },
                &draft,
                &theme,
                &mut hits,
            );
        } else if rows_left >= 1 {
            // 终端太矮，框放不下：退回单行，至少让人还能打字。
            y = y.saturating_add(1);
            draw_draft_line(
                buf,
                Rect {
                    x: content_x,
                    y,
                    width: content_w,
                    height: 1,
                },
                &draft,
                &theme,
                &mut hits,
            );
        }
    }
    hits
}

/// 「其他」输入框：跟输入栏同一套圆角边框语言，聚焦时走 `prompt_border_active`。
///
/// 焦点不能只靠底色区分——`bg_visual` 与 `bg_light` 在深色主题里只差 8/255，
/// 截图上根本分不出来。
fn draw_draft_box(
    buf: &mut Buffer,
    area: Rect,
    draft: &AskDraft<'_>,
    theme: &Theme,
    hits: &mut PickerHits,
) {
    let bg = theme.bg_light;
    let border_color = if draft.focused {
        theme.prompt_border_active
    } else {
        theme.prompt_border
    };
    let border = Style::default().fg(border_color).bg(bg);
    let left = area.x;
    let right = area.x + area.width.saturating_sub(1);
    let bottom = area.y + area.height.saturating_sub(1);

    fill_rect(buf, area, Style::default().bg(bg));
    for x in area.x..area.x + area.width {
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.set_char(if x == left {
                '\u{256d}'
            } else if x == right {
                '\u{256e}'
            } else {
                '\u{2500}'
            });
            cell.set_style(border);
        }
        if let Some(cell) = buf.cell_mut((x, bottom)) {
            cell.set_char(if x == left {
                '\u{2570}'
            } else if x == right {
                '\u{256f}'
            } else {
                '\u{2500}'
            });
            cell.set_style(border);
        }
    }
    let field_y = area.y.saturating_add(1);
    for x in [left, right] {
        if let Some(cell) = buf.cell_mut((x, field_y)) {
            cell.set_char('\u{2502}');
            cell.set_style(border);
        }
    }
    // 标题压在上边框上，跟输入栏底边的 `╰─ model ─╯` 一个写法。
    let title = Line::from(vec![
        Span::styled("\u{2500} ", border),
        Span::styled(
            OTHER,
            Style::default()
                .fg(if draft.focused {
                    theme.accent_user
                } else {
                    theme.gray
                })
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" \u{2500}", border),
    ]);
    if area.width > 10 {
        buf.set_line(
            left.saturating_add(1),
            area.y,
            &title,
            area.width.saturating_sub(2),
        );
    }

    let field = Rect {
        x: left.saturating_add(2),
        y: field_y,
        width: area.width.saturating_sub(4),
        height: 1,
    };
    draw_draft_field(buf, field, draft, theme, hits);
}

/// 放不下框时的单行退化版本。
fn draw_draft_line(
    buf: &mut Buffer,
    area: Rect,
    draft: &AskDraft<'_>,
    theme: &Theme,
    hits: &mut PickerHits,
) {
    let bg = theme.bg_light;
    fill_rect(buf, area, Style::default().bg(bg));
    let prefix = Line::from(vec![
        Span::styled("\u{203a} ", Style::default().fg(theme.accent_user).bg(bg)),
        Span::styled(format!("{OTHER} "), Style::default().fg(theme.gray).bg(bg)),
    ]);
    buf.set_line(area.x, area.y, &prefix, area.width);
    let used = 2 + OTHER.width() as u16 + 1;
    draw_draft_field(
        buf,
        Rect {
            x: area.x.saturating_add(used),
            y: area.y,
            width: area.width.saturating_sub(used),
            height: 1,
        },
        draft,
        theme,
        hits,
    );
}

/// 画文本 / 占位，并把真实光标该落的格子记进 `hits`。
fn draw_draft_field(
    buf: &mut Buffer,
    field: Rect,
    draft: &AskDraft<'_>,
    theme: &Theme,
    hits: &mut PickerHits,
) {
    if field.width == 0 {
        return;
    }
    let bg = theme.bg_light;
    let text_style = if draft.focused {
        Style::default().fg(theme.text_primary).bg(bg)
    } else {
        Style::default().fg(theme.gray).bg(bg)
    };
    let placeholder_style = Style::default().fg(theme.gray).bg(bg);
    if draft.text.is_empty() {
        buf.set_line(
            field.x,
            field.y,
            &Line::from(Span::styled("输入具体内容…", placeholder_style)),
            field.width,
        );
        if draft.focused {
            hits.draft_caret = Some(Position::new(field.x, field.y));
        }
    } else {
        let cursor = if draft.focused { draft.cursor } else { 0 };
        let (visible, caret_col) = draft_window(draft.text, cursor, field.width as usize);
        buf.set_line(
            field.x,
            field.y,
            &Line::from(Span::styled(visible, text_style)),
            field.width,
        );
        if draft.focused {
            hits.draft_caret = Some(Position::new(field.x.saturating_add(caret_col), field.y));
        }
    }
    hits.draft_input = field;
}

/// 字段装不下时把可见窗口往右滑，让光标始终留在框内。
///
/// 真实光标一旦跑出字段，输入法的候选条就又锚到别处去了——这正是要修的那个偏移。
/// 返回 `(可见文本, 光标相对字段左沿的列偏移)`。
fn draft_window(text: &str, cursor: usize, field_w: usize) -> (String, u16) {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let width_of = |cs: &[char]| -> usize { cs.iter().filter_map(|c| c.width()).sum() };
    let mut start = 0usize;
    // 光标自己也要占一格，所以留一格余量。
    while start < cursor && width_of(&chars[start..cursor]).saturating_add(1) > field_w {
        start += 1;
    }
    let mut visible = String::new();
    let mut used = 0usize;
    for c in &chars[start..] {
        let cw = c.width().unwrap_or(0);
        if used + cw > field_w {
            break;
        }
        visible.push(*c);
        used += cw;
    }
    (visible, width_of(&chars[start..cursor]) as u16)
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

    /// 单选任何时候都只该有一个实心点。空格按过几行就点亮几行（`picked` 掺进了
    /// 单选的显示），看着像多选——虽然 `accept_ask` 只取 `selected`。
    #[test]
    fn single_select_shows_exactly_one_filled_dot() {
        let prompt = sample_prompt(false);
        let picked = vec![true, true, true];
        let (painted, _) = paint(&prompt, 1, &picked, "", false);
        assert_eq!(
            painted.matches(glyphs::filled_dot()).count(),
            1,
            "{painted}"
        );
        assert_eq!(
            painted.matches(glyphs::hollow_dot()).count(),
            picked.len() - 1,
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

    /// 聚焦才报真实光标位置，且必须落在输入框里——`frame.rs` 拿它去调
    /// `set_cursor_position`，报到框外输入法候选条又要飘。
    #[test]
    fn draft_caret_only_when_focused() {
        let prompt = sample_prompt(false);
        let other = labels(prompt.questions.first().unwrap()).len() - 1;
        let (focused, hits_f) = paint(&prompt, other, &vec![false; other + 1], "", true);
        let (blurred, hits_b) = paint(&prompt, other, &vec![false; other + 1], "", false);

        let caret = hits_f.draft_caret.expect("聚焦时要给出真实光标位置");
        assert!(
            hits_f.draft_input.contains(caret),
            "光标 {caret:?} 落在输入框 {:?} 外",
            hits_f.draft_input
        );
        assert!(hits_b.draft_caret.is_none(), "失焦不该占用终端光标");

        assert!(focused.contains("输入具体内容"), "{focused}");
        assert!(blurred.contains("输入具体内容"), "{blurred}");
        assert!(hits_f.draft_input.width > 0 && hits_f.draft_input.height > 0);
        assert!(hits_b.draft_input.width > 0 && hits_b.draft_input.height > 0);
    }

    /// 光标要跟着字走：打了字之后不能还停在字段左沿。
    #[test]
    fn draft_caret_follows_the_text() {
        let prompt = sample_prompt(false);
        let other = labels(prompt.questions.first().unwrap()).len() - 1;
        let (_, empty) = paint(&prompt, other, &vec![false; other + 1], "", true);
        let (_, typed) = paint(&prompt, other, &vec![false; other + 1], "ab", true);
        let x0 = empty.draft_caret.unwrap().x;
        let x1 = typed.draft_caret.unwrap().x;
        assert_eq!(x1, x0 + 2, "两个半角字符应让光标右移两列");

        // CJK 占两列，光标偏移要按显示宽度算，不是按字符数。
        let (_, cjk) = paint(&prompt, other, &vec![false; other + 1], "中", true);
        assert_eq!(cjk.draft_caret.unwrap().x, x0 + 2);
    }

    /// 聚焦 / 失焦的边框要用两支不同的颜色——只靠底色分不出来（深色主题里
    /// `bg_visual` 与 `bg_light` 只差 8/255，截图上看着一模一样）。
    #[test]
    fn draft_box_border_follows_focus() {
        let _g = crate::theme::test_guard();
        let theme = Theme::current();
        let prompt = sample_prompt(false);
        let other = labels(prompt.questions.first().unwrap()).len() - 1;
        let picked = vec![false; other + 1];

        let corner = |focused: bool| {
            let area = Rect::new(0, 0, 48, 14);
            let mut buf = Buffer::empty(area);
            render(
                &mut buf,
                area,
                &prompt,
                other,
                &picked,
                AskDraft {
                    text: "",
                    cursor: 0,
                    focused,
                },
            );
            // 找上边框左上角那颗圆角。
            for y in area.y..area.y + area.height {
                for x in area.x..area.x + area.width {
                    if buf[(x, y)].symbol() == "\u{256d}" {
                        return buf[(x, y)].style().fg;
                    }
                }
            }
            None
        };

        assert_eq!(corner(true), Some(theme.prompt_border_active));
        assert_eq!(corner(false), Some(theme.prompt_border));
        assert_ne!(theme.prompt_border, theme.prompt_border_active);
    }

    /// 长文本要把窗口往右滑，光标不能溢出字段。
    #[test]
    fn draft_window_keeps_the_caret_inside() {
        let (visible, col) = draft_window("abcdefghij", 10, 5);
        assert!(col < 5, "光标列 {col} 应留在宽 5 的字段里");
        assert!(visible.chars().count() <= 5, "{visible}");
        assert!(visible.ends_with('j'), "窗口应跟到末尾：{visible}");
    }

    #[test]
    fn draft_hit_rect_empty_when_other_inactive() {
        let prompt = sample_prompt(false);
        let (_, hits) = paint(&prompt, 0, &[false, false, false], "", false);
        assert_eq!(hits.draft_input.width, 0);
    }
}
