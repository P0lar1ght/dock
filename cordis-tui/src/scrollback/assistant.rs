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
    renderer.push(text);
    let output = renderer.finish_into_output(Some(syntect));
    let mermaid_bodies: Vec<String> = output
        .code_blocks
        .iter()
        .filter(|cb| cb.info.split_whitespace().next() == Some("mermaid"))
        .map(|cb| cb.body.clone())
        .collect();
    let mut lines = output.lines;
    for cb in output.code_blocks.iter().rev() {
        if cb.info.split_whitespace().next() != Some("mermaid") {
            continue;
        }
        let at = cb.output_line_range.end.min(lines.len());
        lines.insert(at, mermaid_affordance(theme));
    }
    let lines = if width == 0 {
        lines
    } else {
        word_wrap_lines(lines, width)
    };
    let mut mermaid = Vec::new();
    let mut i = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        if mermaid::is_affordance_text(&text) {
            if let Some(body) = mermaid_bodies.get(i) {
                mermaid.push((idx, body.clone()));
                i += 1;
            }
        }
    }
    Rendered { lines, mermaid }
}

fn mermaid_affordance(theme: &Theme) -> Line<'static> {
    mermaid::line(theme)
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
}
