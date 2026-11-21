//! 条件命令：在任务的工作目录跑一次（`bash -lc`，和后台命令同一种跑法），交回退出码
//! 和输出。判断「满足没有」、写给模型的说明也在这里。

use std::path::Path;
use std::process::Stdio;
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

/// 在 `cwd` 跑一次 `command`，最多等 `timeout`。超时就停掉（`kill_on_drop`）。
pub async fn run_check(command: &str, cwd: &Path, timeout: Duration) -> WatchObservation {
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-lc")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if cwd.is_dir() {
        cmd.current_dir(cwd);
    } else {
        return WatchObservation::failed(format!("工作目录 {} 不在了", cwd.display()));
    }
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return WatchObservation::failed(format!("命令没跑起来：{e}")),
    };
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => WatchObservation::failed(format!("超过 {} 秒没跑完，已停掉", timeout.as_secs())),
        Ok(Err(e)) => WatchObservation::failed(format!("命令没跑完：{e}")),
        Ok(Ok(out)) => {
            let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
            if !out.stderr.is_empty() {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            if output.len() > OUTPUT_CAP {
                let mut cut = output.len() - OUTPUT_CAP;
                while !output.is_char_boundary(cut) {
                    cut += 1;
                }
                output.drain(..cut);
            }
            WatchObservation {
                exit: out.status.code(),
                output,
                error: None,
            }
        }
    }
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
