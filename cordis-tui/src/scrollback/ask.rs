//! Ask-user tool card — questions from args while pending; Q→A after answer.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::grok::wrapping::word_wrap_lines;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

pub fn is_ask_tool(name: &str) -> bool {
    matches!(
        name,
        "ask_user_question" | "AskUserQuestion" | "Ask" | "Ask User"
    )
}

pub fn lines(
    name: &str,
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    let _ = name;
    let qa = parse_qa_pairs(content);
    let pending = parse_pending_questions(arguments);
    let declined =
        content.starts_with("User declined") || content.starts_with("No user is available");
    let failed = content.starts_with("Error");

    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed || declined;
    let count = if !qa.is_empty() {
        qa.len()
    } else {
        pending.len().max(1)
    };
    let title = header_title(&qa, &pending, declined, failed, running);
    let mut header = header_line(count, &title, muted, failed || declined, theme, width);
    prepend_diamond(&mut header, theme, failed || declined);
    if running && !open {
        live::mark_running(&mut header, theme);
        return vec![header];
    }
    if !open {
        return vec![header];
    }

    let mut out = vec![header, Line::from("")];
    if failed || declined {
        for line in content.lines() {
            out.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(if failed {
                    theme.accent_error
                } else {
                    theme.gray
                }),
            )));
        }
        return if width == 0 {
            out
        } else {
            word_wrap_lines(out, width)
        };
    }

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
    } else if !pending.is_empty() {
        for (i, question) in pending.iter().enumerate() {
            out.push(Line::from(vec![
                Span::styled(format!("  {}. ", i + 1), theme.muted()),
                Span::styled(question.clone(), theme.primary()),
            ]));
            if running {
                out.push(Line::from(Span::styled(
                    "     等待回答\u{2026}".to_string(),
                    theme.dim(),
                )));
            }
        }
    } else if !content.is_empty() {
        for line in content.lines() {
            out.push(Line::from(Span::styled(line.to_string(), theme.muted())));
        }
    }

    if width == 0 {
        out
    } else {
        word_wrap_lines(out, width)
    }
}

fn header_title(
    qa: &[(String, String)],
    pending: &[String],
    declined: bool,
    failed: bool,
    running: bool,
) -> String {
    if failed {
        return "失败".into();
    }
    if declined {
        return "已拒绝".into();
    }
    if let Some((q, _)) = qa.first() {
        return q.clone();
    }
    if let Some(q) = pending.first() {
        return q.clone();
    }
    if running {
        "等待回答".into()
    } else {
        String::new()
    }
}

fn header_line(
    count: usize,
    title: &str,
    muted: bool,
    bad: bool,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let label_fg = if bad {
        theme.accent_error
    } else if muted {
        theme.gray
    } else {
        theme.accent_tool
    };
    let title_fg = if muted {
        theme.gray
    } else {
        theme.text_primary
    };
    let count_label = if count <= 1 {
        String::new()
    } else {
        format!(" · {count}")
    };
    let mut spans = vec![Span::styled(
        format!("Ask{count_label} "),
        Style::default().fg(label_fg).add_modifier(Modifier::BOLD),
    )];
    if !title.is_empty() {
        spans.push(Span::styled(
            title.to_string(),
            Style::default().fg(title_fg),
        ));
    }
    let line = Line::from(spans);
    if width == 0 {
        line
    } else {
        truncate_line(line, width.saturating_sub(2).max(8))
    }
}

fn prepend_diamond(line: &mut Line<'static>, theme: &Theme, bad: bool) {
    let fg = if bad {
        theme.accent_error
    } else {
        theme.accent_tool
    };
    line.spans.insert(
        0,
        Span::styled(
            format!("{} ", glyphs::diamond_filled()),
            Style::default().fg(fg),
        ),
    );
}

