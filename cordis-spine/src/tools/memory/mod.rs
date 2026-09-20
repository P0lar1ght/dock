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

use cordis::{plugin, Context, Inject, Plugin};
use cordis_base::config::{load_memory_config, MemoryConfig};
use dock_memory::layout::MemoryRoot;
use dock_memory::storage::{format_with_line_numbers, read_memory_file};
use dock_memory::{format_search_results, search_memory};

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
}

/// Named `"memory"` — live-looked by TUI / tools / compact flush hook.
#[derive(Clone)]
pub struct Memory {
    inner: Arc<MemoryInner>,
}

impl Memory {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MemoryInner {
                config: Mutex::new(load_memory_config()),
                session_override: Mutex::new(None),
            }),
        }
    }

    pub fn config(&self) -> MemoryConfig {
        self.inner.config.lock().unwrap().clone()
    }

    pub fn reload(&self) {
        *self.inner.config.lock().unwrap() = load_memory_config();
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
        *slot = Some(!currently);
        Ok(!currently)
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
            let mem_search = memory.clone();
            let search: ToolBody = std::sync::Arc::new(move |call| {
                let mem = mem_search.clone();
                Box::pin(async move { memory_search(&mem, call) })
            });
            let mem_get = memory.clone();
            let get: ToolBody = std::sync::Arc::new(move |call| {
                let mem = mem_get.clone();
                Box::pin(async move { memory_get(&mem, call) })
            });
            own_registered(
                ctx,
                vec![
                    tools.register_deferred(
                        ToolSpec {
                            name: "memory_search".into(),
                            description: "Search cross-session local memory for relevant knowledge chunks. Returns ranked results from $DOCK_HOME/memory (topics/observations) and legacy ~/.dock/memory when present.\n\nUse this proactively when a question references prior work, decisions, or conventions you do not have in the current transcript.\n\nMemory is historical context, not automatically the current plan. Verify recalled facts against live sources before relying on them.".into(),
                            parameters_json: SEARCH_PARAMS.into(),
                        },
                        search,
                    )?,
                    tools.register_deferred(
                        ToolSpec {
                            name: "memory_get".into(),
                            description: "Read a memory file by path. Returns the file content with line numbers, optionally limited to a range of lines.\n\nUse after memory_search returns a relevant result. Line numbers are 1-based and match the from parameter.".into(),
                            parameters_json: GET_PARAMS.into(),
                        },
                        get,
                    )?,
                ],
            )?;
            Ok(None)
        },
    )
}

fn memory_search(memory: &Memory, call: ToolCall) -> ToolResult {
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
    match search_memory(&root, query, max) {
        Ok(hits) => {
            let min_score = v.get("min_score").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let filtered: Vec<_> = hits.into_iter().filter(|h| h.score >= min_score).collect();
            tool_result(call, format_search_results(&filtered))
        }
        Err(e) => tool_result(call, format!("Error: {e}")),
    }
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
        let _ = std::fs::create_dir_all(&mem.workspace.observations);
        let files: Vec<_> = std::fs::read_dir(&mem.workspace.observations)
            .unwrap()
            .flatten()
            .collect();
        assert!(
            files.is_empty(),
            "failed stream must not write observations"
        );
    }
}
