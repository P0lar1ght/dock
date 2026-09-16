//! StatusBar widget - displays context info at the top.

//! Copied from grok-build/crates/codegen/xai-grok-pager/src/views/status_bar.rs

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use super::theme::Theme;
use crate::grok::line_utils::truncate_str;

/// 顶栏相邻两段之间至少留这么多列。低于它三段会贴死成一串看不断的字。
const SEG_GAP: u16 = 2;

/// Status bar showing context information.
///
/// Displays: token count, current turn, view mode, etc.
/// Respects layout: first 3 cols and last 2 cols are empty.
pub struct StatusBar<'a> {
    /// Left-aligned content (e.g., "Context: 5.2k tokens")
    pub left: &'a str,
    /// Center content (e.g., "Turn 2/3")
    pub center: Option<&'a str>,
    /// Right-aligned content (e.g. context occupancy). flash 文案走这条。
    pub right: Option<&'a str>,
    /// 多 span 的右段。占用率是一条渐变色的数字 + dim 分隔符 + 协议名，悬停时
    /// 整段换成进度条，一个 `Style` 盖不住，所以走这条。设了它就压过
    /// [`Self::right`]。
    pub right_line: Option<Line<'a>>,
}

impl<'a> StatusBar<'a> {
    /// Create a new status bar with left content.
    pub fn new(left: &'a str) -> Self {
        Self {
            left,
            center: None,
            right: None,
            right_line: None,
        }
    }

    /// Add center content.
    pub fn center(mut self, text: &'a str) -> Self {
        self.center = Some(text);
        self
    }

    /// Add right content.
    pub fn right(mut self, text: &'a str) -> Self {
        self.right = Some(text);
        self
    }

    /// 多 span 的右段（占用率渐变 + 分隔符 + 协议，或悬停时的进度条）。
    pub fn right_line(mut self, line: Line<'a>) -> Self {
        self.right_line = Some(line);
        self
    }
}

/// 把一条 `Line` 按显示宽度截到 `budget` 列，逐 span 走、保住各自的样式。
///
/// 右段是渐变色数字 + dim 分隔符 + 协议名，整条拍平成 `&str` 再 `truncate_str`
/// 会把颜色全丢掉；窄窗下颜色恰恰是唯一还在报警的东西。
fn truncate_line(line: &Line<'_>, budget: usize) -> Line<'static> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in &line.spans {
        let w = UnicodeWidthStr::width(span.content.as_ref());
        if used + w <= budget {
            out.push(Span::styled(span.content.to_string(), span.style));
            used += w;
            continue;
        }
        // 这一段放不下：截到剩余预算，末尾留一列给省略号。
        let room = budget.saturating_sub(used);
        if room > 1 {
            let head = truncate_str(span.content.as_ref(), room);
            if !head.is_empty() {
                out.push(Span::styled(head, span.style));
            }
        } else if room == 1 {
            out.push(Span::styled("…".to_string(), span.style));
        }
        break;
    }
    Line::from(out)
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let theme = Theme::current();

        // Layout: outer block already has 2-char horizontal padding
        // No additional margins needed
        let left_margin = 0u16;
        let right_margin = 0u16;
        let content_x = area.x + left_margin;
        let content_width = area.width.saturating_sub(left_margin + right_margin);

        if content_width < 10 {
            return;
        }

        let style = Style::default().fg(theme.gray).bg(theme.bg_base);

        // Fill background (the whole row)
        buf.set_style(area, Style::default().bg(theme.bg_base));

        // 三段预算：right 优先、center 次之、left 吃剩下的，每相邻两段之间
        // 留 SEG_GAP 列，各自截断后再画。旧实现 last-write-wins：长 cwd 铺满
        // 整行，right 最后画直接盖掉 center 的尾巴，窄窗下中间那段被啃成乱码。
        let right_spans = self
            .right_line
            .as_ref()
            .map(|l| truncate_line(l, content_width as usize));
        let right_text = match right_spans {
            Some(_) => None,
            None => self.right.map(|t| truncate_str(t, content_width as usize)),
        };
        let right_w = match (&right_spans, right_text.as_deref()) {
            (Some(l), _) => l
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum::<usize>() as u16,
            (None, Some(t)) => UnicodeWidthStr::width(t) as u16,
            (None, None) => 0,
        };

        let gap_for = |w: u16| if w > 0 { SEG_GAP } else { 0 };
        // center 要么整段显示，要么不显示：`第 5 轮` 截成 `第` 比不画更难读，
        // 而且它是可省的装饰位，left 的 cwd 尾巴才是要保的信息。
        let for_center = content_width.saturating_sub(right_w + gap_for(right_w));
        let center_text = self
            .center
            .filter(|c| UnicodeWidthStr::width(*c) as u16 <= for_center);
        let center_w = center_text
            .map(|c| UnicodeWidthStr::width(c) as u16)
            .unwrap_or(0);

        let for_left = for_center.saturating_sub(center_w + gap_for(center_w));
        // 预算为 0 时干脆不画：`truncate_str(_, 0)` 返回的是 `…`，仍占 1 列，
        // 极窄下会被 right 覆盖 —— 又变回"两段抢同一格"。
        let left_text = if for_left == 0 {
            String::new()
        } else {
            truncate_str(self.left, for_left as usize)
        };
        let left_w = UnicodeWidthStr::width(left_text.as_str()) as u16;

        if left_w > 0 {
            let left_span = Span::styled(left_text.as_str(), style);
            buf.set_span(content_x, area.y, &left_span, left_w);
        }

        // Center 在 left 与 right 之间的窗口里居中。窗口是 budget 出来的且
        // 含两侧留白，所以既压不到任何一段，也不会和它们贴死。
        if let Some(center) = center_text {
            let window_w = content_width.saturating_sub(left_w + right_w);
            let center_x = content_x + left_w + window_w.saturating_sub(center_w) / 2;
            let center_span = Span::styled(center, style);
            buf.set_span(center_x, area.y, &center_span, center_w);
        }

        let right_x = content_x + content_width.saturating_sub(right_w);
        if let Some(line) = right_spans.as_ref() {
            let mut x = right_x;
            for span in &line.spans {
                let w = UnicodeWidthStr::width(span.content.as_ref()) as u16;
                if w == 0 {
                    continue;
                }
                buf.set_span(x, area.y, span, w);
                x += w;
            }
        } else if let Some(right) = right_text.as_deref() {
            let right_span = Span::styled(right, style);
            buf.set_span(right_x, area.y, &right_span, right_w);
        }
    }
}

