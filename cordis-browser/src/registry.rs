//! 会话 → 标签页的运行时名册：`$DOCK_HOME/browser/sessions/<pid>.json`。
//!
//! MCP 服务每次某个会话的标签页变了（开、关、切）就整份重写自己那一份（先写临时文件
//! 再改名，读方不会读到半截）；都关了（或退出）就删掉。两个读方：
//! - 网关按它把「某个会话正在用的标签页」对到 CDP target 上，好推画面（见 [`crate::view`]）。
//! - 要离开 Chromium 的 hub 按它看别的 Dock 进程还有没有页开在上面（[`others_in`]），
//!   有就不关浏览器。
//!
//! 一个 hub 一份（产品里一个进程只有一个 hub，就是 `<pid>.json`，见 [`next_owner`]）：
//! TUI 和 GUI 各起一个 Dock、共用一个 Chromium 时互不覆盖。进程被杀留下的旧文件不影响
//! 正确性——读方最后还要去 Chromium 里找这个 target，找不到就当没有；[`others_in`] 还
//! 跳过进程已经不在的。这是运行时状态，不是会话数据，不进会话目录。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// 新起一个 hub 的名册文件名（不带 `.json`）。进程里第一个就是 `<pid>`，之后的加序号
/// （测试在一个进程里起好几个 hub 当几个 Dock 进程用，各写各的）。
pub fn next_owner() -> String {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let pid = std::process::id();
    match SEQ.fetch_add(1, Ordering::Relaxed) {
        0 => pid.to_string(),
        n => format!("{pid}-{n}"),
    }
}

fn own_file(dir: &Path, owner: &str) -> PathBuf {
    dir.join(format!("{owner}.json"))
}

/// 整份写下 `owner` 这个 hub 的名册（空了就删文件）。写失败不影响工具调用，只是网关
/// 看不到画面。
pub fn publish(owner: &str, entries: &BTreeMap<String, SessionTabs>) {
    publish_in(&registry_dir(), owner, entries);
}

pub fn publish_in(dir: &Path, owner: &str, entries: &BTreeMap<String, SessionTabs>) {
    let file = own_file(dir, owner);
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
    let tmp = dir.join(format!(".{owner}.json.tmp"));
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &file);
    }
}

/// 别的 hub 名下的全部标签页 target id：跳过 `owner` 自己那份、进程已经不在的
/// （`kill -9` 留下的旧文件）和读不了的。
pub fn others_in(dir: &Path, owner: &str) -> HashSet<String> {
    let own = format!("{owner}.json");
    let Ok(read) = std::fs::read_dir(dir) else {
        return HashSet::new();
    };
    let mut out = HashSet::new();
    for path in read.filter_map(Result::ok).map(|e| e.path()) {
        if path.extension().is_none_or(|x| x != "json")
            || path.file_name().is_some_and(|n| *n == *own)
        {
            continue;
        }
        let Some(v) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        else {
            continue;
        };
        let alive = v
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok())
            .is_some_and(pid_alive);
        if !alive {
            continue;
        }
        let sessions = v.get("sessions").and_then(Value::as_object);
        for entry in sessions.into_iter().flat_map(|m| m.values()) {
            let targets = entry.get("targets").and_then(Value::as_array);
            out.extend(
                targets
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.as_str().map(String::from)),
            );
        }
    }
    out
}

/// 进程还在吗（僵尸也算在）。
#[cfg(unix)]
pub(crate) fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 只做存在 / 权限检查，不发信号。
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// 非 unix 上查不了：当它还活着，不关别人可能还在用的浏览器。
#[cfg(not(unix))]
pub(crate) fn pid_alive(_pid: u32) -> bool {
    true
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
        publish_in(dir.path(), "4242", &entries);
        let got = lookup_in(dir.path(), "t1").unwrap();
        assert_eq!(got.targets, vec!["A", "B"]);
        assert_eq!(got.active.as_deref(), Some("B"));
        assert!(lookup_in(dir.path(), "t2").is_none());
        // 没有残留临时文件。
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["4242.json".to_string()]);

        publish_in(dir.path(), "4242", &BTreeMap::new());
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

    #[test]
    fn hubs_in_one_process_get_their_own_files() {
        let a = next_owner();
        let b = next_owner();
        assert_ne!(a, b);
        let pid = std::process::id().to_string();
        assert!(a.starts_with(&pid) && b.starts_with(&pid), "{a} {b}");
    }

    #[test]
    fn others_skip_own_file_and_dead_processes() {
        let dir = tempfile::tempdir().unwrap();
        let tabs = |ids: &[&str]| SessionTabs {
            targets: ids.iter().map(|s| s.to_string()).collect(),
            active: ids.last().map(|s| s.to_string()),
        };
        // 自己（和别的 hub 同一个进程，pid 活着）。
        publish_in(
            dir.path(),
            "me",
            &BTreeMap::from([("s".to_string(), tabs(&["MINE"]))]),
        );
        publish_in(
            dir.path(),
            "other",
            &BTreeMap::from([
                ("a".to_string(), tabs(&["X", "Y"])),
                ("b".to_string(), tabs(&["Z"])),
            ]),
        );
        // `kill -9` 留下的：进程早没了。
        std::fs::write(
            dir.path().join("99999999.json"),
            r#"{"pid":99999999,"sessions":{"gui":{"targets":["DEAD"],"active":"DEAD"}}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("broken.json"), "{not json").unwrap();
        std::fs::write(dir.path().join("nopid.json"), r#"{"sessions":{}}"#).unwrap();

        let mut got: Vec<String> = others_in(dir.path(), "me").into_iter().collect();
        got.sort();
        assert_eq!(got, vec!["X", "Y", "Z"]);
        assert!(others_in(&dir.path().join("missing"), "me").is_empty());
    }
}
