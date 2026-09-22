//! Agent message — Grok `StreamingMarkdownRenderer` + pager mermaid affordance.

use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use ratatui::text::Line;

use unicode_width::UnicodeWidthStr;

use crate::grok::line_utils::fit_line_to_width;
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

fn line_display_width(line: &Line<'_>) -> usize {
    line.spans.iter().map(|s| s.content.width()).sum()
}

fn render_uncached(text: &str, theme: &Theme, width: usize) -> Rendered {
    let syntect = cordis_markdown::default_syntect();
    let mut renderer = cordis_markdown::StreamingMarkdownRenderer::new(md_style::style(), true);
    // 表格与流程图得在**排版时**就知道能用多宽。正文靠后面的 word wrap 兜底，表格与流程图
    // 不行——word wrap 对它们会破坏对齐与图表连线。渲染器自己会按可用宽度排版。
    if width > 0 {
        renderer.set_max_table_width(Some(width));
    }
    renderer.push(text);
    let output = renderer.finish_into_output(Some(syntect));
    let lines = output.lines;

    struct MermaidBlockMeta<'a> {
        start: usize,
        end: usize,
        body: &'a str,
    }

    let mut mermaid_blocks: Vec<MermaidBlockMeta<'_>> = output
        .code_blocks
        .iter()
        .filter(|cb| cb.info.split_whitespace().next() == Some("mermaid"))
        .map(|cb| MermaidBlockMeta {
            start: cb.output_line_range.start.min(lines.len()),
            end: cb.output_line_range.end.min(lines.len()),
            body: cb.body.as_str(),
        })
        .collect();
    mermaid_blocks.sort_by_key(|m| m.start);

    // 标记哪些行属于 Mermaid 字符画（图表行绝对不走普通 word_wrap_lines）
    let mut is_diagram_line = vec![false; lines.len()];
    for mb in &mermaid_blocks {
        for row in mb.start..mb.end {
            if let Some(slot) = is_diagram_line.get_mut(row) {
                *slot = true;
            }
        }
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut mermaid: Vec<(usize, String)> = Vec::new();
    let mut mb_iter = mermaid_blocks.iter().peekable();

    for (i, line) in lines.into_iter().enumerate() {
        // 先检查是否有空图表（start == end == i）需要在此处插入 affordance 行
        while let Some(mb) = mb_iter.peek() {
            if mb.start == i && mb.start == mb.end {
                mermaid.push((out.len(), mb.body.to_string()));
                out.push(mermaid_affordance(theme, width));
                mb_iter.next();
            } else {
                break;
            }
        }

        let is_diag = is_diagram_line.get(i).copied().unwrap_or(false);
        if is_diag {
            // Mermaid 字符图行：保持整行原样，若超出面板宽度做安全右裁切，绝不拆成折行碎片
            if width > 0 && line_display_width(&line) > width {
                out.push(fit_line_to_width(line, width));
            } else {
                out.push(line);
            }
        } else if width == 0 {
            out.push(line);
        } else {
            out.extend(word_wrap_lines(vec![line], width));
        }

        // 检查当前行是否为某个非空图表的结束行（i + 1 == mb.end）
        while let Some(mb) = mb_iter.peek() {
            if mb.end == i + 1 && mb.start < mb.end {
                mermaid.push((out.len(), mb.body.to_string()));
                out.push(mermaid_affordance(theme, width));
                mb_iter.next();
            } else {
                break;
            }
        }
    }

    // 处理位于末尾的空图表
    for mb in mb_iter {
        mermaid.push((out.len(), mb.body.to_string()));
        out.push(mermaid_affordance(theme, width));
    }

    Rendered {
        lines: out,
        mermaid,
    }
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

    /// 流程图字符画不参与普通文本的 word wrap：
    /// 流程图内部有大量的对齐空格、居中对齐符号（例如 `▼`、`│`、`┌` 等）。
    /// 字符图行必须保持完整，不能被当成普通段落拆成折行碎片。
    #[test]
    fn mermaid_diagram_lines_are_not_word_wrapped() {
        let theme = Theme::groknight();
        let md = concat!(
            "```mermaid\n",
            "flowchart TD\n",
            "    A[Start Node Here] --> B{Is it working?}\n",
            "    B -->|Yes| C[Ship it]\n",
            "    B -->|No| D[Debug it]\n",
            "```\n",
        );
        let rendered = render(md, &theme, 60);
        assert_eq!(rendered.mermaid.len(), 1);
        let joined: String = rendered
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        // 验证含有 box-drawing 制表符与箭头
        assert!(
            joined.contains('┌') || joined.contains('╭'),
            "got:\n{joined}"
        );
        assert!(joined.contains('│'), "got:\n{joined}");
        assert!(
            joined.contains('▼') || joined.contains('▲'),
            "got:\n{joined}"
        );
        // 验证每一行都不以孤立的断裂碎片出现，节点文字完整
        assert!(joined.contains("Start Node Here"), "got:\n{joined}");
        assert!(joined.contains("Is it working?"), "got:\n{joined}");
        assert!(joined.contains("Ship it"), "got:\n{joined}");
        // 验证 affordance 行排在图表后面
        let affordance_line_idx = rendered.mermaid[0].0;
        let affordance_row_text: String = rendered.lines[affordance_line_idx]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(affordance_row_text.contains("[Open Image]"));
    }

    /// 图表前后的普通段落文本，仍然遵循正常的 word wrap 规则。
    #[test]
    fn text_around_mermaid_still_wraps_normally() {
        let theme = Theme::groknight();
        let md = concat!(
            "This is a long introductory sentence that definitely exceeds the small column limit and needs to be wrapped.\n\n",
            "```mermaid\nflowchart TD\nA-->B\n```\n\n",
            "This is a trailing sentence after the diagram that also needs normal word wrapping across lines.\n",
        );
        let rendered = render(md, &theme, 35);
        assert_eq!(rendered.mermaid.len(), 1);
        // 验证前后的普通长句被折成了多行
        assert!(rendered.lines.len() > 10);
        let joined: String = rendered
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("introductory") && joined.contains("sentence"));
        assert!(joined.contains("trailing") && joined.contains("sentence"));
        assert!(joined.contains("[Open Image]"));
    }
}
