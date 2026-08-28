//! Generic tool card — copied from grok pager `OtherToolCallBlock`
//! (`collapsed_line`, expanded muted output, AskUserQuestion Q&A).
//!
//! Pager draws `◆` via `prepend_bullet` in the gutter. We have no gutter, so
//! the same glyph is inserted on the header line.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::grok::wrapping::word_wrap_lines;
use crate::theme::Theme;

pub fn lines(
    name: &str,
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    expanded: bool,
    running: bool,
) -> Vec<Line<'static>> {
    let muted = !expanded && !running;
    let summary = argument_summary(name, arguments);
    let mut header = if is_shell(name) {
        shell_header(&summary, theme, muted, width)
    } else {
        collapsed_line(name, &summary, theme, muted, Some(width.saturating_sub(2)))
    };
    prepend_diamond(&mut header, theme);
    if running && !expanded {
        header.spans.push(Span::styled("  运行中…".to_string(), theme.muted()));
        return vec![header];
    }
    if !expanded {
        return vec![header];
    }

    let mut out = vec![header];
    let qa = parse_ask_user_qa_pairs(content);
    if !qa.is_empty() {
        for (i, (question, answer)) in qa.iter().enumerate() {
            out.push(Line::from(vec![
                Span::styled(format!("  {}. ", i + 1), theme.muted()),
                Span::styled(question.clone(), theme.primary()),
            ]));
            let a_line = if answer.is_empty() {
                Line::from(Span::styled("     (no answer)".to_string(), theme.dim()))
            } else {
                Line::from(vec![
                    Span::styled("     \u{2192} ".to_string(), theme.fg(theme.accent_user)),
                    Span::styled(answer.clone(), theme.fg(theme.accent_user)),
                ])
            };
            out.push(a_line);
        }
        return if width == 0 {
            out
        } else {
            word_wrap_lines(out, width)
        };
    }

    out.push(Line::from(""));
    if !is_shell(name) && !arguments.trim().is_empty() {
        out.push(Line::from(Span::styled("输入".to_string(), theme.dim())));
        let input = pretty_args(arguments);
        let styled: Vec<Line<'static>> = input
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), theme.muted())))
            .collect();
        let wrap_w = width.saturating_sub(2).max(20);
        out.extend(word_wrap_lines(styled, wrap_w));
        out.push(Line::from(""));
    }
    if running && content.is_empty() {
        out.push(Line::from(Span::styled("运行中…".to_string(), theme.muted())));
        return out;
    }
    if content.is_empty() {
        return out;
    }
    out.push(Line::from(Span::styled("输出".to_string(), theme.dim())));
    let styled: Vec<Line<'static>> = content
        .lines()
        .map(|line| Line::from(Span::styled(line.to_string(), theme.muted())))
        .collect();
    let wrap_w = width.saturating_sub(2).max(20);
    out.extend(word_wrap_lines(styled, wrap_w));
    out
}

fn is_shell(name: &str) -> bool {
    matches!(name, "bash" | "run_terminal_cmd" | "execute")
}

fn shell_header(command: &str, theme: &Theme, muted: bool, width: usize) -> Line<'static> {
    let cmd_style = if muted { theme.muted() } else { theme.primary() };
    let cmd = if command.trim().is_empty() {
        "\u{2026}".to_string()
    } else {
        command.replace('\n', " ")
    };
    let line = Line::from(vec![
        Span::styled("$ ".to_string(), theme.dim()),
        Span::styled(cmd, cmd_style),
    ]);
    if width == 0 {
        line
    } else {
        truncate_line(line, width.saturating_sub(2).max(8))
    }
}

fn pretty_args(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| raw.to_string())
}

fn json_str<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()))
}

fn argument_summary(name: &str, arguments: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let picked = match name {
        "bash" | "run_terminal_cmd" | "execute" => json_str(&v, &["command"]),
        "read_file" | "write_file" => json_str(&v, &["target_file", "path"]),
        "list_dir" => json_str(&v, &["target_directory", "path"]),
        "grep" => json_str(&v, &["pattern"]),
        "glob" => json_str(&v, &["glob_pattern", "pattern"]),
        "search_replace" => json_str(&v, &["file_path", "path"]),
        _ => None,
    };
    if let Some(s) = picked {
        return s.to_string();
    }
    if let Some(obj) = v.as_object() {
        if let Some((_, serde_json::Value::String(s))) = obj.iter().next() {
            return s.clone();
        }
    }
    let flat = arguments.replace('\n', " ");
    if flat.chars().count() > 60 {
        format!("{}…", flat.chars().take(59).collect::<String>())
    } else {
        flat
    }
}

