//! Pre-compaction memory flush policy and response processing (no embedding dedup).

use cordis_base::config::MemoryFlushConfig;

use crate::text_utils::{has_markdown_headers, is_no_reply};

/// Whether a memory flush should run before the next compaction.
pub fn should_flush(
    total_tokens: u64,
    context_window: u64,
    compact_threshold_percent: u8,
    flush_config: &MemoryFlushConfig,
    last_flush_compaction: u64,
    current_compaction_count: u64,
) -> bool {
    if !flush_config.enabled {
        return false;
    }
    if last_flush_compaction == current_compaction_count {
        return false;
    }
    if context_window == 0 {
        return false;
    }
    let compact_at = context_window.saturating_mul(compact_threshold_percent as u64) / 100;
    let flush_at = compact_at.saturating_sub(flush_config.soft_threshold_tokens);
    total_tokens >= flush_at
}

pub const FLUSH_SYSTEM_PROMPT: &str = "\
You are a memory assistant. Extract ALL useful information from this conversation \
that would help you be more effective in future sessions with this user. \
Write a concise markdown summary with ## headers covering:

- **Decisions & rationale** — what was chosen and why
- **Technical context** — architecture, APIs, patterns, tools, file paths discussed
- **Debugging techniques & tools** — external APIs, CLI commands, query patterns, \
investigation workflows, or services discovered or used during debugging
- **Problems & solutions** — bugs found, how they were fixed, workarounds

Prioritize reusable mechanisms, rules, and root causes over a narration of the task. \
When project structure matters, name the concrete directories and stable repo-relative paths; \
include an absolute workspace root only when it is operationally necessary and appears in the conversation. \
Copy exact identifiers and numerical values only when they appear verbatim in the supplied conversation. \
Never infer a missing value; omit the detail instead.

Omit any section where there is nothing substantive to report. \
Do NOT include user preferences like OS, shell, or editor — these belong in global memory. \
Do NOT include an ephemeral progress section — transient status is not useful for future sessions.

Respond with NO_REPLY if nothing genuinely useful was learned — a routine task \
that followed standard patterns, brief Q&A, or sessions with no novel decisions \
or discoveries are not worth persisting. Only write content that a future session \
would concretely benefit from.";

pub const FLUSH_DELTA_SYSTEM_PROMPT: &str = "\
You are a memory assistant performing an incremental update. The previous \
flush output for this session is shown below. Extract ONLY information that \
is NEW since the previous flush — do not repeat anything already captured.

Write a concise markdown summary with ## headers covering only NEW items in:
- **Decisions & rationale** — new decisions since last flush
- **Technical context** — new architecture, APIs, patterns discovered
- **Debugging techniques** — new techniques used since last flush
- **Problems & solutions** — new bugs found and fixes

Prioritize reusable mechanisms, rules, and root causes over a narration of the task. \
Respond with NO_REPLY if nothing genuinely new and useful has happened since \
the previous flush.

--- Previous flush content ---
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlushResult {
    NothingToStore,
    Accepted(String),
    Rejected(String),
}

pub fn process_flush_response(response: &str, config: &MemoryFlushConfig) -> FlushResult {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return FlushResult::NothingToStore;
    }
    if is_no_reply(trimmed) {
        return FlushResult::NothingToStore;
    }

    let content = if trimmed.chars().count() > config.max_flush_write_chars {
        trimmed
            .chars()
            .take(config.max_flush_write_chars)
            .collect::<String>()
    } else {
        trimmed.to_string()
    };

    if !has_markdown_headers(&content) {
        return FlushResult::Rejected(
            "flush response lacks markdown structure (no ## headers)".into(),
        );
    }
    FlushResult::Accepted(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_flush_respects_enabled_and_cycle() {
        let mut cfg = MemoryFlushConfig::default();
        assert!(should_flush(90_000, 100_000, 80, &cfg, 0, 1));
        assert!(!should_flush(90_000, 100_000, 80, &cfg, 1, 1));
        cfg.enabled = false;
        assert!(!should_flush(90_000, 100_000, 80, &cfg, 0, 1));
    }

    #[test]
    fn process_accepts_headers() {
        let cfg = MemoryFlushConfig::default();
        match process_flush_response("## Decisions\n- use rust", &cfg) {
            FlushResult::Accepted(s) => assert!(s.contains("Decisions")),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            process_flush_response("NO_REPLY", &cfg),
            FlushResult::NothingToStore
        );
        assert!(matches!(
            process_flush_response("no structure here", &cfg),
            FlushResult::Rejected(_)
        ));
    }
}
