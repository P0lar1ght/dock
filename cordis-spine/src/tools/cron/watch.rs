//! 条件命令：在任务的工作目录跑一次（`bash -lc`，和后台命令同一种跑法），交回退出码
//! 和输出。判断「满足没有」、写给模型的说明也在这里。

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::WatchWhen;

/// 一次检查最多跑这么久，超了就停掉、记成没查成（不触发）。
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(120);
/// 留下的输出上限（判断「变了没」和匹配正则都看这一段）。
const OUTPUT_CAP: usize = 64 * 1024;
/// 送给模型的输出结尾。
const CONTEXT_TAIL: usize = 2000;

/// 一次检查的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchObservation {
    /// 退出码；被信号杀掉是 `None`。
    pub exit: Option<i32>,
    /// stdout，后面接 stderr。
    pub output: String,
    /// 没跑起来 / 超时：这次不算数，基线不动、不触发。
    pub error: Option<String>,
}

impl WatchObservation {
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            error: Some(error.into()),
            ..Self::default()
        }
    }

    /// 退出码 + 输出的摘要。
    pub(super) fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!("{:?}\0", self.exit));
        hasher.update(self.output.as_bytes());
        hasher.finalize()[..12]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// 这次的结果满不满足条件（`Changed` 要和上一次比，不在这里）。
    pub(super) fn satisfies(&self, when: &WatchWhen) -> bool {
        match when {
            WatchWhen::Changed => false,
            WatchWhen::Succeeds => self.exit == Some(0),
            WatchWhen::Fails => self.exit != Some(0),
            WatchWhen::Matches { pattern } => regex::Regex::new(pattern)
                .map(|re| re.is_match(&self.output))
                .unwrap_or(false),
        }
    }

    /// 触发时写给模型的说明（英文，进 system-reminder）：为什么触发、退出码、输出结尾。
    pub(super) fn context(&self, command: &str, when: &WatchWhen) -> String {
        let why = match when {
            WatchWhen::Changed => "produced different output than at the last check".to_string(),
            WatchWhen::Succeeds => "now succeeds".to_string(),
            WatchWhen::Fails => "now fails".to_string(),
            WatchWhen::Matches { pattern } => format!("output now matches /{pattern}/"),
        };
        let exit = match self.exit {
            Some(code) => format!("exit code {code}"),
            None => "killed by a signal".to_string(),
        };
        let total = self.output.chars().count();
        let tail: String = if total > CONTEXT_TAIL {
            self.output.chars().skip(total - CONTEXT_TAIL).collect()
        } else {
            self.output.clone()
        };
        let label = if total > CONTEXT_TAIL {
            format!("Output (last {CONTEXT_TAIL} of {total} characters)")
        } else {
            "Output".to_string()
        };
        let body = if tail.trim().is_empty() {
            "(no output)".to_string()
        } else {
            format!("```text\n{}\n```", tail.trim_end())
        };
        format!("This run was triggered because the watch command `{command}` {why} ({exit}).\n{label}:\n{body}")
    }
}

/// 在 `cwd` 跑一次条件命令（`bash -lc`）。命令自成一个进程组：跑完后它留下的后台进程、
/// 超时 / 被取消时整棵进程树都一起停掉——检查每分钟跑一次，不能每次留一个孤儿。
/// stdout / stderr 边跑边读，只留结尾 [`OUTPUT_CAP`] 字节。
pub async fn run_check(command: &str, cwd: &Path, timeout: Duration) -> WatchObservation {
    if !cwd.is_dir() {
        return WatchObservation::failed(format!("工作目录 {} 不在了", cwd.display()));
    }
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return WatchObservation::failed(format!("命令没跑起来：{e}")),
    };
    // 先于 `child` 析构（局部变量倒序析构）：超时 / 取消时组长还没被收，组号不会被复用。
    let group = Group(AtomicU32::new(child.id().unwrap_or(0)));
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let run = async {
        let wait = async {
            let status = child.wait().await;
            // bash 退出了：组里剩下的（后台进程）一起停掉，它们占着的管道才会关。
            group.kill();
            status
        };
        tokio::join!(wait, read_tail(stdout), read_tail(stderr))
    };
    let (status, out, err) = match tokio::time::timeout(timeout, run).await {
        Err(_) => {
            return WatchObservation::failed(format!("超过 {} 秒没跑完，已停掉", timeout.as_secs()))
        }
        Ok((Err(e), _, _)) => return WatchObservation::failed(format!("命令没跑完：{e}")),
        Ok((Ok(status), out, err)) => (status, out, err),
    };
    let mut output = String::from_utf8_lossy(&out).into_owned();
    if !err.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&String::from_utf8_lossy(&err));
    }
    if output.len() > OUTPUT_CAP {
        let mut cut = output.len() - OUTPUT_CAP;
        while !output.is_char_boundary(cut) {
            cut += 1;
        }
        output.drain(..cut);
    }
    WatchObservation {
        exit: status.code(),
        output,
        error: None,
    }
}

/// 条件命令的进程组（组号 = bash 的 pid）。析构时整组 SIGKILL：超时、驱动停下
/// 丢掉这次检查时，不只 bash（`kill_on_drop`），它起的子进程也一起走。只杀一次，
/// 杀过就不再碰这个组号（免得组号被别的进程复用后误杀）。
/// 存的是组号，0 = 没有 / 已经杀过。
struct Group(AtomicU32);

