//! On-disk compaction segments (`segment_NNN.md` + `INDEX.md`).
//!
//! Adapted from grok-build `xai-compaction-transcript` path conventions and INDEX
//! shape. Dock writes a simpler LogEvent-based markdown body (no ConversationItem
//! port); the store layout matches Grok so tools can find prior turns later.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use cordis_base::types::{LogEvent, TurnEndStatus};

use super::summary::format_compact_summary;

pub const COMPACTION_DIR: &str = "compaction";
pub const INDEX_FILE: &str = "INDEX.md";
const SEGMENT_PREFIX: &str = "segment_";

pub const INDEX_HEADER: &str = "# Compaction Segment Index\n\n\
     | Segment | File | Turns | Approx bytes | Keywords |\n\
     |---|---|---|---|---|\n";

fn segment_label(index: u64) -> String {
    format!("{index:03}")
}

pub fn segment_filename(index: u64) -> String {
    format!("{SEGMENT_PREFIX}{}.md", segment_label(index))
}

pub fn parse_segment_index(filename: &str) -> Option<u64> {
    filename
        .strip_prefix(SEGMENT_PREFIX)?
        .strip_suffix(".md")?
        .parse()
        .ok()
}

pub fn compaction_dir(session_dir: &Path) -> PathBuf {
    session_dir.join(COMPACTION_DIR)
}

/// Next free segment index under `compaction/` (0 if none yet).
pub fn next_segment_index(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            parse_segment_index(&name.to_string_lossy())
        })
        .max()
        .map(|n| n + 1)
        .unwrap_or(0)
}

/// Best-effort keywords for the INDEX row (identifier-shaped tokens, capped).
pub fn extract_keywords(summary: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for token in summary.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if token.len() < 4 {
            continue;
        }
        let lower = token.to_ascii_lowercase();
        const STOP: &[&str] = &[
            "section",
            "summary",
            "current",
            "work",
            "errors",
            "analysis",
            "primary",
            "request",
            "intent",
            "technical",
            "concepts",
            "pending",
            "problem",
            "solving",
            "include",
            "outline",
            "describe",
            "specific",
            "messages",
            "feedback",
            "snippet",
            "snippets",
            "session",
            "explicit",
            "thorough",
            "language",
            "important",
            "convention",
        ];
        if STOP.contains(&lower.as_str()) {
            continue;
        }
        if seen.iter().any(|s: &String| s == token) {
            continue;
        }
        seen.push(token.to_string());
        if seen.len() >= 8 {
            break;
        }
    }
    seen
}

pub fn render_index_row(
    index: u64,
    turn_count: usize,
    approx_bytes: usize,
    keywords: &[String],
) -> String {
    let kw = keywords
        .iter()
        .map(|k| format!("\"{k}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "| {label} | {file} | {turn_count} | {approx_bytes} | {kw} |\n",
        label = segment_label(index),
        file = segment_filename(index),
    )
}

fn role_label(event: &LogEvent) -> &'static str {
    match event {
        LogEvent::User(_) => "Human",
        LogEvent::LlmStream(_) => "Assistant",
        LogEvent::ToolExecute { .. } => "Function",
        LogEvent::SystemReminder(_) | LogEvent::Prompt(_) => "System",
        LogEvent::PreStep | LogEvent::Notice { .. } | LogEvent::TurnEnd(_) => "System",
    }
}

fn event_body(event: &LogEvent) -> String {
    match event {
        LogEvent::User(t) | LogEvent::SystemReminder(t) | LogEvent::Prompt(t) => t.clone(),
        LogEvent::LlmStream(out) => {
            let mut text = out.text.clone();
            if !out.tool_calls.is_empty() {
                let names: Vec<&str> = out.tool_calls.iter().map(|c| c.name.as_str()).collect();
                text.push_str(&format!("\n[Called tools: {}]", names.join(", ")));
            }
            text
        }
        LogEvent::ToolExecute { name, content, .. } => format!("[tool_response:{name}]\n{content}"),
        LogEvent::Notice { title, body, .. } => {
            if body.is_empty() {
                title.clone()
            } else {
                format!("{title}\n{body}")
            }
        }
        LogEvent::TurnEnd(TurnEndStatus::Failed(error)) => format!("本轮出错：{error}"),
        LogEvent::TurnEnd(TurnEndStatus::Cancelled) => "本轮已停止".into(),
        LogEvent::PreStep | LogEvent::TurnEnd(TurnEndStatus::Completed) => String::new(),
    }
}

