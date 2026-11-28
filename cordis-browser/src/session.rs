//! Chromiumoxide CDP：一个共享的 Chromium 进程（[`Chromium`]），每个会话一组标签页
//! （[`ConnectedSession`]）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::browser::Browser;
use chromiumoxide::cdp::browser_protocol::accessibility::{
    EnableParams as AxEnableParams, GetFullAxTreeParams,
};
use chromiumoxide::cdp::browser_protocol::browser::CloseParams as BrowserCloseParams;
use chromiumoxide::cdp::browser_protocol::dom::{
    FocusParams, GetBoxModelParams, ResolveNodeParams, ScrollIntoViewIfNeededParams,
    SetFileInputFilesParams,
};
use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    InsertTextParams, MouseButton,
};
use chromiumoxide::cdp::browser_protocol::network::{
    BlockPattern, EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent,
    EventResponseReceived, EventWebSocketClosed, EventWebSocketCreated, EventWebSocketFrameError,
    EventWebSocketFrameReceived, EventWebSocketFrameSent, EventWebSocketHandshakeResponseReceived,
    EventWebSocketWillSendHandshakeRequest, GetResponseBodyParams, ResourceType,
    SetBlockedUrLsParams, WebSocketFrame,
};
use chromiumoxide::cdp::browser_protocol::page::{
    EventFrameNavigated, EventJavascriptDialogClosed, EventJavascriptDialogOpening,
    EventLifecycleEvent, FrameId, FrameTree, GetFrameTreeParams, GetNavigationHistoryParams,
    HandleJavaScriptDialogParams, NavigateToHistoryEntryParams,
};
use chromiumoxide::cdp::browser_protocol::system_info::GetProcessInfoParams;
use chromiumoxide::cdp::browser_protocol::target::{
    ActivateTargetParams, CloseTargetParams, GetTargetsParams, TargetId,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    CallArgument, CallFunctionOnParams, EvaluateParams, EventConsoleApiCalled, RemoteObject,
};
use chromiumoxide::keys;
use chromiumoxide::layout::Point;
use chromiumoxide::page::{Page, ScreenshotParams};
use futures_util::StreamExt;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use cordis_base::config::dock_home;

use crate::snapshot::{self, LeanSnapshot, RefEntry};
use crate::wait::{poll_until, retry_async, with_timeout};
use crate::TabInfo;

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);
/// 连已经在跑的 Chromium：本机回环，连不上就是没在跑（或文件过期），别久等。
const ATTACH_TIMEOUT: Duration = Duration::from_secs(3);
const DEVTOOLS_ACTIVE_PORT: &str = "DevToolsActivePort";
const NAV_TIMEOUT: Duration = Duration::from_secs(45);
/// DOMContentLoaded 之后最多再等 `load` 这么久。图片、广告、统计脚本常把 `load` 拖上好几秒，
/// 页面早就能读能点了（本地测：DOM 立刻就绪、一张图 4 秒，以前 navigate 陪着等满 4 秒）。
const LOAD_GRACE: Duration = Duration::from_secs(1);
const ACTION_TIMEOUT: Duration = Duration::from_secs(15);
/// 关浏览器后最多等它的进程退出这么久（见 [`Chromium::close`]）。
const CLOSE_WAIT: Duration = Duration::from_secs(10);
/// 自己拉起的 Chromium 多久看一次退没退（收尸）。
const REAP_POLL: Duration = Duration::from_millis(200);

#[derive(Clone, Debug)]
pub struct PendingDialog {
    pub message: String,
    pub dialog_type: String,
    pub default_prompt: Option<String>,
    /// 弹出它的那一页：开着对话框的页在关掉它之前不响应别的 CDP 命令。
    pub target: String,
}

/// 盯着一组页的 JS 对话框：`seq` 每弹出一个加一（调用方等它变，就知道这次调用把对话框
/// 弹出来了），`pending` 是还没关掉的那个。
#[derive(Clone)]
pub struct DialogWatch {
    pub seq: tokio::sync::watch::Receiver<u64>,
    pub pending: Arc<std::sync::Mutex<Option<PendingDialog>>>,
}

impl DialogWatch {
    pub fn pending(&self) -> Option<PendingDialog> {
        self.pending.lock().ok().and_then(|g| g.clone())
    }
}

const CAPTURE_CAP: usize = 100;
const EVAL_RESULT_MAX: usize = 8_192;
const LINE_MAX: usize = 512;

#[derive(Clone, Debug)]
pub struct NetworkEntry {
    pub method: String,
    pub url: String,
    pub status: Option<i64>,
    pub resource_type: String,
    request_id: String,
}

#[derive(Clone, Debug)]
pub struct ConsoleEntry {
    pub level: String,
    pub text: String,
}

struct CaptureBuffers {
    network: Vec<NetworkEntry>,
    console: Vec<ConsoleEntry>,
}

impl CaptureBuffers {
    fn new() -> Self {
        Self {
            network: Vec::new(),
            console: Vec::new(),
        }
    }

    fn push_network(&mut self, entry: NetworkEntry) {
        if let Some(existing) = self
            .network
            .iter_mut()
            .rev()
            .find(|e| e.request_id == entry.request_id)
        {
            if entry.status.is_some() {
                existing.status = entry.status;
            }
            if !entry.resource_type.is_empty() {
                existing.resource_type = entry.resource_type;
            }
            return;
        }
        self.network.push(entry);
        if self.network.len() > CAPTURE_CAP {
            let drop_n = self.network.len() - CAPTURE_CAP;
            self.network.drain(0..drop_n);
        }
    }

    fn update_network_status(&mut self, request_id: &str, status: i64, resource_type: &str) {
        if let Some(existing) = self
            .network
            .iter_mut()
            .rev()
            .find(|e| e.request_id == request_id)
        {
            existing.status = Some(status);
            if !resource_type.is_empty() {
                existing.resource_type = resource_type.to_string();
            }
        }
    }

    fn push_console(&mut self, entry: ConsoleEntry) {
        self.console.push(entry);
        if self.console.len() > CAPTURE_CAP {
            let drop_n = self.console.len() - CAPTURE_CAP;
            self.console.drain(0..drop_n);
        }
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

fn remote_object_text(obj: &RemoteObject) -> String {
    if let Some(v) = obj.value.as_ref() {
        match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    } else if let Some(d) = obj.description.as_ref() {
        d.clone()
    } else {
        format!("{:?}", obj.r#type)
    }
}

/// 一个 Chromium 进程，所有会话共用（共用 profile，登录态共享）。
///
/// 先按 user-data 下的 `DevToolsActivePort` 连已经在跑的那个（另一个 Dock 进程拉起的，
/// 或者 GUI 正开着看的）；连不上才自己拉起。进程不归任何一个 Dock：自己拉起的也是脱开的
/// 进程（见 [`spawn_detached`]），拉起后同样只是连上去。关不关由离开的那一方看还有没有
/// 别人在用（[`crate::hub`]），谁最后一个走谁关。
pub struct Chromium {
    browser: Arc<Browser>,
    handler: JoinHandle<()>,
    /// 自己拉起的才有：hub 没好好离开就被丢掉时用它收拾（[`Chromium::kill_launched`]）。
    launched: Option<Launched>,
}

impl Chromium {
    /// `headed` comes from cockpit pref + `DOCK_BROWSER_HEADED` override
    /// (see [`cordis_base::config::effective_browser_headed`]); default remains headless.
    /// Attaching to a running instance keeps whatever mode it already has.
    pub async fn attach_or_launch(headed: bool) -> Result<Self, String> {
        let user_data_dir = browser_user_data_dir();
        std::fs::create_dir_all(&user_data_dir)
            .map_err(|e| format!("create user-data-dir {}: {e}", user_data_dir.display()))?;
        if let Some(ws) = devtools_ws_url(&user_data_dir) {
            if let Ok(Ok((browser, handler))) =
                tokio::time::timeout(ATTACH_TIMEOUT, Browser::connect(ws)).await
            {
                return Ok(Self {
                    browser: Arc::new(browser),
                    handler: spawn_handler(handler),
                    launched: None,
                });
            }
        }
        Self::launch(&user_data_dir, headed).await
    }

    async fn launch(user_data_dir: &Path, headed: bool) -> Result<Self, String> {
        let exe = discover_chrome()?;
        // 无头的 UA 写着 `HeadlessChrome`：换成同版本的普通 Chrome（agent 和用户在面板里
        // 用的就是这个浏览器，网站看到的应该和真 Chrome 一样）。
        let ua = if headed {
            None
        } else {
            chrome_version(&exe)
                .await
                .and_then(|v| desktop_user_agent(&v))
        };
        let args = launch_args(user_data_dir, headed, ua.as_deref());
        let launched = spawn_detached(&exe, &args)
            .await
            .map_err(|e| format!("launch Chromium ({exe}): {e}"))?;
        // 视口不模拟（chromiumoxide 的 `HandlerConfig` 默认就是 `None`，和连上别人拉起的
        // 那个一样）：有头时窗口说了算，无头时就是窗口大小。连接用 LAUNCH_TIMEOUT，不用
        // ATTACH_TIMEOUT：那个是「连别人的、连不上就是没在跑」，这里进程刚起来，机器忙时
        // 3 秒可能不够（以前 `Browser::launch` 把连接也算在 LAUNCH_TIMEOUT 里）。
        let connected = with_timeout(
            LAUNCH_TIMEOUT,
            async {
                Browser::connect(launched.ws.clone())
                    .await
                    .map_err(|e| format!("connect: {e}"))
            },
            || "timed out connecting to the Chromium it launched".into(),
        )
        .await;
        match connected {
            Ok((browser, handler)) => Ok(Self {
                browser: Arc::new(browser),
                handler: spawn_handler(handler),
                launched: Some(launched),
            }),
            Err(e) => {
                // 拉起来了却连不上：别留一个谁也不认的进程。
                launched.kill();
                Err(format!("launch Chromium ({exe}): {e}"))
            }
        }
    }

    /// 浏览器级 CDP 的 ws 地址（`browser_js` 的执行器用它另连一条）。
    pub fn ws_url(&self) -> String {
        self.browser.websocket_address().clone()
    }

    /// CDP 连接还在吗。Chromium 被关掉（或别的进程拉起的那个退出了）时 handler 流结束。
    pub fn is_alive(&self) -> bool {
        !self.handler.is_finished()
    }

    /// Chromium 里现在的全部 target id（页、iframe、worker…）。
    pub async fn live_targets(&self) -> Result<Vec<String>, String> {
        Ok(self
            .browser
            .execute(GetTargetsParams::default())
            .await
            .map_err(|e| format!("getTargets: {e}"))?
            .result
            .target_infos
            .iter()
            .map(|t| t.target_id.inner().to_string())
            .collect())
    }

    /// 关掉整个浏览器（CDP `Browser.close`）：没有别人在用它的时候。
    ///
    /// 等到浏览器进程真退了才返回（最多 [`CLOSE_WAIT`]）：`Browser.close` 一回话就返回的话，
    /// 紧跟着的 `browser_open` 会按还没删的 `DevToolsActivePort` 连上这个正在退出的 Chromium，
    /// 开页报 `Failed to open a new tab`；拉起的新进程也可能撞上还没放开的 profile。
    pub async fn close(self) {
        let pid = self.browser_pid().await;
        let Self {
            browser, handler, ..
        } = self;
        match Arc::try_unwrap(browser) {
            Ok(mut browser) => {
                let _ = browser.close().await;
            }
            // 还有标签页组攥着句柄：直接发同一条命令。
            Err(shared) => {
                let _ = shared.execute(BrowserCloseParams::default()).await;
            }
        }
        handler.abort();
        wait_until_exited(pid).await;
    }

    /// 浏览器主进程的 pid（`SystemInfo.getProcessInfo` 里 `type` 是 `browser` 的那个）。
    /// 连上别人拉起的也拿得到；问不到就是 `None`。
    async fn browser_pid(&self) -> Option<u32> {
        let r = self
            .browser
            .execute(GetProcessInfoParams::default())
            .await
            .ok()?;
        r.result
            .process_info
            .iter()
            .find(|p| p.r#type == "browser")
            .and_then(|p| u32::try_from(p.id).ok())
    }

    /// 只断开自己这条 CDP 连接，浏览器留给还在用它的人。
    pub fn disconnect(self) {
        self.handler.abort();
    }

    /// 自己拉起的就直接杀掉进程（同步，不经 CDP）；连上别人的什么也不做。只给 hub 没走
    /// `shutdown` 就被丢掉时兜底，好好离开走 [`Chromium::close`] / [`Chromium::disconnect`]。
    pub fn kill_launched(&self) {
        if let Some(launched) = &self.launched {
            launched.kill();
        }
    }
}

/// 等 `pid` 退出，最多 [`CLOSE_WAIT`]；等不到也返回（不让收尾挂住）。
#[cfg(unix)]
async fn wait_until_exited(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    let _ = poll_until(CLOSE_WAIT, Duration::from_millis(50), || async move {
        Ok(!crate::registry::pid_alive(pid))
    })
    .await;
}

/// 非 unix 上查不了进程在不在，不等。
#[cfg(not(unix))]
async fn wait_until_exited(_pid: Option<u32>) {}

/// 自己拉起、还没连上的 Chromium：CDP 地址，和连不上时收拾它用的进程句柄。
struct Launched {
    ws: String,
    child: Arc<std::sync::Mutex<std::process::Child>>,
}

impl Launched {
    fn kill(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

/// 拉起一个脱开的 Chromium，从 stderr 读出它的 CDP 地址（`DevTools listening on ws://…`）。
///
/// 不用 chromiumoxide 的 `Browser::launch`：它拉起的子进程带 `kill_on_drop`，句柄一丢
/// Chromium 就被杀，拉起它的 Dock 一走（`browser_close` 掉最后一组、进程退出），别的 Dock
/// 进程连在上面的标签页全没了。这里用 std 的 `Command`（丢句柄不杀），在 unix 上自成一个
/// 进程组（拉起它的终端挂断、按 Ctrl-C 都不波及）。一条线程读 stderr：读到地址后接着读完，
/// 别让管道写满卡住 Chromium。另一条线程收尸，不留僵尸进程。
async fn spawn_detached(exe: &str, args: &[String]) -> Result<Launched, String> {
    use std::io::BufRead;
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let stderr = child.stderr.take().ok_or("spawn: no stderr pipe")?;
    let child = Arc::new(std::sync::Mutex::new(child));
    // 收尸：轮询 `try_wait`。不阻塞在 `wait` 上攥着锁（等地址超时时这边还要 kill），也不等
    // stderr 读到头（Chromium 的子进程也拿着 stderr，可能比它晚退；[`Chromium::close`] 等的
    // 就是主进程退出，僵尸也算没退）。
    let reaper = child.clone();
    std::thread::Builder::new()
        .name("chromium-reaper".into())
        .spawn(move || loop {
            let running = reaper
                .lock()
                .map(|mut c| matches!(c.try_wait(), Ok(None)))
                .unwrap_or(false);
            if !running {
                break;
            }
            std::thread::sleep(REAP_POLL);
        })
        .map_err(|e| {
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
            }
            format!("spawn reaper: {e}")
        })?;
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<String, String>>();
    std::thread::Builder::new()
        .name("chromium-stderr".into())
        .spawn(move || {
            let mut tx = Some(tx);
            // 没等到地址就退出时，报错带上最后几行 stderr。
            let mut tail: std::collections::VecDeque<String> = Default::default();
            let mut reader = std::io::BufReader::new(stderr);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if tx.is_none() {
                    continue;
                }
                let line = String::from_utf8_lossy(&buf);
                if let Some(ws) = devtools_listening_url(&line) {
                    if let Some(tx) = tx.take() {
                        let _ = tx.send(Ok(ws));
                    }
                } else {
                    tail.push_back(line.trim_end().to_string());
                    if tail.len() > 5 {
                        tail.pop_front();
                    }
                }
            }
            if let Some(tx) = tx.take() {
                let tail = Vec::from(tail).join(" | ");
                let _ = tx.send(Err(format!(
                    "Chromium exited before printing its DevTools address: {tail}"
                )));
            }
        })
        .map_err(|e| {
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
            }
            format!("spawn stderr reader: {e}")
        })?;
    let launched = |ws| Launched {
        ws,
        child: child.clone(),
    };
    match tokio::time::timeout(LAUNCH_TIMEOUT, rx).await {
        Ok(Ok(Ok(ws))) => Ok(launched(ws)),
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(_)) => Err("stderr reader stopped without an answer".into()),
        Err(_) => {
            launched(String::new()).kill();
            Err(format!(
                "timed out after {}s waiting for Chromium's DevTools address",
                LAUNCH_TIMEOUT.as_secs()
            ))
        }
    }
}

/// stderr 里报 CDP 地址的那一行：`DevTools listening on ws://127.0.0.1:<port>/devtools/browser/<id>`。
pub fn devtools_listening_url(line: &str) -> Option<String> {
    let (_, ws) = line.rsplit_once("listening on ")?;
    let ws = ws.trim();
    (ws.starts_with("ws") && ws.contains("devtools/browser")).then(|| ws.to_string())
}

/// 拉起 Chromium 的完整参数（带 `--`）。和以前交给 chromiumoxide `BrowserConfig` 拼出来的
/// 一样：[`LAUNCH_ARGS`] 加上它自己补的那几条（调试端口、profile、关扩展、关沙箱、无头三件套、
/// 不报 `AutomationControlled`），有头再给窗口大小，无头再换 UA。
///
/// 不带 `--enable-automation`（`navigator.webdriver = true`、顶上一条提示栏）：Google 搜索
/// 这类站点见了就一直弹人机验证，人在面板里也过不去。有头时不模拟视口（窗口说了算），
/// 不然默认 800×600 只画窗口一角。
fn launch_args(user_data_dir: &Path, headed: bool, user_agent: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = LAUNCH_ARGS.iter().map(|a| format!("--{a}")).collect();
    args.extend(
        [
            "--remote-debugging-port=0",
            "--disable-extensions",
            "--no-sandbox",
            "--disable-setuid-sandbox",
            "--disable-blink-features=AutomationControlled",
        ]
        .map(String::from),
    );
    args.push(format!("--user-data-dir={}", user_data_dir.display()));
    if headed {
        args.push("--window-size=1280,900".into());
    } else {
        args.extend(["--headless", "--hide-scrollbars", "--mute-audio"].map(String::from));
    }
    if let Some(ua) = user_agent {
        args.push(format!("--user-agent={ua}"));
    }
    args
}

fn spawn_handler(mut handler: chromiumoxide::Handler) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(h) = handler.next().await {
            if h.is_err() {
                break;
            }
        }
    })
}

