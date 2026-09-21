//! Manual dream: consolidate observations into topic markdown (LLM-driven).

use std::path::Path;

use cordis_base::config::MemoryDreamConfig;

use crate::text_utils::{has_markdown_headers, is_no_reply};

/// Whether auto-dream may run (manual `/dream` always allowed when memory+dream enabled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DreamEligibility {
    Ready,
    TooSoon { hours_since: u64, min_hours: u64 },
    NotEnoughSessions { sessions: u64, min_sessions: u64 },
}

pub fn auto_dream_eligibility(
    hours_since_last: Option<u64>,
    sessions_since_last: u64,
    config: &MemoryDreamConfig,
) -> DreamEligibility {
    if !config.enabled {
        return DreamEligibility::TooSoon {
            hours_since: 0,
            min_hours: config.min_hours,
        };
    }
    if let Some(h) = hours_since_last {
        if h < config.min_hours {
            return DreamEligibility::TooSoon {
                hours_since: h,
                min_hours: config.min_hours,
            };
        }
    }
    if sessions_since_last < config.min_sessions {
        return DreamEligibility::NotEnoughSessions {
            sessions: sessions_since_last,
            min_sessions: config.min_sessions,
        };
    }
    DreamEligibility::Ready
}

pub const DREAM_SYSTEM_PROMPT: &str = "\
You are performing a dream — a reflective pass over memory files. \
Synthesize recent observations into durable, well-organized topic notes \
so future sessions orient quickly.

You will receive observation files (and optionally existing topics). Your job:

1. **Merge** related information into coherent topic summaries
2. **Resolve** contradictions — if a recent observation disproves an older fact, keep only the current truth
3. **Convert** relative dates to absolute dates when possible
4. **Discard** ephemeral details (greetings, tool noise, message counts, current state)
5. **Preserve** decisions, rationale, architecture, preferences, and problem/solution pairs

Respond with a single markdown document. Use ## headers to separate topics. \
Each topic should be self-contained and useful to a future session.

If the observations contain nothing worth persisting, respond with NO_REPLY.";

const MAX_DREAM_INPUT_CHARS: usize = 32_000;

#[derive(Debug)]
pub struct DreamMessage {
    pub content: String,
    pub observation_paths: Vec<std::path::PathBuf>,
}

#[derive(Debug)]
pub struct DreamResult {
    pub status: DreamStatus,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DreamStatus {
    Completed { chars_written: usize },
    NothingToConsolidate,
    Failed(String),
}

pub fn build_dream_user_message(
    observation_dir: &Path,
    existing_topics: Option<&str>,
) -> Option<DreamMessage> {
    let mut paths = list_md_files(observation_dir);
    paths.sort();
    if paths.is_empty() && existing_topics.map(str::trim).unwrap_or("").is_empty() {
        return None;
    }

    let mut buf = String::new();
    if let Some(mem) = existing_topics {
        let trimmed = mem.trim();
        if !trimmed.is_empty() {
            buf.push_str("--- Existing Topics (merge) ---\n\n");
            let cap = MAX_DREAM_INPUT_CHARS / 2;
            if trimmed.len() <= cap {
                buf.push_str(trimmed);
            } else {
                let mut end = cap;
                while end > 0 && !trimmed.is_char_boundary(end) {
                    end -= 1;
                }
                if let Some(prefix) = trimmed.get(..end) {
                    buf.push_str(prefix);
                }
            }
        }
    }

    let mut used = Vec::new();
    for path in &paths {
        if buf.len() >= MAX_DREAM_INPUT_CHARS {
            break;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }
        if !buf.is_empty() {
            buf.push_str("\n\n");
        }
        buf.push_str("--- Observation: ");
        buf.push_str(&path.display().to_string());
        buf.push_str(" ---\n\n");
        buf.push_str(&content);
        used.push(path.clone());
    }

    if buf.trim().is_empty() {
        return None;
    }
    Some(DreamMessage {
        content: buf,
        observation_paths: used,
    })
}

pub fn process_dream_response(response: &str) -> DreamStatus {
    let trimmed = response.trim();
    if trimmed.is_empty() || is_no_reply(trimmed) {
        return DreamStatus::NothingToConsolidate;
    }
    if !has_markdown_headers(trimmed) {
        return DreamStatus::Failed("dream 响应缺少 markdown 结构（无 ## 标题）".into());
    }
    DreamStatus::Completed {
        chars_written: trimmed.len(),
    }
}

/// Split a dream markdown document into per-topic files under `topics_dir`.
pub fn write_topics_from_dream(topics_dir: &Path, content: &str) -> std::io::Result<usize> {
    std::fs::create_dir_all(topics_dir)?;
    let sections = split_topics(content);
    let mut written = 0usize;
    for (title, body) in sections {
        let slug = crate::slug::slugify(&title, 48);
        if slug.is_empty() {
            continue;
        }
        let path = topics_dir.join(format!("{slug}.md"));
        let file = format!("# {title}\n\n{body}\n");
        std::fs::write(&path, file)?;
        written += 1;
    }
    Ok(written)
}

fn split_topics(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut title = String::from("notes");
    let mut body = String::new();
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if !body.trim().is_empty() || title != "notes" {
                out.push((title, body.trim().to_string()));
            }
            title = rest.trim().to_string();
            body.clear();
        } else if let Some(rest) = line.strip_prefix("# ") {
            if !body.trim().is_empty() {
                out.push((title, body.trim().to_string()));
            }
            title = rest.trim().to_string();
            body.clear();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    if !body.trim().is_empty() {
        out.push((title, body.trim().to_string()));
    }
    out
}

fn list_md_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(list_md_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_message_reads_observations() {
        let tmp = tempfile::tempdir().unwrap();
        let obs = tmp.path().join("observations");
        std::fs::create_dir_all(&obs).unwrap();
        std::fs::write(obs.join("a.md"), "## Note\nhello dream").unwrap();
        let msg = build_dream_user_message(&obs, None).unwrap();
        assert!(msg.content.contains("hello dream"));
        assert_eq!(msg.observation_paths.len(), 1);
    }

    #[test]
    fn process_dream_accepts() {
        assert!(matches!(
            process_dream_response("## Topic\nbody"),
            DreamStatus::Completed { .. }
        ));
        assert_eq!(
            process_dream_response("NO_REPLY"),
            DreamStatus::NothingToConsolidate
        );
    }

    #[test]
    fn write_topics_splits() {
        let tmp = tempfile::tempdir().unwrap();
        let n = write_topics_from_dream(tmp.path(), "## Auth\nuse JWT\n\n## Database\npostgres\n")
            .unwrap();
        assert_eq!(n, 2);
        assert!(tmp.path().join("auth.md").exists());
        assert!(tmp.path().join("database.md").exists());
    }
}
