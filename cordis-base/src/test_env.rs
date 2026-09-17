//! 测试用的进程级状态（`DOCK_HOME`、当前目录、其它 env）互斥。
//!
//! `std::env::set_var` 与 `std::env::set_current_dir` 是进程级全局：多个测试
//! 模块各持一把自己的锁时仍会互相覆盖，表现为随机失败。这里用**同一把**锁
//! 串行化所有使用者，用例本身继续并行。
//!
//! 用法：
//!
//! ```ignore
//! let _env = crate::test_env::scoped().home();
//! let _env = crate::test_env::scoped().cwd(dir.path());
//! let _env = crate::test_env::scoped().home().set("DOCK_BROWSER_HEADED", "1");
//! ```
//!
//! guard drop 时恢复原值并释放锁。一次 `scoped()` 只持一把锁，所以链式设置多
//! 个变量不会自我死锁。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static PROCESS_ENV_LOCK: Mutex<()> = Mutex::new(());

/// 占用进程级状态的互斥并记录要恢复的项，drop 时统一还原。
pub struct EnvScope {
    _lock: MutexGuard<'static, ()>,
    restore: Vec<(&'static str, Option<OsString>)>,
    prev_cwd: Option<PathBuf>,
    _dir: Option<tempfile::TempDir>,
}

impl EnvScope {
    /// 把 `DOCK_HOME` 指向新的临时目录（临时目录随 guard 一起释放）。
    pub fn home(mut self) -> Self {
        let dir = tempfile::tempdir().unwrap();
        self = self.set("DOCK_HOME", dir.path());
        self._dir = Some(dir);
        self
    }

    /// 记录原值（同一个 key 只记一次，多次设置以最初的原值为准）。
    fn record(&mut self, key: &'static str) {
        if !self.restore.iter().any(|(k, _)| *k == key) {
            self.restore.push((key, std::env::var_os(key)));
        }
    }

    /// 设置一个环境变量。
    pub fn set(mut self, key: &'static str, value: impl AsRef<OsStr>) -> Self {
        self.record(key);
        std::env::set_var(key, value);
        self
    }

    /// 删除一个环境变量。
    pub fn remove(mut self, key: &'static str) -> Self {
        self.record(key);
        std::env::remove_var(key);
        self
    }

    /// 切换当前目录。
    pub fn cwd(mut self, dir: &Path) -> Self {
        if self.prev_cwd.is_none() {
            self.prev_cwd = std::env::current_dir().ok();
        }
        std::env::set_current_dir(dir).unwrap();
        self
    }
}

impl Drop for EnvScope {
    fn drop(&mut self) {
        if let Some(prev) = self.prev_cwd.take() {
            let _ = std::env::set_current_dir(prev);
        }
        for (key, value) in self.restore.drain(..) {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

/// 开始一个作用域，drop 前独占进程级 env / cwd。
pub fn scoped() -> EnvScope {
    EnvScope {
        _lock: PROCESS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner()),
        restore: Vec::new(),
        prev_cwd: None,
        _dir: None,
    }
}
