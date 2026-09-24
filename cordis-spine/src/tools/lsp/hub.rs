//! 按项目根分的 LSP：named service `"lsp"`。
//!
//! 一个进程里的会话可以在不同项目，language server 要按项目起（rust-analyzer
//! 只认一个 workspace root）。每个根一份 [`LspBackendAdapter`]（各自的
//! `LspManager` 与启动状态），第一次用到时才建。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::Mutex as TokioMutex;

use super::dispatch::LspBackendAdapter;
use super::manager::LspManager;
use super::notify::ToolNotificationHandle;

#[derive(Default)]
pub struct LspHub {
    by_root: Mutex<HashMap<PathBuf, Arc<LspBackendAdapter>>>,
}

impl LspHub {
    /// `root` 这个项目的 LSP；没有就新建（servers 在首次使用时才加载、启动）。
    pub fn for_root(&self, root: &Path) -> Arc<LspBackendAdapter> {
        let key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        self.by_root
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(LspBackendAdapter::new(Arc::new(TokioMutex::new(
                    LspManager::new(
                        Default::default(),
                        key,
                        true,
                        ToolNotificationHandle::noop(),
                    ),
                ))))
            })
            .clone()
    }

    /// 正在跑的会话所在项目的 LSP（[`crate::session::cwd::current_cwd`]）。
    pub fn current(&self) -> Arc<LspBackendAdapter> {
        self.for_root(&crate::session::cwd::current_cwd())
    }

    /// 已经建过的项目根，给 `/cordis inspect` 之类的诊断用。
    pub fn roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<_> = self.by_root.lock().unwrap().keys().cloned().collect();
        roots.sort();
        roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::lsp::LspBackend;

    /// 回归：两个项目各一份 LSP，同一个项目复用同一份——以前只有一个
    /// manager，工作根跟着进程 cwd 走。
    #[tokio::test]
    async fn one_adapter_per_project_root() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let hub = LspHub::default();
        let first = hub.for_root(a.path());
        assert!(Arc::ptr_eq(&first, &hub.for_root(a.path())), "同根复用");
        assert!(!Arc::ptr_eq(&first, &hub.for_root(b.path())), "异根各一份");
        assert_eq!(hub.roots().len(), 2);
        // 启动一次（空目录没有 server，会失败）之后，根仍是这个项目——以前
        // bootstrap 会把它改写成进程 cwd。
        let _ = LspBackend::ensure_ready(&*first).await;
        assert_eq!(
            first.workspace_root().await,
            a.path().canonicalize().unwrap()
        );
    }
}
