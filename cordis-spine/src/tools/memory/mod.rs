//! Local `memory_search` / `memory_get` backed by `dock-memory`.
//!
//! Disk: `$DOCK_HOME/memory/{global,workspace-<slug>}/{topics,observations}/`
//! plus FTS index `$DOCK_HOME/memory/search.sqlite`. Legacy `~/.dock/memory`
//! (flat) and `.dock/memory` remain readable for migration; new writes only
//! go to the topics/observations layout.
//!
//! Default off (`[memory] enabled = false`); `DOCK_MEMORY=1/0` overrides.

mod ops;
mod prompt;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use cordis_base::config::{load_memory_config, MemoryConfig, MemoryEmbeddingConfig};
use dock_memory::index::MemoryIndex;
use dock_memory::layout::MemoryRoot;
use dock_memory::storage::{format_with_line_numbers, read_memory_file};
use dock_memory::{
    embed_missing_chunks_owned, format_search_results, search_memory_with_config, sync_dirty_paths,
    ApiEmbeddingProvider, EmbeddingProvider, MemoryFileWatcher, MemorySearchConfig,
};

use crate::names::{CONTEXT, MEMORY, TOOLS};
use crate::prompt::assemble::ORDER_MEMORY;
use crate::prompt::context_book::{own_sections, ContextBook};
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{ToolCall, ToolResult, ToolSpec};

pub use ops::{run_dream, run_flush, run_remember, run_remember_async};

const SEARCH_PARAMS: &str = r#"{"type":"object","properties":{"query":{"type":"string","description":"Search query. Prefer specific technical terms."},"max_results":{"type":"integer"},"min_score":{"type":"number"}},"required":["query"]}"#;
const GET_PARAMS: &str = r#"{"type":"object","properties":{"path":{"type":"string","description":"Memory file path from memory_search."},"from":{"type":"integer","description":"1-based start line."},"lines":{"type":"integer","description":"Max lines to return."}},"required":["path"]}"#;

struct MemoryInner {
    config: Mutex<MemoryConfig>,
    /// Session override: `None` = follow config; `Some(bool)` toggled via `/memory` `t`.
    /// `DOCK_MEMORY=0` (`force_disabled`) still wins process-wide.
    session_override: Mutex<Option<bool>>,
    /// Started when memory is enabled; watches `$DOCK_HOME/memory/` for external `.md` edits.
    watcher: Mutex<Option<MemoryFileWatcher>>,
    /// Tools table used to (un)register resident memory_* sampler tools.
    tools: Mutex<Option<Tools>>,
    /// Live disposables while `memory_search` / `memory_get` are resident.
    tool_regs: Mutex<Vec<Disposable>>,
}

/// Named `"memory"` — live-looked by TUI / tools / compact flush hook.
#[derive(Clone)]
pub struct Memory {
    inner: Arc<MemoryInner>,
}

impl Memory {
    pub fn new() -> Self {
        let mem = Self {
            inner: Arc::new(MemoryInner {
                config: Mutex::new(load_memory_config()),
                session_override: Mutex::new(None),
                watcher: Mutex::new(None),
                tools: Mutex::new(None),
                tool_regs: Mutex::new(Vec::new()),
            }),
        };
        if mem.enabled() {
            mem.ensure_watcher();
        }
        mem
    }

    pub fn config(&self) -> MemoryConfig {
        self.inner.config.lock().unwrap().clone()
    }

    pub fn reload(&self) {
        *self.inner.config.lock().unwrap() = load_memory_config();
        if self.enabled() {
            self.ensure_watcher();
        }
        self.sync_tool_registration();
    }

    /// Effective on/off: process force-off → session override → config.
    pub fn enabled(&self) -> bool {
        let cfg = self.config();
        if cfg.force_disabled {
            return false;
        }
        if let Some(over) = *self.inner.session_override.lock().unwrap() {
            return over;
        }
        cfg.enabled
    }

