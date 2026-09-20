//! Flush / dream / remember orchestration via Dock's existing LLM sampler.

use cordis::Context;
use cordis_base::config::load_memory_config;
use cordis_base::stream_acc::StreamDelta;
use cordis_base::types::{LogEvent, PromptRequest};
use dock_memory::dream::{
    build_dream_user_message, process_dream_response, write_topics_from_dream, DreamStatus,
    DREAM_SYSTEM_PROMPT,
};
use dock_memory::flush::{
    process_flush_response, should_flush, FlushResult, FLUSH_DELTA_SYSTEM_PROMPT,
    FLUSH_SYSTEM_PROMPT,
};
use dock_memory::index::MemoryIndex;
use dock_memory::layout::MemoryRoot;
use dock_memory::storage::{save_remember_note, write_flush_observation};

use crate::error::{Error, Result};
use crate::llm::compact::{estimate_context_tokens, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT};
use crate::llm::sampler::Llm;
use crate::names::{LLM, MEMORY, SESSIONS, SYSTEM_PROMPT};
use crate::prompt::assemble::SystemPrompt;
use crate::session::log::Sessions;
use crate::tools::memory::Memory;

/// Run memory flush before compact when gates pass. Fail-open.
pub async fn maybe_flush_before_compact(ctx: &Context) {
    let Some(memory) = ctx.get::<Memory>(MEMORY) else {
        return;
    };
    let cfg = memory.config();
    if !memory.enabled() || !cfg.flush.enabled {
        return;
    }
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return;
    };
    let system = ctx
        .get::<SystemPrompt>(SYSTEM_PROMPT)
        .map(|p| p.assemble_on(ctx))
        .unwrap_or_default();
    let used = {
        let estimate = estimate_context_tokens(&system, &sessions.model_history());
        let u = sessions.usage();
        if u.official {
            u.prompt.max(estimate)
        } else {
            estimate
        }
    };
    let window = sessions.usage().window;
    let cycle = sessions.compaction_count().saturating_add(1);
    if !should_flush(
        used,
        window,
        DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT,
        &cfg.flush,
        sessions.last_flush_compaction(),
        cycle,
    ) {
        return;
    }
    match run_flush(ctx, false).await {
        Ok(msg) => {
            tracing::info!(target: "dock_memory", %msg, "pre-compact memory flush");
            sessions.mark_flushed_for_compaction(cycle);
        }
        Err(e) => {
            tracing::warn!(target: "dock_memory", error = %e, "pre-compact memory flush failed");
        }
    }
}

/// Manual `/flush` or compact-hook flush.
pub async fn run_flush(ctx: &Context, force: bool) -> Result<String> {
    let memory = ctx
        .get::<Memory>(MEMORY)
        .ok_or_else(|| Error::Compact("memory service not mounted".into()))?;
    let cfg = memory.config();
    if !memory.enabled() {
        return Err(Error::Compact(
            "Memory is disabled. Set [memory] enabled = true or DOCK_MEMORY=1.".into(),
        ));
    }
    if !force && !cfg.flush.enabled {
        return Err(Error::Compact("memory flush is disabled in config".into()));
    }
    let sessions = ctx.require::<Sessions>(SESSIONS)?;
    let llm = ctx.require::<Llm>(LLM)?;

    let history = sessions.model_history();
    let mut prompt_history: Vec<LogEvent> = history
        .iter()
        .filter(|e| {
            matches!(
                e,
                LogEvent::User(_) | LogEvent::LlmStream(_) | LogEvent::ToolExecute { .. }
            )
        })
        .cloned()
        .collect();
    // Keep a bounded window.
    if prompt_history.len() > 40 {
        prompt_history = prompt_history.split_off(prompt_history.len() - 40);
    }
    prompt_history.push(LogEvent::User(
        "Now write the memory summary as described in the system prompt.".into(),
    ));

    let system = if let Some(prev) = sessions.last_flush_content() {
        format!("{FLUSH_DELTA_SYSTEM_PROMPT}{prev}")
    } else {
        FLUSH_SYSTEM_PROMPT.to_string()
    };

    let iso = ctx.isolate("sessions");
    let output = llm
        .stream_observed(
            &iso,
            PromptRequest {
                system,
                history: prompt_history,
                tools: Vec::new(),
            },
            |_delta: &StreamDelta| {},
        )
        .await;

    if let Some(err) = output.error {
        return Err(Error::Compact(format!("memory flush LLM failed: {err}")));
    }

    match process_flush_response(&output.text, &cfg.flush) {
        FlushResult::NothingToStore => Ok("Memory flush: nothing to store.".into()),
        FlushResult::Rejected(reason) => Ok(format!("Memory flush rejected: {reason}")),
        FlushResult::Accepted(content) => {
            let root = memory.root();
            let mut index = MemoryIndex::open_or_create(&root.search_db())
                .map_err(|e| Error::Compact(format!("memory index: {e}")))?;
            let path = write_flush_observation(&root, &content, &mut index)
                .map_err(|e| Error::Compact(format!("memory write: {e}")))?;
            sessions.set_last_flush_content(Some(content));
            Ok(format!("Memory flush wrote {}", path.display()))
        }
    }
}

