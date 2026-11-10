//! 拉起并看管一个 headless Dock（`dock serve`）。
//!
//! 控制通道是 Dock 的 stdin / stdout，一行一个 JSON（见 `cordis-gateway/src/serve.rs`）：
//! 启动后 Dock 写 `{"event":"ready","ws":…,"ticket":…}`；往 stdin 写
//! `{"cmd":"ticket"}` 换一张新 ticket（有效期 1 小时；每次连 WS 都要一张新的，
//! 省得管过期）；GUI 退出时
//! 关掉 stdin，Dock 跟着退出。
//!
//! 前端只拿到 `ws` 地址和 ticket，Dock 进程本身只在这里。

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

/// 等 Dock 启动 / 回一张 ticket 的上限。Dock 起来要挂整棵插件树（含 MCP 握手），
/// 冷启动可能要几秒。
const WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Serialize)]
pub struct Connection {
    pub ws: String,
    pub ticket: String,
}

#[derive(Default)]
struct Inner {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    ws: Option<String>,
    /// `ready` 里带的那张 ticket，第一次连接直接用，省一次往返。
    first_ticket: Option<String>,
    /// 等 ticket 的请求，按顺序一一对应 Dock 回的 `ticket` 事件。
    waiters: Vec<Sender<Result<String, String>>>,
    /// Dock 退出 / 起不来的原因；有值就不再等。
    dead: Option<String>,
    ready_waiters: Vec<Sender<Result<(), String>>>,
    /// 第几次拉起。旧进程的读线程收尾时对不上就不动新进程的状态
    /// （切到远程再马上切回本机时，旧进程的「已退出」会晚到）。
    generation: u64,
    /// 拉起过、还没被 [`DockProcess::stop`] 收掉。
    started: bool,
}

impl Inner {
    /// 清空、换下一代：旧读线程认得出自己过期了。
    fn reset(&mut self) {
        let generation = self.generation + 1;
        *self = Inner {
            generation,
            ..Inner::default()
        };
    }
}

#[derive(Clone, Default)]
pub struct DockProcess {
    inner: Arc<Mutex<Inner>>,
}

impl DockProcess {
    pub fn spawn(&self, app: AppHandle) {
        self.inner.lock().unwrap().started = true;
        let bin = dock_bin();
        let origin = webview_origin();
        let mut cmd = Command::new(&bin);
        cmd.args(["serve", "--origin", &origin, "--application", "dock-gui"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        // 每个会话用自己的 cwd；Dock 进程本身在 App 的数据目录里起步。**不能**用
        // $HOME：Dock 把「启动目录/.dock」当项目层，而 `$HOME/.dock` 正是用户层，
        // 会被读两遍（预设、项目配置都会串）。
        // `DOCK_GUI_CWD`：开发时指定启动目录（比如带项目级 `.dock/config.toml` 的仓库）。
        let dir = std::env::var_os("DOCK_GUI_CWD")
            .map(PathBuf::from)
            .ok_or(())
            .or_else(|_| app.path().app_data_dir().map_err(|_| ()));
        match dir {
            Ok(dir) if std::fs::create_dir_all(&dir).is_ok() => {
                cmd.current_dir(dir);
            }
            _ => {
                cmd.current_dir(std::env::temp_dir());
            }
        }
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                self.fail(format!("启动 Dock 失败（{}）：{e}", bin.display()));
                return;
            }
        };
        let stdout = child.stdout.take().expect("piped stdout");
        let generation = {
            let mut inner = self.inner.lock().unwrap();
            inner.stdin = child.stdin.take();
            inner.child = Some(child);
            inner.generation
        };
        let this = self.clone();
        std::thread::spawn(move || this.read_events(BufReader::new(stdout), app, generation));
    }

