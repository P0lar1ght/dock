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
use dock_memory::rewrite::{rewrite_user_message, REMEMBER_REWRITE_SYSTEM_PROMPT};
use dock_memory::storage::{save_remember_note, write_flush_observation};

use crate::error::{Error, Result};
use crate::llm::compact::{estimate_context_tokens, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT};
use crate::llm::sampler::Llm;
use crate::names::{LLM, MEMORY, SESSIONS, SYSTEM_PROMPT};
use crate::prompt::assemble::SystemPrompt;
use crate::session::log::Sessions;
use crate::tools::memory::{embed_missing_after_write, spawn_embed_missing_after_write, Memory};

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
        .ok_or_else(|| Error::Compact("记忆服务未挂载".into()))?;
    let cfg = memory.config();
    if !memory.enabled() {
        return Err(Error::Compact(
            "记忆功能已关闭。请设置 [memory] enabled = true 或 DOCK_MEMORY=1。".into(),
        ));
    }
    if !force && !cfg.flush.enabled {
        return Err(Error::Compact("配置中已关闭 memory flush".into()));
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
        return Err(Error::Compact(format!("记忆 flush 的 LLM 调用失败：{err}")));
    }

    match process_flush_response(&output.text, &cfg.flush) {
        FlushResult::NothingToStore => Ok("记忆 flush：没有需要保存的内容。".into()),
        FlushResult::Rejected(reason) => Ok(format!("记忆 flush 被拒绝：{reason}")),
        FlushResult::Accepted(content) => {
            let root = memory.root();
            let mut index = MemoryIndex::open_or_create(&root.search_db())
                .map_err(|e| Error::Compact(format!("记忆索引：{e}")))?;
            let path = write_flush_observation(&root, &content, &mut index)
                .map_err(|e| Error::Compact(format!("记忆写入：{e}")))?;
            sessions.set_last_flush_content(Some(content));
            embed_missing_after_write(&memory.config().embedding, &root).await;
            Ok(format!("记忆 flush 已写入 {}", path.display()))
        }
    }
}

/// Manual `/dream` — consolidate workspace observations into topics.
pub async fn run_dream(ctx: &Context) -> Result<String> {
    let memory = ctx
        .get::<Memory>(MEMORY)
        .ok_or_else(|| Error::Compact("记忆服务未挂载".into()))?;
    let cfg = memory.config();
    if !memory.enabled() {
        return Err(Error::Compact(
            "记忆功能已关闭。请设置 [memory] enabled = true 或 DOCK_MEMORY=1。".into(),
        ));
    }
    if !cfg.dream.enabled {
        return Err(Error::Compact("配置中已关闭 memory dream".into()));
    }
    let llm = ctx.require::<Llm>(LLM)?;
    let root = memory.root();
    root.ensure_layout()
        .map_err(|e| Error::Compact(format!("记忆目录：{e}")))?;

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
        return Ok("Dream：没有可整理的观察记录。".into());
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
        return Err(Error::Compact(format!("记忆 dream 的 LLM 调用失败：{err}")));
    }

    match process_dream_response(&output.text) {
        DreamStatus::NothingToConsolidate => Ok("Dream：没有需要整理的内容。".into()),
        DreamStatus::Failed(reason) => Ok(format!("Dream 失败：{reason}")),
        DreamStatus::Completed { .. } => {
            let n = write_topics_from_dream(&root.workspace.topics, &output.text)
                .map_err(|e| Error::Compact(format!("dream 写入：{e}")))?;
            let mut index = MemoryIndex::open_or_create(&root.search_db())
                .map_err(|e| Error::Compact(format!("记忆索引：{e}")))?;
            let _ = index.reindex_tree(&root);
            let _ = dock_memory::manifest::regenerate_scope(
                &root.workspace,
                dock_memory::MemoryScope::Workspace,
            );
            // Reindex + embed topics first; archive is not in the searchable tree
            // (`reindex_tree` skips archive/), so drop old index rows after rename.
            embed_missing_after_write(&cfg.embedding, &root).await;
            let archive = root.workspace.archive.clone();
            let _ = std::fs::create_dir_all(&archive);
            for path in msg.observation_paths {
                if let Some(name) = path.file_name() {
                    let dest = dock_memory::unique_archive_path(&archive, name);
                    if std::fs::rename(&path, &dest).is_ok() {
                        let _ = index.delete_path(&path);
                    }
                }
            }
            Ok(format!("Dream 已写入 {n} 个主题文件。"))
        }
    }
}