/// 读 Chromium 写在 user-data 下的 `DevToolsActivePort`（第一行端口，第二行
/// `/devtools/browser/<id>`），拼成 CDP 的 ws 地址。
pub fn devtools_ws_url(user_data_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(user_data_dir.join(DEVTOOLS_ACTIVE_PORT)).ok()?;
    parse_devtools_active_port(&raw)
}

pub fn parse_devtools_active_port(raw: &str) -> Option<String> {
    let mut lines = raw.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim();
    if port == 0 || !path.starts_with("/devtools/browser/") {
        return None;
    }
    Some(format!("ws://127.0.0.1:{port}{path}"))
}

/// 本组的页（`ours`）自己开的新页：`(新页, 打开它的页)`，按要收的顺序。新页开的新页也算
/// （先收父页，再收它开的）。`targets` 是 Chrome 的 `(target id, type, opener id)`；只收
/// `page`，已经在本组的不收，别的会话的页开的不收。
pub fn opened_by(
    ours: &[String],
    targets: &[(String, String, Option<String>)],
) -> Vec<(String, String)> {
    let mut known: Vec<String> = ours.to_vec();
    let mut out = Vec::new();
    loop {
        let next = targets.iter().find(|(id, kind, opener)| {
            kind == "page"
                && !known.contains(id)
                && opener.as_ref().is_some_and(|o| known.contains(o))
        });
        let Some((id, _, Some(opener))) = next else {
            return out;
        };
        known.push(id.clone());
        out.push((id.clone(), opener.clone()));
    }
}

/// 拉起 Chromium 的参数：chromiumoxide 的默认参数（照抄 puppeteer）去掉 `enable-automation`。
/// `key` 或 `key=value`（多个值用逗号连）；不带前导 `--`，由 [`launch_args`] 补上。
const LAUNCH_ARGS: &[&str] = &[
    "disable-background-networking",
    "enable-features=NetworkService,NetworkServiceInProcess",
    "disable-background-timer-throttling",
    "disable-backgrounding-occluded-windows",
    "disable-breakpad",
    "disable-client-side-phishing-detection",
    "disable-component-extensions-with-background-pages",
    "disable-default-apps",
    "disable-dev-shm-usage",
    "disable-features=TranslateUI",
    "disable-hang-monitor",
    "disable-ipc-flooding-protection",
    "disable-popup-blocking",
    "disable-prompt-on-repost",
    "disable-renderer-backgrounding",
    "disable-sync",
    "force-color-profile=srgb",
    "metrics-recording-only",
    "no-first-run",
    "password-store=basic",
    "use-mock-keychain",
    "enable-blink-features=IdleDetection",
    "lang=en_US",
    "force-renderer-accessibility",
];

/// `chrome --version` 的版本号（`Google Chrome 154.0.7712.3` → `154.0.7712.3`）；跑不了就 `None`。
async fn chrome_version(exe: &str) -> Option<String> {
    let exe = exe.to_string();
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || {
            std::process::Command::new(exe).arg("--version").output()
        }),
    )
    .await
    .ok()?
    .ok()?
    .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()) && w.contains('.'))
        .map(String::from)
}

/// 同版本普通桌面 Chrome 的 UA（Chrome 自己也只报大版本，后面是 `.0.0.0`）。
pub fn desktop_user_agent(version: &str) -> Option<String> {
    let major = version.split('.').next()?;
    if major.is_empty() || !major.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let platform = if cfg!(target_os = "macos") {
        "Macintosh; Intel Mac OS X 10_15_7"
    } else if cfg!(target_os = "windows") {
        "Windows NT 10.0; Win64; x64"
    } else {
        "X11; Linux x86_64"
    };
    Some(format!(
        "Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"
    ))
}

/// 一个会话的标签页组：自己开的页、快照 refs、对话框、network / console 捕获。
/// 进程是共享的 [`Chromium`]，这里只攥句柄。
pub struct ConnectedSession {
    browser: Arc<Browser>,
    pages: Vec<Page>,
    active: usize,
    dialog_tasks: Vec<JoinHandle<()>>,
    capture_tasks: Vec<JoinHandle<()>>,
    /// 本组只记一个：两页先后弹对话框时后一个盖掉前一个，前一个关掉之前本组一直被
    /// 挡着（报错里写的是后一个）。边缘情况，处理掉后一个再看快照就能发现前一个。
    pending_dialog: Arc<std::sync::Mutex<Option<PendingDialog>>>,
    dialog_seq: Arc<tokio::sync::watch::Sender<u64>>,
    capture: Arc<AsyncMutex<CaptureBuffers>>,
    /// 审计开着时（Dock 在 `_meta["dock/audit"]` 里带了目录）这一组的网络请求记进去。
    audit: AuditSlot,
    /// 沙箱的主机规则翻成的拦截模式（`Network.setBlockedURLs`）；各页共用，后开的页也套上。
    blocked: BlockedSlot,
    pub refs: HashMap<String, RefEntry>,
    /// 每一页主框架换过几次文档（`Page.frameNavigated`，没有父框架）。按页记：后台标签页
    /// 自己刷新不碰当前页的 refs；同文档内的跳转（SPA 路由、锚点）不算换文档。
    docs: Arc<std::sync::Mutex<HashMap<String, u64>>>,
    /// `refs` 是哪一页、第几份文档上拍的。那一页换了文档就作废（[`Self::drop_stale_refs`]）。
    refs_doc: Option<(String, u64)>,
}

type BlockedSlot = Arc<std::sync::Mutex<Vec<BlockPattern>>>;

/// 沙箱主机规则（`(通配, 阻断?)`，按顺序）→ 拦截模式。`*.example.com` 也管 `example.com` 本身；
/// 带不带端口都匹配。
fn block_patterns(rules: &[(String, bool)]) -> Vec<BlockPattern> {
    let mut out = Vec::new();
    for (host, block) in rules {
        let host = host.trim().to_ascii_lowercase();
        let mut hosts = vec![host.clone()];
        if let Some(bare) = host.strip_prefix("*.") {
            hosts.push(bare.to_string());
        }
        for h in hosts {
            out.push(BlockPattern::new(format!("*://{h}/*"), *block));
            out.push(BlockPattern::new(format!("*://{h}:*/*"), *block));
        }
    }
    out
}

async fn apply_blocked(page: &Page, patterns: &[BlockPattern]) {
    let params = SetBlockedUrLsParams {
        url_patterns: Some(patterns.to_vec()),
    };
    let _ = page.execute(params).await;
}

/// 一组标签页的审计日志（`None` = 没在审计）。各页的网络监听共用这一格。
pub type AuditSlot = Arc<std::sync::Mutex<Option<Arc<dock_sandbox::audit::store::AuditLog>>>>;

impl ConnectedSession {
    /// 在共享的 Chromium 里给这个会话开第一个标签页（空白页）。先挂好对话框与 console /
    /// network 监听再跳转（[`ConnectedSession::navigate`]）：第一页加载时弹的框、打的日志、
    /// 发的请求都收得到，跳转也走同一条有上限、会和对话框赛跑的路。
    pub async fn open(chromium: &Chromium) -> Result<Self, String> {
        let browser = chromium.browser.clone();
        let page = with_timeout(
            ACTION_TIMEOUT,
            async {
                browser
                    .new_page("about:blank")
                    .await
                    .map_err(|e| format!("new_page: {e}"))
            },
            || "timed out opening a tab".into(),
        )
        .await?;

        let pending_dialog = Arc::new(std::sync::Mutex::new(None));
        let dialog_seq = Arc::new(tokio::sync::watch::channel(0u64).0);
        let capture = Arc::new(AsyncMutex::new(CaptureBuffers::new()));
        let docs = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let audit: AuditSlot = Arc::default();
        let blocked: BlockedSlot = Arc::default();
        let mut dialog_tasks = Vec::new();
        let mut capture_tasks = Vec::new();
        dialog_tasks.push(
            Self::spawn_dialog_listener(&page, pending_dialog.clone(), dialog_seq.clone()).await?,
        );
        capture_tasks.extend(
            Self::attach_page_capture(
                &page,
                capture.clone(),
                docs.clone(),
                audit.clone(),
                blocked.clone(),
            )
            .await?,
        );

        Ok(Self {
            browser,
            pages: vec![page],
            active: 0,
            dialog_tasks,
            capture_tasks,
            pending_dialog,
            dialog_seq,
            capture,
            audit,
            blocked,
            refs: HashMap::new(),
            docs,
            refs_doc: None,
        })
    }

