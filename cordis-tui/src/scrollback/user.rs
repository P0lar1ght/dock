//! User prompt block — copied from grok pager `scrollback/blocks/user.rs`
//! (`prompt_styles`, `prompt_band_color_for`, `wrap_prompt_lines`).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::grok::glyphs;
use crate::grok::wrapping::{word_wrap_line_with_joiners, RtOptions};
use crate::theme::Theme;

fn prompt_styles(theme: &Theme, terminal_native: bool) -> (Style, Style, Style) {
    let prefix_color = match theme.accent_user {
        ratatui::style::Color::Reset => ratatui::style::Color::Cyan,
        c => c,
    };
    let mut prefix_style = theme.fg(prefix_color);
    let mut text_style = theme.fg(theme.text_primary);
    let mut skill_style = theme.fg(theme.accent_skill);
    if terminal_native {
        prefix_style = prefix_style.add_modifier(Modifier::BOLD);
        text_style = text_style.add_modifier(Modifier::BOLD);
        skill_style = skill_style.add_modifier(Modifier::BOLD);
    }
    (prefix_style, text_style, skill_style)
}

fn prompt_band_color_for(
    theme: &Theme,
    is_selected: bool,
    terminal_native: bool,
) -> Option<ratatui::style::Color> {
    use ratatui::style::Color;
    if terminal_native {
        Some(if is_selected {
            Color::Gray
        } else {
            Color::DarkGray
        })
    } else {
        match theme.bg_light {
            Color::Reset => None,
            c => Some(c),
        }
    }
}

fn with_band(mut line: Line<'static>, band: Option<ratatui::style::Color>) -> Line<'static> {
    if let Some(c) = band {
        for span in &mut line.spans {
            span.style = span.style.bg(c);
        }
        line.style = line.style.bg(c);
    }
    line
}

/// Skill token = first `/…` on the first line (Grok `UserPromptBlock::skill`).
fn skill_token_end(text: &str) -> Option<usize> {
    let first = text.lines().next().unwrap_or("");
    if !first.starts_with('/') {
        return None;
    }
    let end = first.find(char::is_whitespace).unwrap_or(first.len());
    (end > 1).then_some(end)
}

pub fn lines(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let terminal_native = false;
    let (prefix_style, text_style, skill_style) = prompt_styles(theme, terminal_native);
    let band = prompt_band_color_for(theme, false, terminal_native);

    let is_bash = text.starts_with('!');
    let is_cron = text.starts_with('\u{21BB}');
    let body = if is_bash {
        text.strip_prefix('!').unwrap_or(text).trim_start()
    } else if is_cron {
        text.trim_start_matches('\u{21BB}').trim_start()
    } else {
        text
    };
    let prefix = if is_bash {
        "$ "
    } else if is_cron {
        "\u{21BB}  "
    } else {
        glyphs::prompt_arrow()
    };
    let prefix_width = prefix.width();
    let content_width = width.saturating_sub(prefix_width).max(1);
    let skill_end = if is_bash || is_cron {
        None
    } else {
        skill_token_end(body)
    };

    let logical: Vec<&str> = if body.is_empty() {
        vec![""]
    } else {
        body.lines().collect()
    };

    let mut out = Vec::new();
    let mut byte_off = 0usize;
    for (logical_idx, line_text) in logical.iter().enumerate() {
        let content_line = if let Some(end) = skill_end {
            if logical_idx == 0 {
                token_styled_line(line_text, byte_off, end, skill_style, text_style)
            } else {
                Line::from(Span::styled(line_text.to_string(), text_style))
            }
        } else {
            Line::from(Span::styled(line_text.to_string(), text_style))
        };
        let (wrapped, _) =
            word_wrap_line_with_joiners(&content_line, RtOptions::new(content_width));
        for (wrap_idx, wrapped_line) in wrapped.into_iter().enumerate() {
            let is_first = logical_idx == 0 && wrap_idx == 0;
            let head = if is_first {
                prefix
            } else {
                // indent continuation with prefix width of spaces
                ""
            };
            let indent = " ".repeat(prefix_width);
            let line_prefix = if is_first { head } else { indent.as_str() };
            let mut spans = vec![Span::styled(line_prefix.to_string(), prefix_style)];
            spans.extend(wrapped_line.spans.into_iter().map(|s| Span {
                content: s.content.to_string().into(),
                style: s.style,
            }));
            out.push(with_band(Line::from(spans), band));
        }
        byte_off += line_text.len() + 1;
    }
    if out.is_empty() {
        out.push(with_band(
            Line::from(Span::styled(prefix.to_string(), prefix_style)),
            band,
        ));
    }
    out
}

fn token_styled_line(
    line_text: &str,
    _line_start: usize,
    token_end: usize,
    token_style: Style,
    body_style: Style,
) -> Line<'static> {
    let end = token_end.min(line_text.len());
    if end == 0 {
        return Line::from(Span::styled(line_text.to_string(), body_style));
    }
    let token = line_text[..end].to_string();
    let rest = line_text[end..].to_string();
    if rest.is_empty() {
        Line::from(Span::styled(token, token_style))
    } else {
        Line::from(vec![
            Span::styled(token, token_style),
            Span::styled(rest, body_style),
        ])
    }
}
