//! `LogEvent::Notice` 卡片——**只给用户看**的通知。
//!
//! 与工具卡的区别是它没有对应的工具调用：模型既没发起它，也看不到它
//! （`Sessions::model_history` 把 `Notice` 滤掉）。workflow 的子代理上报与 run
//! 收尾走这条：run 还在跑时推给模型会让它在半份结果上开一轮，但用户该看得见。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::scrollback::card;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;
use cordis_spine::NoticeKind;

/// 折叠时正文最多显示几行。
const COLLAPSED_BODY_LINES: usize = 2;

pub fn header_id(index: usize) -> String {
    format!("notice:{index}")
}

pub fn lines(
    kind: NoticeKind,
    title: &str,
    body: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
) -> Vec<Line<'static>> {
    let accent = accent_of(kind, theme);
    let label = label_of(kind);
    let open = mode != ToolMode::Collapsed;

    let mut header = Line::from(vec![
        Span::styled(
            format!("{} ", glyphs::diamond_filled()),
            Style::default().fg(accent),
        ),
        Span::styled(
            format!("{label} "),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(title.to_string(), Style::default().fg(theme.text_primary)),
    ]);
    if width > 0 {
        header = truncate_line(header, card::header_width(width));
    }
    let header = card::finish_header(header, width, mode, theme);

    let body_lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    if body_lines.is_empty() {
        return vec![header];
    }
    let mut out = vec![header];
    // 折叠态也留两行正文：一条上报的价值全在正文里，只剩标题等于没显示。
    let shown = if open {
        body_lines.as_slice()
    } else {
        &body_lines[..body_lines.len().min(COLLAPSED_BODY_LINES)]
    };
    for line in shown {
        out.push(card::indent(Line::from(Span::styled(
            (*line).to_string(),
            theme.muted(),
        ))));
    }
    let hidden = body_lines.len() - shown.len();
    if hidden > 0 {
        out.push(card::indent(card::elision(hidden, "行", theme)));
    }
    out
}

fn accent_of(kind: NoticeKind, theme: &Theme) -> Color {
    match kind {
        NoticeKind::WorkflowReport => theme.accent_running,
        NoticeKind::WorkflowDone => theme.accent_success,
        NoticeKind::Other => theme.gray_bright,
    }
}

fn label_of(kind: NoticeKind) -> &'static str {
    match kind {
        NoticeKind::WorkflowReport => "上报",
        NoticeKind::WorkflowDone => "工作流",
        NoticeKind::Other => "通知",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn collapsed_keeps_the_first_body_lines() {
        let theme = Theme::current();
        let out = lines(
            NoticeKind::WorkflowReport,
            "deep-research · research-planner 上报",
            "第一行\n第二行\n第三行\n第四行",
            &theme,
            80,
            ToolMode::Collapsed,
        );
        let text = text_of(&out);
        assert!(text.contains("research-planner"), "{text}");
        assert!(text.contains("第一行") && text.contains("第二行"), "{text}");
        assert!(!text.contains("第四行"), "折叠态不该全展开：{text}");
    }

    #[test]
    fn expanded_shows_the_whole_body() {
        let theme = Theme::current();
        let out = lines(
            NoticeKind::WorkflowReport,
            "t",
            "第一行\n第二行\n第三行\n第四行",
            &theme,
            80,
            ToolMode::Expanded,
        );
        let text = text_of(&out);
        assert!(text.contains("第四行"), "{text}");
    }

    /// 没有正文就只剩标题一行，不该多画一个空卡体。
    #[test]
    fn an_empty_body_is_just_the_header() {
        let theme = Theme::current();
        let out = lines(
            NoticeKind::WorkflowDone,
            "工作流 deep-research · 完成",
            "   \n\n",
            &theme,
            80,
            ToolMode::Expanded,
        );
        assert_eq!(out.len(), 1, "{:?}", text_of(&out));
    }
}