    /// 本组标签页的 CDP target id（GUI 按它把画面对到会话上）。
    pub fn target_ids(&self) -> Vec<String> {
        self.pages
            .iter()
            .map(|p| p.target_id().inner().to_string())
            .collect()
    }

    /// 当前活动标签页的 target id。
    pub fn active_target_id(&self) -> Option<String> {
        self.pages
            .get(self.active)
            .map(|p| p.target_id().inner().to_string())
    }

    /// 和 Chrome 对一遍本组的标签页：
    /// - 去掉已经不在 Chrome 里的（渲染进程崩了被回收、被别的连接关掉）。留着它们，之后每个
    ///   调用都报 `receiver is gone`，名册也一直指着死页。活动页没了就换成最后一页。
    /// - 收下本组的页自己开的新页（`target=_blank` 链接、`window.open`，见 [`opened_by`]）：
    ///   插在打开它的页右边并成为活动页，和真浏览器一样——agent 下一步就在新页上。
    ///
    /// 返回有没有变；问不到 Chrome 时什么也不动。
    pub async fn sync_targets(&mut self) -> Result<bool, String> {
        let infos: Vec<(String, String, Option<String>)> = self
            .browser
            .execute(GetTargetsParams::default())
            .await
            .map_err(|e| format!("getTargets: {e}"))?
            .result
            .target_infos
            .iter()
            .map(|t| {
                (
                    t.target_id.inner().to_string(),
                    t.r#type.clone(),
                    t.opener_id.as_ref().map(|o| o.inner().to_string()),
                )
            })
            .collect();
        let alive: std::collections::HashSet<&str> =
            infos.iter().map(|(id, _, _)| id.as_str()).collect();
        let active = self.active_target_id();
        let before = self.pages.len();
        self.pages
            .retain(|p| alive.contains(p.target_id().inner().as_str()));
        let mut changed = self.pages.len() != before;
        if changed {
            self.active = active
                .and_then(|id| self.target_ids().iter().position(|t| *t == id))
                .unwrap_or(self.pages.len().saturating_sub(1));
        }
        for (new, opener) in opened_by(&self.target_ids(), &infos) {
            // 刚开出来、chromiumoxide 还没挂上的页拿不到：下一次调用再收。
            let Ok(page) = self.browser.get_page(TargetId::from(new)).await else {
                continue;
            };
            let Some(at) = self.target_ids().iter().position(|t| *t == opener) else {
                continue;
            };
            self.dialog_tasks
                .push(self.dialog_listener_for(&page).await?);
            self.capture_tasks.extend(
                Self::attach_page_capture(
                    &page,
                    self.capture.clone(),
                    self.docs.clone(),
                    self.audit.clone(),
                    self.blocked.clone(),
                )
                .await?,
            );
            self.pages.insert(at + 1, page);
            self.active = at + 1;
            changed = true;
        }
        if changed {
            self.refs.clear();
        }
        Ok(changed)
    }

