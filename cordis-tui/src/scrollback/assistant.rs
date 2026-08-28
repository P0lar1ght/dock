//! Agent message — Grok `StreamingMarkdownRenderer` + pager mermaid affordance.

use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::md_style;
use crate::grok::wrapping::word_wrap_lines;
use crate::theme::Theme;

pub fn lines(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let syntect = cordis_markdown::default_syntect();
    let mut renderer = cordis_markdown::StreamingMarkdownRenderer::new(md_style::style(), true);
    renderer.push(text);
    let output = renderer.finish_into_output(Some(&syntect));
    let mut lines = output.lines;
    // Copied from grok `mermaid_content` affordance row (no PNG / overlay).
    for cb in output.code_blocks.iter().rev() {
        if cb.info.split_whitespace().next() != Some("mermaid") {
            continue;
        }
        let at = cb.output_line_range.end.min(lines.len());
        lines.insert(at, mermaid_affordance(theme));
    }
    if width == 0 {
        return lines;
    }
    word_wrap_lines(lines, width)
}

fn mermaid_affordance(theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{} mermaid", glyphs::diamond_hollow()),
            theme.dim(),
        ),
        Span::styled("  [Copy Source]".to_string(), theme.muted()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mermaid_gets_affordance_row() {
        let theme = Theme::groknight();
        let md = "```mermaid\nflowchart TD\nA-->B\n```\n";
        let rendered = lines(md, &theme, 80);
        let joined: String = rendered
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
            joined.contains("[Copy Source]"),
            "expected mermaid affordance, got:\n{joined}"
        );
        assert!(joined.contains("mermaid"));
    }
}
