//! 跑一条 hook 命令、读懂它的回答、几条回答取最严。
//!
//! 和 Claude Code / Codex 一样：
//! - 退出码 0：stdout 能解析成 JSON 就按结构读（`decision`、`continue`、
//!   `hookSpecificOutput.permissionDecision` / `additionalContext`）；不是 JSON 时，
//!   `UserPromptSubmit` / `SessionStart` 的 stdout 当作附加上下文，别的事件忽略。
//! - 退出码 2：拦下，stderr 是理由。
//! - 其它退出码、超时、起不来：不拦，只记一句（hook 坏了不该让 agent 停摆）。
//! - 多条回答合并：deny > ask > allow，上下文按顺序攒。

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// stdout / stderr 各收这么多，再多的丢掉。
const MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Debug, Default)]
pub struct HookRun {
    pub exit: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub spawn_error: Option<String>,
}

/// 在 `cwd` 里用 `sh -c` 跑 `command`，stdin 喂 `input`。命令在自己的进程组里，超时
/// 连同它起的子进程一起杀掉。
pub fn run_command(
    command: &str,
    input: &Value,
    cwd: &Path,
    project: &Path,
    timeout: Duration,
) -> HookRun {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .env("CLAUDE_PROJECT_DIR", project)
        .env("DOCK_PROJECT_DIR", project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // 自成一个会话（进程组），超时时 killpg 能把它起的子进程一起带走。
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return HookRun {
                spawn_error: Some(e.to_string()),
                ..HookRun::default()
            }
        }
    };
    let pid = child.id();
    let stdin_text = input.to_string();
    if let Some(mut stdin) = child.stdin.take() {
        std::thread::spawn(move || {
            let _ = stdin.write_all(stdin_text.as_bytes());
        });
    }
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.by_ref().take(MAX_OUTPUT as u64).read_to_end(&mut buf);
                // 剩下的读掉丢弃，别让子进程卡在写满的管道上。
                let _ = std::io::copy(&mut p, &mut std::io::sink());
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = read(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = read(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= timeout => {
                timed_out = true;
                kill_group(pid);
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break None,
        }
    };
    HookRun {
        exit: if timed_out {
            None
        } else {
            status.and_then(|s| s.code())
        },
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
        timed_out,
        spawn_error: None,
    }
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    unsafe {
        libc::killpg(pid as i32, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Decision {
    #[default]
    None,
    Allow,
    Ask(String),
    Deny(String),
}

impl Decision {
    fn rank(&self) -> u8 {
        match self {
            Self::None => 0,
            Self::Allow => 1,
            Self::Ask(_) => 2,
            Self::Deny(_) => 3,
        }
    }
}

/// 一个事件上所有匹配的 hook 合起来的结论。
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// 只有 `PreToolUse` 用：放行 / 要确认 / 拒绝。
    pub decision: Decision,
    /// 拦下（退出码 2，或 `"decision": "block"`）的理由。
    pub block: Option<String>,
    /// 要给模型看的附加上下文，按 hook 顺序。
    pub context: Vec<String>,
    /// `"continue": false` 的理由（记一句，Dock 不据此停掉整个会话）。
    pub stop: Option<String>,
    /// 跑坏了的 hook（不拦，只报）。
    pub errors: Vec<String>,
}

impl Outcome {
    pub fn merge(&mut self, other: Outcome) {
        if other.decision.rank() > self.decision.rank() {
            self.decision = other.decision;
        }
        if self.block.is_none() {
            self.block = other.block;
        }
        self.context.extend(other.context);
        if self.stop.is_none() {
            self.stop = other.stop;
        }
        self.errors.extend(other.errors);
    }
}

/// 读一条 hook 的回答。`command` 只用来写报错。
pub fn decode(event: &str, command: &str, run: &HookRun) -> Outcome {
    let mut out = Outcome::default();
    if let Some(e) = &run.spawn_error {
        out.errors.push(format!("{command}: 起不来：{e}"));
        return out;
    }
    if run.timed_out {
        out.errors.push(format!("{command}: 超时，已停掉"));
        return out;
    }
    match run.exit {
        Some(2) => {
            let reason = run.stderr.trim();
            out.block = Some(if reason.is_empty() {
                format!("blocked by {event} hook")
            } else {
                reason.to_string()
            });
        }
        Some(0) => {
            let stdout = run.stdout.trim();
            match serde_json::from_str::<Value>(stdout) {
                Ok(Value::Object(json)) => read_json(event, &json, &mut out),
                _ if !stdout.is_empty() && matches!(event, "UserPromptSubmit" | "SessionStart") => {
                    out.context.push(stdout.to_string());
                }
                _ => {}
            }
        }
        code => {
            let reason = run.stderr.trim();
            out.errors.push(format!(
                "{command}: 退出码 {}{}",
                code.map_or("?".into(), |c| c.to_string()),
                if reason.is_empty() {
                    String::new()
                } else {
                    format!("：{reason}")
                }
            ));
        }
    }
    out
}

fn read_json(event: &str, json: &serde_json::Map<String, Value>, out: &mut Outcome) {
    let text = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let reason = text(json.get("reason"));
    match json.get("decision").and_then(Value::as_str) {
        Some("block") => {
            out.block = Some(
                reason
                    .clone()
                    .unwrap_or_else(|| format!("blocked by {event} hook")),
            );
        }
        Some("approve") => out.decision = Decision::Allow,
        _ => {}
    }
    if json.get("continue").and_then(Value::as_bool) == Some(false) {
        out.stop = Some(text(json.get("stopReason")).unwrap_or_default());
    }
    if let Some(specific) = json.get("hookSpecificOutput").and_then(Value::as_object) {
        let why = text(specific.get("permissionDecisionReason"))
            .or_else(|| reason.clone())
            .unwrap_or_else(|| format!("{event} hook"));
        match specific.get("permissionDecision").and_then(Value::as_str) {
            Some("deny") => out.decision = Decision::Deny(why),
            Some("ask") => out.decision = Decision::Ask(why),
            Some("allow") => out.decision = Decision::Allow,
            _ => {}
        }
        if let Some(ctx) = text(specific.get("additionalContext")) {
            out.context.push(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cmd: &str) -> HookRun {
        let dir = std::env::temp_dir();
        run_command(
            cmd,
            &serde_json::json!({ "hook_event_name": "PreToolUse", "tool_name": "Bash" }),
            &dir,
            &dir,
            Duration::from_secs(5),
        )
    }

    #[test]
    fn exit_two_blocks_with_stderr_and_reads_stdin() {
        let r = run(r#"input=$(cat); case "$input" in *Bash*) echo "no shell" >&2; exit 2;; esac"#);
        let o = decode("PreToolUse", "x", &r);
        assert_eq!(o.block.as_deref(), Some("no shell"));
    }

    #[test]
    fn json_answers_are_read_and_merged_strictest_first() {
        let ask = decode(
            "PreToolUse",
            "x",
            &run(
                r#"echo '{"hookSpecificOutput":{"permissionDecision":"ask","permissionDecisionReason":"risky"}}'"#,
            ),
        );
        assert_eq!(ask.decision, Decision::Ask("risky".into()));
        let mut merged = decode(
            "PreToolUse",
            "x",
            &run(r#"echo '{"hookSpecificOutput":{"permissionDecision":"allow"}}'"#),
        );
        merged.merge(ask);
        merged.merge(decode("PreToolUse", "x", &run("exit 0")));
        assert_eq!(
            merged.decision,
            Decision::Ask("risky".into()),
            "ask 比 allow 严"
        );
        let ctx = decode("UserPromptSubmit", "x", &run("echo 'today is friday'"));
        assert_eq!(ctx.context, vec!["today is friday"]);
        let plain = decode("PreToolUse", "x", &run("echo 'just chatter'"));
        assert!(
            plain.context.is_empty(),
            "别的事件的纯文本 stdout 不当上下文"
        );
    }

    #[test]
    fn a_broken_or_slow_hook_does_not_block() {
        let o = decode("Stop", "boom", &run("echo oops >&2; exit 1"));
        assert!(o.block.is_none());
        assert!(o.errors[0].contains("退出码 1"), "{:?}", o.errors);
        let dir = std::env::temp_dir();
        let slow = run_command(
            "sleep 5",
            &serde_json::json!({}),
            &dir,
            &dir,
            Duration::from_millis(200),
        );
        assert!(slow.timed_out);
        assert!(decode("Stop", "sleep", &slow).errors[0].contains("超时"));
    }
}