fn parse_pending_questions(arguments: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return Vec::new();
    };
    let Some(arr) = v.get("questions").and_then(|q| q.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|q| {
            q.get("question")
                .and_then(|s| s.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .collect()
}

/// Parse ask tool result into `(question, display_answer)` pairs.
///
/// For `"Other" user notes: …` the display answer is the freeform notes
/// (what the user typed), not the bare `Other` wire label.
pub fn parse_qa_pairs(output: &str) -> Vec<(String, String)> {
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
            // Labels are quoted: `Label" …` then optional annotations, then
            // either `, "next` or end-of-body.
            let label_end = remaining.find('"').unwrap_or(remaining.len());
            let label = remaining[..label_end].to_string();
            remaining = if label_end < remaining.len() {
                &remaining[label_end + 1..]
            } else {
                &remaining[label_end..]
            };

            let mut notes: Option<String> = None;
            let mut preview_skip = false;
            if remaining.starts_with(" selected preview:") {
                preview_skip = true;
                remaining = &remaining[" selected preview:".len()..];
            }
            if preview_skip {
                // Preview may contain newlines; stop at ` user notes:` or `, "`.
                if let Some(n) = remaining.find(" user notes: ") {
                    remaining = &remaining[n..];
                } else if let Some(n) = remaining.find(", \"") {
                    remaining = &remaining[n..];
                } else {
                    remaining = "";
                }
            }
            if let Some(rest) = remaining.strip_prefix(" user notes: ") {
                let note_end = rest.find(", \"").unwrap_or(rest.len());
                notes = Some(rest[..note_end].to_string());
                remaining = &rest[note_end..];
            }
            if remaining.starts_with(", ") {
                remaining = &remaining[2..];
            }

            let answer = match notes {
                Some(n) if label.eq_ignore_ascii_case("Other") || label == "其他" => n,
                Some(n) => format!("{label} · {n}"),
                None => label,
            };
            pairs.push((question, answer));
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
    fn answered_card_shows_qa() {
        let theme = Theme::current();
        let content = r#"User has answered your questions: "Which DB?"="Redis", "Which UI?"="React". You can now continue with the user's answers in mind."#;
        let lines = lines(
            "ask_user_question",
            r#"{"questions":[]}"#,
            content,
            &theme,
            80,
            ToolMode::Expanded,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Ask"), "{text}");
        assert!(text.contains("Which DB?"), "{text}");
        assert!(text.contains("Redis"), "{text}");
        assert!(text.contains("React"), "{text}");
    }

    #[test]
    fn other_notes_shown_instead_of_bare_other() {
        let content = r#"User has answered your questions: "你希望我接下来以哪种模式协助你？"="Other" user notes: www, "哪些功能是你最想测试的？ (可多选)"="文件操作". You can now continue with the user's answers in mind."#;
        let pairs = parse_qa_pairs(content);
        assert_eq!(pairs.len(), 2, "{pairs:?}");
        assert_eq!(pairs[0].1, "www");
        assert_eq!(pairs[1].1, "文件操作");
        assert!(!pairs[0].1.contains('"'), "{pairs:?}");
    }

    #[test]
    fn pending_card_lists_questions() {
        let theme = Theme::current();
        let args = r#"{"questions":[{"question":"Pick one?","options":[{"label":"A"}]},{"question":"Pick two?","options":[{"label":"B"}]}]}"#;
        let lines = lines(
            "ask_user_question",
            args,
            "",
            &theme,
            80,
            ToolMode::Expanded,
            true,
        );
        let text = plain(&lines);
        assert!(text.contains("Pick one?"), "{text}");
        assert!(text.contains("Pick two?"), "{text}");
        assert!(text.contains("等待回答"), "{text}");
    }

    #[test]
    fn collapsed_hides_body() {
        let theme = Theme::current();
        let content = r#"User has answered your questions: "Q"="A". You can now continue with the user's answers in mind."#;
        let lines = lines(
            "ask_user_question",
            "{}",
            content,
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Ask"), "{text}");
        assert!(!text.contains("\u{2192}"), "{text}");
    }
}
