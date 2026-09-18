//! Agent message — Grok `StreamingMarkdownRenderer` + pager mermaid affordance.

use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use ratatui::text::Line;

use crate::grok::md_style;
use crate::grok::mermaid;
use crate::grok::wrapping::word_wrap_lines;
use crate::theme::Theme;

#[derive(Clone, Default)]
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    /// `(wrapped_line_index, mermaid source)` for `[Copy Source]` rows.
    pub mermaid: Vec<(usize, String)>,
}

const CACHE_CAP: usize = 24;
static CACHE: Mutex<Vec<(u64, usize, Rendered)>> = Mutex::new(Vec::new());

pub fn render(text: &str, theme: &Theme, width: usize) -> Rendered {
    if text.trim().is_empty() {
        return Rendered::default();
    }
    let hash = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        // `md_style::style()` reads the live palette, so rendered lines are
        // palette-specific. Keying on the text alone hands GrokNight spans back
        // after a switch to GrokDay.
        crate::theme::Theme::current_kind().hash(&mut h);
        h.finish()
    };
    if let Ok(guard) = CACHE.lock() {
        if let Some((_, _, value)) = guard
            .iter()
            .rev()
            .find(|(h, w, _)| *h == hash && *w == width)
        {
            return value.clone();
        }
    }
    let value = render_uncached(text, theme, width);
    if let Ok(mut guard) = CACHE.lock() {
        guard.push((hash, width, value.clone()));
        if guard.len() > CACHE_CAP {
            let extra = guard.len() - CACHE_CAP;
            guard.drain(..extra);
        }
    }
    value
}

fn render_uncached(text: &str, theme: &Theme, width: usize) -> Rendered {
    let syntect = cordis_markdown::default_syntect();
    let mut renderer = cordis_markdown::StreamingMarkdownRenderer::new(md_style::style(), true);
    // 表格得在**排版时**就知道能用多宽。正文靠后面的 word wrap 兜底，表格不
    // 行——`word_wrap_line` 对表格行是直接 `fit_line_to_width` 裁掉的（折行会
    // 毁掉列对齐），所以排宽了就是右边一截连同右边框被切掉。渲染器自己会按列
    // 等比收窄。
    if width > 0 {
        renderer.set_max_table_width(Some(width));
    }
    renderer.push(text);
    let output = renderer.finish_into_output(Some(syntect));
    let mut lines = output.lines;
    // 插入位置按原 `output.lines` 坐标算。倒序插入互不影响低位的坐标，
    // 但每个先插的高位元素会被后续所有更低的插入推后一位：最终下标 =
    // 原下标 + 升序排名。wrap 之后靠这份下标显式映射，不再回认文本
    // （窄窗下折行会把一个块认成两个，第二块的源码被第一块吃掉）。
    //
    // 源码跟着 `inserts` 走，不另开一个按 `code_blocks` 顺序的平行数组：
    // 这里按位置排过序，平行数组只有在「块顺序恰好等于位置顺序」时才对得上。
    let mut inserts: Vec<(usize, &str)> = output
        .code_blocks
        .iter()
        .filter(|cb| cb.info.split_whitespace().next() == Some("mermaid"))
        .map(|cb| (cb.output_line_range.end.min(lines.len()), cb.body.as_str()))
        .collect();
    inserts.sort_by_key(|(at, _)| *at);
    // `(最终行号, 源码)`，按行号升序。
    let mut affordances: Vec<(usize, &str)> = Vec::with_capacity(inserts.len());
    for (rank, (at, body)) in inserts.iter().enumerate().rev() {
        lines.insert(*at, mermaid_affordance(theme, width));
        affordances.push((at + rank, *body));
    }
    affordances.reverse();
    let mut mermaid = Vec::new();
    let lines: Vec<Line<'static>> = if width == 0 {
        mermaid.extend(affordances.iter().map(|(at, body)| (*at, body.to_string())));
        lines
    } else {
        // affordance 行不参与 word wrap：宽度不够时少画按钮
        // （`mermaid::line_for_width`），折行会让 `[Copy Source]` 掉到
        // 下一行，映射和点击都错位。
        let mut out = Vec::new();
        let mut next = affordances.iter().peekable();
        for (i, line) in lines.into_iter().enumerate() {
            match next.peek() {
                Some((at, body)) if *at == i => {
                    mermaid.push((out.len(), body.to_string()));
                    next.next();
                    out.push(line);
                }
                _ => out.extend(word_wrap_lines(vec![line], width)),
            }
        }
        out
    };
    Rendered { lines, mermaid }
}

