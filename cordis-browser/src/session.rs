//! Chromiumoxide CDP：一个共享的 Chromium 进程（[`Chromium`]），每个会话一组标签页
//! （[`ConnectedSession`]）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::browser::{Browser, BrowserConfig};
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
    EventRequestWillBeSent, EventResponseReceived,
};
use chromiumoxide::cdp::browser_protocol::page::{
    EventJavascriptDialogOpening, FrameId, FrameTree, GetFrameTreeParams,
    GetNavigationHistoryParams, HandleJavaScriptDialogParams, NavigateToHistoryEntryParams,
};
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
const ACTION_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
pub struct PendingDialog {
    pub message: String,
    pub dialog_type: String,
    pub default_prompt: Option<String>,
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
/// 或者 GUI 正开着看的）；连不上才自己拉起。只有自己拉起的才归自己关。
pub struct Chromium {
    browser: Arc<Browser>,
    handler: JoinHandle<()>,
    launched: bool,
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
                    launched: false,
                });
            }
        }
        Self::launch(&user_data_dir, headed).await
    }

    async fn launch(user_data_dir: &Path, headed: bool) -> Result<Self, String> {
        let exe = discover_chrome()?;
        let mut builder = BrowserConfig::builder()
            .chrome_executable(&exe)
            .user_data_dir(user_data_dir)
            .no_sandbox()
            .launch_timeout(LAUNCH_TIMEOUT)
            // chromiumoxide Arg::from does not auto-prepend "--" for bare keys in 0.9,
            // but Builder::arg historically treated "--foo" as key "--foo" and some
            // formatters double the dashes — pass bare flag names without leading "--".
            // (Lynn: "--force-…" became "----force-…" and accessibility never applied.)
            .arg("disable-dev-shm-usage")
            .arg("force-renderer-accessibility");

        // Headless by default (CI / servers). Prefer with_head when cockpit/env says so.
        // Default Viewport 800×600 Emulation-overrides layout; headed windows then paint
        // only a corner on grey chrome. Clear viewport so the OS window drives layout.
        if headed {
            builder = builder.with_head().viewport(None).window_size(1280, 900);
        }

        let config = builder.build().map_err(|e| format!("BrowserConfig: {e}"))?;

        let (browser, handler) = with_timeout(
            LAUNCH_TIMEOUT,
            async {
                Browser::launch(config)
                    .await
                    .map_err(|e| format!("launch: {e}"))
            },
            || "timed out launching Chromium".into(),
        )
        .await
        .map_err(|e| format!("launch Chromium ({exe}): {e}"))?;

        Ok(Self {
            browser: Arc::new(browser),
            handler: spawn_handler(handler),
            launched: true,
        })
    }

    /// CDP 连接还在吗。Chromium 被关掉（或别的进程拉起的那个退出了）时 handler 流结束。
    pub fn is_alive(&self) -> bool {
        !self.handler.is_finished()
    }

    /// 自己拉起的才关进程；连上的只断开，不关别人的浏览器。
    pub async fn shutdown(self) {
        let Self {
            browser,
            handler,
            launched,
        } = self;
        if launched {
            match Arc::try_unwrap(browser) {
                Ok(mut browser) => {
                    let _ = browser.close().await;
                    let _ = browser.wait().await;
                }
                // 还有标签页组攥着句柄：发 CDP Browser.close，进程退出后 kill_on_drop 兜底。
                Err(shared) => {
                    let _ = shared.execute(BrowserCloseParams::default()).await;
                }
            }
        }
        handler.abort();
    }
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

/// 一个会话的标签页组：自己开的页、快照 refs、对话框、network / console 捕获。
/// 进程是共享的 [`Chromium`]，这里只攥句柄。
pub struct ConnectedSession {
    browser: Arc<Browser>,
    pages: Vec<Page>,
    active: usize,
    dialog_tasks: Vec<JoinHandle<()>>,
    capture_tasks: Vec<JoinHandle<()>>,
    pending_dialog: Arc<std::sync::Mutex<Option<PendingDialog>>>,
    capture: Arc<AsyncMutex<CaptureBuffers>>,
    pub refs: HashMap<String, RefEntry>,
}