    /// Toggle session override. Returns new effective enabled, or `Err` if
    /// process-wide force-off (`DOCK_MEMORY=0`) blocks enabling.
    pub fn toggle_session(&self) -> Result<bool, &'static str> {
        let cfg = self.config();
        if cfg.force_disabled {
            return Err("Memory is forced off for this process (DOCK_MEMORY=0).");
        }
        let mut slot = self.inner.session_override.lock().unwrap();
        let currently = slot.unwrap_or(cfg.enabled);
        let next = !currently;
        *slot = Some(next);
        drop(slot);
        if next {
            self.ensure_watcher();
        }
        self.sync_tool_registration();
        Ok(next)
    }

    pub fn session_override(&self) -> Option<bool> {
        *self.inner.session_override.lock().unwrap()
    }

    pub fn root_for_cwd(&self, cwd: &std::path::Path) -> MemoryRoot {
        MemoryRoot::open_default(cwd)
    }

    pub fn root(&self) -> MemoryRoot {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.root_for_cwd(&cwd)
    }

    /// Start [`MemoryFileWatcher`] on `$DOCK_HOME/memory/` once when enabled.
    pub fn ensure_watcher(&self) {
        if !self.enabled() {
            return;
        }
        {
            let slot = self.inner.watcher.lock().unwrap();
            if slot.is_some() {
                return;
            }
        }
        let root = self.root();
        let _ = root.ensure_layout();
        let mut slot = self.inner.watcher.lock().unwrap();
        if slot.is_some() {
            return;
        }
        *slot = MemoryFileWatcher::start(&root.home);
    }

    /// If the watcher reports dirty paths, reindex/delete them before search.
    pub fn sync_dirty_if_needed(&self, root: &MemoryRoot) {
        self.ensure_watcher();
        let dirty = {
            let guard = self.inner.watcher.lock().unwrap();
            let Some(watcher) = guard.as_ref() else {
                return;
            };
            if !watcher.is_dirty() {
                return;
            }
            watcher.take_dirty()
        };
        if dirty.is_empty() {
            return;
        }
        let Ok(mut index) = MemoryIndex::open_or_create(&root.search_db()) else {
            return;
        };
        sync_dirty_paths(root, &mut index, &dirty);
    }

    /// True when a watcher is running (tests / diagnostics).
    #[cfg(test)]
    fn has_watcher(&self) -> bool {
        self.inner.watcher.lock().unwrap().is_some()
    }

    /// Bind the live [`Tools`] table and sync resident memory tools to [`Self::enabled`].
    pub fn bind_tools(&self, tools: Tools) {
        *self.inner.tools.lock().unwrap() = Some(tools);
        self.sync_tool_registration();
    }

    /// Drop resident `memory_search` / `memory_get` (plugin dispose).
    pub fn clear_tool_registration(&self) {
        let regs = std::mem::take(&mut *self.inner.tool_regs.lock().unwrap());
        for d in regs {
            d.dispose_sync();
        }
    }

    /// Enabled → `register` (sampler-visible). Disabled → remove from the table.
    /// Handlers still no-op with a disabled message if somehow invoked.
    pub fn sync_tool_registration(&self) {
        self.clear_tool_registration();
        if !self.enabled() {
            return;
        }
        let tools = {
            let guard = self.inner.tools.lock().unwrap();
            match guard.clone() {
                Some(t) => t,
                None => return,
            }
        };

        let mem_search = self.clone();
        let search: ToolBody = std::sync::Arc::new(move |call| {
            let mem = mem_search.clone();
            Box::pin(async move { memory_search(&mem, call).await })
        });
        let mem_get = self.clone();
        let get: ToolBody = std::sync::Arc::new(move |call| {
            let mem = mem_get.clone();
            Box::pin(async move { memory_get(&mem, call) })
        });

        let mut regs = Vec::new();
        match tools.register(
            ToolSpec {
                name: "memory_search".into(),
                description: "Search cross-session local memory for relevant knowledge chunks. Returns ranked results from $DOCK_HOME/memory (topics/observations) and legacy ~/.dock/memory when present.\n\nUse this proactively when a question references prior work, decisions, or conventions you do not have in the current transcript.\n\nMemory is historical context, not automatically the current plan. Verify recalled facts against live sources before relying on them.".into(),
                parameters_json: SEARCH_PARAMS.into(),
            },
            search,
        ) {
            Ok(d) => regs.push(d),
            Err(e) => tracing::warn!(target: "dock_memory", error = %e, "failed to register memory_search"),
        }
        match tools.register(
            ToolSpec {
                name: "memory_get".into(),
                description: "Read a memory file by path. Returns the file content with line numbers, optionally limited to a range of lines.\n\nUse after memory_search returns a relevant result. Line numbers are 1-based and match the from parameter.".into(),
                parameters_json: GET_PARAMS.into(),
            },
            get,
        ) {
            Ok(d) => regs.push(d),
            Err(e) => tracing::warn!(target: "dock_memory", error = %e, "failed to register memory_get"),
        }
        *self.inner.tool_regs.lock().unwrap() = regs;
    }
}

