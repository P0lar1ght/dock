//! Filesystem access policy + forget for Dock memory.
//!
//! Writes only to `topics/*.md` and `observations/_inbox/*.md`.
//! `MEMORY.md`, `archive/`, and databases are read-only.
//! `/memory` delete goes through [`forget`] (tombstone + archive + index).

use std::path::{Component, Path, PathBuf};

use crate::index::MemoryIndex;
use crate::layout::{MemoryRoot, MemoryScope, ScopePaths};

/// Largest file [`forget`] will hash and remove (Grok-scale).
pub const MAX_FORGET_FILE_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathClass {
    Outside,
    Manifest(MemoryScope),
    Topic(MemoryScope),
    Observation(MemoryScope),
    Archive(MemoryScope),
    Protected(MemoryScope),
    Nested(MemoryScope),
}

impl PathClass {
    pub fn is_writable(self) -> bool {
        matches!(self, Self::Topic(_) | Self::Observation(_))
    }

    pub fn scope(self) -> Option<MemoryScope> {
        match self {
            Self::Outside => None,
            Self::Manifest(s)
            | Self::Topic(s)
            | Self::Observation(s)
            | Self::Archive(s)
            | Self::Protected(s)
            | Self::Nested(s) => Some(s),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AccessError {
    #[error("memory path must be absolute: {0}")]
    RelativePath(PathBuf),
    #[error("memory path contains traversal components: {0}")]
    Traversal(PathBuf),
    #[error("writes are not allowed to protected memory path: {0}")]
    Protected(PathBuf),
    #[error("memory files must live directly under topics/ or observations/_inbox/: {0}")]
    NestedPath(PathBuf),
    #[error("memory writes require a .md file: {0}")]
    NonMarkdown(PathBuf),
    #[error("memory path is outside configured scopes: {0}")]
    Outside(PathBuf),
    #[error("memory file is too large to forget ({size} bytes; limit {limit} bytes): {path}")]
    TooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Index(#[from] rusqlite::Error),
}

pub type AccessResult<T> = Result<T, AccessError>;

#[derive(Debug)]
pub struct MemoryAccessPolicy {
    global_root: PathBuf,
    workspace_root: PathBuf,
}

impl MemoryAccessPolicy {
    pub fn new(root: &MemoryRoot) -> Self {
        Self {
            global_root: root.global.root.clone(),
            workspace_root: root.workspace.root.clone(),
        }
    }

    pub fn classify(&self, path: &Path) -> PathClass {
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return PathClass::Outside;
        }
        for (scope, scope_root) in [
            (MemoryScope::Global, &self.global_root),
            (MemoryScope::Workspace, &self.workspace_root),
        ] {
            if let Ok(rel) = path.strip_prefix(scope_root) {
                return classify_relative(rel, scope);
            }
        }
        PathClass::Outside
    }

    /// Validate a write target: only topics/ or observations/_inbox/ `.md`.
    pub fn assert_writable(&self, path: &Path) -> AccessResult<PathClass> {
        if !path.is_absolute() {
            return Err(AccessError::RelativePath(path.to_path_buf()));
        }
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(AccessError::Traversal(path.to_path_buf()));
        }
        let class = self.classify(path);
        match class {
            PathClass::Topic(_) | PathClass::Observation(_) => {
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    return Err(AccessError::NonMarkdown(path.to_path_buf()));
                }
                Ok(class)
            }
            PathClass::Nested(_) => Err(AccessError::NestedPath(path.to_path_buf())),
            PathClass::Outside => Err(AccessError::Outside(path.to_path_buf())),
            _ => Err(AccessError::Protected(path.to_path_buf())),
        }
    }
}

fn classify_relative(rel: &Path, scope: MemoryScope) -> PathClass {
    let parts: Vec<_> = rel.components().collect();
    match parts.as_slice() {
        [Component::Normal(a)] if *a == "MEMORY.md" => PathClass::Manifest(scope),
        [Component::Normal(a), Component::Normal(file)]
            if *a == "topics"
                && Path::new(file).extension().and_then(|e| e.to_str()) == Some("md") =>
        {
            PathClass::Topic(scope)
        }
        [Component::Normal(a), Component::Normal(b), Component::Normal(file)]
            if *a == "observations"
                && *b == "_inbox"
                && Path::new(file).extension().and_then(|e| e.to_str()) == Some("md") =>
        {
            PathClass::Observation(scope)
        }
        [Component::Normal(a), ..] if *a == "archive" => PathClass::Archive(scope),
        [Component::Normal(a), ..] if *a == "topics" || *a == "observations" => {
            PathClass::Nested(scope)
        }
        [Component::Normal(a), ..]
            if matches!(
                a.to_str(),
                Some("memory_state.sqlite" | "index.sqlite" | "search.sqlite")
            ) || a.to_string_lossy().ends_with(".sqlite")
                || a.to_string_lossy().ends_with(".db") =>
        {
            PathClass::Protected(scope)
        }
        [] => PathClass::Protected(scope),
        _ => PathClass::Protected(scope),
    }
}

#[derive(Debug, Clone)]
pub struct ForgetResult {
    pub archived_to: Option<PathBuf>,
    pub tombstone_id: String,
    pub index_removed: usize,
}

/// Forget a memory file: verify content hash, archive under `archive/`, tombstone in state db, drop index rows.
pub fn forget(
    root: &MemoryRoot,
    path: &Path,
    expected_content_hash: &str,
    index: &mut MemoryIndex,
) -> AccessResult<ForgetResult> {
    let policy = MemoryAccessPolicy::new(root);
    let class = policy.classify(path);
    if !matches!(class, PathClass::Topic(_) | PathClass::Observation(_)) {
        return Err(AccessError::Protected(path.to_path_buf()));
    }
    let scope = class.scope().expect("scoped");
    let scope_paths = root.scope(scope);

    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(AccessError::Protected(path.to_path_buf()));
    }
    if meta.len() > MAX_FORGET_FILE_BYTES {
        return Err(AccessError::TooLarge {
            path: path.to_path_buf(),
            size: meta.len(),
            limit: MAX_FORGET_FILE_BYTES,
        });
    }
    // Bound the read so a racing grow cannot inflate into a huge allocation.
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    {
        use std::io::Read as _;
        std::fs::File::open(path)?
            .take(MAX_FORGET_FILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
    }
    if bytes.len() as u64 > MAX_FORGET_FILE_BYTES {
        return Err(AccessError::TooLarge {
            path: path.to_path_buf(),
            size: bytes.len() as u64,
            limit: MAX_FORGET_FILE_BYTES,
        });
    }
    let actual = blake3::hash(&bytes);
    let actual_hex = actual.to_hex().to_string();
    if actual_hex != expected_content_hash {
        return Err(AccessError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "content hash mismatch (expected {expected_content_hash}, got {actual_hex}); re-open and confirm delete"
            ),
        )));
    }

    let rel = path
        .strip_prefix(&scope_paths.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let tombstone_id = format!(
        "{}",
        blake3::hash(format!("forget:{rel}:{actual_hex}").as_bytes()).to_hex()
    );

    insert_tombstone(scope_paths, &tombstone_id, &rel, &actual_hex)?;

    std::fs::create_dir_all(&scope_paths.archive)?;
    let name = path
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_else(|| std::ffi::OsString::from("forgotten.md"));
    let dest = unique_archive_path(&scope_paths.archive, &name);
    std::fs::rename(path, &dest)?;

    let index_removed = index.delete_path(path)?;
    let _ = crate::manifest::regenerate_scope(scope_paths, scope);

    Ok(ForgetResult {
        archived_to: Some(dest),
        tombstone_id,
        index_removed,
    })
}

