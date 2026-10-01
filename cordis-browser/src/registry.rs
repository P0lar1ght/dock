//! 会话 → 标签页的运行时名册：`$DOCK_HOME/browser/sessions/<pid>.json`。
//!
//! MCP 服务每次某个会话的标签页变了（开、关、切）就整份重写自己那一份（先写临时文件
//! 再改名，读方不会读到半截）；都关了（或退出）就删掉。网关按它把「某个会话正在用的标签页」对到
//! CDP target 上，好推画面（见 [`crate::view`]）。
//!
//! 一个进程一份：TUI 和 GUI 各起一个 Dock、共用一个 Chromium 时互不覆盖。进程被杀
//! 留下的旧文件不影响正确性——读方最后还要去 Chromium 里找这个 target，找不到就当没有。
//! 这是运行时状态，不是会话数据，不进会话目录。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use cordis_base::config::dock_home;

/// 一个会话的标签页：CDP target id，与当前活动的那个。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionTabs {
    pub targets: Vec<String>,
    pub active: Option<String>,
}

pub fn registry_dir() -> PathBuf {
    dock_home().join("browser").join("sessions")
}

fn own_file(dir: &Path) -> PathBuf {
    dir.join(format!("{}.json", std::process::id()))
}

/// 整份写下本进程的名册（空了就删文件）。写失败不影响工具调用，只是网关看不到画面。
pub fn publish(entries: &BTreeMap<String, SessionTabs>) {
    publish_in(&registry_dir(), entries);
}

pub fn publish_in(dir: &Path, entries: &BTreeMap<String, SessionTabs>) {
    let file = own_file(dir);
    if entries.is_empty() {
        let _ = std::fs::remove_file(&file);
        return;
    }
    let sessions: serde_json::Map<String, Value> = entries
        .iter()
        .map(|(id, tabs)| {
            (
                id.clone(),
                json!({ "targets": tabs.targets, "active": tabs.active }),
            )
        })
        .collect();
    let body = json!({ "pid": std::process::id(), "sessions": sessions }).to_string();
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = dir.join(format!(".{}.json.tmp", std::process::id()));
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &file);
    }
}

/// 找某个会话的标签页。多份文件都有它时取最新写的那份。
pub fn lookup(session: &str) -> Option<SessionTabs> {
    lookup_in(&registry_dir(), session)
}

pub fn lookup_in(dir: &Path, session: &str) -> Option<SessionTabs> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    files.into_iter().find_map(|(_, path)| {
        let raw = std::fs::read_to_string(path).ok()?;
        let v: Value = serde_json::from_str(&raw).ok()?;
        let entry = v.get("sessions")?.get(session)?;
        let targets = entry
            .get("targets")?
            .as_array()?
            .iter()
            .filter_map(|t| t.as_str().map(String::from))
            .collect();
        let active = entry
            .get("active")
            .and_then(Value::as_str)
            .map(String::from);
        Some(SessionTabs { targets, active })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_lookup_and_empty_removes() {
        let dir = tempfile::tempdir().unwrap();
        let mut entries = BTreeMap::new();
        entries.insert(
            "t1".to_string(),
            SessionTabs {
                targets: vec!["A".into(), "B".into()],
                active: Some("B".into()),
            },
        );
        publish_in(dir.path(), &entries);
        let got = lookup_in(dir.path(), "t1").unwrap();
        assert_eq!(got.targets, vec!["A", "B"]);
        assert_eq!(got.active.as_deref(), Some("B"));
        assert!(lookup_in(dir.path(), "t2").is_none());
        // 没有残留临时文件。
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![format!("{}.json", std::process::id())]);

        publish_in(dir.path(), &BTreeMap::new());
        assert!(lookup_in(dir.path(), "t1").is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn other_process_files_are_read_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("99999999.json"),
            r#"{"pid":99999999,"sessions":{"gui":{"targets":["X"],"active":"X"}}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("broken.json"), "{not json").unwrap();
        let got = lookup_in(dir.path(), "gui").unwrap();
        assert_eq!(got.active.as_deref(), Some("X"));
    }
}