    /// 把 `browser_js` 脚本里新开的页（`tabs.new`）收进本组，放在最后。刚开出来的页
    /// chromiumoxide 可能还没挂上，等一会儿（最多 3 秒）。已经在本组的不重复收。
    pub async fn adopt(&mut self, target: &str) -> Result<(), String> {
        if self.target_ids().iter().any(|t| t == target) {
            return Ok(());
        }
        let mut page = None;
        for _ in 0..30 {
            if let Ok(p) = self
                .browser
                .get_page(TargetId::from(target.to_string()))
                .await
            {
                page = Some(p);
                break;
            }
            // 脚本里开了又关掉的页：Chrome 里已经没有了，不用干等（以前白等满 3 秒）。
            if !self.chrome_has_target(target).await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let page =
            page.ok_or_else(|| format!("tab {target} opened by the script is not reachable"))?;
        self.dialog_tasks
            .push(self.dialog_listener_for(&page).await?);
        self.capture_tasks.extend(
            Self::attach_page_capture(
                &page,
                self.capture.clone(),
                self.docs.clone(),
                self.audit.clone(),
                self.blocked.clone(),
            )
            .await?,
        );
        self.pages.push(page);
        Ok(())
    }

    /// Chrome 里还有没有这个 target。问不到时当它还在（让调用方照常等）。
    async fn chrome_has_target(&self, target: &str) -> bool {
        match self.browser.execute(GetTargetsParams::default()).await {
            Ok(r) => r
                .result
                .target_infos
                .iter()
                .any(|t| t.target_id.inner() == target),
            Err(_) => true,
        }
    }

    /// 记下 `browser_js` 执行器看到、本组自己的监听没赶上的对话框（弹在这次脚本才开的页上：
    /// 页登记进本组时对话框已经开着了）。之后的调用照常被挡、`browser_handle_dialog` 对准那一页。
    /// 已经记着一个的不覆盖。
    pub fn note_dialog(&mut self, target: &str, dialog_type: &str, message: &str) {
        if !self.target_ids().iter().any(|t| t == target) {
            return;
        }
        if let Ok(mut g) = self.pending_dialog.lock() {
            if g.is_none() {
                *g = Some(PendingDialog {
                    message: message.to_string(),
                    dialog_type: dialog_type.to_string(),
                    default_prompt: None,
                    target: target.to_string(),
                });
            }
        }
    }

    /// 把本组的某一页设成活动页（`browser_js` 的 `tabs.select`）。不是本组的页不动。
    pub fn activate(&mut self, target: &str) -> bool {
        match self.target_ids().iter().position(|t| t == target) {
            Some(i) => {
                self.active = i;
                self.refs.clear();
                true
            }
            None => false,
        }
    }

    /// 一个标签页都不剩了（[`ConnectedSession::sync_targets`] 之后）。
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    async fn dialog_listener_for(&self, page: &Page) -> Result<JoinHandle<()>, String> {
        Self::spawn_dialog_listener(page, self.pending_dialog.clone(), self.dialog_seq.clone())
            .await
    }

    /// 记下页上弹出的对话框，并让 `seq` 加一（等着的调用据此先返回）；对话框关掉
    /// （agent 处理了，或者人在面板里点掉了）就清掉。
    async fn spawn_dialog_listener(
        page: &Page,
        pending: Arc<std::sync::Mutex<Option<PendingDialog>>>,
        seq: Arc<tokio::sync::watch::Sender<u64>>,
    ) -> Result<JoinHandle<()>, String> {
        let mut opened = page
            .event_listener::<EventJavascriptDialogOpening>()
            .await
            .map_err(|e| format!("dialog listener: {e}"))?;
        let mut closed = page
            .event_listener::<EventJavascriptDialogClosed>()
            .await
            .map_err(|e| format!("dialog listener: {e}"))?;
        let target = page.target_id().as_ref().to_string();
        Ok(tokio::spawn(async move {
            loop {
                tokio::select! {
                    ev = opened.next() => {
                        let Some(ev) = ev else { break };
                        let info = PendingDialog {
                            message: ev.message.clone(),
                            dialog_type: ev.r#type.as_ref().to_string(),
                            default_prompt: ev.default_prompt.clone(),
                            target: target.clone(),
                        };
                        if let Ok(mut g) = pending.lock() {
                            *g = Some(info);
                        }
                        seq.send_modify(|n| *n += 1);
                    }
                    ev = closed.next() => {
                        if ev.is_none() {
                            break;
                        }
                        if let Ok(mut g) = pending.lock() {
                            if g.as_ref().is_some_and(|d| d.target == target) {
                                *g = None;
                            }
                        }
                    }
                }
            }
        }))
    }

    /// 给调用方一份对话框的观察句柄（不借着本组，调用跑着时也能看）。
    pub fn dialog_watch(&self) -> DialogWatch {
        DialogWatch {
            seq: self.dialog_seq.subscribe(),
            pending: self.pending_dialog.clone(),
        }
    }

    /// 开 / 关这一组的审计（`None` = 关）。
    pub fn set_audit(&self, log: Option<Arc<dock_sandbox::audit::store::AuditLog>>) {
        *self.audit.lock().unwrap() = log;
    }

    /// 套上沙箱的主机规则（`[]` = 不拦）：现有的每一页，以及之后开的页。
    pub async fn set_blocked(&self, rules: Vec<(String, bool)>) {
        let patterns = block_patterns(&rules);
        *self.blocked.lock().unwrap() = patterns.clone();
        for page in &self.pages {
            apply_blocked(page, &patterns).await;
        }
    }

    async fn attach_page_capture(
        page: &Page,
        capture: Arc<AsyncMutex<CaptureBuffers>>,
        docs: Arc<std::sync::Mutex<HashMap<String, u64>>>,
        audit: AuditSlot,
        blocked: BlockedSlot,
    ) -> Result<Vec<JoinHandle<()>>, String> {
        let patterns = blocked.lock().unwrap().clone();
        if !patterns.is_empty() {
            apply_blocked(page, &patterns).await;
        }
        // Network/Runtime are enabled by chromiumoxide's frame/network managers
        // on target attach. Do not re-send Enable — it can time out the command
        // queue. Event listeners alone surface request/console traffic.
        let mut tasks = Vec::with_capacity(5);
        if let Some(task) = audit_network(page, audit).await {
            tasks.push(task);
        }
        // 主框架换了文档：这一页拍的 refs 作废（点击 / 输入带走了页面也算，不只 browser_navigate）。
        if let Ok(mut events) = page.event_listener::<EventFrameNavigated>().await {
            let target = page.target_id().inner().to_string();
            tasks.push(tokio::spawn(async move {
                while let Some(ev) = events.next().await {
                    if ev.frame.parent_id.is_none() {
                        *docs.lock().unwrap().entry(target.clone()).or_default() += 1;
                    }
                }
            }));
        }
        if let Ok(mut events) = page.event_listener::<EventRequestWillBeSent>().await {
            let capture = capture.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(ev) = events.next().await {
                    let method = ev.request.method.clone();
                    let url = truncate_str(&ev.request.url, LINE_MAX);
                    let resource_type = ev
                        .r#type
                        .as_ref()
                        .map(|t| t.as_ref().to_string())
                        .unwrap_or_default();
                    let request_id = ev.request_id.inner().to_string();
                    let mut g = capture.lock().await;
                    g.push_network(NetworkEntry {
                        method,
                        url,
                        status: None,
                        resource_type,
                        request_id,
                    });
                }
            }));
        }
        if let Ok(mut events) = page.event_listener::<EventResponseReceived>().await {
            let capture = capture.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(ev) = events.next().await {
                    let request_id = ev.request_id.inner().to_string();
                    let status = ev.response.status;
                    let resource_type = ev.r#type.as_ref().to_string();
                    let mut g = capture.lock().await;
                    g.update_network_status(&request_id, status, &resource_type);
                }
            }));
        }
        if let Ok(mut events) = page.event_listener::<EventConsoleApiCalled>().await {
            let capture = capture.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(ev) = events.next().await {
                    let level = ev.r#type.as_ref().to_string();
                    let text = truncate_str(
                        &ev.args
                            .iter()
                            .map(remote_object_text)
                            .collect::<Vec<_>>()
                            .join(" "),
                        LINE_MAX,
                    );
                    let mut g = capture.lock().await;
                    g.push_console(ConsoleEntry { level, text });
                }
            }));
        }
        Ok(tasks)
    }

    pub fn pending_dialog(&self) -> Option<PendingDialog> {
        self.pending_dialog.lock().ok().and_then(|g| g.clone())
    }

    pub fn active_page(&self) -> Result<&Page, String> {
        self.pages
            .get(self.active)
            .ok_or_else(|| "no active tab".into())
    }

    pub async fn navigate(&mut self, url: &str) -> Result<String, String> {
        let page = self
            .pages
            .get(self.active)
            .ok_or_else(|| "no active tab".to_string())?
            .clone();
        let ws = self.browser.websocket_address().clone();
        with_timeout(NAV_TIMEOUT, goto_ready(&page, &ws, url), || {
            format!(
                "did not reach DOMContentLoaded within {}s",
                NAV_TIMEOUT.as_secs()
            )
        })
        .await
        .map_err(|e| format!("navigate({url}): {e}"))?;
        self.refs.clear();
        // 页里自己的 `location.href`：chromiumoxide 记的网址跟不上页内（锚点）跳转。
        let cur: Option<String> = page
            .evaluate("location.href")
            .await
            .ok()
            .and_then(|v| v.into_value().ok());
        match cur {
            Some(cur) => Ok(cur),
            None => Ok(page
                .url()
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| url.into())),
        }
    }

    pub async fn snapshot(
        &mut self,
        interactive: bool,
        frame_selector: Option<&str>,
    ) -> Result<LeanSnapshot, String> {
        let page = self.active_page()?.clone();
        let frame_id = self.resolve_frame_id(&page, frame_selector).await?;
        let snap = with_timeout(
            ACTION_TIMEOUT,
            async {
                page.execute(AxEnableParams::default())
                    .await
                    .map_err(|e| format!("ax enable: {e}"))?;
                let mut builder = GetFullAxTreeParams::builder();
                if let Some(fid) = frame_id.clone() {
                    builder = builder.frame_id(fid);
                }
                let resp = page
                    .execute(builder.build())
                    .await
                    .map_err(|e| format!("getFullAXTree: {e}"))?;
                Ok::<_, String>(snapshot::build_lean_snapshot(
                    &resp.result.nodes,
                    interactive,
                ))
            },
            || "snapshot timeout".into(),
        )
        .await
        .map_err(|e| format!("browser_snapshot: {e}"))?;
        self.refs = snap.refs.clone();
        let target = page.target_id().inner().to_string();
        let doc = self.doc_of(&target);
        self.refs_doc = Some((target, doc));
        Ok(snap)
    }

    /// `target` 这一页主框架换过几次文档。
    fn doc_of(&self, target: &str) -> u64 {
        self.docs.lock().unwrap().get(target).copied().unwrap_or(0)
    }

    /// 拍 refs 的那一页换了文档：refs 作废，不拿旧文档的节点号去点新文档。
    fn drop_stale_refs(&mut self) {
        let stale = self
            .refs_doc
            .as_ref()
            .is_some_and(|(target, doc)| self.doc_of(target) != *doc);
        if stale {
            self.refs.clear();
            self.refs_doc = None;
        }
    }

    pub async fn click_ref(&mut self, ref_id: &str) -> Result<String, String> {
        let (key, entry) = self.lookup_ref(ref_id)?;
        let backend = snapshot::backend_id(&entry)
            .ok_or_else(|| format!("ref `{key}` has no backend DOM node"))?;
        let page = self.active_page()?.clone();

        retry_async(3, Duration::from_millis(150), || {
            let page = page.clone();
            async move {
                page.execute(
                    ScrollIntoViewIfNeededParams::builder()
                        .backend_node_id(backend)
                        .build(),
                )
                .await
                .map_err(|e| e.to_string())?;
                let model = page
                    .execute(
                        GetBoxModelParams::builder()
                            .backend_node_id(backend)
                            .build(),
                    )
                    .await
                    .map_err(|e| e.to_string())?
                    .result
                    .model;
                let point =
                    chromiumoxide::layout::ElementQuad::from_quad(&model.content).quad_center();
                page.click(point).await.map_err(|e| e.to_string())?;
                Ok::<_, String>(())
            }
        })
        .await
        .map_err(|e| ref_gone(&key, e))?;
        Ok(format!("clicked @{key} ({})", entry.role))
    }

    pub async fn type_ref(
        &mut self,
        ref_id: Option<&str>,
        text: &str,
        submit: bool,
    ) -> Result<String, String> {
        let page = self.active_page()?.clone();
        if let Some(raw) = ref_id {
            let (key, entry) = self.lookup_ref(raw)?;
            let backend = snapshot::backend_id(&entry)
                .ok_or_else(|| format!("ref `{key}` has no backend DOM node"))?;
            Self::focus_backend(&page, backend)
                .await
                .map_err(|e| ref_gone(&key, e))?;
        }

        page.execute(InsertTextParams::new(text))
            .await
            .map_err(|e| format!("type: {e}"))?;

        if submit {
            // Enter
            let down = DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::KeyDown)
                .key("Enter")
                .code("Enter")
                .windows_virtual_key_code(13)
                .native_virtual_key_code(13)
                .build()
                .map_err(|e| e.to_string())?;
            page.execute(down).await.map_err(|e| e.to_string())?;
            let up = DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::KeyUp)
                .key("Enter")
                .code("Enter")
                .windows_virtual_key_code(13)
                .native_virtual_key_code(13)
                .build()
                .map_err(|e| e.to_string())?;
            page.execute(up).await.map_err(|e| e.to_string())?;
        }

        Ok(format!(
            "typed {} chars{}",
            text.chars().count(),
            if submit { " + Enter" } else { "" }
        ))
    }

    pub async fn screenshot(&self, full_page: bool) -> Result<PathBuf, String> {
        let page = self.active_page()?.clone();
        let dir = browser_screenshot_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("screenshot dir {}: {e}", dir.display()))?;
        let path = dir.join(format!(
            "shot-{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        let params = ScreenshotParams::builder().full_page(full_page).build();
        page.save_screenshot(params, &path)
            .await
            .map_err(|e| format!("screenshot: {e}"))?;
        Ok(path)
    }

    /// Sync-friendly tab rows for the `/browser` cockpit cache.
    pub async fn tab_infos(&self) -> Vec<TabInfo> {
        let mut out = Vec::with_capacity(self.pages.len());
        for (i, p) in self.pages.iter().enumerate() {
            let u = p.url().await.ok().flatten().unwrap_or_default();
            out.push(TabInfo {
                index: i,
                url: u,
                active: i == self.active,
            });
        }
        out
    }

    pub async fn tabs(
        &mut self,
        action: &str,
        index: Option<usize>,
        url: Option<&str>,
    ) -> Result<String, String> {
        match action {
            "list" | "" => {
                let mut lines = Vec::new();
                for (i, p) in self.pages.iter().enumerate() {
                    let u = p.url().await.ok().flatten().unwrap_or_default();
                    let mark = if i == self.active { "*" } else { " " };
                    lines.push(format!("{mark} [{i}] {u}"));
                }
                Ok(if lines.is_empty() {
                    "(no tabs)".into()
                } else {
                    lines.join("\n")
                })
            }
            "new" => {
                let u = url.unwrap_or("about:blank");
                let page = self
                    .browser
                    .new_page(u)
                    .await
                    .map_err(|e| format!("new tab: {e}"))?;
                self.dialog_tasks
                    .push(self.dialog_listener_for(&page).await?);
                self.capture_tasks.extend(
                    Self::attach_page_capture(
                        &page,
                        self.capture.clone(),
                        self.docs.clone(),
                        self.audit.clone(),
                        self.blocked.clone(),
                    )
                    .await?,
                );
                self.pages.push(page);
                self.active = self.pages.len() - 1;
                self.refs.clear();
                Ok(format!("opened tab {} -> {u}", self.active))
            }
            "switch" => {
                let i = index.ok_or_else(|| "switch requires index".to_string())?;
                if i >= self.pages.len() {
                    return Err(format!(
                        "tab index {i} out of range (0..{})",
                        self.pages.len()
                    ));
                }
                let page = &self.pages[i];
                let tid = page.target_id().clone();
                self.browser
                    .execute(ActivateTargetParams::new(tid))
                    .await
                    .map_err(|e| format!("activate tab: {e}"))?;
                let _ = page.bring_to_front().await;
                self.active = i;
                self.refs.clear();
                Ok(format!("switched to tab {i}"))
            }
            "close" => {
                let i = index.unwrap_or(self.active);
                if i >= self.pages.len() {
                    return Err(format!("tab index {i} out of range"));
                }
                if self.pages.len() == 1 {
                    return Err("cannot close the last tab; use browser_close".into());
                }
                let page = self.pages.remove(i);
                let tid = page.target_id().clone();
                let _ = self.browser.execute(CloseTargetParams::new(tid)).await;
                if self.active >= self.pages.len() {
                    self.active = self.pages.len().saturating_sub(1);
                } else if i < self.active {
                    self.active -= 1;
                }
                self.refs.clear();
                Ok(format!("closed tab {i}; active={}", self.active))
            }
            other => Err(format!(
                "unknown tabs action `{other}` (list|new|switch|close)"
            )),
        }
    }

    fn lookup_ref(&mut self, ref_id: &str) -> Result<(String, RefEntry), String> {
        self.drop_stale_refs();
        let key = snapshot::parse_ref(ref_id).ok_or_else(|| "missing ref".to_string())?;
        let entry = self.refs.get(&key).cloned().ok_or_else(|| {
            format!(
                "unknown ref `{key}` — call browser_snapshot first (have {} refs)",
                self.refs.len()
            )
        })?;
        Ok((key, entry))
    }

    async fn focus_backend(
        page: &Page,
        backend: chromiumoxide::cdp::browser_protocol::dom::BackendNodeId,
    ) -> Result<(), String> {
        page.execute(
            ScrollIntoViewIfNeededParams::builder()
                .backend_node_id(backend)
                .build(),
        )
        .await
        .map_err(|e| e.to_string())?;
        page.execute(FocusParams::builder().backend_node_id(backend).build())
            .await
            .map_err(|e| format!("focus: {e}"))?;
        Ok(())
    }

    async fn call_js_on_backend(
        page: &Page,
        backend: chromiumoxide::cdp::browser_protocol::dom::BackendNodeId,
        function_declaration: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, String> {
        let resolved = page
            .execute(
                ResolveNodeParams::builder()
                    .backend_node_id(backend)
                    .build(),
            )
            .await
            .map_err(|e| format!("resolveNode: {e}"))?;
        let object_id = resolved
            .result
            .object
            .object_id
            .ok_or_else(|| "resolveNode returned no objectId".to_string())?;
        let mut builder = CallFunctionOnParams::builder()
            .function_declaration(function_declaration)
            .object_id(object_id)
            .return_by_value(true)
            .await_promise(true)
            .user_gesture(true);
        for a in args {
            builder = builder.argument(CallArgument::builder().value(a).build());
        }
        let call = builder.build().map_err(|e| e.to_string())?;
        let resp = page
            .execute(call)
            .await
            .map_err(|e| format!("callFunctionOn: {e}"))?;
        if let Some(exc) = resp.result.exception_details {
            return Err(format!("js exception: {exc:?}"));
        }
        Ok(resp.result.result.value)
    }

    pub async fn hover_ref(&mut self, ref_id: &str) -> Result<String, String> {
        let (key, entry) = self.lookup_ref(ref_id)?;
        let backend = snapshot::backend_id(&entry)
            .ok_or_else(|| format!("ref `{key}` has no backend DOM node"))?;
        let page = self.active_page()?.clone();
        retry_async(3, Duration::from_millis(150), || {
            let page = page.clone();
            async move {
                page.execute(
                    ScrollIntoViewIfNeededParams::builder()
                        .backend_node_id(backend)
                        .build(),
                )
                .await
                .map_err(|e| e.to_string())?;
                let model = page
                    .execute(
                        GetBoxModelParams::builder()
                            .backend_node_id(backend)
                            .build(),
                    )
                    .await
                    .map_err(|e| e.to_string())?
                    .result
                    .model;
                let point =
                    chromiumoxide::layout::ElementQuad::from_quad(&model.content).quad_center();
                page.move_mouse(point).await.map_err(|e| e.to_string())?;
                Ok::<_, String>(())
            }
        })
        .await
        .map_err(|e| ref_gone(&key, e))?;
        Ok(format!("hovered @{key} ({})", entry.role))
    }

    /// Press a key or chord (`Enter`, `Tab`, `Control+a`, `Meta+Shift+t`).
    pub async fn press_key(&mut self, key: &str, ref_id: Option<&str>) -> Result<String, String> {
        let page = self.active_page()?.clone();
        if let Some(raw) = ref_id {
            let (k, entry) = self.lookup_ref(raw)?;
            let backend = snapshot::backend_id(&entry)
                .ok_or_else(|| format!("ref `{k}` has no backend DOM node"))?;
            Self::focus_backend(&page, backend)
                .await
                .map_err(|e| ref_gone(&k, e))?;
        }
        let (modifiers, key_name) = parse_key_chord(key)?;
        let down = key_event(&key_name, modifiers, KeyPhase::Down).map_err(|e| {
            format!(
                "{e} (from `{key}`). Use names like Enter, Tab, Escape, ArrowDown, a, Control+a"
            )
        })?;
        page.execute(down).await.map_err(|e| e.to_string())?;
        let up = key_event(&key_name, modifiers, KeyPhase::Up)?;
        page.execute(up).await.map_err(|e| e.to_string())?;
        Ok(format!("pressed {key}"))
    }

    pub async fn select_option(
        &mut self,
        ref_id: &str,
        value: Option<&str>,
        label: Option<&str>,
    ) -> Result<String, String> {
        if value.is_none() && label.is_none() {
            return Err("select_option requires value and/or label".into());
        }
        let (key, entry) = self.lookup_ref(ref_id)?;
        let backend = snapshot::backend_id(&entry)
            .ok_or_else(|| format!("ref `{key}` has no backend DOM node"))?;
        let page = self.active_page()?.clone();
        Self::focus_backend(&page, backend)
            .await
            .map_err(|e| ref_gone(&key, e))?;
        let js = r#"function(value, label) {
  const el = this;
  const opts = el.options ? Array.from(el.options) : [];
  let opt = null;
  if (value != null && value !== '') {
    opt = opts.find(o => String(o.value) === String(value));
  }
  if (!opt && label != null && label !== '') {
    const want = String(label);
    opt = opts.find(o => String(o.label) === want || String(o.textContent).trim() === want);
  }
  if (!opt) {
    return { ok: false, error: 'option not found' };
  }
  el.value = opt.value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true, value: String(opt.value), label: String(opt.label || opt.textContent || '').trim() };
}"#;
        let args = vec![
            value
                .map(|s| serde_json::Value::String(s.to_string()))
                .unwrap_or(serde_json::Value::Null),
            label
                .map(|s| serde_json::Value::String(s.to_string()))
                .unwrap_or(serde_json::Value::Null),
        ];
        let out = Self::call_js_on_backend(&page, backend, js, args).await?;
        let ok = out
            .as_ref()
            .and_then(|v| v.get("ok"))
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        if !ok {
            let err = out
                .as_ref()
                .and_then(|v| v.get("error"))
                .and_then(|x| x.as_str())
                .unwrap_or("select failed");
            return Err(format!("{err} for @{key}"));
        }
        let selected = out
            .as_ref()
            .and_then(|v| v.get("value"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        Ok(format!("selected `{selected}` on @{key}"))
    }

    pub async fn fill_form(&mut self, fields: &[(String, String)]) -> Result<String, String> {
        if fields.is_empty() {
            return Err("fields must be a non-empty array of {ref, value}".into());
        }
        let page = self.active_page()?.clone();
        let mut filled = Vec::new();
        let js = r#"function(value) {
  const el = this;
  const v = value == null ? '' : String(value);
  if (el.isContentEditable) {
    el.textContent = v;
  } else if ('value' in el) {
    el.value = v;
  } else {
    return { ok: false, error: 'element is not fillable' };
  }
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true };
}"#;
        for (ref_id, value) in fields {
            let (key, entry) = self.lookup_ref(ref_id)?;
            let backend = snapshot::backend_id(&entry)
                .ok_or_else(|| format!("ref `{key}` has no backend DOM node"))?;
            Self::focus_backend(&page, backend)
                .await
                .map_err(|e| ref_gone(&key, e))?;
            let out = Self::call_js_on_backend(
                &page,
                backend,
                js,
                vec![serde_json::Value::String(value.clone())],
            )
            .await
            .map_err(|e| ref_gone(&key, e))?;
            let ok = out
                .as_ref()
                .and_then(|v| v.get("ok"))
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            if !ok {
                let err = out
                    .as_ref()
                    .and_then(|v| v.get("error"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("fill failed");
                return Err(format!("{err} for @{key}"));
            }
            filled.push(format!("@{key}"));
        }
        Ok(format!(
            "filled {} field(s): {}",
            filled.len(),
            filled.join(", ")
        ))
    }

    pub async fn wait_for(
        &mut self,
        text: Option<&str>,
        selector: Option<&str>,
        timeout_ms: Option<u64>,
    ) -> Result<String, String> {
        let timeout = Duration::from_millis(timeout_ms.unwrap_or(30_000).max(1));
        let page = self.active_page()?.clone();
        let text = text.map(|s| s.to_string()).filter(|s| !s.is_empty());
        let selector = selector.map(|s| s.to_string()).filter(|s| !s.is_empty());

        if text.is_none() && selector.is_none() {
            tokio::time::sleep(timeout).await;
            return Ok(format!("waited {}ms", timeout.as_millis()));
        }

        let text_c = text.clone();
        let sel_c = selector.clone();
        poll_until(timeout, Duration::from_millis(100), || {
            let page = page.clone();
            let text_c = text_c.clone();
            let sel_c = sel_c.clone();
            async move {
                if let Some(sel) = sel_c.as_ref() {
                    let expr = format!(
                        "!!document.querySelector({})",
                        serde_json::to_string(sel).unwrap_or_else(|_| "null".into())
                    );
                    let found: bool = page
                        .evaluate(expr.as_str())
                        .await
                        .map_err(|e| format!("selector eval: {e}"))?
                        .into_value()
                        .map_err(|e| format!("selector value: {e}"))?;
                    if !found {
                        return Ok(false);
                    }
                }
                if let Some(t) = text_c.as_ref() {
                    let body: String = page
                        .evaluate(
                            "(() => (document.body && (document.body.innerText || document.body.textContent)) || '')()",
                        )
                        .await
                        .map_err(|e| format!("text eval: {e}"))?
                        .into_value()
                        .map_err(|e| format!("text value: {e}"))?;
                    if !body.contains(t) {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        })
        .await
        .map_err(|e| {
            let mut parts = Vec::new();
            if let Some(t) = &text {
                parts.push(format!("text={t:?}"));
            }
            if let Some(s) = &selector {
                parts.push(format!("selector={s:?}"));
            }
            format!("{e} ({})", parts.join(", "))
        })?;

        let mut msg = String::from("wait satisfied");
        if let Some(t) = text {
            msg.push_str(&format!(" text={t:?}"));
        }
        if let Some(s) = selector {
            msg.push_str(&format!(" selector={s:?}"));
        }
        Ok(msg)
    }

    pub async fn navigate_back(&mut self) -> Result<String, String> {
        let page = self.active_page()?.clone();
        let hist = page
            .execute(GetNavigationHistoryParams {})
            .await
            .map_err(|e| format!("getNavigationHistory: {e}"))?
            .result;
        let idx = hist.current_index;
        if idx <= 0 {
            return Err("no previous history entry".into());
        }
        let entry = hist
            .entries
            .get(idx as usize - 1)
            .ok_or_else(|| "no previous history entry".to_string())?;
        with_timeout(
            NAV_TIMEOUT,
            async {
                page.execute(NavigateToHistoryEntryParams::new(entry.id))
                    .await
                    .map_err(|e| format!("navigateToHistoryEntry: {e}"))?;
                Ok::<_, String>(())
            },
            || "navigate_back timeout".into(),
        )
        .await?;
        self.refs.clear();
        // Give the page a moment; URL may still be settling.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let cur = page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| entry.url.clone());
        Ok(format!("navigated back to {cur}"))
    }

    /// 关掉本组自己的标签页；Chromium 进程归 [`Chromium`] 管。
    pub async fn shutdown(mut self) {
        self.refs.clear();
        for t in self.dialog_tasks.drain(..) {
            t.abort();
        }
        for t in self.capture_tasks.drain(..) {
            t.abort();
        }
        for page in self.pages.drain(..) {
            let tid = page.target_id().clone();
            let _ = self.browser.execute(CloseTargetParams::new(tid)).await;
        }
    }

    async fn point_for_backend(
        page: &Page,
        backend: chromiumoxide::cdp::browser_protocol::dom::BackendNodeId,
    ) -> Result<Point, String> {
        page.execute(
            ScrollIntoViewIfNeededParams::builder()
                .backend_node_id(backend)
                .build(),
        )
        .await
        .map_err(|e| e.to_string())?;
        let model = page
            .execute(
                GetBoxModelParams::builder()
                    .backend_node_id(backend)
                    .build(),
            )
            .await
            .map_err(|e| e.to_string())?
            .result
            .model;
        Ok(chromiumoxide::layout::ElementQuad::from_quad(&model.content).quad_center())
    }

    async fn resolve_point(
        &mut self,
        page: &Page,
        ref_id: Option<&str>,
        x: Option<f64>,
        y: Option<f64>,
        which: &str,
    ) -> Result<(Point, String), String> {
        if let (Some(x), Some(y)) = (x, y) {
            return Ok((Point { x, y }, format!("coords ({x},{y})")));
        }
        let raw = ref_id.ok_or_else(|| format!("{which} requires ref or x/y coordinates"))?;
        let (key, entry) = self.lookup_ref(raw)?;
        let backend = snapshot::backend_id(&entry)
            .ok_or_else(|| format!("ref `{key}` has no backend DOM node"))?;
        let point = Self::point_for_backend(page, backend)
            .await
            .map_err(|e| ref_gone(&key, e))?;
        Ok((point, format!("@{key}")))
    }

    /// Drag from source ref/coords to target ref/coords via CDP mouse events.
    #[allow(clippy::too_many_arguments)] // 绘制 / 布局 / 注册参数天然多，抽结构体只是把参数搬个家，留给需要时再拆
    pub async fn drag(
        &mut self,
        source_ref: Option<&str>,
        target_ref: Option<&str>,
        start_x: Option<f64>,
        start_y: Option<f64>,
        end_x: Option<f64>,
        end_y: Option<f64>,
        steps: Option<u32>,
    ) -> Result<String, String> {
        let page = self.active_page()?.clone();
        let (start, start_label) = self
            .resolve_point(&page, source_ref, start_x, start_y, "drag source")
            .await?;
        let (end, end_label) = self
            .resolve_point(&page, target_ref, end_x, end_y, "drag target")
            .await?;
        let steps = steps.unwrap_or(10).max(1) as i64;

        // Move to start, press, move with button held, release.
        page.move_mouse(start)
            .await
            .map_err(|e| format!("drag move start: {e}"))?;
        let press = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MousePressed)
            .x(start.x)
            .y(start.y)
            .button(MouseButton::Left)
            .buttons(1)
            .click_count(1)
            .build()
            .map_err(|e| e.to_string())?;
        page.execute(press)
            .await
            .map_err(|e| format!("drag press: {e}"))?;

        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let x = start.x + (end.x - start.x) * t;
            let y = start.y + (end.y - start.y) * t;
            let mv = DispatchMouseEventParams::builder()
                .r#type(DispatchMouseEventType::MouseMoved)
                .x(x)
                .y(y)
                .button(MouseButton::Left)
                .buttons(1)
                .build()
                .map_err(|e| e.to_string())?;
            page.execute(mv)
                .await
                .map_err(|e| format!("drag move: {e}"))?;
        }

        let release = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MouseReleased)
            .x(end.x)
            .y(end.y)
            .button(MouseButton::Left)
            .buttons(0)
            .click_count(1)
            .build()
            .map_err(|e| e.to_string())?;
        page.execute(release)
            .await
            .map_err(|e| format!("drag release: {e}"))?;
        Ok(format!("dragged from {start_label} to {end_label}"))
    }

    /// Accept or dismiss a pending JS dialog (alert/confirm/prompt/beforeunload).
    pub async fn handle_dialog(
        &mut self,
        accept: bool,
        prompt_text: Option<&str>,
    ) -> Result<String, String> {
        let pending = self.pending_dialog();
        // 对话框在哪一页就处理哪一页（不一定是活动页）。
        let page = match pending.as_ref().and_then(|d| {
            self.pages
                .iter()
                .find(|p| p.target_id().as_ref() == d.target.as_str())
        }) {
            Some(p) => p.clone(),
            None => self.active_page()?.clone(),
        };
        let mut params = HandleJavaScriptDialogParams::new(accept);
        if let Some(t) = prompt_text {
            params.prompt_text = Some(t.to_string());
        }
        page.execute(params)
            .await
            .map_err(|e| format!("handleJavaScriptDialog: {e}"))?;
        if let Ok(mut g) = self.pending_dialog.lock() {
            *g = None;
        }
        let action = if accept { "accepted" } else { "dismissed" };
        match pending {
            Some(d) => {
                let mut out = format!(
                    "{action} {dtype} dialog: {msg}",
                    dtype = d.dialog_type,
                    msg = d.message
                );
                if let Some(p) = d.default_prompt.as_ref().filter(|s| !s.is_empty()) {
                    out.push_str(&format!(" (default_prompt={p:?})"));
                }
                Ok(out)
            }
            None => Ok(format!("{action} dialog (no tracked pending event)")),
        }
    }

    /// Set files on an `<input type=file>` identified by snapshot ref.
    pub async fn file_upload(&mut self, ref_id: &str, paths: &[String]) -> Result<String, String> {
        if paths.is_empty() {
            return Err("paths must be a non-empty array of file paths".into());
        }
        let mut abs = Vec::with_capacity(paths.len());
        for p in paths {
            let pb = PathBuf::from(p);
            if !pb.is_file() {
                return Err(format!("file not found: {p}"));
            }
            abs.push(
                pb.canonicalize()
                    .map_err(|e| format!("canonicalize {p}: {e}"))?
                    .display()
                    .to_string(),
            );
        }
        let (key, entry) = self.lookup_ref(ref_id)?;
        let backend = snapshot::backend_id(&entry)
            .ok_or_else(|| format!("ref `{key}` has no backend DOM node"))?;
        let page = self.active_page()?.clone();
        let params = SetFileInputFilesParams::builder()
            .files(abs.clone())
            .backend_node_id(backend)
            .build()
            .map_err(|e| e.to_string())?;
        page.execute(params)
            .await
            .map_err(|e| ref_gone(&key, format!("setFileInputFiles: {e}")))?;
        Ok(format!(
            "uploaded {} file(s) to @{key}: {}",
            abs.len(),
            abs.join(", ")
        ))
    }

    /// Override viewport size via Emulation.setDeviceMetricsOverride.
    pub async fn resize(&mut self, width: i64, height: i64) -> Result<String, String> {
        if width <= 0 || height <= 0 {
            return Err("width and height must be positive integers".into());
        }
        if width > 10_000_000 || height > 10_000_000 {
            return Err("width/height out of CDP range".into());
        }
        let page = self.active_page()?.clone();
        page.execute(SetDeviceMetricsOverrideParams::new(
            width, height, 1.0, false,
        ))
        .await
        .map_err(|e| format!("setDeviceMetricsOverride: {e}"))?;
        Ok(format!("resized viewport to {width}x{height}"))
    }

    /// Run JS in the page (or optional same-origin iframe) and return a truncated string/JSON.
    pub async fn evaluate(
        &mut self,
        expression: &str,
        frame_selector: Option<&str>,
    ) -> Result<String, String> {
        if expression.trim().is_empty() {
            return Err("expression is required".into());
        }
        let page = self.active_page()?.clone();
        let frame_id = self.resolve_frame_id(&page, frame_selector).await?;
        let mut builder = EvaluateParams::builder()
            .expression(expression)
            .return_by_value(true)
            .await_promise(true)
            .user_gesture(true);
        if let Some(fid) = frame_id {
            let ctx = page
                .frame_execution_context(fid.clone())
                .await
                .map_err(|e| format!("frame execution context: {e}"))?
                .ok_or_else(|| {
                    "no execution context for frame (cross-origin or not ready)".to_string()
                })?;
            builder = builder.context_id(ctx);
        }
        let params = builder.build().map_err(|e| e.to_string())?;
        let result = with_timeout(
            ACTION_TIMEOUT,
            async {
                page.evaluate(params)
                    .await
                    .map_err(|e| format!("evaluate: {e}"))
            },
            || "evaluate timeout".into(),
        )
        .await
        .map_err(|e| format!("browser_evaluate: {e}"))?;

        let obj = result.object();
        let rendered = if let Some(v) = obj.value.as_ref() {
            match v {
                serde_json::Value::String(s) => s.clone(),
                other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
            }
        } else if let Some(d) = obj.description.as_ref() {
            d.clone()
        } else {
            "undefined".into()
        };
        Ok(truncate_str(&rendered, EVAL_RESULT_MAX))
    }

    pub async fn console_messages(&self) -> Result<String, String> {
        let g = self.capture.lock().await;
        if g.console.is_empty() {
            return Ok("(no console messages captured yet)".into());
        }
        let lines: Vec<String> = g
            .console
            .iter()
            .map(|e| format!("{}: {}", e.level, e.text))
            .collect();
        Ok(lines.join("\n"))
    }

    pub async fn network_requests(&self) -> Result<String, String> {
        let g = self.capture.lock().await;
        if g.network.is_empty() {
            return Ok("(no network requests captured yet)".into());
        }
        let lines: Vec<String> = g
            .network
            .iter()
            .map(|e| {
                let status = e
                    .status
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "-".into());
                let ty = if e.resource_type.is_empty() {
                    "-"
                } else {
                    e.resource_type.as_str()
                };
                format!("{} {} {} {}", e.method, e.url, status, ty)
            })
            .collect();
        Ok(lines.join("\n"))
    }

    /// Resolve optional CSS `frame_selector` to a CDP FrameId. Main frame when None.
    /// Cross-origin / unlocatable iframes fail with a clear error.
    async fn resolve_frame_id(
        &self,
        page: &Page,
        frame_selector: Option<&str>,
    ) -> Result<Option<FrameId>, String> {
        let Some(sel) = frame_selector.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        let sel_json = serde_json::to_string(sel).unwrap_or_else(|_| "\"\"".into());
        let probe = format!(
            r#"(function() {{
  const el = document.querySelector({sel_json});
  if (!el) return {{ error: 'no element matching frame_selector' }};
  const tag = String(el.tagName || '').toUpperCase();
  if (tag !== 'IFRAME' && tag !== 'FRAME') {{
    return {{ error: 'frame_selector did not match an iframe/frame' }};
  }}
  try {{
    void el.contentDocument;
  }} catch (e) {{
    return {{
      error: 'cross-origin iframe (CDP cannot enter)',
      crossOrigin: true,
      src: String(el.src || ''),
      name: String(el.name || '')
    }};
  }}
  return {{
    ok: true,
    src: String(el.src || ''),
    name: String(el.name || '')
  }};
}})()"#
        );
        let probed = page
            .evaluate(probe.as_str())
            .await
            .map_err(|e| format!("frame probe: {e}"))?;
        let val = probed
            .value()
            .cloned()
            .ok_or_else(|| "frame probe returned no value".to_string())?;
        if let Some(err) = val.get("error").and_then(|x| x.as_str()) {
            return Err(err.into());
        }
        let name = val
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let src = val
            .get("src")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();

        let tree = page
            .execute(GetFrameTreeParams {})
            .await
            .map_err(|e| format!("getFrameTree: {e}"))?
            .result
            .frame_tree;
        let mut frames = Vec::new();
        flatten_frame_tree(&tree, &mut frames);
        // Prefer named child frames; skip the root (no parent).
        let children: Vec<&_> = frames.iter().filter(|f| f.parent_id.is_some()).collect();
        if let Some(found) = children.iter().find(|f| {
            (!name.is_empty() && f.name.as_deref() == Some(name.as_str()))
                || (!src.is_empty()
                    && (f.url == src
                        || f.url.ends_with(src.trim_start_matches('/'))
                        || src.ends_with(f.url.trim_start_matches('/'))))
        }) {
            return Ok(Some(found.id.clone()));
        }
        if children.len() == 1 && name.is_empty() && src.is_empty() {
            return Ok(Some(children[0].id.clone()));
        }
        // Same-origin but unmatched: if exactly one child, use it when selector matched.
        if children.len() == 1 {
            return Ok(Some(children[0].id.clone()));
        }
        Err(format!(
            "iframe matched selector but could not locate CDP frame (name={name:?} src={src:?}; {} child frame(s))",
            children.len()
        ))
    }
}

