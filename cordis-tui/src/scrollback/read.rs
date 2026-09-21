//! Read tool card — port of grok `ReadToolCallBlock` display.
//!
//! Collapsed: `◆ Read path (start-end)`. Open: gutter line numbers + body
//! (syntect when the path has a known syntax). Truncated: head 5 / tail 3.

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Mutex;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::FontStyle;

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::grok::wrapping::word_wrap_lines;
use crate::scrollback::card;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

/// Grok ReadToolCallBlock truncation (visual wrapped lines).
const FIRST_LINES: usize = 5;
const LAST_LINES: usize = 3;

const BODY_CACHE_CAP: usize = 16;
static BODY_CACHE: Mutex<Vec<(u64, Vec<Line<'static>>)>> = Mutex::new(Vec::new());

fn body_cache_key(path: &str, content: &str, mode: ToolMode, width: usize) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    content.hash(&mut h);
    std::mem::discriminant(&mode).hash(&mut h);
    width.hash(&mut h);
    // Gutter and un-highlightable lines take their colour from the live
    // palette, so a cached body belongs to the theme that built it.
    crate::theme::Theme::current_kind().hash(&mut h);
    h.finish()
}

pub fn is_read_tool(name: &str) -> bool {
    matches!(name, "read_file" | "read" | "memory_get")
}

pub fn lines(
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let meta = ReadMeta::from_args(arguments);
    let open = mode != ToolMode::Collapsed;
    let muted = !open && !running;

    let mut header = header_line(&meta, theme, muted, card::header_width(width));
    prepend_diamond(&mut header, theme);
    if running && !open {
        live::mark_running(&mut header, theme);
    }
    let header = card::finish_header(header, width, mode, theme);
    if !open {
        return vec![header];
    }

    let mut out = vec![header];
    if running && content.is_empty() {
        out.push(Line::from(""));
        out.push(card::indent(Line::from(live::running_body(theme))));
        return out;
    }
    if content.trim().is_empty() || content.starts_with("Error") {
        if !content.is_empty() {
            out.push(Line::from(""));
            let style = if content.starts_with("Error") {
                Style::default().fg(theme.accent_error)
            } else {
                theme.muted()
            };
            let rows: Vec<Line<'static>> = content
                .lines()
                .map(|line| Line::from(Span::styled(line.to_string(), style)))
                .collect();
            out.extend(card::body(rows, width));
        }
        return out;
    }

    out.push(Line::from(""));
    let body = render_body(&meta, content, theme, width, mode);
    out.extend(body);
    out
}

struct ReadMeta {
    path: String,
    /// 1-based start line from args (default 1).
    offset: usize,
    limit: Option<usize>,
}

impl ReadMeta {
    fn from_args(arguments: &str) -> Self {
        let v: serde_json::Value =
            serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
        let path = ["target_file", "path", "file_path"]
            .iter()
            .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
            .unwrap_or("")
            .to_string();
        let offset = v
            .get("offset")
            .or_else(|| v.get("from"))
            .and_then(|x| x.as_i64().or_else(|| x.as_u64().map(|n| n as i64)))
            .unwrap_or(1)
            .max(1) as usize;
        // `memory_get` uses `lines`; ordinary read tools use `limit`.
        let limit = v
            .get("limit")
            .or_else(|| v.get("lines"))
            .and_then(|x| x.as_u64().or_else(|| x.as_i64().map(|n| n.max(0) as u64)))
            .map(|n| n.max(1) as usize);
        Self {
            path,
            offset,
            limit,
        }
    }

    fn skill_name(&self) -> Option<&str> {
        let path = Path::new(&self.path);
        if path.file_name().is_some_and(|n| n == "SKILL.md") {
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
        } else {
            None
        }
    }

    fn display_path(&self) -> &str {
        if self.path.is_empty() {
            "…"
        } else {
            &self.path
        }
    }
}