impl ConnectedSession {
    /// 在共享的 Chromium 里给这个会话开第一个标签页。
    pub async fn open(chromium: &Chromium, url: Option<&str>) -> Result<Self, String> {
        let browser = chromium.browser.clone();
        let start = url.unwrap_or("about:blank");
        let page = with_timeout(
            NAV_TIMEOUT,
            async {
                browser
                    .new_page(start)
                    .await
                    .map_err(|e| format!("new_page: {e}"))
            },
            || "timed out opening page".into(),
        )
        .await
        .map_err(|e| format!("new_page({start}): {e}"))?;

        let pending_dialog = Arc::new(std::sync::Mutex::new(None));
        let capture = Arc::new(AsyncMutex::new(CaptureBuffers::new()));
        let mut dialog_tasks = Vec::new();
        let mut capture_tasks = Vec::new();
        dialog_tasks.push(Self::spawn_dialog_listener(&page, pending_dialog.clone()).await?);
        capture_tasks.extend(Self::attach_page_capture(&page, capture.clone()).await?);

        Ok(Self {
            browser,
            pages: vec![page],
            active: 0,
            dialog_tasks,
            capture_tasks,
            pending_dialog,
            capture,
            refs: HashMap::new(),
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
                .push(Self::spawn_dialog_listener(&page, self.pending_dialog.clone()).await?);
            self.capture_tasks
                .extend(Self::attach_page_capture(&page, self.capture.clone()).await?);
            self.pages.insert(at + 1, page);
            self.active = at + 1;
            changed = true;
        }
        if changed {
            self.refs.clear();
        }
        Ok(changed)
    }

    /// 一个标签页都不剩了（[`ConnectedSession::sync_targets`] 之后）。
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    async fn spawn_dialog_listener(
        page: &Page,
        pending: Arc<std::sync::Mutex<Option<PendingDialog>>>,
    ) -> Result<JoinHandle<()>, String> {
        let mut events = page
            .event_listener::<EventJavascriptDialogOpening>()
            .await
            .map_err(|e| format!("dialog listener: {e}"))?;
        Ok(tokio::spawn(async move {
            while let Some(ev) = events.next().await {
                let info = PendingDialog {
                    message: ev.message.clone(),
                    dialog_type: ev.r#type.as_ref().to_string(),
                    default_prompt: ev.default_prompt.clone(),
                };
                if let Ok(mut g) = pending.lock() {
                    *g = Some(info);
                }
            }
        }))
    }

    async fn attach_page_capture(
        page: &Page,
        capture: Arc<AsyncMutex<CaptureBuffers>>,
    ) -> Result<Vec<JoinHandle<()>>, String> {
        // Network/Runtime are enabled by chromiumoxide's frame/network managers
        // on target attach. Do not re-send Enable — it can time out the command
        // queue. Event listeners alone surface request/console traffic.
        let mut tasks = Vec::with_capacity(3);
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
        with_timeout(
            NAV_TIMEOUT,
            async {
                page.goto(url).await.map_err(|e| format!("goto: {e}"))?;
                Ok::<_, String>(())
            },
            || "navigate timeout".into(),
        )
        .await
        .map_err(|e| format!("navigate({url}): {e}"))?;
        self.refs.clear();
        let cur = page
            .url()
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| url.into());
        Ok(cur)
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
        Ok(snap)
    }

    pub async fn click_ref(&mut self, ref_id: &str) -> Result<String, String> {
        let key = snapshot::parse_ref(ref_id).ok_or_else(|| "missing ref".to_string())?;
        let entry = self.refs.get(&key).cloned().ok_or_else(|| {
            format!(
                "unknown ref `{key}` — call browser_snapshot first (have {} refs)",
                self.refs.len()
            )
        })?;
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
        .await?;

        self.refs.clear();
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
            let key = snapshot::parse_ref(raw).ok_or_else(|| "missing ref".to_string())?;
            let entry = self
                .refs
                .get(&key)
                .cloned()
                .ok_or_else(|| format!("unknown ref `{key}` — call browser_snapshot first"))?;
            let backend = snapshot::backend_id(&entry)
                .ok_or_else(|| format!("ref `{key}` has no backend DOM node"))?;
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
            self.refs.clear();
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
                    .push(Self::spawn_dialog_listener(&page, self.pending_dialog.clone()).await?);
                self.capture_tasks
                    .extend(Self::attach_page_capture(&page, self.capture.clone()).await?);
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

    fn lookup_ref(&self, ref_id: &str) -> Result<(String, RefEntry), String> {
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
        .await?;
        Ok(format!("hovered @{key} ({})", entry.role))
    }

    /// Press a key or chord (`Enter`, `Tab`, `Control+a`, `Meta+Shift+t`).
    pub async fn press_key(&mut self, key: &str, ref_id: Option<&str>) -> Result<String, String> {
        let page = self.active_page()?.clone();
        if let Some(raw) = ref_id {
            let (k, entry) = self.lookup_ref(raw)?;
            let backend = snapshot::backend_id(&entry)
                .ok_or_else(|| format!("ref `{k}` has no backend DOM node"))?;
            Self::focus_backend(&page, backend).await?;
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
        Self::focus_backend(&page, backend).await?;
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
        self.refs.clear();
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
            Self::focus_backend(&page, backend).await?;
            let out = Self::call_js_on_backend(
                &page,
                backend,
                js,
                vec![serde_json::Value::String(value.clone())],
            )
            .await?;
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
        self.refs.clear();
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
        &self,
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
        let point = Self::point_for_backend(page, backend).await?;
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

        self.refs.clear();
        Ok(format!("dragged from {start_label} to {end_label}"))
    }

    /// Accept or dismiss a pending JS dialog (alert/confirm/prompt/beforeunload).
    pub async fn handle_dialog(
        &mut self,
        accept: bool,
        prompt_text: Option<&str>,
    ) -> Result<String, String> {
        let page = self.active_page()?.clone();
        let pending = self.pending_dialog();
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
            .map_err(|e| format!("setFileInputFiles: {e}"))?;
        self.refs.clear();
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
    let main = parts[parts.len() - 1].to_string();
    // Allow bare modifier names only as the key itself (rare); otherwise require a main key.
    if parts.len() > 1
        && normalize_modifier(&main).is_some()
        && keys::get_key_definition(&main).is_none()
    {
        return Err(format!("chord `{raw}` is missing a main key"));
    }
    Ok((modifiers, main))
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