/// 审计时记哪些响应的正文（页面文档、XHR、fetch）；图片、字体、脚本只记大小。
const AUDIT_BODY_TYPES: &[ResourceType] = &[
    ResourceType::Document,
    ResourceType::Xhr,
    ResourceType::Fetch,
];
/// 比这大的响应不去取正文（只记大小）。
const AUDIT_BODY_FETCH_MAX: f64 = 4.0 * 1024.0 * 1024.0;

/// 审计开着时把这一页的网络请求记成 [`dock_sandbox::audit::record::Flow`]（`tag = browser:<标签页>`）。
/// 浏览器里看到的是明文：HTTPS 的网址、头、正文都在，`decrypted` 记成 true。审计关着时
/// 只看一眼那一格就跳过。
///
/// WebSocket 不走上面那几个事件（CDP 单独报 `webSocket*`）：握手记一条 `net`（`ws` / `wss`、
/// 101），之后的帧交给 [`dock_sandbox::audit::ws::Recorder`] 按段记（CDP 给的是解压好的内容，
/// 字节数按内容算）。
async fn audit_network(page: &Page, slot: AuditSlot) -> Option<JoinHandle<()>> {
    use dock_sandbox::audit::record::{header_pairs, now_ms, Captured, Flow, Message};
    use dock_sandbox::audit::ws::Dir;
    let mut sent = page.event_listener::<EventRequestWillBeSent>().await.ok()?;
    let mut answered = page.event_listener::<EventResponseReceived>().await.ok()?;
    let mut finished = page.event_listener::<EventLoadingFinished>().await.ok()?;
    let mut failed = page.event_listener::<EventLoadingFailed>().await.ok()?;
    let mut ws_created = page.event_listener::<EventWebSocketCreated>().await.ok()?;
    let mut ws_request = page
        .event_listener::<EventWebSocketWillSendHandshakeRequest>()
        .await
        .ok()?;
    let mut ws_response = page
        .event_listener::<EventWebSocketHandshakeResponseReceived>()
        .await
        .ok()?;
    let mut ws_sent = page
        .event_listener::<EventWebSocketFrameSent>()
        .await
        .ok()?;
    let mut ws_received = page
        .event_listener::<EventWebSocketFrameReceived>()
        .await
        .ok()?;
    let mut ws_error = page
        .event_listener::<EventWebSocketFrameError>()
        .await
        .ok()?;
    let mut ws_closed = page.event_listener::<EventWebSocketClosed>().await.ok()?;
    let page = page.clone();
    let tag = format!("browser:{}", page.target_id().inner());
    Some(tokio::spawn(async move {
        let cfg = cordis_base::config::load_sandbox_config();
        let cap = (cfg.body_cap_kb as usize) * 1024;
        let redact = cfg.redact_secrets;
        let headers = |h: &chromiumoxide::cdp::browser_protocol::network::Headers| {
            let pairs = h
                .inner()
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                v.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| v.to_string()),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            header_pairs(pairs, redact)
        };
        let mut pending: HashMap<String, (Flow, Option<ResourceType>, std::time::Instant)> =
            HashMap::new();
        let mut sockets = WsSockets::new(tag.clone(), cap, slot.clone());
        loop {
            // 每种事件各走一条通道，跨通道的先后丢了；chromiumoxide 按到达顺序往各通道里放，
            // 所以同时有好几条就绪时按生命周期从前往后取（请求先于响应、握手先于帧），
            // 不然会先处理响应 / 帧、找不到还没登记的请求 / 连接而丢掉。
            tokio::select! {
                biased;
                Some(ev) = sent.next() => {
                    let on = slot.lock().unwrap().is_some();
                    if !on {
                        pending.clear();
                        continue;
                    }
                    let Some((scheme, host, port, path)) = split_url(&ev.request.url) else { continue };
                    let mut body = Captured::default();
                    for entry in ev.request.post_data_entries.iter().flatten() {
                        if let Some(bytes) = entry.bytes.as_ref() {
                            use base64::Engine;
                            if let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(AsRef::<str>::as_ref(bytes)) {
                                body.push(&raw, cap);
                            }
                        }
                    }
                    let flow = Flow {
                        tag: tag.clone(),
                        kind: "http",
                        scheme,
                        host,
                        port,
                        method: Some(ev.request.method.clone()),
                        path: Some(path),
                        status: None,
                        request: Some(Message { headers: headers(&ev.request.headers), body }),
                        response: None,
                        bytes_up: 0,
                        bytes_down: 0,
                        started_ms: now_ms(),
                        duration_ms: 0,
                        decrypted: true,
                        note: ev.r#type.as_ref().map(|t| t.as_ref().to_string()),
                        error: None,
                        ws: None,
                    };
                    let id = ev.request_id.inner().to_string();
                    // 重定向：同一个请求 id 又来一次，前一跳照样记一条。
                    if let Some((mut prev, _, t0)) = pending.insert(id, (flow, ev.r#type.clone(), std::time::Instant::now())) {
                        prev.duration_ms = t0.elapsed().as_millis() as u64;
                        prev.note = Some("重定向".into());
                        if let Some(log) = slot.lock().unwrap().clone() {
                            log.flow(prev);
                        }
                    }
                }
                Some(ev) = answered.next() => {
                    if let Some((flow, _, _)) = pending.get_mut(ev.request_id.inner()) {
                        flow.status = u16::try_from(ev.response.status).ok();
                        flow.response = Some(Message { headers: headers(&ev.response.headers), body: Captured::default() });
                        if let Some(ip) = ev.response.remote_ip_address.as_ref() {
                            let kind = flow.note.take().unwrap_or_default();
                            flow.note = Some(format!("{kind} {ip}").trim().to_string());
                        }
                    }
                }
                Some(ev) = finished.next() => {
                    let Some((mut flow, kind, t0)) = pending.remove(ev.request_id.inner()) else { continue };
                    flow.bytes_down = ev.encoded_data_length.max(0.0) as u64;
                    flow.duration_ms = t0.elapsed().as_millis() as u64;
                    let wants_body = cap > 0
                        && kind.as_ref().is_some_and(|k| AUDIT_BODY_TYPES.contains(k))
                        && ev.encoded_data_length <= AUDIT_BODY_FETCH_MAX;
                    if wants_body {
                        if let Ok(Ok(r)) = tokio::time::timeout(
                            Duration::from_secs(5),
                            page.execute(GetResponseBodyParams::new(ev.request_id.clone())),
                        )
                        .await
                        {
                            let raw = if r.result.base64_encoded {
                                use base64::Engine;
                                base64::engine::general_purpose::STANDARD.decode(&r.result.body).unwrap_or_default()
                            } else {
                                r.result.body.clone().into_bytes()
                            };
                            if let Some(m) = flow.response.as_mut() {
                                m.body.push(&raw, cap);
                            }
                        }
                    }
                    if let Some(log) = slot.lock().unwrap().clone() {
                        log.flow(flow);
                    }
                }
                Some(ev) = failed.next() => {
                    let Some((mut flow, _, t0)) = pending.remove(ev.request_id.inner()) else { continue };
                    flow.duration_ms = t0.elapsed().as_millis() as u64;
                    flow.error = Some(ev.error_text.clone());
                    if let Some(log) = slot.lock().unwrap().clone() {
                        log.flow(flow);
                    }
                }
                Some(ev) = ws_created.next() => sockets.created(ev.request_id.inner(), &ev.url),
                Some(ev) = ws_request.next() => sockets.request(ev.request_id.inner(), headers(&ev.request.headers)),
                Some(ev) = ws_response.next() => {
                    let r = &ev.response;
                    let asked = r.request_headers.as_ref().map(&headers);
                    sockets.response(ev.request_id.inner(), r.status, &r.status_text, headers(&r.headers), asked, *ev.timestamp.inner());
                }
                Some(ev) = ws_sent.next() => sockets.frame(ev.request_id.inner(), Dir::Up, &ev.response, *ev.timestamp.inner()),
                Some(ev) = ws_received.next() => sockets.frame(ev.request_id.inner(), Dir::Down, &ev.response, *ev.timestamp.inner()),
                Some(ev) = ws_error.next() => sockets.error(ev.request_id.inner(), &ev.error_message),
                Some(ev) = ws_closed.next() => sockets.closed(ev.request_id.inner()),
                else => break,
            }
        }
    }))
}