    fn read_events(&self, stdout: impl BufRead, app: AppHandle, generation: u64) {
        for line in stdout.lines() {
            let Ok(line) = line else { break };
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                eprintln!("dock-gui: Dock 输出了看不懂的一行：{line}");
                continue;
            };
            let ticket = event
                .get("ticket")
                .and_then(Value::as_str)
                .map(String::from);
            match event.get("event").and_then(Value::as_str) {
                Some("ready") => {
                    let mut inner = self.inner.lock().unwrap();
                    inner.ws = event.get("ws").and_then(Value::as_str).map(String::from);
                    inner.first_ticket = ticket;
                    for w in inner.ready_waiters.drain(..) {
                        let _ = w.send(Ok(()));
                    }
                    drop(inner);
                    let _ = app.emit("dock://ready", ());
                }
                Some("ticket") => {
                    let mut inner = self.inner.lock().unwrap();
                    if !inner.waiters.is_empty() {
                        let w = inner.waiters.remove(0);
                        let _ = w.send(ticket.ok_or_else(|| "Dock 回的 ticket 是空的".into()));
                    }
                }
                Some("error") => {
                    let message = event.get("message").and_then(Value::as_str).unwrap_or("");
                    eprintln!("dock-gui: Dock 控制通道报错：{message}");
                    let mut inner = self.inner.lock().unwrap();
                    if !inner.waiters.is_empty() {
                        let w = inner.waiters.remove(0);
                        let _ = w.send(Err(message.to_string()));
                    }
                }
                _ => {}
            }
        }
        // stdout 关了 = Dock 退出了。
        // 取出来再等，别拿着锁阻塞 `connection()` / `shutdown()`。
        let child = {
            let mut inner = self.inner.lock().unwrap();
            if inner.generation != generation || !inner.started {
                // 已经被 `stop` 收掉 / 换了新进程：不是意外退出，不报。
                return;
            }
            inner.child.take()
        };
        let status = child
            .and_then(|mut c| c.wait().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "未知".into());
        self.fail(format!("Dock 已退出（{status}）"));
        let _ = app.emit("dock://exit", status);
    }

    fn fail(&self, reason: String) {
        eprintln!("dock-gui: {reason}");
        let mut inner = self.inner.lock().unwrap();
        inner.dead = Some(reason.clone());
        for w in inner.waiters.drain(..) {
            let _ = w.send(Err(reason.clone()));
        }
        for w in inner.ready_waiters.drain(..) {
            let _ = w.send(Err(reason.clone()));
        }
    }

    /// 一次 WS 连接要的地址和 ticket。第一次用 `ready` 带的，之后每次向 Dock 要新的。
    pub fn connection(&self) -> Result<Connection, String> {
        self.wait_ready()?;
        let rx = {
            let mut inner = self.inner.lock().unwrap();
            if let Some(reason) = &inner.dead {
                return Err(reason.clone());
            }
            let ws = inner.ws.clone().ok_or("Dock 没报 ws 地址")?;
            if let Some(ticket) = inner.first_ticket.take() {
                return Ok(Connection { ws, ticket });
            }
            let (tx, rx) = mpsc::channel();
            inner.waiters.push(tx);
            let stdin = inner.stdin.as_mut().ok_or("Dock 的 stdin 已关闭")?;
            writeln!(stdin, r#"{{"cmd":"ticket"}}"#)
                .and_then(|_| stdin.flush())
                .map_err(|e| format!("向 Dock 要 ticket 失败：{e}"))?;
            rx
        };
        let ticket = rx
            .recv_timeout(WAIT)
            .map_err(|_| "等 Dock 回 ticket 超时".to_string())??;
        let ws = self.inner.lock().unwrap().ws.clone().unwrap_or_default();
        Ok(Connection { ws, ticket })
    }

    fn wait_ready(&self) -> Result<(), String> {
        let rx = {
            let mut inner = self.inner.lock().unwrap();
            if let Some(reason) = &inner.dead {
                return Err(reason.clone());
            }
            if inner.ws.is_some() {
                return Ok(());
            }
            let (tx, rx) = mpsc::channel();
            inner.ready_waiters.push(tx);
            rx
        };
        rx.recv_timeout(WAIT)
            .map_err(|_| "等 Dock 启动超时".to_string())?
    }

    /// 重新拉起 Dock：只在它已经退出时有用（还活着就什么都不做）。前端的连接会自己重连上。
    pub fn restart(&self, app: AppHandle) {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.dead.is_none() {
                return;
            }
            inner.reset();
        }
        self.spawn(app);
    }

    /// 切回本机：没在跑（GUI 是以远程模式起的，或被 [`stop`](Self::stop) 收掉了）就拉起来。
    pub fn ensure_running(&self, app: AppHandle) {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.started && inner.dead.is_none() {
                return;
            }
            inner.reset();
        }
        self.spawn(app);
    }

    /// 切到远程：收掉本机 Dock，不当成意外退出（不发 `dock://exit`）。
    pub fn stop(&self) {
        {
            let mut inner = self.inner.lock().unwrap();
            if !inner.started {
                return;
            }
            inner.started = false;
            inner.dead = Some("已切到远程 Dock".into());
            for w in inner.waiters.drain(..) {
                let _ = w.send(Err("已切到远程 Dock".into()));
            }
            for w in inner.ready_waiters.drain(..) {
                let _ = w.send(Err("已切到远程 Dock".into()));
            }
        }
        self.shutdown();
    }

    /// 关掉 stdin 让 Dock 自己收尾退出；给它一点时间，还不走再强杀。
    pub fn shutdown(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.stdin = None;
        if let Some(mut child) = inner.child.take() {
            for _ in 0..20 {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Dock 可执行文件：`DOCK_BIN` 优先；开发构建用仓库里 `cargo build -p cordis-app`
/// 出来的 `target/debug/dock`；都没有就找 PATH 上的 `dock`。
pub fn dock_bin() -> PathBuf {
    if let Some(bin) = std::env::var_os("DOCK_BIN") {
        return PathBuf::from(bin);
    }
    if cfg!(debug_assertions) {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/dock");
        if repo.is_file() {
            return repo;
        }
    }
    PathBuf::from("dock")
}

/// ticket 绑在 webview 的 Origin 上，WS 握手时要对得上：开发时是 Vite 的地址，
/// 打包后 macOS 上是 `tauri://localhost`。
fn webview_origin() -> String {
    if let Some(origin) = std::env::var_os("DOCK_GUI_ORIGIN") {
        return origin.to_string_lossy().into_owned();
    }
    if cfg!(debug_assertions) {
        "http://localhost:1420".into()
    } else {
        "tauri://localhost".into()
    }
}
