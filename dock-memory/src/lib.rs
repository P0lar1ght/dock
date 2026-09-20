//! Cross-session memory for Dock.
//!
//! Disk layout (under `$DOCK_HOME/memory/`):
//! ```text
//! memory/
//!   search.sqlite
//!   global/
//!     topics/
//!     observations/_inbox/
//!     archive/
//!     MEMORY.md
//!     memory_state.sqlite
//!   workspace-<slug>/  (same shape)
//! ```
//!
//! Tool names stay `memory_search` / `memory_get`. Default off; `DOCK_MEMORY=1/0`
//! overrides `[memory] enabled`. No public "v2" naming — paths use topics,
//! observations/_inbox, MEMORY.md.

#![deny(clippy::indexing_slicing)]

pub mod access;
pub mod browse;
pub mod chunker;
pub mod config;
pub mod dream;
pub mod embedding;
pub mod flush;
pub mod index;
pub mod keywords;
pub mod layout;
pub mod manifest;
pub mod mmr;
pub mod query_expansion;
pub mod rewrite;
pub mod schema;
pub mod search;
pub mod slug;
pub mod storage;
pub mod text_utils;
pub mod watcher;

pub use access::{forget, AccessError, ForgetResult, MemoryAccessPolicy, PathClass};
pub use config::{MemoryEmbeddingConfig, MemorySearchConfig, MmrConfig, SearchResult};
pub use dream::{
    build_dream_user_message, process_dream_response, DreamEligibility, DreamMessage, DreamResult, DreamStatus, auto_dream_eligibility,
    DREAM_SYSTEM_PROMPT,
};
pub use embedding::{embed_missing_chunks, ApiEmbeddingProvider, EmbeddingProvider};
pub use flush::{
    process_flush_response, should_flush, FlushResult, FLUSH_DELTA_SYSTEM_PROMPT,
    FLUSH_SYSTEM_PROMPT,
};
pub use index::{init_sqlite_vec, ChunkRecord, FtsHit, MemoryIndex, ReindexResult, SearchHit};
pub use layout::{MemoryRoot, MemoryScope, ScopePaths};
pub use manifest::{refresh_all, regenerate_scope, Manifest, ManifestBudget};
pub use rewrite::{rewrite_user_message, REMEMBER_REWRITE_SYSTEM_PROMPT};
pub use search::{format_search_results, search_memory, sync_dirty_paths};
pub use slug::workspace_slug;
pub use storage::{persist_observation, save_remember_note, write_flush_observation};
pub use watcher::MemoryFileWatcher;