fn unique_archive_path(archive: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let base = archive.join(name);
    if !base.exists() {
        return base;
    }
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("md");
    for i in 1..10_000 {
        let candidate = archive.join(format!("{stem}-{i}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    archive.join(format!("{stem}-{}.{}", std::process::id(), ext))
}

fn insert_tombstone(
    scope: &ScopePaths,
    tombstone_id: &str,
    relative_path: &str,
    hash: &str,
) -> AccessResult<()> {
    let path = scope.root.join("memory_state.sqlite");
    let db = rusqlite::Connection::open(&path)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    db.execute(
        "INSERT OR REPLACE INTO memory_tombstones(
            tombstone_id, target_kind, relative_path, provenance_hash, created_at, reason
         ) VALUES (?1, 'file', ?2, ?3, ?4, 'user_forget')",
        rusqlite::params![tombstone_id, relative_path, hash, now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_only_topics_and_inbox() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/access-proj"));
        root.ensure_layout().unwrap();
        let policy = MemoryAccessPolicy::new(&root);
        let topic = root.global.topics.join("prefs.md");
        assert!(policy.assert_writable(&topic).is_ok());
        let inbox = root.workspace.inbox.join("note.md");
        assert!(policy.assert_writable(&inbox).is_ok());
        assert!(policy
            .assert_writable(&root.global.root.join("MEMORY.md"))
            .is_err());
        assert!(policy
            .assert_writable(&root.global.archive.join("x.md"))
            .is_err());
    }

    #[test]
    fn forget_archives_and_drops_index() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/forget-proj"));
        root.ensure_layout().unwrap();
        let path = root.global.topics.join("bye.md");
        let body = "## Bye\n\nforget this\n";
        std::fs::write(&path, body).unwrap();
        let hash = blake3::hash(body.as_bytes()).to_hex().to_string();
        let mut idx = MemoryIndex::open_or_create(&root.search_db()).unwrap();
        idx.reindex_file(&path, "global").unwrap();
        assert!(!idx.search("forget this", 5).unwrap().is_empty());
        let result = forget(&root, &path, &hash, &mut idx).unwrap();
        assert!(!path.exists());
        assert!(result.archived_to.as_ref().unwrap().exists());
        assert!(idx.search("forget this", 5).unwrap().is_empty());
    }

    #[test]
    fn forget_rejects_oversize_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/forget-huge-proj"));
        root.ensure_layout().unwrap();
        let path = root.global.topics.join("huge.md");
        let body = "y".repeat((MAX_FORGET_FILE_BYTES as usize) + 64);
        std::fs::write(&path, &body).unwrap();
        let hash = blake3::hash(body.as_bytes()).to_hex().to_string();
        let mut idx = MemoryIndex::open_or_create(&root.search_db()).unwrap();
        let err = forget(&root, &path, &hash, &mut idx).unwrap_err();
        match err {
            AccessError::TooLarge { size, limit, .. } => {
                assert!(size > limit);
                assert_eq!(limit, MAX_FORGET_FILE_BYTES);
            }
            other => panic!("expected TooLarge, got {other}"),
        }
        assert!(path.exists(), "oversize forget must refuse delete");
    }
}
