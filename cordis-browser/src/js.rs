//! `browser_js`：在本会话的标签页上跑一段 Playwright 脚本（参考 Codex 的 `node_repl`）。
//!
//! 一个 `dock mcp browser` 进程起一个常驻 Node 执行器（[`RUNNER_SOURCE`]），用
//! playwright-core 的 `connectOverCDP` 连同一个 Chromium，stdin / stdout 一行一个 JSON。
//!
//! - Node：`DOCK_NODE`，否则 PATH 里的 `node`（要 18+）。
//! - playwright-core：`DOCK_PLAYWRIGHT_CORE` 指到已有的包目录；否则第一次用时装到
//!   `$DOCK_HOME/browser/js/node_modules`（固定 [`PLAYWRIGHT_CORE_VERSION`]，只是驱动，不下载浏览器）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};

use cordis_base::config::dock_home;

/// 执行器源码，启动时写到 `$DOCK_HOME/browser/js/runner.mjs`。
pub const RUNNER_SOURCE: &str = include_str!("js/runner.mjs");
/// 用法指南（脚本里 `docs()` 读它），和执行器写在同一个目录。
pub const GUIDE: &str = include_str!("js/guide.md");
pub const PLAYWRIGHT_CORE_VERSION: &str = "1.64.0";
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

/// 常驻的 Node 执行器。死了（Node 崩了 / 被杀）就在下一次调用时重起。
pub struct JsRunner {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    pending: Pending,
    next: AtomicU64,
}

impl JsRunner {
    /// 找到 Node、备好 playwright-core、拉起执行器并等它报就绪。
    pub async fn start() -> Result<Self, String> {
        let dir = runner_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let node = node_binary()?;
        let playwright = playwright_core(&dir).await?;
        let script = dir.join("runner.mjs");
        std::fs::write(&script, RUNNER_SOURCE)
            .map_err(|e| format!("write {}: {e}", script.display()))?;
        let guide = dir.join("guide.md");
        std::fs::write(&guide, GUIDE).map_err(|e| format!("write {}: {e}", guide.display()))?;

        let mut child = Command::new(&node)
            .arg(&script)
            .arg(playwright.join("index.mjs"))
            .arg(&guide)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("start {node}: {e}"))?;
        let stdin = child.stdin.take().ok_or("runner stdin")?;
        let stdout = child.stdout.take().ok_or("runner stdout")?;
        let stderr_tail: Arc<std::sync::Mutex<std::collections::VecDeque<String>>> = Arc::default();
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(keep_stderr_tail(
                BufReader::new(stderr),
                stderr_tail.clone(),
            ));
        }
        let pending: Pending = Arc::default();
        let (ready_tx, ready_rx) = oneshot::channel();
        tokio::spawn(read_replies(
            BufReader::new(stdout),
            pending.clone(),
            ready_tx,
        ));
        match tokio::time::timeout(READY_TIMEOUT, ready_rx).await {
            Ok(Ok(())) => {}
            _ => {
                let _ = child.kill().await;
                // 给 stderr 一点时间把最后几行读完（ESM 加载错误、版本不对都在那里）。
                tokio::time::sleep(Duration::from_millis(100)).await;
                let tail: Vec<String> = stderr_tail.lock().unwrap().iter().cloned().collect();
                let detail = if tail.is_empty() {
                    String::new()
                } else {
                    format!("\nstderr:\n{}", tail.join("\n"))
                };
                return Err(format!(
                    "the browser_js runner did not start ({node} {}). Node 18+ and playwright-core are required.{detail}",
                    script.display()
                ));
            }
        }
        Ok(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending,
            next: AtomicU64::new(1),
        })
    }

    pub async fn is_alive(&self) -> bool {
        matches!(self.child.lock().await.try_wait(), Ok(None))
    }

    /// 发一个请求，等回复（最多 `wait`）。
    pub async fn call(&self, mut req: Value, wait: Duration) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        req["id"] = json!(id);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let line = format!("{req}\n");
        let sent = self.stdin.lock().await.write_all(line.as_bytes()).await;
        if let Err(e) = sent {
            self.pending.lock().unwrap().remove(&id);
            return Err(format!("browser_js runner is gone: {e}"));
        }
        let out = tokio::time::timeout(wait, rx).await;
        self.pending.lock().unwrap().remove(&id);
        match out {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(_)) => Err("browser_js runner exited during the call".into()),
            Err(_) => Err(format!(
                "browser_js did not answer within {}s",
                wait.as_secs()
            )),
        }
    }

    pub async fn shutdown(&self) {
        let _ = self.child.lock().await.kill().await;
    }
}

/// 执行器 stderr 的最后几行：启动失败时带进报错，平时不看。
const STDERR_TAIL: usize = 20;

async fn keep_stderr_tail(
    stderr: BufReader<tokio::process::ChildStderr>,
    tail: Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
) {
    let mut lines = stderr.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let mut tail = tail.lock().unwrap();
        if tail.len() == STDERR_TAIL {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

async fn read_replies(
    stdout: BufReader<tokio::process::ChildStdout>,
    pending: Pending,
    ready: oneshot::Sender<()>,
) {
    let mut ready = Some(ready);
    let mut lines = stdout.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if v.get("ready").is_some() {
            if let Some(tx) = ready.take() {
                let _ = tx.send(());
            }
            continue;
        }
        // 先返回了的调用（对话框、超时）晚到的回复没人等：丢掉。
        if let Some(tx) = v
            .get("id")
            .and_then(Value::as_u64)
            .and_then(|id| pending.lock().unwrap().remove(&id))
        {
            let _ = tx.send(v);
        }
    }
}

pub fn runner_dir() -> PathBuf {
    dock_home().join("browser").join("js")
}

fn node_binary() -> Result<String, String> {
    let node = std::env::var("DOCK_NODE")
        .ok()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "node".into());
    let ok = std::process::Command::new(&node)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    match ok {
        Some(v) if node_major(&v).is_some_and(|m| m >= 18) => Ok(node),
        Some(v) => Err(format!("browser_js needs Node 18+, found {v} ({node})")),
        None => Err(format!(
            "browser_js needs Node 18+ (`{node}` not found; set DOCK_NODE)"
        )),
    }
}