/// 一页上的 WebSocket（CDP 的 `webSocket*` 事件）记成 [`dock_sandbox::audit::ws`]：握手一回来
/// 记握手那条、开 [`Recorder`]，之后的帧交给它。各种事件走不同的通道、先后会乱：赶在握手回来
/// 之前到的帧先攒着，开了 Recorder 再按顺序补上。帧的时间用 CDP 自己的时间戳（从握手回来算）。
struct WsSockets {
    tag: String,
    cap: usize,
    slot: AuditSlot,
    map: HashMap<String, Sock>,
}

enum Sock {
    /// 握手还没回来：握手那条、什么时候开始的、先到的帧（方向、操作码、内容、CDP 时间）。
    Pending(
        Box<dock_sandbox::audit::record::Flow>,
        std::time::Instant,
        Vec<(dock_sandbox::audit::ws::Dir, u8, Vec<u8>, f64)>,
    ),
    /// 连上了：记录器和握手回来时的 CDP 时间。
    Open(Arc<dock_sandbox::audit::ws::Recorder>, f64),
}

impl WsSockets {
    fn new(tag: String, cap: usize, slot: AuditSlot) -> Self {
        Self {
            tag,
            cap,
            slot,
            map: HashMap::new(),
        }
    }

    fn emit(&self, flow: dock_sandbox::audit::record::Flow) {
        let log = self.slot.lock().unwrap().clone();
        if let Some(log) = log {
            log.flow(flow);
        }
    }

