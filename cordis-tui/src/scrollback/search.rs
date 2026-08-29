//! Search / grep / glob card — simplified grok `SearchToolCallBlock`.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::grok::glyphs;
use crate::grok::line_utils::truncate_line;
use crate::grok::wrapping::word_wrap_lines;
use crate::scrollback::live;
use crate::scrollback::tool::ToolMode;
use crate::theme::Theme;

const FIRST_FILES: usize = 6;
const HITS_PER_FILE_TRUNC: usize = 3;

pub fn is_search_tool(name: &str) -> bool {
    matches!(name, "grep" | "search" | "glob")
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
    let is_glob = name == "glob";
    let pattern = pattern_from_args(arguments, is_glob).unwrap_or_default();
    let path = path_from_args(arguments);
    let failed = content.starts_with("Error");
    let open = mode != ToolMode::Collapsed;
    let muted = (!open && !running) || failed;

    let parsed = if is_glob {
        Parsed::files_only(content)
    } else {
        Parsed::from_grep(content)
    };

    let mut header = header_line(
        is_glob,
        &pattern,
        path.as_deref(),
        &parsed,
        muted,
        theme,
        width.saturating_sub(2).max(8),
    );
    prepend_diamond(&mut header, theme, failed);
    if running && !open {
        live::mark_running(&mut header, theme);
        return vec![header];
    }
    if !open {
        return vec![header];
    }

    let mut out = vec![header, Line::from("")];
    if failed {
        for line in content.lines() {
            out.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(theme.accent_error),
            )));
        }
        return out;
    }
    if parsed.is_empty() {
        out.push(Line::from(Span::styled(
            "  (no results)".to_string(),
            theme.muted(),
        )));
        return out;
    }

    let trunc = mode == ToolMode::Truncated;
    let file_cap = if trunc { FIRST_FILES } else { usize::MAX };
    let hit_cap = if trunc {
        HITS_PER_FILE_TRUNC
    } else {
        usize::MAX
    };
    let mut shown_files = 0usize;
    for file in &parsed.files {
        if shown_files >= file_cap {
            let rest = parsed.files.len() - shown_files;
            out.push(Line::from(Span::styled(
                format!("  \u{2026} +{rest} more files"),
                theme.muted(),
            )));
            break;
        }
        out.push(Line::from(Span::styled(
            format!("  {}", file.path),
            theme.fg(theme.path),
        )));
        for (i, hit) in file.hits.iter().enumerate() {
            if i >= hit_cap {
                let rest = file.hits.len() - hit_cap;
                out.push(Line::from(Span::styled(
                    format!("    \u{2026} +{rest} more"),
                    theme.muted(),
                )));
                break;
            }
            if let Some(n) = hit.line {
                out.push(Line::from(vec![
                    Span::styled(format!("    {n:>4}  "), theme.dim()),
                    Span::styled(hit.text.clone(), theme.primary()),
                ]));
            } else if !hit.text.is_empty() {
                out.push(Line::from(Span::styled(
                    format!("    {}", hit.text),
                    theme.muted(),
                )));
            }
        }
        shown_files += 1;
    }
    if width > 0 {
        out = word_wrap_lines(out, width);
    }
    out
}

struct Hit {
    line: Option<usize>,
    text: String,
}

struct FileHits {
    path: String,
    hits: Vec<Hit>,
}

struct Parsed {
    files: Vec<FileHits>,
    match_count: usize,
}

impl Parsed {
    fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    fn files_only(content: &str) -> Self {
        if content.trim().is_empty() || content == "no matches" {
            return Self {
                files: Vec::new(),
                match_count: 0,
            };
        }
        let files: Vec<FileHits> = content
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(|path| FileHits {
                path: path.to_string(),
                hits: Vec::new(),
            })
            .collect();
        let match_count = files.len();
        Self { files, match_count }
    }

