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

/// 这张卡的折叠 key。
///
/// 下标单用不住：Esc 撤回一轮会把事件表从那一轮截断（`Sessions::rewind`），
/// 之后新来的卡会捡到被撤掉那张留下的展开态。混进内容摘要就钉死了——同一个
/// 下标换了内容就是另一张卡。
pub fn header_id(index: usize, title: &str, body: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    title.hash(&mut hasher);
    body.hash(&mut hasher);
    format!("notice:{index}:{:x}", hasher.finish())
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

    let source: Vec<Line<'static>> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| Line::from(Span::styled(l.to_string(), theme.muted())))
        .collect();
    if source.is_empty() {
        return vec![header];
    }
    // 走 `card::body`（折行 + 统一缩进）而不是光 `card::indent`：一条上报是模型
    // 写的整段话，不折行就直接冲出终端右边缘，窄窗下更是只看得见开头。
    let rows = card::body(source, width);
    let mut out = vec![header];
    // 折叠态按**画出来的行**算，不是按原文的行：一行原文在窄窗下能折成七八行，
    // 按原文数的话「收起」反而比展开还高。
    let shown = if open {
        rows.len()
    } else {
        rows.len().min(COLLAPSED_BODY_LINES)
    };
    out.extend(rows.iter().take(shown).cloned());
    let hidden = rows.len() - shown;
    if hidden > 0 {
        out.push(card::elision(hidden, "行", theme));
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

    /// 折叠态按**画出来的行**收：一行原文在窄窗下能折成好几行，按原文数的话
    /// 「收起」反而比展开还高。
    #[test]
    fn collapsing_counts_wrapped_rows_not_source_lines() {
        let theme = Theme::current();
        let long = "本环境联网被封，web_search 与 web_fetch 均报 SSRF blocked，只能读本地只读证据；四个子问题全部无法取证，任何结论都只能算推测";
        let out = lines(
            NoticeKind::WorkflowReport,
            "t",
            long,
            &theme,
            44,
            ToolMode::Collapsed,
        );
        // 标题 + 两行正文 + 一条省略脚注。
        assert_eq!(out.len(), 4, "{:?}", text_of(&out));
        assert!(text_of(&out).contains("还有"), "{:?}", text_of(&out));
        for line in &out {
            let row: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                unicode_width::UnicodeWidthStr::width(row.as_str()) <= 44,
                "{row:?}"
            );
        }
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

    /// 同一个下标换了内容就得换 key，否则撤回一轮之后新卡会顶着旧卡的展开态。
    #[test]
    fn the_fold_key_follows_the_content() {
        assert_ne!(
            header_id(3, "a 上报", "第一份结论"),
            header_id(3, "b 上报", "第一份结论")
        );
        assert_ne!(
            header_id(3, "a 上报", "第一份结论"),
            header_id(3, "a 上报", "第二份结论")
        );
        assert_eq!(
            header_id(3, "a 上报", "第一份结论"),
            header_id(3, "a 上报", "第一份结论"),
            "同一张卡每帧都得算出同一个 key，不然折不起来"
        );
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
