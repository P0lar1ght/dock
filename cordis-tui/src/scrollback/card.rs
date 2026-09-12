//! 工具卡片的公共外壳：缩进、换行宽度、截断脚注、折叠指示符。
//!
//! 这里只管**外壳**。哪些字段进标题、diff 怎么算、syntect 怎么高亮，仍然由各
//! 卡片自己决定 —— 外壳不认识任何工具名。
//!
//! 抽出来之前，14 个卡片模块各自手搓这几件事，结果是同一屏里并存：
//!
//! * 四种截断脚注（`… +4 lines` / `… +2 more files` / `… (6 more lines)` /
//!   `… 还有 6 行`）；
//! * 三套正文缩进（0 / 2 / 行号 gutter），左边缘是锯齿状的；
//! * 两套换行宽度（`width` 与 `width - 2`），右边距忽宽忽窄。
//!
//! 缩进定成 2 列不是随便挑的：`◆ ` 正好 2 列，所以正文左边缘和标题文字对齐，
//! 不用再加一条 gutter 之类的额外装饰就能让正文看着属于这张卡。

use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::wrapping::word_wrap_lines;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

/// 正文缩进，正好是 `◆ ` 的宽度。
pub const BODY_INDENT: usize = 2;

/// 正文换行宽度。比面板窄 [`BODY_INDENT`]（缩进吃掉的）再留一格右边距。
///
/// 下限 20：再窄下去换行本身就没意义了，交给 `truncate_line` 裁。
pub fn body_width(pane: usize) -> usize {
    pane.saturating_sub(BODY_INDENT + 1).max(20)
}

/// 折叠指示符的宽度：两格间隔 + 一个字形。
pub const FOLD_MARK: usize = BODY_INDENT + 1;

/// 标题正文可用宽度 —— 给行尾的折叠指示符留出位置。
pub fn header_width(pane: usize) -> usize {
    pane.saturating_sub(FOLD_MARK).max(8)
}

/// 把一段正文按统一缩进 + 统一宽度排好。
pub fn body(lines: Vec<Line<'static>>, pane: usize) -> Vec<Line<'static>> {
    let wrapped = if pane == 0 {
        lines
    } else {
        word_wrap_lines(lines, body_width(pane))
    };
    wrapped.into_iter().map(indent).collect()
}

/// 给一行正文加上统一缩进。
pub fn indent(mut line: Line<'static>) -> Line<'static> {
    line.spans.insert(0, Span::raw(" ".repeat(BODY_INDENT)));
    line
}

/// 统一的省略脚注：`… 还有 N 行` / `… 还有 N 个文件`。
///
/// `unit` 是量词，因为各卡片省略的东西不一样（行 / 文件 / 项）——省掉多少
/// **什么**是信息，不该为了统一而丢掉；统一的是这句话的形状。
pub fn elision(hidden: usize, unit: &str, theme: &Theme) -> Line<'static> {
    indent(Line::from(Span::styled(
        format!("\u{2026} 还有 {hidden} {unit}"),
        theme.muted(),
    )))
}

/// 头尾保留、中间省略，压一条 [`elision`] 脚注。
///
/// `first` / `last` 由各卡片自己定：shell 输出 2 行就够，`read` 要 5 行才看得
/// 出上下文，`list_dir` 要 12 行才像个目录。统一的是机制和文案，不是条数。
pub fn head_tail(
    rows: Vec<Line<'static>>,
    first: usize,
    last: usize,
    unit: &str,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let total = rows.len();
    if total <= first + last {
        return rows;
    }
    let hidden = total - first - last;
    let mut out: Vec<Line<'static>> = rows.iter().take(first).cloned().collect();
    out.push(elision(hidden, unit, theme));
    out.extend(rows.into_iter().skip(total - last));
    out
}

/// 折叠指示符，钉在标题行末尾。
///
/// `›` 收起、`⌄` 还有更多、`⌃` 全展开。一列宽、与语言无关，而且**每张可折叠
/// 的卡都有** —— 原先只有一半卡片带「（点击展开）」，最常见的那批（Read /
/// Edit / Bash / List / Search）反倒没有，用户根本不知道能点。
///
/// 打开覆盖层的卡片（子代理 / 后台任务）不用它：那是另一种动作，仍旧写
/// 「（点击查看）」。
pub fn fold_mark(mode: ToolMode, theme: &Theme) -> Span<'static> {
    let glyph = match mode {
        ToolMode::Collapsed => glyphs::chevron(),
        ToolMode::Truncated => "\u{2304}",
        ToolMode::Expanded => "\u{2303}",
    };
    Span::styled(format!("  {glyph}"), theme.dim())
}

