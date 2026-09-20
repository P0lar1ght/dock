//! Write observations / remember notes; reindex after writes.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::index::MemoryIndex;
use crate::layout::{MemoryRoot, MemoryScope};

const MAX_OBSERVATION_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum MemoryWriteError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("observation is {actual} bytes, exceeding the {limit}-byte limit")]
    TooLarge { actual: usize, limit: usize },
    #[error(transparent)]
    Index(#[from] rusqlite::Error),
}

/// Persist an observation markdown file under the scope's `observations/` dir.
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
    std::fs::create_dir_all(&scope.observations)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = format!("{prefix}-{stamp}.md");
    let path = scope.observations.join(&name);
    let mut tmp = tempfile::Builder::new()
        .prefix(".obs-")
        .suffix(".tmp")
        .tempfile_in(&scope.observations)?;
    tmp.write_all(trimmed.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(path)
}

/// Flush accepted content → workspace observations + FTS reindex.
pub fn write_flush_observation(
    root: &MemoryRoot,
    content: &str,
    index: &mut MemoryIndex,
) -> Result<PathBuf, MemoryWriteError> {
    root.ensure_layout()?;
    let path = persist_observation(&root.workspace, content, "flush")?;
    index.reindex_file(&path, "workspace")?;
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
    Ok(path)
}

pub fn normalize_memory_content(content: &str) -> String {
    content.trim().to_string()
}

/// Safe read of a memory file; path must resolve under allowed roots.
pub fn read_memory_file(
    root: &MemoryRoot,
    path: &Path,
    from: Option<usize>,
    lines: Option<usize>,
) -> Result<String, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("cannot resolve path: {e}"))?;
    let allowed = root.read_roots().iter().any(|r| {
        r.canonicalize()
            .ok()
            .is_some_and(|cr| canonical.starts_with(cr))
            || canonical.starts_with(r)
    });
    if !allowed {
        return Err("path is outside memory directories".into());
    }
    let text = std::fs::read_to_string(&canonical).map_err(|e| e.to_string())?;
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
    fn remember_writes_and_indexes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/remember-proj"));
        let mut idx = MemoryIndex::open_or_create(&root.search_db()).unwrap();
        let path = save_remember_note(&root, "always open PR links", &mut idx).unwrap();
        assert!(path.exists());
        let hits = idx.search("PR links", 5).unwrap();
        assert!(!hits.is_empty());
    }
}