/// Render one segment markdown document from the pre-compact history + summary.
pub fn render_segment_md(
    history: &[LogEvent],
    summary: &str,
    index: u64,
    timestamp: &str,
) -> String {
    let cleaned = format_compact_summary(summary);
    let mut turns = String::new();
    for (i, event) in history.iter().enumerate() {
        let body = event_body(event);
        if body.trim().is_empty() && matches!(event, LogEvent::PreStep) {
            continue;
        }
        turns.push_str(&format!("### Turn {i} ({})\n", role_label(event)));
        if !body.is_empty() {
            turns.push_str(&body);
            turns.push('\n');
        }
        turns.push('\n');
    }
    let turn_count = history.len();
    let summary_body = if cleaned.trim().is_empty() {
        "(empty)"
    } else {
        cleaned.trim()
    };
    format!(
        "# HISTORICAL -- DO NOT EDIT\n\
         # Record of compaction segment {label} (detail=verbose) from this same task.\n\
         # Use read_file or grep to look up details, but do not modify.\n\n\
         ## Segment metadata\n\
         - Index: {label}\n\
         - Turn count: {turn_count}\n\
         - Timestamp: {timestamp}\n\n\
         ## Summary (curated by compaction step)\n\n\
         {summary_body}\n\n\
         ## Verbatim turns\n\n\
         {turns}",
        label = segment_label(index),
    )
}

/// Write `compaction/segment_NNN.md` and append an `INDEX.md` row.
///
/// Returns the assigned segment index. Fail-open callers may ignore IO errors.
pub fn persist_compaction_segment(
    session_dir: &Path,
    history: &[LogEvent],
    summary: &str,
) -> std::io::Result<u64> {
    let dir = compaction_dir(session_dir);
    fs::create_dir_all(&dir)?;
    let index = next_segment_index(&dir);
    let timestamp = chrono_like_now();
    let md = render_segment_md(history, summary, index, &timestamp);
    let approx_bytes = md.len();
    let path = dir.join(segment_filename(index));
    atomic_write(&path, md.as_bytes())?;

    let index_path = dir.join(INDEX_FILE);
    if !index_path.is_file() {
        atomic_write(&index_path, INDEX_HEADER.as_bytes())?;
    }
    let row = render_index_row(
        index,
        history.len(),
        approx_bytes,
        &extract_keywords(summary),
    );
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&index_path)?
        .write_all(row.as_bytes())?;
    Ok(index)
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Compact ISO-ish stamp without pulling chrono into spine for this one line.
    format!("unix:{secs}")
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_base::types::LlmOutput;

    #[test]
    fn segment_filename_round_trips() {
        assert_eq!(segment_filename(5), "segment_005.md");
        assert_eq!(parse_segment_index("segment_005.md"), Some(5));
        assert_eq!(parse_segment_index("notes.md"), None);
    }

    #[test]
    fn index_row_matches_columns() {
        let row = render_index_row(2, 9, 1234, &["Foo".into(), "bar_baz".into()]);
        assert_eq!(
            row,
            "| 002 | segment_002.md | 9 | 1234 | \"Foo\", \"bar_baz\" |\n"
        );
    }

    #[test]
    fn persist_writes_segment_and_index() {
        let tmp = tempfile::tempdir().unwrap();
        let session = tmp.path().join("sess");
        fs::create_dir_all(&session).unwrap();
        let history = vec![
            LogEvent::User("fix auth".into()),
            LogEvent::LlmStream(LlmOutput {
                text: "looking".into(),
                ..LlmOutput::default()
            }),
        ];
        let idx = persist_compaction_segment(&session, &history, "Summary: AuthMiddleware work.")
            .unwrap();
        assert_eq!(idx, 0);
        let dir = compaction_dir(&session);
        let md = fs::read_to_string(dir.join("segment_000.md")).unwrap();
        assert!(md.contains("# HISTORICAL -- DO NOT EDIT"));
        assert!(md.contains("fix auth"));
        assert!(md.contains("AuthMiddleware") || md.contains("Summary"));
        let index = fs::read_to_string(dir.join("INDEX.md")).unwrap();
        assert!(index.starts_with("# Compaction Segment Index"));
        assert!(index.contains("segment_000.md"));

        let idx2 = persist_compaction_segment(&session, &history, "second").unwrap();
        assert_eq!(idx2, 1);
        assert!(dir.join("segment_001.md").is_file());
    }
}
