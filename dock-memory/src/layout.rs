//! On-disk layout under `$DOCK_HOME/memory/`.
//!
//! Per scope (Grok memory-v2 layout without the "v2" name):
//! ```text
//! {global,workspace-<slug>}/
//!   topics/
//!   observations/_inbox/   # new observations land here
//!   archive/               # dream / forget destination
//!   MEMORY.md              # generated index (read-only)
//!   memory_state.sqlite    # revisions / tombstones
//! ```
//! Shared FTS (+ optional vec) index: `$DOCK_HOME/memory/search.sqlite`.
//!
//! One-time migrate: flat `observations/*.md` → `observations/_inbox/`.
//! Reads still accept leftover flat paths until migrated.

use std::path::{Path, PathBuf};

use crate::slug::workspace_slug;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    Global,
    Workspace,
}

#[derive(Debug, Clone)]
pub struct ScopePaths {
    pub root: PathBuf,
    pub topics: PathBuf,
    /// Parent `observations/` directory (may still hold legacy flat `.md`).
    pub observations: PathBuf,
    /// Canonical write target: `observations/_inbox/`.
    pub inbox: PathBuf,
    pub archive: PathBuf,
}

impl ScopePaths {
    fn from_root(root: PathBuf) -> Self {
        let observations = root.join("observations");
        Self {
            topics: root.join("topics"),
            inbox: observations.join("_inbox"),
            archive: root.join("archive"),
            observations,
            root,
        }
    }

    fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.topics)?;
        std::fs::create_dir_all(&self.observations)?;
        std::fs::create_dir_all(&self.inbox)?;
        std::fs::create_dir_all(&self.archive)?;
        ensure_state_db(&self.root.join("memory_state.sqlite"))?;
        migrate_flat_observations_once(self)?;
        Ok(())
    }

    /// Observation directories to scan (inbox first, then legacy flat).
    pub fn observation_read_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.inbox.clone()];
        // Legacy flat files (not yet migrated / race) — read-compat.
        if self.observations.is_dir() {
            dirs.push(self.observations.clone());
        }
        dirs
    }
}

/// Resolved memory roots for one cwd.
#[derive(Debug, Clone)]
pub struct MemoryRoot {
    pub home: PathBuf,
    pub slug: String,
    pub global: ScopePaths,
    pub workspace: ScopePaths,
}

impl MemoryRoot {
    pub fn open(dock_home: &Path, cwd: &Path) -> Self {
        let home = dock_home.join("memory");
        let slug = workspace_slug(cwd);
        let global_dir = home.join("global");
        let workspace_dir = home.join(format!("workspace-{slug}"));
        Self {
            home,
            slug,
            global: ScopePaths::from_root(global_dir),
            workspace: ScopePaths::from_root(workspace_dir),
        }
    }

    pub fn open_default(cwd: &Path) -> Self {
        Self::open(&cordis_base::config::dock_home(), cwd)
    }

    pub fn search_db(&self) -> PathBuf {
        self.home.join("search.sqlite")
    }

    pub fn ensure_layout(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.home)?;
        self.global.ensure()?;
        self.workspace.ensure()?;
        // Refresh lean MEMORY.md indexes (idempotent; cheap when empty).
        let _ = crate::manifest::refresh_all(self);
        Ok(())
    }

    pub fn scope(&self, scope: MemoryScope) -> &ScopePaths {
        match scope {
            MemoryScope::Global => &self.global,
            MemoryScope::Workspace => &self.workspace,
        }
    }

    /// Paths that tools may read (new layout + legacy read-only compat).
    pub fn read_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![
            self.home.clone(),
            self.global.root.clone(),
            self.workspace.root.clone(),
        ];
        // Legacy layout (~/.dock/memory flat / .dock/memory) — read-only.
        let legacy_project = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".dock")
            .join("memory");
        if legacy_project.exists() {
            roots.push(legacy_project);
        }
        roots
    }

    pub fn classify_source(&self, path: &Path) -> &'static str {
        if path.starts_with(&self.workspace.root) {
            "workspace"
        } else if path.starts_with(&self.global.root) {
            "global"
        } else {
            "legacy"
        }
    }
}

const STATE_SCHEMA_VERSION: &str = "1";

fn ensure_state_db(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let db = rusqlite::Connection::open(path)
        .map_err(|e| std::io::Error::other(format!("open state db: {e}")))?;
    db.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS memory_tombstones (
            tombstone_id TEXT PRIMARY KEY,
            target_kind TEXT NOT NULL,
            relative_path TEXT NOT NULL UNIQUE,
            provenance_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            reason TEXT NOT NULL
        );
        ",
    )
    .map_err(|e| std::io::Error::other(format!("state schema: {e}")))?;
    db.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
        rusqlite::params![STATE_SCHEMA_VERSION],
    )
    .map_err(|e| std::io::Error::other(format!("state meta: {e}")))?;
    Ok(())
}

/// Move flat `observations/*.md` into `_inbox/` once (never move subdirs / `_inbox`).
fn migrate_flat_observations_once(scope: &ScopePaths) -> std::io::Result<()> {
    let Ok(entries) = std::fs::read_dir(&scope.observations) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        let dest = scope.inbox.join(name);
        if dest.exists() {
            // Collision: leave flat file; indexer still reads both.
            continue;
        }
        match std::fs::rename(&path, &dest) {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(
                    from = %path.display(),
                    to = %dest.display(),
                    error = %e,
                    "failed to migrate flat observation into _inbox"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_dirs_created_with_inbox_and_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/proj-layout"));
        root.ensure_layout().unwrap();
        assert!(root.global.topics.is_dir());
        assert!(root.global.observations.is_dir());
        assert!(root.global.inbox.is_dir());
        assert!(root.global.archive.is_dir());
        assert!(root.workspace.inbox.is_dir());
        assert!(root.workspace.archive.is_dir());
        assert!(root.global.root.join("memory_state.sqlite").is_file());
        assert!(root
            .workspace
            .root
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("workspace-"));
        assert_eq!(root.search_db(), tmp.path().join("memory/search.sqlite"));
    }

    #[test]
    fn migrates_flat_observations_into_inbox() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/proj-migrate"));
        // Pre-create flat layout (Phase-1 style).
        std::fs::create_dir_all(&root.workspace.observations).unwrap();
        let flat = root.workspace.observations.join("flush-old.md");
        std::fs::write(&flat, "## Old\n\nflat note\n").unwrap();
        root.ensure_layout().unwrap();
        assert!(!flat.exists(), "flat file should move");
        assert!(root.workspace.inbox.join("flush-old.md").is_file());
    }

    #[test]
    fn migrate_skips_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/proj-collide"));
        std::fs::create_dir_all(&root.global.inbox).unwrap();
        std::fs::create_dir_all(&root.global.observations).unwrap();
        std::fs::write(root.global.inbox.join("same.md"), "inbox\n").unwrap();
        std::fs::write(root.global.observations.join("same.md"), "flat\n").unwrap();
        root.ensure_layout().unwrap();
        assert!(root.global.observations.join("same.md").is_file());
        assert_eq!(
            std::fs::read_to_string(root.global.inbox.join("same.md")).unwrap(),
            "inbox\n"
        );
    }
}
