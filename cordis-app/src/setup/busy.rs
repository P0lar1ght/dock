//! 升级前「进行中」探测：其它 dock 进程，或 `$DOCK_HOME` 下的锁文件。

use std::fs;
use std::path::{Path, PathBuf};

use cordis_base::config::dock_home;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BusyReason {
    OtherDockProcess { pid: u32, detail: String },
    HeldLock { path: PathBuf },
}

impl BusyReason {
    pub fn message(&self) -> String {
        match self {
            Self::OtherDockProcess { pid, detail } => {
                format!("检测到其它 dock 进程（pid {pid}：{detail}）")
            }
            Self::HeldLock { path } => {
                format!("锁正被持有，有进行中的任务：{}", path.display())
            }
        }
    }
}

/// 收集所有忙原因（进程 + 被持有的锁）。空 = 可以安静升级。
pub fn busy_reasons() -> Vec<BusyReason> {
    let mut out = Vec::new();
    out.extend(other_dock_processes());
    out.extend(held_locks_under_home(&dock_home()));
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

fn held_locks_under_home(home: &Path) -> Vec<BusyReason> {
    if !home.exists() {
        return Vec::new();
    }
    // 已知产品锁：schedules；sessions 下若出现 *.lock 也算。
    let mut candidates = vec![home.join("schedules.json.lock")];
    let sessions = home.join("sessions");
    if sessions.is_dir() {
        candidates.extend(collect_lock_files(&sessions, 4).unwrap_or_default());
    }
    candidates
        .into_iter()
        .filter(|path| lock_is_held(path))
        .map(|path| BusyReason::HeldLock { path })
        .collect()
}

/// 锁文件用完不删（cron 的 `schedules.json.lock` 一直留着），所以「文件在」不等于
/// 「有人在用」：试拿一次非阻塞排他锁，拿不到才算被持有。
fn lock_is_held(path: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock))
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

    fn held_locks() -> Vec<BusyReason> {
        busy_reasons()
            .into_iter()
            .filter(|r| matches!(r, BusyReason::HeldLock { .. }))
            .collect()
    }

    #[test]
    fn held_schedule_lock_is_busy() {
        let _env = test_env::scoped().home();
        let home = dock_home();
        fs::create_dir_all(&home).unwrap();
        let path = home.join("schedules.json.lock");
        fs::write(&path, "").unwrap();
        let holder = fs::File::open(&path).unwrap();
        holder.lock().unwrap();
        let reasons = held_locks();
        assert_eq!(reasons, vec![BusyReason::HeldLock { path }]);
    }

    #[test]
    fn leftover_unheld_lock_file_is_not_busy() {
        // cron 用完从不删锁文件：只要用过一次定时任务，它就一直在。
        let _env = test_env::scoped().home();
        let home = dock_home();
        fs::create_dir_all(home.join("sessions/k/s")).unwrap();
        fs::write(home.join("schedules.json.lock"), "").unwrap();
        fs::write(home.join("sessions/k/s/turn.lock"), "").unwrap();
        let reasons = held_locks();
        assert!(reasons.is_empty(), "{reasons:?}");
    }

    #[test]
    fn empty_home_not_busy_for_locks() {
        let _env = test_env::scoped().home();
        let locks = held_locks();
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
