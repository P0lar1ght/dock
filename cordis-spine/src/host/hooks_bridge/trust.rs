//! 项目里的 hooks 文件第一次跑之前要用户点头：一个克隆来的仓库自带的 `.dock/hooks.json`
//! / `.claude/settings.json` 就是一串会在本机跑的 shell 命令。点过头的按「文件路径 +
//! 内容指纹」记在 `$DOCK_HOME/hooks-trust.json`，文件一改就要重新问。用户目录下的不问。
//!
//! 项目自带的插件（`.dock/plugins/<id>/`）同一个道理，按「插件目录 + 内容指纹」记在
//! `$DOCK_HOME/plugins-trust.json`（见 `dynamic_runner::persist`）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

/// 项目 hooks 文件的信任记录。
pub const HOOKS_FILE: &str = "hooks-trust.json";
/// 项目插件目录的信任记录。
pub const PLUGINS_FILE: &str = "plugins-trust.json";

pub fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `$DOCK_HOME` 下的一份信任记录。
pub fn store_path(file: &str) -> PathBuf {
    cordis_base::config::dock_home().join(file)
}

fn load(store: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(store)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn is_trusted(path: &Path, digest: &str) -> bool {
    is_trusted_at(&store_path(HOOKS_FILE), path, digest)
}

pub fn remember(path: &Path, digest: &str) -> Result<(), String> {
    remember_at(&store_path(HOOKS_FILE), path, digest)
}

/// `path` 现在这份内容（`digest`）在信任记录 `store` 里点过头没有。
pub fn is_trusted_at(store: &Path, path: &Path, digest: &str) -> bool {
    load(store)
        .get(&path.display().to_string())
        .map(String::as_str)
        == Some(digest)
}

pub fn remember_at(store: &Path, path: &Path, digest: &str) -> Result<(), String> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut all = load(store);
    all.insert(path.display().to_string(), digest.to_string());
    let target = store.to_path_buf();
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    // 临时文件按进程 + 序号起名：两个 Dock 进程（或同一进程两处）同时记，不会写进同一个
    // 临时文件里互相截断。
    let tmp = target.with_extension(format!(
        "json.{}-{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}