    fn from_grep(content: &str) -> Self {
        if content.trim().is_empty() || content == "no matches" {
            return Self {
                files: Vec::new(),
                match_count: 0,
            };
        }
        let mut files: Vec<FileHits> = Vec::new();
        let mut match_count = 0usize;
        for raw in content.lines() {
            let Some((path, line_no, text)) = split_grep_line(raw) else {
                // Non-matching line (e.g. error tail) — skip.
                continue;
            };
            match_count += 1;
            if let Some(last) = files.last_mut() {
                if last.path == path {
                    last.hits.push(Hit {
                        line: Some(line_no),
                        text: text.to_string(),
                    });
                    continue;
                }
            }
            files.push(FileHits {
                path: path.to_string(),
                hits: vec![Hit {
                    line: Some(line_no),
                    text: text.to_string(),
                }],
            });
        }
        Self { files, match_count }
    }
}

/// `path:line:text` — path may contain `:`, so take last two splits carefully:
/// find `:digits:` pattern.
fn split_grep_line(raw: &str) -> Option<(&str, usize, &str)> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            let rest = &raw[i + 1..];
            if let Some((num, text)) = rest.split_once(':') {
                if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
                    let line_no: usize = num.parse().ok()?;
                    let path = &raw[..i];
                    if !path.is_empty() {
                        return Some((path, line_no, text));
                    }
                }
            }
        }
        i += 1;
    }
    None
}

fn header_line(
    is_glob: bool,
    pattern: &str,
    path: Option<&str>,
    parsed: &Parsed,
    muted: bool,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let text = if muted {
        theme.muted()
    } else {
        theme.primary()
    };
    let bold = text.add_modifier(Modifier::BOLD);
    let label = if is_glob { "Glob " } else { "Search " };
    let mut spans = vec![Span::styled(label.to_string(), bold)];
    if !pattern.is_empty() {
        let shown = if is_glob {
            pattern.to_string()
        } else {
            format!("\"{pattern}\"")
        };
        spans.push(Span::styled(
            shown,
            if muted {
                theme.muted()
            } else {
                theme.fg(theme.command)
            },
        ));
    }
    if let Some(p) = path {
        if !p.is_empty() && p != "." {
            spans.push(Span::styled(format!(" in {p}"), theme.muted()));
        }
    }
    let summary = if parsed.is_empty() {
        " (no matches)".to_string()
    } else if is_glob {
        format!(" ({} files)", parsed.match_count)
    } else if parsed.files.len() == 1 {
        format!(" ({} matches)", parsed.match_count)
    } else {
        format!(
            " ({} matches in {} files)",
            parsed.match_count,
            parsed.files.len()
        )
    };
    spans.push(Span::styled(summary, theme.dim()));
    let line = Line::from(spans);
    if width == 0 {
        line
    } else {
        truncate_line(line, width)
    }
}

fn pattern_from_args(arguments: &str, is_glob: bool) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let keys: &[&str] = if is_glob {
        &["glob_pattern", "pattern"]
    } else {
        &["pattern"]
    };
    keys.iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(str::to_string))
}

fn path_from_args(arguments: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    ["path", "target_directory"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(str::to_string))
}

fn prepend_diamond(line: &mut Line<'static>, theme: &Theme, failed: bool) {
    let fg = if failed {
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
    fn grep_collapsed_summary() {
        let theme = Theme::current();
        let content = "a.rs:1:foo\na.rs:2:bar\nb.rs:3:baz\n";
        let lines = lines(
            "grep",
            r#"{"pattern":"ba"}"#,
            content,
            &theme,
            80,
            ToolMode::Collapsed,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("Search "), "{text}");
        assert!(text.contains("\"ba\""), "{text}");
        assert!(text.contains("3 matches in 2 files"), "{text}");
        assert!(!text.contains("foo"), "{text}");
    }

    #[test]
    fn grep_expanded_groups_by_file() {
        let theme = Theme::current();
        let content = "a.rs:10:hello\nb.rs:1:world\n";
        let lines = lines(
            "grep",
            r#"{"pattern":"x"}"#,
            content,
            &theme,
            80,
            ToolMode::Expanded,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("a.rs"), "{text}");
        assert!(text.contains("hello"), "{text}");
        assert!(text.contains("  10"), "{text}");
    }

    #[test]
    fn split_grep_handles_colon_in_path() {
        assert_eq!(
            split_grep_line("C:/x/a.rs:12:body"),
            Some(("C:/x/a.rs", 12, "body"))
        );
    }
}
