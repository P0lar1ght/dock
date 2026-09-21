//! Write observations / remember notes; reindex after writes.
//!
//! New observations always land in `observations/_inbox/`.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::index::MemoryIndex;
use crate::layout::{MemoryRoot, MemoryScope};

const MAX_OBSERVATION_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum MemoryWriteError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("观察记录为 {actual} 字节，超过 {limit} 字节上限")]
    TooLarge { actual: usize, limit: usize },
    #[error(transparent)]
    Index(#[from] rusqlite::Error),
}

/// Persist an observation markdown file under the scope's `observations/_inbox/`.
pub fn persist_observation(
    scope: &crate::layout::ScopePaths,
    content: &str,
    prefix: &str,
) -> Result<PathBuf, MemoryWriteError> {
    let trimmed = content.trim();
    if trimmed.len() > MAX_OBSERVATION_BYTES {
        return Err(MemoryWriteError::TooLarge {
            actual: trimmed.len(),
            limit: MAX_OBSERVATION_BYTES,
        });
    }
    std::fs::create_dir_all(&scope.inbox)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = format!("{prefix}-{stamp}.md");
    let path = scope.inbox.join(&name);
    let mut tmp = tempfile::Builder::new()
        .prefix(".obs-")
        .suffix(".tmp")
        .tempfile_in(&scope.inbox)?;
    tmp.write_all(trimmed.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(path)
}

/// Flush accepted content → workspace inbox + FTS reindex.
pub fn write_flush_observation(
    root: &MemoryRoot,
    content: &str,
    index: &mut MemoryIndex,
) -> Result<PathBuf, MemoryWriteError> {
    root.ensure_layout()?;
    let path = persist_observation(&root.workspace, content, "flush")?;
    index.reindex_file(&path, "workspace")?;
    let _ = crate::manifest::regenerate_scope(&root.workspace, MemoryScope::Workspace);
    Ok(path)
}

/// `/remember` — simple global observation (cross-project preferences).
pub fn save_remember_note(
    root: &MemoryRoot,
    content: &str,
    index: &mut MemoryIndex,
) -> Result<PathBuf, MemoryWriteError> {
    let normalized = normalize_memory_content(content);
    if normalized.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty note").into());
    }
    root.ensure_layout()?;
    let body = format!("## Remember\n\n{normalized}\n");
    let path = persist_observation(&root.global, &body, "remember")?;
    index.reindex_file(&path, "global")?;
    let _ = crate::manifest::regenerate_scope(&root.global, MemoryScope::Global);
    Ok(path)
}

pub fn normalize_memory_content(content: &str) -> String {
    content.trim().to_string()
}

/// Largest file `memory_get` / [`read_memory_file`] will return (Grok-scale).
pub const MAX_MEMORY_READ_BYTES: u64 = 256 * 1024;

/// Safe read of a memory markdown file; path must resolve under allowed roots.
///
/// Only `.md` files are readable. SQLite/binary/oversize paths are rejected with
/// a clear error (no full-body load of huge files).
pub fn read_memory_file(
    root: &MemoryRoot,
    path: &Path,
    from: Option<usize>,
    lines: Option<usize>,
) -> Result<String, String> {
    if path.extension().and_then(|e| e.to_str()) != Some("md") {
        return Err(format!(
            "memory_get only reads .md files under the memory root (refused: {})",
            path.display()
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("cannot resolve path: {e}"))?;
    if canonical.extension().and_then(|e| e.to_str()) != Some("md") {
        return Err(format!(
            "memory_get only reads .md files under the memory root (refused: {})",
            canonical.display()
        ));
    }
    let allowed = root.read_roots().iter().any(|r| {
        r.canonicalize()
            .ok()
            .is_some_and(|cr| canonical.starts_with(cr))
            || canonical.starts_with(r)
    });
    if !allowed {
        return Err("path is outside memory directories".into());
    }
    let meta = std::fs::metadata(&canonical).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(format!("not a regular file: {}", canonical.display()));
    }
    if meta.len() > MAX_MEMORY_READ_BYTES {
        return Err(format!(
            "memory file is too large to read ({} bytes; limit {} bytes): {}",
            meta.len(),
            MAX_MEMORY_READ_BYTES,
            canonical.display()
        ));
    }
    let text = std::fs::read_to_string(&canonical).map_err(|e| e.to_string())?;
    if text.len() as u64 > MAX_MEMORY_READ_BYTES {
        return Err(format!(
            "memory file is too large to read ({} bytes; limit {} bytes): {}",
            text.len(),
            MAX_MEMORY_READ_BYTES,
            canonical.display()
        ));
    }
    let all: Vec<&str> = text.split('\n').collect();
    let start = from.unwrap_or(1).max(1);
    let start_idx = start.saturating_sub(1).min(all.len());
    let take = lines.unwrap_or(all.len());
    let slice = all
        .get(start_idx..(start_idx + take).min(all.len()))
        .unwrap_or(&[]);
    Ok(slice.join("\n"))
}

pub fn format_with_line_numbers(content: &str, first_line_num: usize) -> String {
    if content.is_empty() {
        return String::new();
    }
    content
        .split('\n')
        .enumerate()
        .map(|(i, line)| format!("{}→{}", first_line_num + i, line))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn scope_label(scope: MemoryScope) -> &'static str {
    match scope {
        MemoryScope::Global => "global",
        MemoryScope::Workspace => "workspace",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_writes_inbox_and_indexes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/remember-proj"));
        let mut idx = MemoryIndex::open_or_create(&root.search_db()).unwrap();
        let path = save_remember_note(&root, "always open PR links", &mut idx).unwrap();
        assert!(path.exists());
        assert!(
            path.starts_with(&root.global.inbox),
            "path={} inbox={}",
            path.display(),
            root.global.inbox.display()
        );
        let hits = idx.search("PR links", 5).unwrap();
        assert!(!hits.is_empty());
    }

    #[test]
    fn read_memory_file_rejects_non_md_and_sqlite() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/read-gate-proj"));
        root.ensure_layout().unwrap();
        let sqlite = root.search_db();
        // ensure search db exists
        let _ = crate::index::MemoryIndex::open_or_create(&sqlite).unwrap();
        let err = read_memory_file(&root, &sqlite, None, None).unwrap_err();
        assert!(
            err.contains(".md") || err.to_lowercase().contains("refused"),
            "unexpected: {err}"
        );
        let bin = root.global.root.join("notes.bin");
        std::fs::write(&bin, b"not markdown").unwrap();
        let err = read_memory_file(&root, &bin, None, None).unwrap_err();
        assert!(err.contains(".md"), "unexpected: {err}");
    }

    #[test]
    fn read_memory_file_rejects_oversize_md() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/read-size-proj"));
        root.ensure_layout().unwrap();
        let path = root.global.topics.join("huge.md");
        let body = "x".repeat((MAX_MEMORY_READ_BYTES as usize) + 8);
        std::fs::write(&path, &body).unwrap();
        let err = read_memory_file(&root, &path, None, None).unwrap_err();
        assert!(err.contains("too large"), "unexpected: {err}");
    }

    #[test]
    fn read_memory_file_allows_md_under_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/read-ok-proj"));
        root.ensure_layout().unwrap();
        let path = root.global.topics.join("ok.md");
        std::fs::write(&path, "## Hello\n\nworld\n").unwrap();
        let body = read_memory_file(&root, &path, None, None).unwrap();
        assert!(body.contains("world"));
    }
}