    fn created(&mut self, id: &str, url: &str) {
        if self.slot.lock().unwrap().is_none() {
            return;
        }
        let Some((scheme, host, port, path)) = split_url(url) else {
            return;
        };
        let flow = dock_sandbox::audit::record::Flow {
            tag: self.tag.clone(),
            kind: "http",
            scheme,
            host,
            port,
            method: Some("GET".into()),
            path: Some(path),
            status: None,
            request: None,
            response: None,
            bytes_up: 0,
            bytes_down: 0,
            started_ms: dock_sandbox::audit::record::now_ms(),
            duration_ms: 0,
            decrypted: true,
            note: None,
            error: None,
            ws: None,
        };
        self.map.insert(
            id.to_string(),
            Sock::Pending(Box::new(flow), std::time::Instant::now(), Vec::new()),
        );
    }

    fn request(&mut self, id: &str, headers: Vec<(String, String)>) {
        if let Some(Sock::Pending(flow, _, _)) = self.map.get_mut(id) {
            flow.request = Some(message(headers));
        }
    }

    fn response(
        &mut self,
        id: &str,
        status: i64,
        status_text: &str,
        headers: Vec<(String, String)>,
        asked: Option<Vec<(String, String)>>,
        ts: f64,
    ) {
        use dock_sandbox::audit::ws::{self, Recorder};
        let Some(Sock::Pending(mut flow, t0, early)) = self.map.remove(id) else {
            return;
        };
        flow.status = u16::try_from(status).ok();
        flow.response = Some(message(headers));
        if flow.request.is_none() {
            flow.request = asked.map(message);
        }
        flow.duration_ms = t0.elapsed().as_millis() as u64;
        if status != 101 {
            flow.error = Some(
                format!("握手没成：{status} {status_text}")
                    .trim()
                    .to_string(),
            );
            return self.emit(*flow);
        }
        let conn = ws::conn_id();
        flow.ws = Some(ws::Ws {
            conn: conn.clone(),
            part: None,
        });
        let slot = self.slot.clone();
        let sink: ws::Sink = Arc::new(move |flow| {
            let log = slot.lock().unwrap().clone();
            if let Some(log) = log {
                log.flow(flow);
            }
        });
        let rec = Recorder::start(
            sink,
            conn,
            self.tag.clone(),
            flow.scheme,
            flow.host.clone(),
            flow.port,
            true,
            self.cap,
        );
        self.emit(*flow);
        for (dir, op, raw, at) in early {
            ws::record_frame(&rec, dir, op, &raw, self.cap, Some(since(ts, at)));
        }
        self.map.insert(id.to_string(), Sock::Open(rec, ts));
    }

    fn frame(&mut self, id: &str, dir: dock_sandbox::audit::ws::Dir, f: &WebSocketFrame, ts: f64) {
        let op = f.opcode as u8;
        // 文本帧给的是字符串，其余是 base64。
        let raw = if op == 1 {
            f.payload_data.clone().into_bytes()
        } else {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(&f.payload_data)
                .unwrap_or_default()
        };
        match self.map.get_mut(id) {
            Some(Sock::Open(rec, base)) => dock_sandbox::audit::ws::record_frame(
                rec,
                dir,
                op,
                &raw,
                self.cap,
                Some(since(*base, ts)),
            ),
            Some(Sock::Pending(_, _, early)) => early.push((dir, op, raw, ts)),
            None => {}
        }
    }

    fn error(&mut self, id: &str, error: &str) {
        match self.map.get_mut(id) {
            Some(Sock::Open(rec, _)) => rec.error(error),
            Some(Sock::Pending(flow, _, _)) => flow.error = Some(error.to_string()),
            None => {}
        }
    }

    fn closed(&mut self, id: &str) {
        match self.map.remove(id) {
            Some(Sock::Open(rec, _)) => rec.finish(None),
            Some(Sock::Pending(mut flow, t0, _)) => {
                flow.duration_ms = t0.elapsed().as_millis() as u64;
                flow.error.get_or_insert_with(|| "握手没完成就关了".into());
                self.emit(*flow);
            }
            None => {}
        }
    }
}

fn message(headers: Vec<(String, String)>) -> dock_sandbox::audit::record::Message {
    dock_sandbox::audit::record::Message {
        headers,
        body: Default::default(),
    }
}

/// CDP 时间（秒）`from` 到 `to` 过了多少毫秒；先到的帧可能比握手还早一点，算 0。
fn since(from: f64, to: f64) -> u64 {
    ((to - from) * 1000.0).max(0.0) as u64
}

/// `https://host:port/path?q` → (`https`, host, port, `/path?q`)。`data:` / `blob:` 这类不算出网。
fn split_url(url: &str) -> Option<(&'static str, String, u16, String)> {
    const SCHEMES: &[(&str, &str, u16)] = &[
        ("https://", "https", 443),
        ("http://", "http", 80),
        ("wss://", "wss", 443),
        ("ws://", "ws", 80),
    ];
    let (scheme, rest, default) = SCHEMES
        .iter()
        .find_map(|(prefix, scheme, port)| url.strip_prefix(prefix).map(|r| (*scheme, r, *port)))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, rest) = v6.split_once(']')?;
        (
            h.to_string(),
            rest.strip_prefix(':')
                .and_then(|p| p.parse().ok())
                .unwrap_or(default),
        )
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().unwrap_or(default)),
            None => (authority.to_string(), default),
        }
    };
    Some((scheme, host, port, path))
}

/// 跳到 `url`，到 DOMContentLoaded 就算到了；之后最多再等 [`LOAD_GRACE`] 让 `load` 收尾。
///
/// 不经 chromiumoxide 发 `Page.navigate`：它每页只有一个「进行中的跳转」，新的跳转排在上一次
/// 后面，要等上一次 `load` 完（或者等满它自己的 30 秒请求超时，错误页就是这样）才发出去——
/// 上一页一张慢图就能把下一次跳转拖住好几秒。这里另开一条到这一页的 CDP 连接发命令
/// （[`navigate_direct`]）：它在跳转提交时就回话，带着 `loaderId` / `errorText`；之后按
/// chromiumoxide 收到的 lifecycle 事件认这一次跳转的文档。
async fn goto_ready(page: &Page, browser_ws: &str, url: &str) -> Result<(), String> {
    let main = page
        .execute(GetFrameTreeParams::default())
        .await
        .map_err(|e| format!("getFrameTree: {e}"))?
        .result
        .frame_tree
        .frame
        .id;
    let listen = |e: chromiumoxide::error::CdpError| format!("listen: {e}");
    let mut lifecycle = page
        .event_listener::<EventLifecycleEvent>()
        .await
        .map_err(listen)?;
    let mut navigated = page
        .event_listener::<EventFrameNavigated>()
        .await
        .map_err(listen)?;
    let page_ws = page_ws_url(browser_ws, page.target_id().inner())
        .ok_or_else(|| format!("unexpected CDP address {browser_ws}"))?;
    let sent = navigate_direct(&page_ws, url).await?;
    if let Some(e) = sent.error_text {
        return Err(e);
    }
    // 没有 loaderId：页内跳转（只改了锚点），没有新文档要等。
    let Some(mut want) = sent.loader_id else {
        return Ok(());
    };
    // 等这份文档的 DOMContentLoaded。中途被重定向成别的文档（`location.replace`、meta
    // refresh）就改等最新提交的那份。
    loop {
        tokio::select! {
            Some(ev) = navigated.next() => {
                if ev.frame.id == main {
                    want = ev.frame.loader_id.inner().to_string();
                }
            }
            Some(ev) = lifecycle.next() => {
                if ev.frame_id == main && ev.name == "DOMContentLoaded" && ev.loader_id.inner() == &want {
                    break;
                }
            }
            else => return Err("CDP event stream closed".into()),
        }
    }
    let grace = tokio::time::sleep(LOAD_GRACE);
    tokio::pin!(grace);
    loop {
        tokio::select! {
            Some(ev) = lifecycle.next() => {
                if ev.frame_id == main && ev.name == "load" && ev.loader_id.inner() == &want {
                    return Ok(());
                }
            }
            _ = &mut grace => return Ok(()),
        }
    }
}

/// `ws://host:port/devtools/browser/<id>` → 某一页的 `ws://host:port/devtools/page/<target>`。
pub fn page_ws_url(browser_ws: &str, target: &str) -> Option<String> {
    let (base, _) = browser_ws.split_once("/devtools/browser/")?;
    Some(format!("{base}/devtools/page/{target}"))
}

struct Navigated {
    loader_id: Option<String>,
    error_text: Option<String>,
}

/// 在一条单独的 CDP 连接上发 `Page.navigate`，等它回话（跳转提交、或者失败）就断开。
/// 断开不会取消跳转（跳转是浏览器侧的状态）。
async fn navigate_direct(page_ws: &str, url: &str) -> Result<Navigated, String> {
    use async_tungstenite::tungstenite::Message;
    let (mut ws, _) = async_tungstenite::tokio::connect_async(page_ws)
        .await
        .map_err(|e| format!("connect {page_ws}: {e}"))?;
    let msg = serde_json::json!({
        "id": 1,
        "method": "Page.navigate",
        // typed：和在地址栏里输入一样（不带当前页的 Referer）。
        "params": { "url": url, "transitionType": "typed" },
    });
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .map_err(|e| format!("Page.navigate: {e}"))?;
    while let Some(frame) = ws.next().await {
        let frame = frame.map_err(|e| format!("Page.navigate: {e}"))?;
        let Message::Text(text) = frame else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if v.get("id").and_then(|i| i.as_u64()) != Some(1) {
            continue;
        }
        let _ = ws.close(None).await;
        if let Some(err) = v.get("error") {
            return Err(err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Page.navigate failed")
                .to_string());
        }
        let field = |k: &str| {
            v.pointer(&format!("/result/{k}"))
                .and_then(|x| x.as_str())
                .filter(|x| !x.is_empty())
                .map(str::to_string)
        };
        return Ok(Navigated {
            loader_id: field("loaderId"),
            error_text: field("errorText"),
        });
    }
    Err("the CDP connection closed before Page.navigate answered".into())
}

/// 快照之后页面变了、ref 指的节点已经不在了：CDP 只说「找不到节点」，换成一句能照着做的话。
/// refs 不再每做一个动作就全部作废（以前点一下、填一下之后整份快照都失效，模型只能再拍一次），
/// 只有节点真的没了才走到这里。
fn ref_gone(key: &str, e: String) -> String {
    let lower = e.to_ascii_lowercase();
    let gone = [
        "no node with given id",
        "could not find node",
        "node is detached",
        "does not belong to the document",
        "no node found",
    ]
    .iter()
    .any(|m| lower.contains(m));
    if gone {
        format!("ref @{key} is no longer on the page (it changed since the snapshot) — take a new browser_snapshot. ({e})")
    } else {
        e
    }
}

fn flatten_frame_tree(
    tree: &FrameTree,
    out: &mut Vec<chromiumoxide::cdp::browser_protocol::page::Frame>,
) {
    out.push(tree.frame.clone());
    if let Some(children) = tree.child_frames.as_ref() {
        for child in children {
            flatten_frame_tree(child, out);
        }
    }
}

/// Independent profile under `$DOCK_HOME/browser/user-data` — never the user's default Chrome.
pub fn browser_user_data_dir() -> PathBuf {
    dock_home().join("browser").join("user-data")
}

pub fn browser_screenshot_dir() -> PathBuf {
    dock_home().join("browser").join("screenshots")
}

