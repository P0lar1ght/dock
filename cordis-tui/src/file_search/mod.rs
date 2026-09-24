//! @-completion. Chrome matches the slash dropdown; matching uses the
//! `xai-fuzzy-file-search` crate under `cordis-tui/fuzzy-file-search` (nucleo + ignore walk).

mod context;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use context::{detect, normalize_display_path};

use xai_fuzzy_file_search::FuzzyFileMatcher;

use crate::slash::{SlashCmd, SlashPick, SuggestionRow, MAX_VISIBLE_SUGGESTIONS};

struct MatcherCache {
    root: PathBuf,
    hidden: bool,
    matcher: FuzzyFileMatcher,
}

static MATCHER: Mutex<Option<MatcherCache>> = Mutex::new(None);

#[derive(Debug, Clone)]
pub struct FileHit {
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct FileSearchSnapshot {
    pub open: bool,
    pub selected: usize,
    pub matches: Vec<FileHit>,
}

impl FileSearchSnapshot {
    pub fn current(&self) -> Option<&FileHit> {
        self.matches.get(self.selected)
    }

    pub fn suggestion_rows(&self) -> Vec<SuggestionRow> {
        self.matches
            .iter()
            .map(|hit| SuggestionRow {
                display: hit.path.clone(),
                description: if hit.is_dir { "dir" } else { "file" }.into(),
                pick: SlashPick::Builtin(SlashCmd::Help),
            })
            .collect()
    }
}

/// `root` 是这一页的工作目录（`@` 补全在它下面找文件）。
pub fn snapshot(
    text: &str,
    cursor: usize,
    selected: usize,
    dismissed: bool,
    root: &Path,
) -> FileSearchSnapshot {
    if dismissed {
        return FileSearchSnapshot {
            open: false,
            selected: 0,
            matches: Vec::new(),
        };
    }
    let Some(ctx) = detect(text, cursor) else {
        return FileSearchSnapshot {
            open: false,
            selected: 0,
            matches: Vec::new(),
        };
    };
    let hits = nucleo_hits(
        root,
        ctx.matcher_query(),
        ctx.is_dir_mode(),
        ctx.is_hidden_mode(),
    );
    let selected = if hits.is_empty() {
        0
    } else {
        selected.min(hits.len() - 1)
    };
    FileSearchSnapshot {
        open: true,
        selected,
        matches: hits,
    }
}

pub fn desired_rows(snap: &FileSearchSnapshot) -> u16 {
    (snap.matches.len().min(MAX_VISIBLE_SUGGESTIONS)) as u16
}

fn nucleo_hits(root: &Path, query: &str, dir_only: bool, hidden: bool) -> Vec<FileHit> {
    let mut guard = MATCHER.lock().unwrap();
    let stale = guard
        .as_ref()
        .is_none_or(|c| c.root != root || c.hidden != hidden);
    if stale {
        let mut matcher = FuzzyFileMatcher::new(root);
        matcher.restart_walk_with(|w| {
            w.hidden(!hidden);
            w
        });
        *guard = Some(MatcherCache {
            root: root.to_path_buf(),
            hidden,
            matcher,
        });
    }
    let Some(cache) = guard.as_mut() else {
        return Vec::new();
    };
    cache.matcher.set_query(query, dir_only);
    cache.matcher.tick(30);
    cache
        .matcher
        .get_top_k(MAX_VISIBLE_SUGGESTIONS.max(24))
        .into_iter()
        .map(|hit| FileHit {
            path: normalize_display_path(&hit.path.to_string()).to_string(),
            is_dir: hit.is_dir,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_at_is_open() {
        let snap = snapshot("@", 1, 0, false, Path::new("."));
        assert!(snap.open);
    }

    #[test]
    fn dismissed_is_closed() {
        let snap = snapshot("@src", 4, 0, true, Path::new("."));
        assert!(!snap.open);
    }

    #[test]
    fn prose_without_at_is_closed() {
        let snap = snapshot("hello", 5, 0, false, Path::new("."));
        assert!(!snap.open);
    }
}
