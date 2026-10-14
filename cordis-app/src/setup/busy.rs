//! 升级前「进行中」探测：其它 dock 进程，或 `$DOCK_HOME` 下的锁文件。

use std::fs;
use std::path::{Path, PathBuf};

use cordis_base::config::dock_home;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BusyReason {
    OtherDockProcess { pid: u32, detail: String },
    LockFile { path: PathBuf },
}

impl BusyReason {
    pub fn message(&self) -> String {
        match self {
            Self::OtherDockProcess { pid, detail } => {
                format!("检测到其它 dock 进程（pid {pid}：{detail}）")
            }
            Self::LockFile { path } => {
                format!("检测到锁文件，可能有进行中的任务：{}", path.display())
            }
        }
    }
}

/// 收集所有忙原因（进程 + 锁）。空 = 可以安静升级。
pub fn busy_reasons() -> Vec<BusyReason> {
    let mut out = Vec::new();
    out.extend(other_dock_processes());
    out.extend(lock_files_under_home(&dock_home()));
    out
}

fn other_dock_processes() -> Vec<BusyReason> {
    let self_pid = std::process::id();
    let mut out = Vec::new();
    // 不引入 sysinfo：用 ps。macOS / Linux 通用列。
    let Ok(output) = std::process::Command::new("ps")
        .args(["-ax", "-o", "pid=,command="])
        .output()
    else {
        return out;
    };
    if !output.status.success() {
        return out;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((pid_s, cmd)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = pid_s.trim().parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let cmd = cmd.trim();
        if looks_like_dock_process(cmd) {
            out.push(BusyReason::OtherDockProcess {
                pid,
                detail: truncate(cmd, 120),
            });
        }
    }
    out
}

fn looks_like_dock_process(cmd: &str) -> bool {
    // 匹配 PATH 上的 `dock`、安装目录里的 dock、以及 cargo 跑起来的 debug/release dock。
    // 避开 `dock-gui`、本脚本注释、grep 自身等误伤：要求命令里有独立的 dock 可执行片段。
    let lower = cmd.to_ascii_lowercase();
    if lower.contains("dock-gui") || lower.contains("dock_gui") {
        return false;
    }
    // `/path/to/dock` 或 `dock serve` 或 `target/debug/dock`
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let Some(exe) = tokens.first() else {
        return false;
    };
    let base = Path::new(exe)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    base == "dock"
}

fn lock_files_under_home(home: &Path) -> Vec<BusyReason> {
    let mut out = Vec::new();
    if !home.exists() {
        return out;
    }
    // 已知产品锁：schedules；sessions 下若出现 *.lock 也算。
    let schedule_lock = home.join("schedules.json.lock");
    if schedule_lock.exists() {
        out.push(BusyReason::LockFile {
            path: schedule_lock,
        });
    }
    let sessions = home.join("sessions");
    if sessions.is_dir() {
        if let Ok(walk) = collect_lock_files(&sessions, 4) {
            for path in walk {
                out.push(BusyReason::LockFile { path });
            }
        }
    }
    out
}

fn collect_lock_files(root: &Path, max_depth: u32) -> std::io::Result<Vec<PathBuf>> {
    fn walk(dir: &Path, depth: u32, max_depth: u32, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        if depth > max_depth {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                walk(&path, depth + 1, max_depth, out)?;
            } else if name.ends_with(".lock") || name == "lock" {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, 0, max_depth, &mut out)?;
    Ok(out)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_base::test_env;

    #[test]
    fn schedule_lock_is_busy() {
        let _env = test_env::scoped().home();
        let home = dock_home();
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("schedules.json.lock"), "x").unwrap();
        let reasons = busy_reasons();
        assert!(
            reasons
                .iter()
                .any(|r| matches!(r, BusyReason::LockFile { .. })),
            "{reasons:?}"
        );
    }

    #[test]
    fn empty_home_not_busy_for_locks() {
        let _env = test_env::scoped().home();
        let locks: Vec<_> = busy_reasons()
            .into_iter()
            .filter(|r| matches!(r, BusyReason::LockFile { .. }))
            .collect();
        assert!(locks.is_empty(), "{locks:?}");
    }

    #[test]
    fn looks_like_dock_filters_gui() {
        assert!(looks_like_dock_process(
            "/Users/x/.local/bin/dock serve --remote"
        ));
        assert!(looks_like_dock_process("target/debug/dock"));
        assert!(!looks_like_dock_process("/Apps/dock-gui"));
        assert!(!looks_like_dock_process("grep dock"));
    }
}
