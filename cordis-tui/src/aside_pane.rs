//! `/btw` 旁问面板：钉在输入框上方的一块，问完就能关。
//!
//! 只画 `"tui.tabs"` 里那一个旁问页的状态（问题 + 最近一条回复），不持有任何
//! 状态。旁问页本身是只读的、不进标签栏，Esc 关掉即销毁。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::line_utils::truncate_str;
use crate::tabs::AsideView;
use crate::theme::Theme;

/// 面板最多占几行（含标题行）。再多就该提升成正式分页去看。
const MAX_HEIGHT: u16 = 8;

/// 这一块要几行。没有旁问就是 0。
pub fn desired_height(aside: Option<&AsideView>, width: u16) -> u16 {
    let Some(aside) = aside else {
        return 0;
    };
    let body = body_lines(aside, width.saturating_sub(2) as usize);
    // 标题 + 问题 + 正文
    (2 + body.len() as u16).min(MAX_HEIGHT)
}

/// 正文折行；还没答出来时只有一行状态。
///
/// 按**显示列**折（`textwrap` 走 unicode-width），不是按字符数：`set_line` 是
/// 按列裁剪的，中文一个字占两列，按字符折的话每行后半截会被直接丢掉——既不
/// 显示也不换行。
fn body_lines(aside: &AsideView, width: usize) -> Vec<String> {
    let width = width.max(8);
    match &aside.answer {
        None if aside.working => vec!["思考中…".into()],
        None => vec!["（还没有回复）".into()],
        Some(answer) => {
            let mut out = Vec::new();
            for line in answer.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                for wrapped in textwrap::wrap(line, width) {
                    out.push(wrapped.into_owned());
                    if out.len() >= MAX_HEIGHT as usize {
                        return out;
                    }
                }
            }
            if out.is_empty() {
                out.push("（空回复）".into());
            }
            out
        }
    }
}

pub fn paint(buf: &mut Buffer, area: Rect, aside: &AsideView) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let theme = Theme::current();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
                cell.set_style(Style::default().bg(theme.bg_base));
            }
        }
    }

    let hint = if aside.working {
        "Esc:停并关闭"
    } else {
        "Esc:关闭  Ctrl+↑:提升为分页"
    };
    let head = format!("旁问（只读·来自第 {} 页）", aside.origin);
    buf.set_line(
        area.x,
        area.y,
        &Line::from(vec![
            Span::styled(
                head,
                Style::default()
                    .fg(theme.accent_system)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {hint}"), Style::default().fg(theme.gray_dim)),
        ]),
        area.width,
    );

    let inner = area.width.saturating_sub(2) as usize;
    if area.height > 1 {
        buf.set_line(
            area.x,
            area.y + 1,
            &Line::from(vec![
                Span::styled("? ", Style::default().fg(theme.gray)),
                Span::styled(
                    truncate_str(&aside.question, inner),
                    Style::default().fg(theme.accent_user),
                ),
            ]),
            area.width,
        );
    }
    for (i, line) in body_lines(aside, inner).iter().enumerate() {
        let y = area.y + 2 + i as u16;
        if y >= area.y + area.height {
            break;
        }
        buf.set_line(
            area.x,
            y,
            &Line::from(vec![
                Span::styled("› ", Style::default().fg(theme.gray)),
                Span::styled(line.clone(), Style::default().fg(theme.accent_assistant)),
            ]),
            area.width,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(answer: Option<&str>, working: bool) -> AsideView {
        AsideView {
            origin: 1,
            question: "它现在卡在哪个测试".into(),
            answer: answer.map(str::to_string),
            working,
        }
    }

    fn render(aside: &AsideView, width: u16) -> String {
        let height = desired_height(Some(aside), width);
        let area = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        let mut buf = Buffer::empty(area);
        paint(&mut buf, area, aside);
        // 宽字符占两格、续格是空的：挤掉空白才能按整词断言。
        (0..height)
            .map(|y| {
                (0..width)
                    .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                    .collect::<String>()
                    .replace(' ', "")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn no_aside_takes_no_rows() {
        assert_eq!(desired_height(None, 80), 0);
    }

    #[test]
    fn working_shows_a_thinking_line_and_stop_hint() {
        let text = render(&view(None, true), 60);
        assert!(text.contains("思考中"), "{text}");
        assert!(text.contains("Esc"), "{text}");
        assert!(text.contains("旁问"), "{text}");
    }

    #[test]
    fn answer_is_shown_with_the_promote_hint() {
        let text = render(&view(Some("在跑 lib 测试"), false), 60);
        assert!(text.contains("在跑lib测试"), "{text}");
        assert!(text.contains("提升"), "{text}");
    }

    /// 长回复不能把整屏吃掉：面板有硬上限，剩下的去分页里看。
    #[test]
    fn long_answers_are_capped() {
        let long = "很长的一行".repeat(200);
        let aside = view(Some(&long), false);
        assert!(desired_height(Some(&aside), 40) <= MAX_HEIGHT);
    }

    /// 折行按字符走，不能在多字节中间切断。
    #[test]
    fn wrapping_is_char_safe() {
        let aside = view(Some("中文中文中文中文中文中文"), false);
        let text = render(&aside, 20);
        assert!(text.contains('中'), "{text}");
    }

    /// 中文一个字占两列：按字符数折行、按列裁剪的话，每行后半截会被直接丢掉
    /// （既不显示也不换行）。这里逐字断言，一个都不能少。
    #[test]
    fn cjk_answers_do_not_lose_characters() {
        let answer = "一二三四五六七八九十甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳午未申酉";
        let text = render(&view(Some(answer), false), 40);
        for ch in answer.chars() {
            assert!(text.contains(ch), "掉了「{ch}」：{text}");
        }
    }
}
