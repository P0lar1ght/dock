//! Cross-session memory for Dock (Phase 1).
//!
//! Disk layout (under `$DOCK_HOME/memory/`):
//! ```text
//! memory/
//!   search.sqlite
//!   global/{topics,observations}/
//!   workspace-<slug>/{topics,observations}/
//! ```
//!
//! No embeddings / sqlite-vec / watcher GC in this phase. Tool names stay
//! `memory_search` / `memory_get`. Default off; `DOCK_MEMORY=1/0` overrides
//! `[memory] enabled`.

#![deny(clippy::indexing_slicing)]

pub mod browse;
pub mod chunker;
pub mod dream;
pub mod flush;
pub mod index;
pub mod keywords;
pub mod layout;
pub mod search;
pub mod slug;
pub mod storage;
pub mod text_utils;

pub use dream::{
    build_dream_user_message, process_dream_response, DreamMessage, DreamResult, DreamStatus,
    DREAM_SYSTEM_PROMPT,
};
pub use flush::{
    process_flush_response, should_flush, FlushResult, FLUSH_DELTA_SYSTEM_PROMPT,
    FLUSH_SYSTEM_PROMPT,
};
pub use index::{ChunkRecord, FtsHit, MemoryIndex, ReindexResult, SearchHit};
pub use layout::{MemoryRoot, MemoryScope, ScopePaths};
pub use search::{format_search_results, search_memory};
pub use slug::workspace_slug;
pub use storage::{persist_observation, save_remember_note, write_flush_observation};