/// Hit rect for the right-aligned status text (same math as [`StatusBar`]).
pub fn right_rect(area: Rect, text: &str) -> Rect {
    let w = UnicodeWidthStr::width(text) as u16;
    let x = area.x + area.width.saturating_sub(w);
    Rect::new(x, area.y, w.min(area.width), 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Position;
    use ratatui::layout::Rect;

    #[test]
    fn chinese_right_aligns_by_display_width() {
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        StatusBar::new("cwd").right("空闲").render(area, &mut buf);
        // Display width 4, so the pair starts at x=16 — not byte-len 6 at x=14.
        assert_eq!(buf[(16, 0)].symbol(), "空");
        assert_eq!(buf[(18, 0)].symbol(), "闲");
        assert_eq!(UnicodeWidthStr::width("空闲"), 4);
        assert_eq!("空闲".len(), 6);
    }

    #[test]
    fn occupancy_right_hit_covers_label() {
        let area = Rect::new(0, 0, 40, 1);
        let text = "上下文 20.0k/204k";
        let hit = right_rect(area, text);
        let w = UnicodeWidthStr::width(text) as u16;
        assert_eq!(hit.width, w);
        assert_eq!(hit.x, 40 - w);
        assert!(hit.contains(Position { x: hit.x, y: 0 }));
        assert!(!hit.contains(Position { x: 0, y: 0 }));
    }

    fn row(buf: &Buffer, width: u16) -> String {
        // 宽字符占两个格子，第二格是默认空白；按显示宽度步进收集，
        // 才能拼回视觉上的一行。
        let mut out = String::new();
        let mut x = 0u16;
        while x < width {
            let sym = buf[(x, 0)].symbol().to_string();
            let w = UnicodeWidthStr::width(sym.as_str()) as u16;
            out.push_str(&sym);
            x += w.max(1);
        }
        out
    }

    /// 窄窗三段预算：left 截断、center 完整、right 完整，互不覆盖。
    /// 旧实现 left 铺满整行、center 因压线被丢、right 直接盖上去。
    #[test]
    fn narrow_bar_budgets_right_center_left_without_overlap() {
        let area = Rect::new(0, 0, 40, 1);
        let left = "/Users/polar/very-long-repo-name/dock";
        let center = "第 5 轮";
        let right = "上下文 20.0k/204k";
        let mut buf = Buffer::empty(area);
        StatusBar::new(left)
            .center(center)
            .right(right)
            .render(area, &mut buf);
        let text = row(&buf, 40);
        assert!(text.contains('…'), "left cwd should be truncated: {text}");
        assert!(text.contains("第 5 轮"), "center must survive: {text}");
        // 不重叠还不够 —— 三段贴死成一串同样读不出来。留白写死成字面量，
        // 不用 SEG_GAP 拼：那样常数一改断言跟着改，等于自证。
        assert!(
            text.contains("…  第 5 轮"),
            "left 与 center 之间要留白: {text}"
        );
        assert!(
            text.contains("第 5 轮  上下文"),
            "center 与 right 之间要留白: {text}"
        );
        let tail = {
            let w = UnicodeWidthStr::width(right) as u16;
            let mut out = String::new();
            let mut x = 40 - w;
            while x < 40 {
                let sym = buf[(x, 0)].symbol().to_string();
                out.push_str(&sym);
                x += UnicodeWidthStr::width(sym.as_str()).max(1) as u16;
            }
            out
        };
        assert_eq!(tail, right, "right segment must be intact: {text}");
    }

    /// 三段都放得下时行为不变：left 靠左、right 靠右、center 居中。
    #[test]
    fn wide_bar_keeps_all_three_segments() {
        let area = Rect::new(0, 0, 80, 1);
        let left = "cwd";
        let center = "第 5 轮";
        let right = "20.0k/204k";
        let mut buf = Buffer::empty(area);
        StatusBar::new(left)
            .center(center)
            .right(right)
            .render(area, &mut buf);
        let text = row(&buf, 80);
        assert!(text.starts_with(left), "{text}");
        assert!(text.ends_with(right), "{text}");
        assert!(text.contains(center), "{text}");
    }

    /// center 是装饰位：放不下就整段丢掉。截成 `第` 既没信息又占着
    /// left 的 cwd 尾巴 —— 那才是这一行真正要保的东西。
    #[test]
    fn a_center_that_cannot_fit_whole_is_dropped_not_chopped() {
        let area = Rect::new(0, 0, 22, 1);
        let mut buf = Buffer::empty(area);
        StatusBar::new("/Users/polar/dock")
            .center("第 5 轮")
            .right("上下文 20.0k/204k")
            .render(area, &mut buf);
        let text = row(&buf, 22);
        assert!(text.contains("上下文 20.0k/204k"), "{text}");
        assert!(!text.contains('第'), "半个 center 比不画更糟: {text}");
    }
}