/// 标题行收尾：截到可用宽度，再钉上折叠指示符。
///
/// 先截后钉，所以指示符永远画得出来 —— 反过来会被长标题挤掉，而它恰恰是
/// 「这张卡能点」的唯一提示。
pub fn finish_header(
    header: Line<'static>,
    pane: usize,
    mode: ToolMode,
    theme: &Theme,
) -> Line<'static> {
    let mut line = if pane == 0 {
        header
    } else {
        crate::grok::line_utils::truncate_line(header, header_width(pane))
    };
    line.spans.push(fold_mark(mode, theme));
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    fn rows(n: usize) -> Vec<Line<'static>> {
        (1..=n)
            .map(|i| Line::from(Span::raw(format!("L{i:02}"))))
            .collect()
    }

    #[test]
    fn body_indent_lines_up_with_the_header_text() {
        // `◆ ` 是 2 列，正文缩进必须等宽，否则正文和标题文字错开一格。
        assert_eq!(
            BODY_INDENT,
            unicode_width::UnicodeWidthStr::width(
                format!("{} ", glyphs::diamond_filled()).as_str()
            )
        );
    }

    #[test]
    fn body_width_leaves_the_indent_and_a_right_margin() {
        assert_eq!(body_width(80), 77);
        assert_eq!(body_width(30), 27);
        // 窄到没意义时钳住，不返回 0 让 wrap 死循环或炸掉。
        assert_eq!(body_width(4), 20);
        assert_eq!(body_width(0), 20);
    }

    #[test]
    fn head_tail_keeps_both_ends_and_says_how_much_it_hid() {
        let out = head_tail(rows(12), 2, 3, "行", &Theme::current());
        assert_eq!(
            plain(&out),
            vec!["L01", "L02", "  \u{2026} 还有 7 行", "L10", "L11", "L12",]
        );
    }

    #[test]
    fn head_tail_is_a_no_op_when_everything_fits() {
        let out = head_tail(rows(5), 2, 3, "行", &Theme::current());
        assert_eq!(plain(&out), vec!["L01", "L02", "L03", "L04", "L05"]);
    }

    /// 省掉的是「文件」就说文件 —— 统一的是句式，不是把信息抹平。
    #[test]
    fn the_unit_survives_the_unification() {
        let theme = Theme::current();
        assert_eq!(
            plain(&[elision(3, "行", &theme)])[0],
            "  \u{2026} 还有 3 行"
        );
        assert_eq!(
            plain(&[elision(2, "个文件", &theme)])[0],
            "  \u{2026} 还有 2 个文件"
        );
    }

    #[test]
    fn every_fold_state_has_its_own_mark() {
        let theme = Theme::current();
        let marks: Vec<String> = [ToolMode::Collapsed, ToolMode::Truncated, ToolMode::Expanded]
            .into_iter()
            .map(|m| fold_mark(m, &theme).content.to_string())
            .collect();
        assert_eq!(marks.len(), 3);
        let unique: std::collections::HashSet<&String> = marks.iter().collect();
        assert_eq!(unique.len(), 3, "三种折叠态必须看得出区别: {marks:?}");
        for m in &marks {
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(m.as_str()),
                FOLD_MARK,
                "折叠符定宽，否则标题尾巴会随折叠态跳动: {m:?}"
            );
        }
    }

    /// 长标题不能把折叠符挤掉 —— 那是「这张卡能点」的唯一提示 —— 也不能因为
    /// 钉了折叠符就把整行顶出面板。
    #[test]
    fn the_fold_mark_survives_a_header_that_overflows() {
        let theme = Theme::current();
        for pane in [12usize, 20, 40, 80] {
            let header = Line::from(Span::raw("x".repeat(200)));
            let line = finish_header(header, pane, ToolMode::Collapsed, &theme);
            let text = plain(std::slice::from_ref(&line))[0].clone();
            assert!(text.ends_with(glyphs::chevron()), "pane={pane} {text:?}");
            assert!(
                unicode_width::UnicodeWidthStr::width(text.as_str()) <= pane,
                "pane={pane} {text:?}"
            );
        }
    }
}