/// Manual `/dream` — consolidate workspace observations into topics.
pub async fn run_dream(ctx: &Context) -> Result<String> {
    let memory = ctx
        .get::<Memory>(MEMORY)
        .ok_or_else(|| Error::Compact("memory service not mounted".into()))?;
    let cfg = memory.config();
    if !memory.enabled() {
        return Err(Error::Compact(
            "Memory is disabled. Set [memory] enabled = true or DOCK_MEMORY=1.".into(),
        ));
    }
    if !cfg.dream.enabled {
        return Err(Error::Compact("memory dream is disabled in config".into()));
    }
    let llm = ctx.require::<Llm>(LLM)?;
    let root = memory.root();
    root.ensure_layout()
        .map_err(|e| Error::Compact(format!("memory layout: {e}")))?;

    let existing = {
        let mut buf = String::new();
        if let Ok(entries) = std::fs::read_dir(&root.workspace.topics) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    if let Ok(t) = std::fs::read_to_string(&path) {
                        if !buf.is_empty() {
                            buf.push_str("\n\n");
                        }
                        buf.push_str(&t);
                    }
                }
            }
        }
        if buf.is_empty() {
            None
        } else {
            Some(buf)
        }
    };

    let Some(msg) = build_dream_user_message(&root.workspace.inbox, existing.as_deref())
        .or_else(|| build_dream_user_message(&root.workspace.observations, existing.as_deref()))
    else {
        return Ok("Dream: no observations to consolidate.".into());
    };

    let iso = ctx.isolate("sessions");
    let output = llm
        .stream_observed(
            &iso,
            PromptRequest {
                system: DREAM_SYSTEM_PROMPT.to_string(),
                history: vec![LogEvent::User(msg.content)],
                tools: Vec::new(),
            },
            |_delta: &StreamDelta| {},
        )
        .await;

    if let Some(err) = output.error {
        return Err(Error::Compact(format!("memory dream LLM failed: {err}")));
    }

    match process_dream_response(&output.text) {
        DreamStatus::NothingToConsolidate => Ok("Dream: nothing to consolidate.".into()),
        DreamStatus::Failed(reason) => Ok(format!("Dream failed: {reason}")),
        DreamStatus::Completed { .. } => {
            let n = write_topics_from_dream(&root.workspace.topics, &output.text)
                .map_err(|e| Error::Compact(format!("dream write: {e}")))?;
            let mut index = MemoryIndex::open_or_create(&root.search_db())
                .map_err(|e| Error::Compact(format!("memory index: {e}")))?;
            let _ = index.reindex_tree(&root);
            let _ = dock_memory::manifest::regenerate_scope(
                &root.workspace,
                dock_memory::MemoryScope::Workspace,
            );
            // Archive processed observations by renaming aside (keep for audit).
            let archive = root.workspace.archive.clone();
            let _ = std::fs::create_dir_all(&archive);
            for path in msg.observation_paths {
                if let Some(name) = path.file_name() {
                    let _ = std::fs::rename(&path, archive.join(name));
                }
            }
            Ok(format!("Dream wrote {n} topic file(s)."))
        }
    }
}

/// `/remember <text>` — write a global observation.
pub fn run_remember(ctx: &Context, note: &str) -> Result<String> {
    if let Some(memory) = ctx.get::<Memory>(MEMORY) {
        if !memory.enabled() {
            return Err(Error::Compact(
                "Memory is disabled. Set [memory] enabled = true or DOCK_MEMORY=1.".into(),
            ));
        }
        let root = memory.root();
        let mut index = MemoryIndex::open_or_create(&root.search_db())
            .map_err(|e| Error::Compact(format!("memory index: {e}")))?;
        let path = save_remember_note(&root, note, &mut index)
            .map_err(|e| Error::Compact(format!("remember: {e}")))?;
        return Ok(format!("Memory saved to {}", path.display()));
    }
    let cfg = load_memory_config();
    if !cfg.enabled {
        return Err(Error::Compact(
            "Memory is disabled. Set [memory] enabled = true or DOCK_MEMORY=1.".into(),
        ));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let root = MemoryRoot::open_default(&cwd);
    let mut index = MemoryIndex::open_or_create(&root.search_db())
        .map_err(|e| Error::Compact(format!("memory index: {e}")))?;
    let path = save_remember_note(&root, note, &mut index)
        .map_err(|e| Error::Compact(format!("remember: {e}")))?;
    Ok(format!("Memory saved to {}", path.display()))
}