fn node_major(version: &str) -> Option<u32> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// playwright-core 的包目录：`DOCK_PLAYWRIGHT_CORE`，或装在 `dir/node_modules` 里的那份。
async fn playwright_core(dir: &Path) -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("DOCK_PLAYWRIGHT_CORE") {
        let path = PathBuf::from(path);
        if path.join("index.mjs").is_file() {
            return Ok(path);
        }
        return Err(format!(
            "DOCK_PLAYWRIGHT_CORE={} has no index.mjs",
            path.display()
        ));
    }
    let installed = dir.join("node_modules").join("playwright-core");
    if installed_version(&installed).as_deref() == Some(PLAYWRIGHT_CORE_VERSION) {
        return Ok(installed);
    }
    // 自己的 package.json：别让 npm 往上找到别人的项目。
    std::fs::write(
        dir.join("package.json"),
        r#"{"private":true,"name":"dock-browser-js"}"#,
    )
    .map_err(|e| format!("write package.json: {e}"))?;
    let npm = std::env::var("DOCK_NPM").unwrap_or_else(|_| "npm".into());
    let run = Command::new(&npm)
        .args([
            "install",
            // playwright-core 不需要安装脚本；不跑任何包的脚本，纵深防御。
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--loglevel=error",
            &format!("playwright-core@{PLAYWRIGHT_CORE_VERSION}"),
        ])
        .current_dir(dir)
        .stdin(Stdio::null())
        .output();
    let out = tokio::time::timeout(INSTALL_TIMEOUT, run)
        .await
        .map_err(|_| "installing playwright-core timed out".to_string())?
        .map_err(|e| format!("browser_js needs npm to install playwright-core ({npm}): {e}"))?;
    if !out.status.success() || installed_version(&installed).is_none() {
        return Err(format!(
            "npm install playwright-core@{PLAYWRIGHT_CORE_VERSION} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(installed)
}

fn installed_version(pkg: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(pkg.join("package.json")).ok()?;
    serde_json::from_str::<Value>(&raw)
        .ok()?
        .get("version")?
        .as_str()
        .map(str::to_string)
}

/// 执行器的回复 → 给模型的一段话 + 最后一张截图。
pub fn render_reply(reply: &Value) -> (bool, String, Option<PathBuf>) {
    let ok = reply.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let mut parts: Vec<String> = Vec::new();
    if let Some(logs) = reply.get("logs").and_then(Value::as_array) {
        let logs: Vec<&str> = logs.iter().filter_map(Value::as_str).collect();
        if !logs.is_empty() {
            parts.push(format!("log:\n{}", logs.join("\n")));
        }
    }
    if ok {
        let result = reply.get("result").and_then(Value::as_str).unwrap_or("");
        parts.push(if result.is_empty() {
            "(script finished, no return value)".into()
        } else {
            result.to_string()
        });
    } else {
        let err = reply
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("script failed");
        parts.push(format!("Error: {err}"));
    }
    let image = reply
        .get("images")
        .and_then(Value::as_array)
        .and_then(|a| a.last())
        .and_then(Value::as_str)
        .map(PathBuf::from);
    if let Some(img) = &image {
        parts.push(format!("screenshot: {}", img.display()));
    }
    if let Some(hints) = reply.get("hints").and_then(Value::as_array) {
        let hints: Vec<String> = hints
            .iter()
            .filter_map(Value::as_str)
            .map(|h| format!("hint: {h}"))
            .collect();
        if !hints.is_empty() {
            parts.push(hints.join("\n"));
        }
    }
    (ok, parts.join("\n\n"), image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_versions() {
        assert_eq!(node_major("v25.2.1\n"), Some(25));
        assert_eq!(node_major("18.0.0"), Some(18));
        assert_eq!(node_major("garbage"), None);
    }

    #[test]
    fn replies_render_result_logs_and_last_screenshot() {
        let (ok, text, img) = render_reply(&json!({
            "ok": true, "result": "42", "logs": ["a", "b"], "images": ["/x/1.png", "/x/2.png"]
        }));
        assert!(ok);
        assert!(text.starts_with("log:\na\nb"), "{text}");
        assert!(text.contains("42"), "{text}");
        assert_eq!(img, Some(PathBuf::from("/x/2.png")));

        let (ok, text, img) = render_reply(&json!({"ok": false, "error": "boom", "logs": []}));
        assert!(!ok);
        assert_eq!(text, "Error: boom");
        assert!(img.is_none());

        let (_, text, _) = render_reply(&json!({"ok": true, "result": ""}));
        assert!(text.contains("no return value"), "{text}");

        let (_, text, _) =
            render_reply(&json!({"ok": true, "result": "1", "hints": ["sleep less"]}));
        assert!(text.ends_with("hint: sleep less"), "{text}");
    }
}
