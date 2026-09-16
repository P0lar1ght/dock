//! 1/8 格精度的方块进度条。Copied from grok pager `views/progress_bar.rs`。
//!
//! 只保留 `progress_bar_spans`（组进 `Line` 用），并剥掉了 grok 的 legacy ConHost
//! 分支：那边在旧 Windows 控制台上把 `▏▎▍▌▋▊▉` 换成 `░▒▓`，dock 没有对应的
//! 终端探测，加一套只为这一条进度条不划算。

use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// LEFT-fractional block glyphs, indexed 0..=8（0 空，8 满）。
const BLOCKS: [&str; 9] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];

/// 把填充率拆成（整格数, 余下的八分之几）。
fn cell_breakdown(width: u16, value: f32) -> (u16, usize) {
    let value = value.clamp(0.0, 1.0);
    let total_eighths = (value * width as f32 * 8.0).round() as u16;
    let full = (total_eighths / 8).min(width);
    let remainder = (total_eighths % 8) as usize;
    (full, remainder)
}

/// 每格的 `(字形, 是否已填充)`。
fn bar_cells(width: u16, value: f32) -> impl Iterator<Item = (&'static str, bool)> {
    let (full, remainder) = cell_breakdown(width, value);
    (0..width).map(move |i| {
        if i < full {
            ("█", true)
        } else if i == full && remainder > 0 {
            (BLOCKS.get(remainder).copied().unwrap_or(" "), true)
        } else {
            (" ", false)
        }
    })
}

/// 进度条的逐格 span：填充格 `fg` 压 `bg`，空格只有 `bg`。
pub fn progress_bar_spans(width: u16, value: f32, fg: Color, bg: Color) -> Vec<Span<'static>> {
    let fg_style = Style::default().fg(fg).bg(bg);
    let bg_style = Style::default().bg(bg);
    bar_cells(width, value)
        .map(|(symbol, filled)| {
            let style = if filled { fg_style } else { bg_style };
            Span::styled(symbol.to_string(), style)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// 条宽永远等于要求的列数——它要和默认态的 token 串等宽，少一格就会抽动。
    #[test]
    fn bar_is_always_exactly_width_cells() {
        for width in [0u16, 1, 5, 12] {
            for value in [0.0f32, 0.01, 0.42, 0.999, 1.0] {
                let spans = progress_bar_spans(width, value, Color::Red, Color::Black);
                assert_eq!(spans.len(), width as usize, "width={width} value={value}");
            }
        }
    }

    #[test]
    fn full_and_empty_are_the_extremes() {
        assert_eq!(
            text(&progress_bar_spans(4, 1.0, Color::Red, Color::Black)),
            "████"
        );
        assert_eq!(
            text(&progress_bar_spans(4, 0.0, Color::Red, Color::Black)),
            "    "
        );
    }

    /// 半满用整格 + 一个分数格表示，不是四舍五入到整格——1/8 精度就是为这个。
    #[test]
    fn partial_cell_uses_a_fractional_block() {
        let spans = progress_bar_spans(4, 0.3, Color::Red, Color::Black);
        let s = text(&spans);
        assert!(s.starts_with('█'), "{s:?}");
        assert!(
            s.chars()
                .nth(1)
                .is_some_and(|c| BLOCKS.contains(&c.to_string().as_str())),
            "第二格应是分数块: {s:?}"
        );
    }
}