impl Group {
    fn kill(&self) {
        let pid = self.0.swap(0, Ordering::SeqCst);
        #[cfg(unix)]
        if pid != 0 {
            // 组里没人了是 ESRCH，无害。
            unsafe {
                libc::killpg(pid as i32, libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        let _ = pid;
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
    }
}

/// 读到 EOF，只留最后 [`OUTPUT_CAP`] 字节（攒到两倍再裁，内存有上限）。
async fn read_tail(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut kept = Vec::new();
    let Some(mut pipe) = pipe else {
        return kept;
    };
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                kept.extend_from_slice(&buf[..n]);
                if kept.len() > 2 * OUTPUT_CAP {
                    kept.drain(..kept.len() - OUTPUT_CAP);
                }
            }
        }
    }
    if kept.len() > OUTPUT_CAP {
        kept.drain(..kept.len() - OUTPUT_CAP);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_in_the_task_directory_and_reports_exit_and_output() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("status.txt"), "red\n").unwrap();
        let ok = run_check("cat status.txt; echo warn >&2", dir.path(), CHECK_TIMEOUT).await;
        assert_eq!(ok.exit, Some(0), "{ok:?}");
        assert_eq!(ok.output, "red\nwarn\n");
        assert!(ok.error.is_none());
        let failed = run_check("exit 3", dir.path(), CHECK_TIMEOUT).await;
        assert_eq!(failed.exit, Some(3));
        assert!(failed.satisfies(&WatchWhen::Fails));
        assert!(!failed.satisfies(&WatchWhen::Succeeds));
        assert!(ok.satisfies(&WatchWhen::Matches {
            pattern: "(?m)^red$".into()
        }));
        assert!(!ok.satisfies(&WatchWhen::Matches {
            pattern: "green".into()
        }));
        assert_ne!(ok.digest(), failed.digest());
    }

    #[tokio::test]
    async fn a_hung_command_is_stopped_and_not_counted() {
        let dir = tempfile::tempdir().unwrap();
        let hung = run_check("sleep 30", dir.path(), Duration::from_millis(300)).await;
        assert!(
            hung.error.as_deref().is_some_and(|e| e.contains("已停掉")),
            "{hung:?}"
        );
        let gone = run_check("true", &dir.path().join("nope"), CHECK_TIMEOUT).await;
        assert!(gone.error.is_some(), "{gone:?}");
    }

    /// 等 `pid` 退出（被收掉）；最多等两秒。
    #[cfg(unix)]
    async fn gone(pid: i32) -> bool {
        for _ in 0..100 {
            if unsafe { libc::kill(pid, 0) } != 0 {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    fn child_pid(dir: &Path) -> i32 {
        std::fs::read_to_string(dir.join("child.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    /// 超时停掉的是整个进程组：命令起的后台进程、管道里的子进程一起走，不留孤儿
    /// （以前只杀 bash，`sleep` 留下来；#310 审阅）。
    #[cfg(unix)]
    #[tokio::test]
    async fn a_timeout_stops_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let hung = run_check(
            "sleep 30 & echo $! > child.pid; wait",
            dir.path(),
            Duration::from_millis(500),
        )
        .await;
        assert!(hung.error.is_some(), "{hung:?}");
        assert!(gone(child_pid(dir.path())).await, "后台的 sleep 还活着");
    }

    /// 命令跑完了、留了个后台进程占着输出管道：马上交回结果（不等到超时、不算没查成），
    /// 留下的进程停掉——每分钟查一次，不能每次攒一个。
    #[cfg(unix)]
    #[tokio::test]
    async fn leftovers_of_a_finished_check_are_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let done = run_check(
            "sleep 30 & echo $! > child.pid; echo done",
            dir.path(),
            Duration::from_secs(5),
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(3), "{done:?}");
        assert_eq!(done.exit, Some(0), "{done:?}");
        assert_eq!(done.output, "done\n");
        assert!(gone(child_pid(dir.path())).await, "后台的 sleep 还活着");
    }

    /// 输出很多时只留结尾（判断和匹配都看结尾），内存里也不攒整份。
    #[tokio::test]
    async fn only_the_tail_of_a_long_output_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let long = run_check(
            "head -c 300000 /dev/zero | tr '\\0' x; echo; echo END",
            dir.path(),
            CHECK_TIMEOUT,
        )
        .await;
        assert_eq!(long.exit, Some(0), "{:?}", long.error);
        assert!(long.output.len() <= OUTPUT_CAP, "{}", long.output.len());
        assert!(long.output.ends_with("x\nEND\n"));
    }

    #[test]
    fn context_tells_the_model_why_and_shows_the_tail() {
        let obs = WatchObservation {
            exit: Some(1),
            output: format!("{}\nFAILED test_x", "x".repeat(3000)),
            error: None,
        };
        let text = obs.context("cargo test", &WatchWhen::Fails);
        assert!(
            text.contains("`cargo test` now fails (exit code 1)"),
            "{text}"
        );
        assert!(text.contains("FAILED test_x"));
        assert!(text.contains("last 2000 of"), "{text}");
    }
}
