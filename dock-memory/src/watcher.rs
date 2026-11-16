//! File watcher for detecting external memory edits.
//!
//! Watches `$DOCK_HOME/memory/` for `.md` file changes (create, modify, remove) and accumulates the affected paths.
//! The search path checks [`is_dirty`] before each query and syncs the index for all dirty paths:
//! - **created / modified** files are reindexed via `MemoryIndex::reindex_file`
//! - **deleted** files have their stale chunks removed via `MemoryIndex::delete_path`
//!
//! [`is_dirty`]: MemoryFileWatcher::is_dirty

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Watches the memory directory for `.md` file changes.
/// The notify thread inserts dirty paths via `rcu` and the search path swaps them out, so neither side takes a lock.
/// The separate `dirty` flag lets `is_dirty` answer with one atomic load and no allocation.
pub struct MemoryFileWatcher {
    dirty_files: Arc<ArcSwap<HashSet<PathBuf>>>,
    dirty: Arc<AtomicBool>,
    _watcher: RecommendedWatcher,
}

impl MemoryFileWatcher {
    /// Start watching the given memory directory for `.md` file changes.
    ///
    /// Returns `None` if the watcher fails to initialize, after logging a warning.
    pub fn start(memory_dir: &Path) -> Option<Self> {
        let dirty_files: Arc<ArcSwap<HashSet<PathBuf>>> =
            Arc::new(ArcSwap::new(Arc::new(HashSet::new())));
        let dirty = Arc::new(AtomicBool::new(false));

        let df = dirty_files.clone();
        let d = dirty.clone();

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            let Ok(event) = res else { return };
            match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {}
                _ => return,
            }
            for path in &event.paths {
                if path.extension().is_some_and(|ext| ext == "md") {
                    let path = path.clone();
                    df.rcu(move |old| {
                        let mut new = (**old).clone();
                        new.insert(path.clone());
                        new
                    });
                    d.store(true, Ordering::Relaxed);
                }
            }
        })
        .map_err(|e| {
            tracing::warn!(error = %e, "failed to create memory file watcher");
        })
        .ok()?;

        watcher
            .watch(memory_dir, RecursiveMode::Recursive)
            .map_err(|e| {
                tracing::warn!(
                    path = %memory_dir.display(),
                    error = %e,
                    "failed to watch memory directory"
                );
            })
            .ok()?;

        tracing::info!(
            path = %memory_dir.display(),
            "memory file watcher started"
        );

        Some(Self {
            dirty_files,
            dirty,
            _watcher: watcher,
        })
    }

    /// True if any files have changed since the last `take_dirty`.
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// Takes all accumulated dirty paths and resets the dirty state.
    pub fn take_dirty(&self) -> Vec<PathBuf> {
        let old = self.dirty_files.swap(Arc::new(HashSet::new()));
        self.dirty.store(false, Ordering::Relaxed);
        old.iter().cloned().collect()
    }
}

/// 笔记一变就叫一声：图谱页靠它实时刷新（agent 写了新观察，图上就多一个点）。
///
/// 和 [`MemoryFileWatcher`] 分开：那个攒的是「哪些路径要重建索引」，被检索取走就清空；
/// 这个只管通知，谁也不会把它的事件拿走。回调跑在监听线程上，要快，别在里面读盘。
pub struct MemoryChangeWatcher {
    _watcher: RecommendedWatcher,
}

impl MemoryChangeWatcher {
    /// 开始监听 `memory_dir`。目录不存在或监听建不起来回 `None`（记日志，不报错）。
    pub fn start(memory_dir: &Path, on_change: impl Fn() + Send + 'static) -> Option<Self> {
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            let Ok(event) = res else { return };
            match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {}
                _ => return,
            }
            if event.paths.iter().any(|path| is_note_change(path)) {
                on_change();
            }
        })
        .map_err(|e| {
            tracing::warn!(error = %e, "failed to create memory change watcher");
        })
        .ok()?;
        watcher
            .watch(memory_dir, RecursiveMode::Recursive)
            .map_err(|e| {
                tracing::debug!(
                    path = %memory_dir.display(),
                    error = %e,
                    "memory directory is not watchable yet"
                );
            })
            .ok()?;
        Some(Self { _watcher: watcher })
    }
}

/// 这个路径变了算不算「笔记变了」。
///
/// 生成的 `MEMORY.md` 不算：记忆开着时每一步都会重写它（`ensure_layout`），算进来的话
/// 界面会跟着每一步重拉一次图。它的内容全由笔记推出来，笔记没变它也不会真的变。
/// 原子写入用的临时文件（`.` 开头）也不算。
pub fn is_note_change(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    path.extension().is_some_and(|ext| ext == "md") && name != "MEMORY.md" && !name.starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn note_changes_exclude_the_generated_index_and_temp_files() {
        for (path, expected) in [
            ("/m/global/topics/prefs.md", true),
            ("/m/workspace-a/observations/_inbox/flush-1.md", true),
            ("/m/global/archive/gone.md", true),
            ("/m/global/MEMORY.md", false),
            ("/m/global/topics/.note-abc.md", false),
            ("/m/global/topics/.note-abc.tmp", false),
            ("/m/global/memory_state.sqlite", false),
            ("/m/search.sqlite-wal", false),
            ("/m/global/topics", false),
        ] {
            assert_eq!(is_note_change(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn change_watcher_fires_for_notes_but_not_for_the_index() {
        use std::sync::atomic::AtomicUsize;
        use std::time::{Duration, Instant};

        let tmp = TempDir::new().unwrap();
        let topics = tmp.path().join("global/topics");
        std::fs::create_dir_all(&topics).unwrap();
        let fired = Arc::new(AtomicUsize::new(0));
        let counter = fired.clone();
        let Some(_watcher) = MemoryChangeWatcher::start(tmp.path(), move || {
            counter.fetch_add(1, Ordering::Relaxed);
        }) else {
            eprintln!("skipping: could not create file watcher (resource limit?)");
            return;
        };
        // 监听刚建好时，系统可能还在补发建目录的事件：等它过去。
        std::thread::sleep(Duration::from_millis(300));

        std::fs::write(tmp.path().join("global/MEMORY.md"), "# index\n").unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(fired.load(Ordering::Relaxed), 0, "重写 MEMORY.md 不该通知");

        std::fs::write(topics.join("prefs.md"), "# prefs\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while fired.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(fired.load(Ordering::Relaxed) > 0, "写了一篇笔记却没有通知");
        // 记忆目录还没建：起不来，回 None。
        assert!(MemoryChangeWatcher::start(&tmp.path().join("missing"), || {}).is_none());
    }

    #[test]
    fn test_watcher_initially_clean() {
        let tmp = TempDir::new().unwrap();
        let Some(watcher) = MemoryFileWatcher::start(tmp.path()) else {
            eprintln!("skipping: could not create file watcher (resource limit?)");
            return;
        };
        assert!(!watcher.is_dirty());
        assert!(watcher.take_dirty().is_empty());
    }
}