impl Default for Memory {
    fn default() -> Self {
        Self::new()
    }
}

pub fn tool_memory() -> Plugin {
    plugin(
        "tool-memory",
        Inject::from([TOOLS, CONTEXT]),
        |ctx, _: &()| {
            let memory = Memory::new();
            ctx.provide(MEMORY, memory.clone())?;
            let book = ctx.require::<ContextBook>(CONTEXT)?;
            own_sections(
                ctx,
                vec![book.section(ORDER_MEMORY, "memory", move |exec| {
                    let mem = exec.get::<Memory>(MEMORY)?;
                    if !mem.enabled() {
                        return None;
                    }
                    let root = mem.root();
                    let _ = root.ensure_layout();
                    Some(prompt::memory_section_body(&root))
                })?],
            )?;
            let tools = ctx.require::<Tools>(TOOLS)?;
            memory.bind_tools((*tools).clone());
            let mem_dispose = memory.clone();
            own_registered(
                ctx,
                vec![Disposable::from_fn(move || {
                    mem_dispose.clear_tool_registration();
                })],
            )?;
            Ok(None)
        },
    )
}

async fn memory_search(memory: &Memory, call: ToolCall) -> ToolResult {
    if !memory.enabled() {
        return tool_result(
            call,
            "Memory is disabled. Set [memory] enabled = true or DOCK_MEMORY=1.",
        );
    }
    let v: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or_default();
    let query = v.get("query").and_then(|x| x.as_str()).unwrap_or("").trim();
    if query.is_empty() {
        return tool_result(call, "Error: query is required");
    }
    let max = v
        .get("max_results")
        .and_then(|x| x.as_u64())
        .unwrap_or(6)
        .clamp(1, 20) as usize;
    let root = memory.root();
    let _ = root.ensure_layout();
    memory.sync_dirty_if_needed(&root);

    let search_cfg = MemorySearchConfig {
        max_results: max,
        ..MemorySearchConfig::default()
    };
    let query_embedding = embed_query_if_configured(&memory.config().embedding, query).await;
    match search_memory_with_config(&root, query, &search_cfg, query_embedding.as_deref()) {
        Ok(hits) => {
            let min_score = v.get("min_score").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let filtered: Vec<_> = hits.into_iter().filter(|h| h.score >= min_score).collect();
            tool_result(call, format_search_results(&filtered))
        }
        Err(e) => tool_result(call, format!("Error: {e}")),
    }
}

fn to_dock_embedding(cfg: &MemoryEmbeddingConfig) -> dock_memory::MemoryEmbeddingConfig {
    dock_memory::MemoryEmbeddingConfig {
        model: cfg.model.clone(),
        base: cfg.base.clone(),
        api_key: cfg.api_key.clone(),
        dimensions: cfg.dimensions,
    }
}

/// When `[memory.embedding]` is configured, embed the query for hybrid search.
/// Soft-fail → `None` (FTS-only, Grok-like).
pub(crate) async fn embed_query_if_configured(
    cfg: &MemoryEmbeddingConfig,
    query: &str,
) -> Option<Vec<f32>> {
    let dock_cfg = to_dock_embedding(cfg);
    let provider = ApiEmbeddingProvider::from_config(&dock_cfg)?;
    match provider.embed_batch(&[query]).await {
        Ok(mut batch) => batch.pop(),
        Err(e) => {
            tracing::warn!(
                target: "dock_memory",
                error = %e,
                "query embedding failed; FTS-only"
            );
            None
        }
    }
}

/// After remember/flush/dream writes: embed missing chunks when configured (soft-fail).
pub(crate) async fn embed_missing_after_write(cfg: &MemoryEmbeddingConfig, root: &MemoryRoot) {
    let dock_cfg = to_dock_embedding(cfg);
    let Some(provider) = ApiEmbeddingProvider::from_config(&dock_cfg) else {
        return;
    };
    let dims = provider.dimensions();
    let index = match MemoryIndex::open_or_create_with_dimensions(&root.search_db(), dims) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!(
                target: "dock_memory",
                error = %e,
                "open index for embed_missing failed"
            );
            return;
        }
    };
    // Owned index → future is Send (Connection is Send, &Connection is not).
    let n = embed_missing_chunks_owned(index, &provider).await;
    if n > 0 {
        tracing::debug!(target: "dock_memory", embedded = n, "post-write embed_missing");
    }
}

