//! On-disk layout under `$DOCK_HOME/memory/`.

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
    pub observations: PathBuf,
}

impl ScopePaths {
    fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.topics)?;
        std::fs::create_dir_all(&self.observations)?;
        Ok(())
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
            global: ScopePaths {
                topics: global_dir.join("topics"),
                observations: global_dir.join("observations"),
                root: global_dir,
            },
            workspace: ScopePaths {
                topics: workspace_dir.join("topics"),
                observations: workspace_dir.join("observations"),
                root: workspace_dir,
            },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_dirs_created() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/proj-layout"));
        root.ensure_layout().unwrap();
        assert!(root.global.topics.is_dir());
        assert!(root.global.observations.is_dir());
        assert!(root.workspace.topics.is_dir());
        assert!(root.workspace.observations.is_dir());
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
}