fn header_line(meta: &ReadMeta, theme: &Theme, muted: bool, width: usize) -> Line<'static> {
    let text_style = if muted {
        theme.muted()
    } else {
        theme.primary()
    };
    let bold = text_style.add_modifier(Modifier::BOLD);
    let path_style = if muted {
        theme.muted()
    } else {
        theme.fg(theme.path)
    };
    let detail = theme.dim();

    if let Some(skill) = meta.skill_name() {
        let line = Line::from(vec![
            Span::styled("Skill ".to_string(), bold),
            Span::styled(skill.to_string(), path_style),
        ]);
        return if width == 0 {
            line
        } else {
            truncate_line(line, width)
        };
    }

    let range_suffix = match meta.limit {
        Some(lim) => {
            let start = meta.offset;
            let end = start.saturating_add(lim.saturating_sub(1));
            format!(" ({start}-{end})")
        }
        None => String::new(),
    };

    let mut spans = vec![
        Span::styled("Read ".to_string(), bold),
        Span::styled(meta.display_path().to_string(), path_style),
    ];
    if !range_suffix.is_empty() {
        spans.push(Span::styled(range_suffix, detail));
    }
    let line = Line::from(spans);
    if width == 0 {
        line
    } else {
        truncate_line(line, width)
    }
}

fn render_body(
    meta: &ReadMeta,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
) -> Vec<Line<'static>> {
    let key = body_cache_key(&meta.path, content, mode, width);
    if let Ok(guard) = BODY_CACHE.lock() {
        if let Some((_, lines)) = guard.iter().rev().find(|(k, _)| *k == key) {
            return lines.clone();
        }
    }

    let parsed = parse_content_lines(content, meta.offset);
    if parsed.is_empty() {
        return Vec::new();
    }

    // Truncated: only highlight the head/tail source lines. Highlighting the
    // full body then throwing it away is what made open Read cards freeze the
    // UI (build() runs every paint).
    let (slice, elided): (Vec<(usize, String)>, Option<usize>) = if mode == ToolMode::Truncated {
        let total = parsed.len();
        let threshold = FIRST_LINES + LAST_LINES;
        if total > threshold {
            let mut slice = parsed.iter().take(FIRST_LINES).cloned().collect::<Vec<_>>();
            slice.extend(parsed.iter().skip(total - LAST_LINES).cloned());
            (slice, Some(total - threshold))
        } else {
            (parsed, None)
        }
    } else {
        (parsed, None)
    };

    let last_num = slice.last().map(|(n, _)| *n).unwrap_or(meta.offset);
    let gutter_w = digit_count(last_num);
    // gutter 占 `gutter_w + 2` 列，正文再让出公共外壳的右边距。
    let row_w = card::body_width(width).saturating_sub(gutter_w + 2).max(20);

    let gutter_style = theme.dim();
    let text_style = theme.primary();

    let syntect = cordis_markdown::default_syntect();
    let mut highlighter = syntect.highlight_lines_by_file_path(Path::new(&meta.path));

    let mut styled: Vec<Line<'static>> = Vec::with_capacity(slice.len() + 1);
    for (i, (num, text)) in slice.iter().enumerate() {
        if let (Some(hidden), true) = (elided, i == FIRST_LINES) {
            // 原先只画一个光秃秃的 `…`，连省了多少行都不说 —— 全仓唯一一个
            // 不报条数的截断点。
            styled.push(card::elision(hidden, "行", theme));
            // Reset highlighter across the gap so state doesn't bleed.
            highlighter = syntect.highlight_lines_by_file_path(Path::new(&meta.path));
        }
        let gutter = format!("{:>w$}  ", num, w = gutter_w);
        let mut spans = vec![Span::styled(gutter, gutter_style)];
        spans.extend(highlight_spans(text, &mut highlighter, syntect, text_style));
        // 行号 gutter 是这张卡自己的正文语言，但左边缘仍要和别的卡齐。
        styled.push(card::indent(Line::from(spans)));
    }

    let out = if width == 0 {
        styled
    } else {
        // Wrap only the text portion: word_wrap_lines wraps whole lines including
        // gutter. Acceptable for cordis; matching grok joiner hang is heavier.
        word_wrap_lines(styled, row_w.saturating_add(gutter_w + 2))
    };

    if let Ok(mut guard) = BODY_CACHE.lock() {
        guard.push((key, out.clone()));
        if guard.len() > BODY_CACHE_CAP {
            let extra = guard.len() - BODY_CACHE_CAP;
            guard.drain(..extra);
        }
    }
    out
}