fn mermaid_affordance(theme: &Theme, width: usize) -> Line<'static> {
    mermaid::line_for_width(theme, width as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mermaid_gets_affordance_row() {
        let theme = Theme::groknight();
        let md = "```mermaid\nflowchart TD\nA-->B\n```\n";
        let rendered = render(md, &theme, 80);
        let joined: String = rendered
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("[Copy Source]") && joined.contains("[Open Image]"),
            "expected mermaid affordance, got:\n{joined}"
        );
        assert_eq!(rendered.mermaid.len(), 1);
        assert!(rendered.mermaid[0].1.contains("flowchart"));
    }

    /// 表格要跟着面板宽度重排，不是排成自然宽度再被裁掉。
    ///
    /// 表格行**不参与 word wrap**（折行会毁掉列对齐），`word_wrap_line` 对它们
    /// 只做 `fit_line_to_width`。所以排版时不给上限，宽表格就是右边一截连同右
    /// 边框被整齐地切掉——看起来像"表格没画完"。渲染器自己会按列等比收窄。
    #[test]
    fn tables_are_laid_out_at_the_pane_width() {
        use unicode_width::UnicodeWidthStr;

        let theme = Theme::groknight();
        let md = "| 实现 | 权重 | 许可 |\n|---|---|---|\n\
                  | TheoLeeCJ/openjev | 单张 RTX 3090 上用冻结 Qwen3.5-4B 复现，\
                  Safetensors 在子目录 | MIT |\n\
                  | b | RTX 3090 + WebGPU demo | MIT, 242★ |\n";
        for width in [44usize, 60, 100] {
            let rows: Vec<String> = render(md, &theme, width)
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .filter(|s: &String| !s.trim().is_empty())
                .collect();
            assert!(rows.len() >= 4, "width={width} {rows:?}");
            for row in &rows {
                assert!(row.width() <= width, "width={width} 行超出面板：{row:?}");
                // 每一行都得自己收口。少了右边框就说明它是被裁掉的，不是排出来的。
                let last = row.trim_end().chars().last().unwrap_or(' ');
                assert!(
                    matches!(last, '\u{2502}' | '\u{2510}' | '\u{2524}' | '\u{2518}'),
                    "width={width} 表格右边框被裁掉了：{row:?}"
                );
            }
        }
    }

    /// 窄窗下 affordance 行不折行：折成两截会把一个块认成两个，
    /// 两个块时第二块的源码被第一块的第二截吃掉。
    #[test]
    fn two_mermaid_blocks_in_narrow_window_keep_their_own_bodies() {
        let theme = Theme::groknight();
        let md = concat!(
            "```mermaid\nflowchart TD\nA-->B\n```\n\n",
            "过渡段。\n\n",
            "```mermaid\nflowchart LR\nC-->D\n```\n",
        );
        let rendered = render(md, &theme, 40);
        assert_eq!(rendered.mermaid.len(), 2, "one row per block");
        assert!(
            rendered.mermaid[0].1.contains("A-->B"),
            "{:?}",
            rendered.mermaid
        );
        assert!(
            rendered.mermaid[1].1.contains("C-->D"),
            "{:?}",
            rendered.mermaid
        );
        // 每个 affordance 行都只占一行（没被折开）。
        let rows = rendered
            .lines
            .iter()
            .filter(|l| {
                let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                t.contains("[Open Image]")
            })
            .count();
        assert_eq!(rows, 2);
        // 映射行号直接指向 affordance 行本身。
        for (idx, _) in &rendered.mermaid {
            let t: String = rendered.lines[*idx]
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            assert!(
                t.contains("[Open Image]"),
                "line {idx} is not an affordance row: {t}"
            );
        }
    }

    /// 窄窗省按钮的口径：40 列保 [Open Image] + [Copy Source]（丢
    /// [Copy Image Path]），24 列只保 [Open Image]；画出来的列位和
    /// hit_kind 共享同一套按钮表，不会漂移。
    #[test]
    fn affordance_buttons_shrink_with_width_and_stay_hittable() {
        assert_eq!(
            mermaid::hit_kind(12, 40),
            Some(mermaid::AffordanceKind::Open)
        );
        assert_eq!(
            mermaid::hit_kind(27, 40),
            Some(mermaid::AffordanceKind::CopySource)
        );
        assert_eq!(mermaid::hit_kind(27, 24), None);
        assert_eq!(
            mermaid::hit_kind(12, 24),
            Some(mermaid::AffordanceKind::Open)
        );
        let theme = Theme::groknight();
        let line = mermaid::line_for_width(&theme, 24);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("[Open Image]"), "{text}");
        assert!(!text.contains("[Copy Image Path]"), "{text}");
        assert!(!text.contains("[Copy Source]"), "{text}");
    }
}
