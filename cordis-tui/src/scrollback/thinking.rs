//! Thinking block — Grok `ThinkingBlock` collapsed header + click to expand.
//! Streaming shows a truncated last-N tail; done collapses to one line.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::grok::wrapping::word_wrap_lines;
use crate::theme::Theme;

const TRUNCATED_LINES: usize = 3;

pub fn lines(
    text: &str,
    elapsed_ms: Option<u64>,
    streaming: bool,
    theme: &Theme,
    width: usize,
    expanded: bool,
) -> Vec<Line<'static>> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let mut header = header_line(elapsed_ms, streaming, expanded, theme);
    prepend_diamond(&mut header, theme);
    if width > 0 {
        header = truncate_line(header, width);
    }
    if expanded {
        let mut out = vec![header, Line::from("")];
        out.extend(body_lines(text, theme, width));
        return out;
    }
    if streaming {
        let mut out = vec![header];
        out.extend(truncated_tail(text, theme, width));
        return out;
    }
    vec![header]
}

fn header_line(
    elapsed_ms: Option<u64>,
    streaming: bool,
    expanded: bool,
    theme: &Theme,
) -> Line<'static> {
    let label_style = if streaming || expanded {
        theme.primary().add_modifier(Modifier::BOLD)
    } else {
        theme.muted().add_modifier(Modifier::BOLD)
    };
    let mut spans = if streaming {
        vec![Span::styled("思考中".to_string(), label_style)]
    } else if let Some(ms) = elapsed_ms {
        vec![
            Span::styled("思考".to_string(), label_style),
            Span::styled(format!("了 {}", format_time(ms)), theme.muted()),
        ]
    } else {
        vec![Span::styled("思考".to_string(), label_style)]
    };
    if !expanded && !streaming {
        spans.push(Span::styled("  （点击展开）".to_string(), theme.dim()));
    }
    Line::from(spans)
}

fn format_time(ms: u64) -> String {
    let secs = ms as f64 / 1000.0;
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let mins = (secs / 60.0).floor() as u32;
        let remaining = secs - (mins as f64 * 60.0);
        format!("{mins}m{remaining:.0}s")
    }
}

fn body_style(theme: &Theme) -> Style {
    theme
        .muted()
        .add_modifier(Modifier::DIM)
        .add_modifier(Modifier::ITALIC)
}

fn body_lines(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let style = body_style(theme);
    let styled: Vec<Line<'static>> = text
        .lines()
        .map(|line| Line::from(Span::styled(line.to_string(), style)))
        .collect();
    if width == 0 {
        styled
    } else {
        word_wrap_lines(styled, width.saturating_sub(2).max(20))
    }
}

fn truncated_tail(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let wrapped = body_lines(text, theme, width);
    if wrapped.is_empty() {
        return Vec::new();
    }
    if wrapped.len() <= TRUNCATED_LINES {
        return wrapped;
    }
    let mut out = vec![Line::from(Span::styled("…".to_string(), theme.muted()))];
    out.extend(wrapped.into_iter().rev().take(TRUNCATED_LINES).rev());
    out
}

fn prepend_diamond(line: &mut Line<'static>, theme: &Theme) {
    line.spans.insert(
        0,
        Span::styled(
            format!("{} ", glyphs::diamond_filled()),
            Style::default().fg(theme.accent_thinking),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'_>]) -> String {
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
    fn collapsed_hides_body() {
        let theme = Theme::current();
        let text = plain(&lines("secret plan", Some(1200), false, &theme, 80, false));
        assert!(text.contains("思考"), "{text}");
        assert!(text.contains("1.2s"), "{text}");
        assert!(!text.contains("secret plan"), "{text}");
    }

    #[test]
    fn expanded_shows_body() {
        let theme = Theme::current();
        let text = plain(&lines("secret plan", None, false, &theme, 80, true));
        assert!(text.contains("secret plan"), "{text}");
    }

    #[test]
    fn streaming_shows_tail() {
        let theme = Theme::current();
        let text = plain(&lines("step one\nstep two", None, true, &theme, 80, false));
        assert!(text.contains("思考中"), "{text}");
        assert!(text.contains("step two"), "{text}");
    }
}