fn highlight_spans(
    text: &str,
    highlighter: &mut Option<HighlightLines<'_>>,
    syntect: &cordis_markdown::Syntect,
    fallback: Style,
) -> Vec<Span<'static>> {
    let Some(hl) = highlighter.as_mut() else {
        return vec![Span::styled(text.to_string(), fallback)];
    };
    match hl.highlight_line(text, &syntect.syntax_set) {
        Ok(regions) => regions
            .into_iter()
            .map(|(style, s)| Span::styled(s.to_string(), syntect_to_ratatui_fg(style)))
            .collect(),
        Err(_) => vec![Span::styled(text.to_string(), fallback)],
    }
}

fn syntect_to_ratatui_fg(style: syntect::highlighting::Style) -> Style {
    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
    let mut out = Style::default().fg(fg);
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        out = out.add_modifier(Modifier::UNDERLINED);
    }
    out
}

/// Strip workspace `N→` anchors; recover absolute line numbers.
fn parse_content_lines(content: &str, default_start: usize) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut next = default_start;
    for raw in content.lines() {
        let (num, text) = match strip_anchor(raw) {
            Some((n, rest)) => {
                next = n + 1;
                (n, rest.to_string())
            }
            None => {
                let n = next;
                next += 1;
                (n, raw.to_string())
            }
        };
        out.push((num, text));
    }
    out
}

fn strip_anchor(line: &str) -> Option<(usize, &str)> {
    let (num, rest) = line.split_once('\u{2192}')?; // →
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((num.parse().ok()?, rest))
}

fn digit_count(n: usize) -> usize {
    n.checked_ilog10().map_or(1, |d| d as usize + 1)
}

fn prepend_diamond(line: &mut Line<'static>, theme: &Theme) {
    line.spans.insert(
        0,
        Span::styled(
            format!("{} ", glyphs::diamond_filled()),
            Style::default().fg(theme.accent_tool),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'static>]) -> String {
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
    fn collapsed_shows_read_header_hides_body() {
        let theme = Theme::current();
        let body = "1→fn main() {}\nlet x = 1;\n10→done\n";
        let lines = lines(
            r#"{"target_file":"src/main.rs"}"#,
            body,
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Read "), "{text}");
        assert!(text.contains("src/main.rs"), "{text}");
        assert!(!text.contains("fn main"), "{text}");
    }

    #[test]
    fn truncated_strips_anchors_and_hides_middle() {
        let theme = Theme::current();
        let body: String = (1..=20)
            .map(|i| {
                if i == 1 || i % 10 == 0 {
                    format!("{i}→L{i:02}\n")
                } else {
                    format!("L{i:02}\n")
                }
            })
            .collect();
        let lines = lines(
            r#"{"target_file":"a.rs","offset":1}"#,
            &body,
            &theme,
            80,
            ToolMode::Truncated,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("L01"), "{text}");
        assert!(text.contains('\u{2026}'), "{text}");
        assert!(!text.contains("L10"), "{text}");
        assert!(!text.contains('\u{2192}'), "anchors stripped: {text}");
    }

    #[test]
    fn skill_path_uses_skill_label() {
        let theme = Theme::current();
        let lines = lines(
            r#"{"target_file":"/x/skills/deploy/SKILL.md"}"#,
            "1→# Deploy\n",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Skill "), "{text}");
        assert!(text.contains("deploy"), "{text}");
        assert!(!text.contains("Read "), "{text}");
    }

    #[test]
    fn memory_get_is_a_read_tool() {
        assert!(is_read_tool("memory_get"));
        assert!(is_read_tool("read_file"));
        assert!(!is_read_tool("memory_search"));
    }

    #[test]
    fn memory_get_args_map_path_from_lines() {
        let theme = Theme::current();
        let lines = lines(
            r#"{"path":"topics/api.md","from":10,"lines":5}"#,
            "10→hello\n",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Read "), "{text}");
        assert!(text.contains("topics/api.md"), "{text}");
        assert!(text.contains("(10-14)"), "{text}");
    }

    #[test]
    fn limit_shows_range_suffix() {
        let theme = Theme::current();
        let lines = lines(
            r#"{"target_file":"f.rs","offset":10,"limit":5}"#,
            "hello",
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("(10-14)"), "{text}");
    }
}