/// `/remember <text>` — write a global observation (no LLM rewrite).
pub fn run_remember(ctx: &Context, note: &str) -> Result<String> {
    save_remember(ctx, note)
}

fn save_remember(ctx: &Context, note: &str) -> Result<String> {
    if let Some(memory) = ctx.get::<Memory>(MEMORY) {
        if !memory.enabled() {
            return Err(Error::Compact(
                "记忆功能已关闭。请设置 [memory] enabled = true 或 DOCK_MEMORY=1。".into(),
            ));
        }
        let root = memory.root();
        let mut index = MemoryIndex::open_or_create(&root.search_db())
            .map_err(|e| Error::Compact(format!("记忆索引：{e}")))?;
        let path = save_remember_note(&root, note, &mut index)
            .map_err(|e| Error::Compact(format!("remember：{e}")))?;
        spawn_embed_missing_after_write(memory.config().embedding, root);
        return Ok(format!("记忆已保存到 {}", path.display()));
    }
    let cfg = load_memory_config();
    if !cfg.enabled {
        return Err(Error::Compact(
            "记忆功能已关闭。请设置 [memory] enabled = true 或 DOCK_MEMORY=1。".into(),
        ));
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let root = MemoryRoot::open_default(&cwd);
    let mut index = MemoryIndex::open_or_create(&root.search_db())
        .map_err(|e| Error::Compact(format!("记忆索引：{e}")))?;
    let path = save_remember_note(&root, note, &mut index)
        .map_err(|e| Error::Compact(format!("remember：{e}")))?;
    spawn_embed_missing_after_write(cfg.embedding, root);
    Ok(format!("记忆已保存到 {}", path.display()))
}

/// `/remember <text>` with optional Dock sampler rewrite (Grok-aligned).
/// Falls back to the raw note when LLM fails or is unavailable.
pub async fn run_remember_async(ctx: &Context, note: &str) -> Result<String> {
    let trimmed = note.trim();
    if trimmed.is_empty() {
        return Err(Error::Compact(
            "用法：/remember <笔记> — 留空则打开输入框。".into(),
        ));
    }
    let rewritten = try_rewrite_remember(ctx, trimmed).await;
    let body = rewritten.as_deref().unwrap_or(trimmed);
    save_remember(ctx, body)
}

async fn try_rewrite_remember(ctx: &Context, raw: &str) -> Option<String> {
    let llm = ctx.get::<Llm>(LLM)?;
    // Skip rewrite for notes that already look structured.
    if raw.lines().any(|l| l.trim_start().starts_with("## ")) {
        return None;
    }
    const MAX_INPUT: usize = 32 * 1024;
    if raw.len() > MAX_INPUT {
        return None;
    }
    let iso = ctx.isolate("sessions");
    let output = llm
        .stream_observed(
            &iso,
            PromptRequest {
                system: REMEMBER_REWRITE_SYSTEM_PROMPT.to_string(),
                history: vec![LogEvent::User(rewrite_user_message(raw, ""))],
                tools: Vec::new(),
            },
            |_delta: &StreamDelta| {},
        )
        .await;
    if output.error.is_some() {
        return None;
    }
    let text = output.text.trim().to_string();
    if text.is_empty() || !text.contains('#') {
        return None;
    }
    Some(text)
}