/// Discover Chromium/Chrome binary. Prefers `CHROME_PATH`, then chromiumoxide `CHROME`, then PATH.
pub fn discover_chrome() -> Result<String, String> {
    for key in ["CHROME_PATH", "CHROME"] {
        if let Ok(p) = std::env::var(key) {
            let path = PathBuf::from(&p);
            if path.is_file() {
                return Ok(p);
            }
            return Err(format!(
                "{key}={p} is set but not an executable file. Install Chromium or fix the path."
            ));
        }
    }

    // chromiumoxide defaults (DetectionOptions) + a few explicit common paths.
    match chromiumoxide::detection::default_executable(Default::default()) {
        Ok(p) => Ok(p.display().to_string()),
        Err(e) => {
            let common = [
                "/usr/bin/google-chrome-stable",
                "/usr/bin/google-chrome",
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser",
                "/snap/bin/chromium",
            ];
            for c in common {
                if Path::new(c).is_file() {
                    return Ok(c.into());
                }
            }
            Err(format!(
                "Chromium/Chrome not found ({e}). Set CHROME_PATH or install google-chrome / chromium."
            ))
        }
    }
}

/// 按下 / 抬起。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPhase {
    Down,
    Up,
}

/// 一次按键事件（`Input.dispatchKeyEvent`）：按 chromiumoxide 的键表补 code / keyCode /
/// text。模型的 `browser_press_key` 与网关画面上用户的按键共用。
pub fn key_event(
    key_name: &str,
    modifiers: i64,
    phase: KeyPhase,
) -> Result<DispatchKeyEventParams, String> {
    let def =
        keys::get_key_definition(key_name).ok_or_else(|| format!("unknown key `{key_name}`"))?;
    let kind = match phase {
        KeyPhase::Up => DispatchKeyEventType::KeyUp,
        KeyPhase::Down if def.text.is_some() || def.key.len() == 1 => DispatchKeyEventType::KeyDown,
        KeyPhase::Down => DispatchKeyEventType::RawKeyDown,
    };
    let mut builder = DispatchKeyEventParams::builder()
        .r#type(kind)
        .key(def.key)
        .code(def.code)
        .windows_virtual_key_code(def.key_code)
        .native_virtual_key_code(def.key_code)
        .modifiers(modifiers);
    if phase == KeyPhase::Down {
        if let Some(txt) = def.text {
            builder = builder.text(txt);
        } else if def.key.len() == 1 && modifiers == 0 {
            builder = builder.text(def.key);
        }
    }
    builder.build().map_err(|e| e.to_string())
}

/// Bit field: Alt=1, Ctrl=2, Meta=4, Shift=8. Returns (modifiers, main_key).
pub fn parse_key_chord(raw: &str) -> Result<(i64, String), String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("key is required".into());
    }
    let parts: Vec<&str> = s
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("key is required".into());
    }
    let mut modifiers = 0i64;
    for p in &parts[..parts.len() - 1] {
        match normalize_modifier(p) {
            Some(bit) => modifiers |= bit,
            None => {
                return Err(format!(
                    "unknown modifier `{p}` in `{raw}` (use Control/Ctrl, Alt, Meta/Command/Cmd, Shift)"
                ));
            }
        }
    }
    let main = key_alias(parts[parts.len() - 1]);
    // Allow bare modifier names only as the key itself (rare); otherwise require a main key.
    if parts.len() > 1
        && normalize_modifier(&main).is_some()
        && keys::get_key_definition(&main).is_none()
    {
        return Err(format!("chord `{raw}` is missing a main key"));
    }
    Ok((modifiers, main))
}

/// 键表不认、但模型常写的键名换成标准名（`Return` → `Enter`）。键表本来就认的原样留着。
fn key_alias(raw: &str) -> String {
    if keys::get_key_definition(raw).is_some() {
        return raw.to_string();
    }
    let standard = match raw.to_ascii_lowercase().as_str() {
        "return" => "Enter",
        "esc" => "Escape",
        "del" => "Delete",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "pgup" => "PageUp",
        "pgdn" | "pgdown" => "PageDown",
        _ => return raw.to_string(),
    };
    standard.to_string()
}

fn normalize_modifier(p: &str) -> Option<i64> {
    match p.to_ascii_lowercase().as_str() {
        "alt" | "option" => Some(1),
        "control" | "ctrl" => Some(2),
        "meta" | "command" | "cmd" | "super" | "win" | "windows" => Some(4),
        "shift" => Some(8),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_args_match_what_chromiumoxide_used_to_build() {
        let dir = Path::new("/tmp/dock-ud");
        let headless = launch_args(dir, false, Some("Mozilla/5.0 X"));
        for want in [
            "--remote-debugging-port=0",
            "--user-data-dir=/tmp/dock-ud",
            "--no-sandbox",
            "--disable-extensions",
            "--disable-blink-features=AutomationControlled",
            "--headless",
            "--hide-scrollbars",
            "--mute-audio",
            "--user-agent=Mozilla/5.0 X",
            "--enable-features=NetworkService,NetworkServiceInProcess",
        ] {
            assert!(headless.iter().any(|a| a == want), "missing {want}");
        }
        assert!(headless
            .iter()
            .all(|a| a.starts_with("--") && !a.starts_with("---")));
        assert!(!headless.iter().any(|a| a.contains("enable-automation")));
        assert!(!headless.iter().any(|a| a.starts_with("--window-size")));

        let headed = launch_args(dir, true, None);
        assert!(headed.iter().any(|a| a == "--window-size=1280,900"));
        assert!(!headed.iter().any(|a| a == "--headless"));
        assert!(!headed.iter().any(|a| a.starts_with("--user-agent")));
    }

    #[test]
    fn devtools_address_is_read_from_its_stderr_line() {
        assert_eq!(
            devtools_listening_url(
                "\nDevTools listening on ws://127.0.0.1:53111/devtools/browser/ab-cd\n"
            )
            .as_deref(),
            Some("ws://127.0.0.1:53111/devtools/browser/ab-cd")
        );
        assert!(devtools_listening_url("[1128/120000.1:ERROR] something").is_none());
        assert!(devtools_listening_url("listening on http://127.0.0.1:1/").is_none());
    }

    #[test]
    fn host_rules_become_block_patterns_in_order() {
        let p = block_patterns(&[
            ("api.x.example".into(), false),
            ("*.x.example".into(), true),
        ]);
        let got: Vec<(String, bool)> = p.iter().map(|b| (b.url_pattern.clone(), b.block)).collect();
        assert_eq!(
            got,
            vec![
                ("*://api.x.example/*".into(), false),
                ("*://api.x.example:*/*".into(), false),
                ("*://*.x.example/*".into(), true),
                ("*://*.x.example:*/*".into(), true),
                ("*://x.example/*".into(), true),
                ("*://x.example:*/*".into(), true),
            ]
        );
    }

    /// 帧比握手先被处理（事件分通道、先后会乱）：先攒着，握手回来再按顺序补上，一条不丢；
    /// 时间按 CDP 的时间戳从握手回来算。握手没成的记一条带原因的。
    #[test]
    fn websocket_frames_that_beat_the_handshake_are_kept() {
        use chromiumoxide::cdp::browser_protocol::network::WebSocketFrame;
        use dock_sandbox::audit::ws::Dir;
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(
            dock_sandbox::audit::store::AuditLog::open(dir.path(), "browser.jsonl").unwrap(),
        );
        let slot: AuditSlot = Arc::new(std::sync::Mutex::new(Some(log)));
        let text = |s: &str| WebSocketFrame::new(1.0, true, s.to_string());
        let mut socks = WsSockets::new("browser:T1".into(), 1024, slot.clone());
        socks.created("1", "wss://chat.example/live?room=2");
        socks.request("1", vec![("upgrade".into(), "websocket".into())]);
        socks.frame("1", Dir::Up, &text("early"), 10.002);
        socks.response("1", 101, "Switching Protocols", vec![], None, 10.0);
        socks.frame("1", Dir::Down, &text("echo"), 10.250);
        socks.closed("1");
        socks.created("2", "ws://chat.example/denied");
        socks.response("2", 403, "Forbidden", vec![], None, 11.0);
        drop(socks);
        *slot.lock().unwrap() = None;
        let events = dock_sandbox::audit::store::read(dir.path());
        let hs = events.iter().find(|e| e["path"] == "/live?room=2").unwrap();
        assert_eq!(
            (hs["scheme"].as_str(), hs["status"].as_u64()),
            (Some("wss"), Some(101))
        );
        let detail = dock_sandbox::audit::packet::flow_detail(
            dir.path(),
            &dock_sandbox::audit::packet::record_id(hs).unwrap(),
            false,
        )
        .unwrap();
        let msgs: Vec<(String, String, u64)> = detail["ws"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["dir"].as_str().unwrap().into(),
                    m["text"].as_str().unwrap().into(),
                    m["at"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            msgs,
            [
                ("up".into(), "early".into(), 2),
                ("down".into(), "echo".into(), 250)
            ]
        );
        assert_eq!(detail["ws"]["open"], false);
        let denied = events.iter().find(|e| e["path"] == "/denied").unwrap();
        assert_eq!(denied["status"], 403);
        assert!(
            denied["error"].as_str().unwrap().contains("握手没成"),
            "{denied}"
        );
        assert!(denied["ws"].is_null());
    }

    #[test]
    fn audit_urls_split_into_host_and_path() {
        assert_eq!(
            split_url("https://user@a.example:8443/x?y=1"),
            Some(("https", "a.example".into(), 8443, "/x?y=1".into()))
        );
        assert_eq!(
            split_url("http://[::1]/"),
            Some(("http", "::1".into(), 80, "/".into()))
        );
        assert_eq!(
            split_url("https://b.example"),
            Some(("https", "b.example".into(), 443, "/".into()))
        );
        assert!(split_url("data:text/html,x").is_none());
    }

    #[test]
    fn page_ws_url_points_at_the_target() {
        assert_eq!(
            page_ws_url("ws://127.0.0.1:9222/devtools/browser/abc-1", "T1").as_deref(),
            Some("ws://127.0.0.1:9222/devtools/page/T1")
        );
        assert!(page_ws_url("ws://127.0.0.1:9222/other", "T1").is_none());
    }

    #[test]
    fn desktop_user_agent_reports_major_version_only() {
        let ua = desktop_user_agent("154.0.7712.3").unwrap();
        assert!(ua.contains(" Chrome/154.0.0.0 Safari/537.36"), "{ua}");
        assert!(!ua.contains("Headless"), "{ua}");
        assert!(desktop_user_agent("").is_none());
        assert!(desktop_user_agent("beta").is_none());
    }

    #[test]
    fn opened_by_takes_pages_our_tabs_opened_in_order() {
        let t = |id: &str, kind: &str, opener: Option<&str>| {
            (id.to_string(), kind.to_string(), opener.map(String::from))
        };
        let ours = vec!["A".to_string()];
        let targets = vec![
            t("A", "page", None),
            // 新页开的新页排在它后面也能收（先收 C 再收 D）。
            t("D", "page", Some("C")),
            t("C", "page", Some("A")),
            // 别的会话的页开的、不是 page 的、没有 opener 的都不收。
            t("X", "page", None),
            t("Y", "page", Some("X")),
            t("W", "service_worker", Some("A")),
        ];
        assert_eq!(
            opened_by(&ours, &targets),
            vec![
                ("C".to_string(), "A".to_string()),
                ("D".to_string(), "C".to_string())
            ]
        );
        assert!(opened_by(&["A".into(), "C".into(), "D".into()], &targets).is_empty());
    }

    #[test]
    fn user_data_under_dock_home() {
        let _env = cordis_base::test_env::scoped().set("DOCK_HOME", "/tmp/dock-test-home-browser");
        let p = browser_user_data_dir();
        assert!(p.ends_with("browser/user-data"), "{p:?}");
        assert!(!p.to_string_lossy().contains(".config/google-chrome"));
    }

    #[test]
    fn discover_mentions_chrome_path_on_bad_env() {
        let _env = cordis_base::test_env::scoped().set("CHROME_PATH", "/no/such/chrome-binary-xyz");
        let err = discover_chrome().unwrap_err();
        assert!(err.contains("CHROME_PATH"), "{err}");
    }

    #[test]
    fn parse_key_chord_accepts_common_key_aliases() {
        // 模型常写 Return / Esc（实测有一次 `unknown key Return`）：键表不认的常见别名换成标准名。
        assert_eq!(parse_key_chord("Return").unwrap(), (0, "Enter".into()));
        assert_eq!(parse_key_chord("Esc").unwrap(), (0, "Escape".into()));
        assert_eq!(
            parse_key_chord("Shift+Return").unwrap(),
            (8, "Enter".into())
        );
        assert_eq!(parse_key_chord("Down").unwrap(), (0, "ArrowDown".into()));
        assert_eq!(parse_key_chord("PgDn").unwrap(), (0, "PageDown".into()));
        // 键表本来就认的不动。
        assert_eq!(parse_key_chord("a").unwrap(), (0, "a".into()));
    }

    #[test]
    fn parse_key_chord_modifiers() {
        assert_eq!(parse_key_chord("Enter").unwrap(), (0, "Enter".into()));
        assert_eq!(parse_key_chord("Control+a").unwrap(), (2, "a".into()));
        assert_eq!(
            parse_key_chord("Ctrl+Shift+Tab").unwrap(),
            (2 | 8, "Tab".into())
        );
        assert_eq!(
            parse_key_chord("Meta+Shift+t").unwrap(),
            (4 | 8, "t".into())
        );
        assert!(parse_key_chord("Foo+a").is_err());
        assert!(parse_key_chord("").is_err());
    }
}