/// Fire-and-forget embed after sync writes when a Tokio runtime is available.
pub(crate) fn spawn_embed_missing_after_write(cfg: MemoryEmbeddingConfig, root: MemoryRoot) {
    let dock_cfg = to_dock_embedding(&cfg);
    if ApiEmbeddingProvider::from_config(&dock_cfg).is_none() {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::debug!(target: "dock_memory", "skip embed_missing: no tokio runtime");
        return;
    };
    handle.spawn(async move {
        embed_missing_after_write(&cfg, &root).await;
    });
}

fn memory_get(memory: &Memory, call: ToolCall) -> ToolResult {
    if !memory.enabled() {
        return tool_result(
            call,
            "Memory is disabled. Set [memory] enabled = true or DOCK_MEMORY=1.",
        );
    }
    let v: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or_default();
    let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("").trim();
    if path.is_empty() {
        return tool_result(call, "Error: path is required");
    }
    let from = v.get("from").and_then(|x| x.as_u64()).map(|n| n as usize);
    let lines = v.get("lines").and_then(|x| x.as_u64()).map(|n| n as usize);
    let root = memory.root();
    match read_memory_file(&root, PathBuf::from(path).as_path(), from, lines) {
        Ok(body) => {
            let start = from.unwrap_or(1).max(1);
            let numbered = format_with_line_numbers(&body, start);
            let nlines = body.split('\n').count();
            tool_result(
                call,
                format!(
                    "**File:** {path}\n**Lines:** {nlines} (from: {}, limit: {})\n\n{numbered}",
                    from.map_or_else(|| "start".into(), |f| f.to_string()),
                    lines.map_or_else(|| "all".into(), |l| l.to_string()),
                ),
            )
        }
        Err(e) => tool_result(call, format!("Error: {e}")),
    }
}

/// Compact-path helper: flush when gates pass. Fail-open on errors.
pub async fn maybe_flush_before_compact(ctx: &Context) {
    ops::maybe_flush_before_compact(ctx).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use dock_memory::index::MemoryIndex;
    use dock_memory::search_memory;
    use dock_memory::storage::save_remember_note;

    #[test]
    fn search_finds_remembered_note() {
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "1");
        let cwd = std::env::current_dir().unwrap();
        let root = MemoryRoot::open_default(&cwd);
        let mut idx = MemoryIndex::open_or_create(&root.search_db()).unwrap();
        save_remember_note(&root, "prefer conventional commits for dock", &mut idx).unwrap();
        let hits = search_memory(&root, "conventional commits", 5).unwrap();
        assert!(!hits.is_empty());
    }

    #[tokio::test]
    async fn context_book_injects_memory_when_enabled() {
        use crate::prompt::context_book::ContextBook;
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "1");
        let root = Context::new();
        crate::install_without_llm(&root).await.unwrap();
        root.plugin(tool_memory(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let book = root.get::<ContextBook>(CONTEXT).unwrap();
        let rendered = book.assemble_on(&root).render();
        assert!(
            rendered.contains("<memory>"),
            "expected <memory> section when enabled; got: {}",
            &rendered[..rendered.len().min(800)]
        );
        assert!(rendered.contains("memory_search"));
        assert!(rendered.contains("MEMORY.md"));
    }

    #[tokio::test]
    async fn context_book_skips_memory_when_disabled() {
        use crate::prompt::context_book::ContextBook;
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "0");
        let root = Context::new();
        crate::install_without_llm(&root).await.unwrap();
        root.plugin(tool_memory(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let book = root.get::<ContextBook>(CONTEXT).unwrap();
        let rendered = book.assemble_on(&root).render();
        assert!(
            !rendered.contains("<memory>"),
            "disabled must not inject <memory>"
        );
    }

    #[test]
    fn enabled_memory_starts_watcher() {
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "1");
        let mem = Memory::new();
        assert!(mem.enabled());
        mem.ensure_watcher();
        // Watcher may fail under tight fd limits; best-effort.
        let _ = mem.has_watcher();
    }

    #[test]
    fn dirty_sync_before_search_reindexes_paths() {
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "1");
        let mem = Memory::new();
        let root = mem.root();
        root.ensure_layout().unwrap();
        let path = root.workspace.topics.join("wire.md");
        std::fs::write(&path, "## Wire\n\nalpha topic about otters\n").unwrap();
        let mut idx = MemoryIndex::open_or_create(&root.search_db()).unwrap();
        let _ = idx.reindex_tree(&root);
        drop(idx);

        std::fs::write(&path, "## Wire\n\nbeta topic about narwhals\n").unwrap();
        let mut idx = MemoryIndex::open_or_create(&root.search_db()).unwrap();
        sync_dirty_paths(&root, &mut idx, &[path]);
        drop(idx);

        mem.sync_dirty_if_needed(&root);
        let hits = search_memory(&root, "narwhals", 5).unwrap();
        assert!(
            hits.iter()
                .any(|h| h.text.to_lowercase().contains("narwhals")),
            "dirty sync path must surface external edits; hits={hits:?}"
        );
    }

    #[tokio::test]
    async fn embedding_path_soft_fails_without_api_key() {
        // Missing API key → from_config None → FTS-only (Grok-like).
        let cfg = MemoryEmbeddingConfig {
            model: Some("text-embedding-3-small".into()),
            base: Some("https://example.invalid/v1".into()),
            api_key: None,
            dimensions: 8,
        };
        assert!(
            embed_query_if_configured(&cfg, "hello").await.is_none(),
            "missing API key must stay FTS-only"
        );
    }

    #[tokio::test]
    async fn search_memory_with_config_accepts_query_embedding() {
        // Exercise hybrid entry: FTS-only when embedding is None; Some(vec) is
        // accepted (vec may degrade if sqlite-vec dims mismatch — soft path).
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "1");
        let root = MemoryRoot::open_default(&std::env::current_dir().unwrap());
        let _ = root.ensure_layout();
        let cfg = MemorySearchConfig {
            max_results: 3,
            min_score: 0.0,
            ..MemorySearchConfig::default()
        };
        assert!(search_memory_with_config(&root, "hello", &cfg, None).is_ok());
        let fake = vec![0.1_f32; 8];
        // Soft: may Err or Ok depending on vec availability / dim mismatch.
        let _ = search_memory_with_config(&root, "hello", &cfg, Some(&fake));
    }
}