/// Copied from grok `OtherToolCallBlock::collapsed_line`.
fn collapsed_line(
    name: &str,
    summary: &str,
    theme: &Theme,
    muted: bool,
    width: Option<usize>,
) -> Line<'static> {
    let text_style = if muted {
        theme.muted()
    } else {
        theme.primary()
    };
    let bold_style = text_style.add_modifier(Modifier::BOLD);

    let mut spans = if let Some((label, content)) = name.split_once(": ") {
        vec![
            Span::styled(format!("{label} "), bold_style),
            Span::styled(content.to_string(), text_style),
        ]
    } else {
        vec![Span::styled(name.to_string(), bold_style)]
    };

    if !summary.is_empty() {
        if let Some(w) = width {
            let used: usize = spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            let summary = format!("  {summary}");
            if used + summary.len() <= w {
                spans.push(Span::styled(summary, theme.muted()));
            }
        } else {
            spans.push(Span::styled(format!("  {summary}"), theme.muted()));
        }
    }

    let line = Line::from(spans);
    match width {
        Some(w) => truncate_line(line, w),
        None => line,
    }
}

/// Copied from grok `prepend_bullet`: insert `{glyph} ` at the start of the line.
fn prepend_diamond(line: &mut Line<'static>, theme: &Theme) {
    let bullet_span = Span::styled(
        format!("{} ", glyphs::diamond_filled()),
        Style::default().fg(theme.accent_tool),
    );
    line.spans.insert(0, bullet_span);
}

/// Copied from grok `OtherToolCallBlock` helper in `tool/other.rs`.
fn parse_ask_user_qa_pairs(output: &str) -> Vec<(String, String)> {
    if let Some(rest) = output.strip_prefix("User has answered your questions: ") {
        let body = rest
            .strip_suffix(". You can now continue with the user's answers in mind.")
            .unwrap_or(rest);
        if body.is_empty() {
            return vec![];
        }
        let mut pairs = Vec::new();
        let mut remaining = body;
        while !remaining.is_empty() {
            if !remaining.starts_with('"') {
                break;
            }
            remaining = &remaining[1..];
            let Some(q_end) = remaining.find("\"=\"") else {
                break;
            };
            let question = remaining[..q_end].to_string();
            remaining = &remaining[q_end + 3..];
            let answer_end = remaining.find(", \"").unwrap_or(remaining.len());
            let mut answer_text = remaining[..answer_end].to_string();
            if answer_text.ends_with('"') {
                answer_text.pop();
            }
            if let Some(ann_start) = answer_text.find(" selected preview:") {
                answer_text.truncate(ann_start);
            }
            if let Some(ann_start) = answer_text.find(" user notes:") {
                answer_text.truncate(ann_start);
            }
            pairs.push((question, answer_text));
            remaining = &remaining[answer_end..];
            if remaining.starts_with(", ") {
                remaining = &remaining[2..];
            }
        }
        return pairs;
    }
    if output.starts_with("User declined to answer") {
        return vec![];
    }
    if output.contains("Questions asked") && output.contains("- \"") {
        let mut pairs = Vec::new();
        let lines: Vec<&str> = output.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i].trim_start_matches([' ', '-']).trim();
            if line.starts_with('"') && line.ends_with('"') {
                let question = line[1..line.len() - 1].to_string();
                let answer = if i + 1 < lines.len() {
                    let next = lines[i + 1].trim();
                    if let Some(a) = next.strip_prefix("Answer: ") {
                        i += 1;
                        a.to_string()
                    } else if next == "(No answer provided)" {
                        i += 1;
                        String::new()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };
                pairs.push((question, answer));
            }
            i += 1;
        }
        if !pairs.is_empty() {
            return pairs;
        }
    }
    vec![]
}
