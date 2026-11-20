//! 项目里的 hooks 文件第一次跑之前要用户点头：一个克隆来的仓库自带的 `.dock/hooks.json`
//! / `.claude/settings.json` 就是一串会在本机跑的 shell 命令。点过头的按「文件路径 +
//! 内容指纹」记在 `$DOCK_HOME/hooks-trust.json`，文件一改就要重新问。用户目录下的不问。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn store_path() -> PathBuf {
    cordis_base::config::dock_home().join("hooks-trust.json")
}

fn load() -> BTreeMap<String, String> {
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn is_trusted(path: &Path, digest: &str) -> bool {
    load().get(&path.display().to_string()).map(String::as_str) == Some(digest)
}

pub fn remember(path: &Path, digest: &str) -> Result<(), String> {
    let mut all = load();
    all.insert(path.display().to_string(), digest.to_string());
    let target = store_path();
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    let tmp = target.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &target).map_err(|e| e.to_string())
}