#[tokio::test]
async fn enabled_registers_memory_tools_in_sampler_table() {
    let _env = cordis_base::test_env::scoped()
        .home()
        .set("DOCK_MEMORY", "1");
    let root = Context::new();
    crate::install_without_llm(&root).await.unwrap();
    root.plugin(tool_memory(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let tools = (*root.get::<Tools>(TOOLS).unwrap()).clone();
    let model: Vec<String> = tools
        .specs_for_model()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(
        model.iter().any(|n| n == "memory_search"),
        "enabled memory_search must be sampler-resident: {model:?}"
    );
    assert!(
        model.iter().any(|n| n == "memory_get"),
        "enabled memory_get must be sampler-resident: {model:?}"
    );
    assert!(
        !tools.is_deferred("memory_search") && !tools.is_deferred("memory_get"),
        "memory tools must not be deferred when enabled"
    );
}

#[tokio::test]
async fn disabled_omits_memory_tools_from_table() {
    let _env = cordis_base::test_env::scoped()
        .home()
        .set("DOCK_MEMORY", "0");
    let root = Context::new();
    crate::install_without_llm(&root).await.unwrap();
    root.plugin(tool_memory(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let tools = (*root.get::<Tools>(TOOLS).unwrap()).clone();
    let names: Vec<String> = tools.specs().into_iter().map(|s| s.name).collect();
    assert!(
        !names
            .iter()
            .any(|n| n == "memory_search" || n == "memory_get"),
        "disabled memory must leave tools out of the table: {names:?}"
    );
}

#[tokio::test]
async fn session_toggle_removes_and_restores_resident_tools() {
    let _env = cordis_base::test_env::scoped()
        .home()
        .set("DOCK_MEMORY", "1");
    let root = Context::new();
    crate::install_without_llm(&root).await.unwrap();
    root.plugin(tool_memory(), ())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let mem = (*root.get::<Memory>(MEMORY).unwrap()).clone();
    let tools = (*root.get::<Tools>(TOOLS).unwrap()).clone();
    assert!(tools
        .specs_for_model()
        .iter()
        .any(|s| s.name == "memory_search"));
    assert!(mem.toggle_session().unwrap() == false);
    let model_off: Vec<_> = tools
        .specs_for_model()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(
        !model_off
            .iter()
            .any(|n| n == "memory_search" || n == "memory_get"),
        "toggle off must remove resident tools: {model_off:?}"
    );
    assert!(mem.toggle_session().unwrap());
    let model_on: Vec<_> = tools
        .specs_for_model()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert!(
        model_on.iter().any(|n| n == "memory_search") && model_on.iter().any(|n| n == "memory_get"),
        "toggle on must restore resident tools: {model_on:?}"
    );
}

#[cfg(test)]
mod compact_hook_tests {
    use cordis_base::config::MemoryFlushConfig;
    use dock_memory::flush::should_flush;

    #[test]
    fn compact_hook_gate_matches_flush_policy() {
        let cfg = MemoryFlushConfig {
            enabled: true,
            soft_threshold_tokens: 4000,
            flush_model: None,
            max_flush_write_chars: 8000,
        };
        // window 100k, compact at 85% = 85000; flush at 85000-4000 = 81000
        assert!(!should_flush(80_000, 100_000, 85, &cfg, 0, 1));
        assert!(should_flush(81_000, 100_000, 85, &cfg, 0, 1));
        // already flushed this cycle
        assert!(!should_flush(90_000, 100_000, 85, &cfg, 1, 1));
        // next cycle
        assert!(should_flush(90_000, 100_000, 85, &cfg, 1, 2));
    }
}

#[cfg(test)]
mod flush_mock_tests {
    use super::*;
    use crate::agent::runtime::BoxFuture;
    use crate::llm::sampler::{Llm, Sampler};
    use crate::names::{LLM, SESSIONS};
    use crate::session::log::Sessions;
    use cordis::Context;
    use cordis_base::stream_acc::StreamDelta;
    use cordis_base::types::{LlmOutput, LogEvent, PromptRequest};
    use std::sync::Arc;

    struct FixedFlush(String);
    impl Sampler for FixedFlush {
        fn sample<'a>(
            &'a self,
            _request: PromptRequest,
            mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            let text = self.0.clone();
            Box::pin(async move {
                on_delta(StreamDelta::Text(text.clone()));
                LlmOutput {
                    text,
                    ..LlmOutput::default()
                }
            })
        }
    }

    #[tokio::test]
    async fn flush_writes_observation_without_network() {
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "1");
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.append(LogEvent::User("we decided to use blake3 for slugs".into()));
        sessions.append(LogEvent::LlmStream(LlmOutput {
            text: "ok, noted blake3".into(),
            ..LlmOutput::default()
        }));
        let _s = root.provide(SESSIONS, sessions).unwrap();
        root.provide(
            LLM,
            Llm::from_sampler(
                root.clone(),
                Arc::new(FixedFlush(
                    "## Decisions\n\n- use blake3 for workspace memory slugs\n".into(),
                )),
            ),
        )
        .unwrap();
        root.provide(MEMORY, Memory::new()).unwrap();

        let msg = run_flush(&root, true).await.unwrap();
        assert!(msg.contains("Memory flush wrote"), "{msg}");
        let mem = MemoryRoot::open_default(&std::env::current_dir().unwrap());
        let files: Vec<_> = std::fs::read_dir(&mem.workspace.observations)
            .unwrap()
            .flatten()
            .collect();
        assert!(!files.is_empty(), "expected observation file");
    }

    struct ErrFlush;
    impl Sampler for ErrFlush {
        fn sample<'a>(
            &'a self,
            _request: PromptRequest,
            _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            Box::pin(async move {
                LlmOutput {
                    // Tempting text that would otherwise be accepted as a flush.
                    text: "## Decisions\n\n- should not be written\n".into(),
                    error: Some("llm request failed: 503".into()),
                    ..LlmOutput::default()
                }
            })
        }
    }

    #[tokio::test]
    async fn flush_short_circuits_on_llm_error_without_writing() {
        let _env = cordis_base::test_env::scoped()
            .home()
            .set("DOCK_MEMORY", "1");
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        sessions.append(LogEvent::User("note something".into()));
        let _s = root.provide(SESSIONS, sessions).unwrap();
        root.provide(LLM, Llm::from_sampler(root.clone(), Arc::new(ErrFlush)))
            .unwrap();
        root.provide(MEMORY, Memory::new()).unwrap();

        let err = run_flush(&root, true)
            .await
            .expect_err("must fail on LlmOutput.error");
        let msg = err.to_string();
        assert!(
            msg.contains("memory flush LLM failed"),
            "unexpected err: {msg}"
        );
        let mem = MemoryRoot::open_default(&std::env::current_dir().unwrap());
        let _ = std::fs::create_dir_all(&mem.workspace.inbox);
        // Watcher start → ensure_layout creates observations/_inbox/; only .md
        // files count as flush writes.
        let md_files: Vec<_> = std::fs::read_dir(&mem.workspace.inbox)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            .collect();
        assert!(
            md_files.is_empty(),
            "failed stream must not write observations; got {md_files:?}"
        );
    }
}
