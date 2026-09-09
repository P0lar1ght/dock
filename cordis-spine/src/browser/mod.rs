//! BUA browser cockpit: named `"browser"` + deferred `browser_*` via chromiumoxide CDP.
//!
//! `/browser` is a live TUI cockpit overlay (status / tabs / last screenshot /
//! approval hint) — live-looked by the TUI like Slot, not a terminal web
//! renderer. Tools land on the `"tools"` table via [`Tools::register_deferred`]
//! so the sampler never sees them; the model discovers them with `search_tool`
//! and calls them with `use_tool`. Profile lives under `$DOCK_HOME/browser/…`,
//! not the user's daily Chrome profile. Lean a11y refs are agent-browser-shaped
//! (reference only — no agent-browser binary / Node runtime).

mod session;
mod snapshot;
mod wait;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Disposable, Inject, Plugin};

use crate::names::{BROWSER, SLASH, TOOLS};
use crate::slash::{ExtraSlashKind, Slash, SlashEntry};
use crate::tools::{own_registered, tool_result, tool_result_with_images, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

use session::ConnectedSession;

/// Planned whitelist. Registered deferred; not in `specs_for_model`.
pub const BROWSER_TOOL_NAMES: &[&str] = &[
    "browser_open",
    "browser_navigate",
    "browser_navigate_back",
    "browser_snapshot",
    "browser_click",
    "browser_hover",
    "browser_type",
    "browser_press_key",
    "browser_select_option",
    "browser_fill_form",
    "browser_wait_for",
    "browser_drag",
    "browser_handle_dialog",
    "browser_file_upload",
    "browser_resize",
    "browser_evaluate",
    "browser_console_messages",
    "browser_network_requests",
    "browser_screenshot",
    "browser_tabs",
    "browser_close",
];

const NEED_OPEN: &str =
    "Error: browser is not connected. Call browser_open first (no /browser required).";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserSession {
    Closed,
    Connected,
}

/// One cached tab row for the `/browser` cockpit (sync-readable; refreshed after CDP ops).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserTabInfo {
    pub index: usize,
    pub url: String,
    pub active: bool,
}

/// Named `"browser"` handle. Call sites live-lookup; do not capture the `Arc`.
/// Clone shares the same session (inner `Arc`).
#[derive(Clone)]
pub struct Browser {
    inner: Arc<BrowserInner>,
}

struct BrowserInner {
    live: tokio::sync::Mutex<Option<ConnectedSession>>,
    connected: AtomicBool,
    last_url: Mutex<String>,
    last_tabs: Mutex<Vec<BrowserTabInfo>>,
    last_screenshot: Mutex<Option<PathBuf>>,
    /// Last `browser_wait_for` outcome line for the `/browser` cockpit.
    last_wait: Mutex<Option<String>>,
    /// Pending JS dialog line (type + message), or last handle_dialog outcome.
    last_dialog: Mutex<Option<String>>,
    /// Last `browser_evaluate` result preview for the `/browser` cockpit.
    last_evaluate: Mutex<Option<String>>,
    /// Last `browser_network_requests` summary line for the `/browser` cockpit.
    last_network: Mutex<Option<String>>,
    /// Optional slash table to refresh `/browser` overlay body on connect/close.
    slash: Mutex<Option<Arc<Slash>>>,
}

impl Browser {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BrowserInner {
                live: tokio::sync::Mutex::new(None),
                connected: AtomicBool::new(false),
                last_url: Mutex::new(String::new()),
                last_tabs: Mutex::new(Vec::new()),
                last_screenshot: Mutex::new(None),
                last_wait: Mutex::new(None),
                last_dialog: Mutex::new(None),
                last_evaluate: Mutex::new(None),
                last_network: Mutex::new(None),
                slash: Mutex::new(None),
            }),
        }
    }

    fn bind_slash(&self, slash: Arc<Slash>) {
        *self.inner.slash.lock().unwrap() = Some(slash);
        self.refresh_slash();
    }

    pub fn session(&self) -> BrowserSession {
        if self.inner.connected.load(Ordering::SeqCst) {
            BrowserSession::Connected
        } else {
            BrowserSession::Closed
        }
    }

    /// Sync snapshot of tabs last seen after a CDP op (empty when Closed).
    pub fn tabs(&self) -> Vec<BrowserTabInfo> {
        self.inner.last_tabs.lock().unwrap().clone()
    }

    /// Path of the most recent `browser_screenshot`, if any this session.
    pub fn last_screenshot(&self) -> Option<PathBuf> {
        self.inner.last_screenshot.lock().unwrap().clone()
    }

    /// Last `browser_wait_for` summary line, if any this session.
    pub fn last_wait(&self) -> Option<String> {
        self.inner.last_wait.lock().unwrap().clone()
    }

    /// Pending / last JS dialog line for the `/browser` cockpit.
    pub fn last_dialog(&self) -> Option<String> {
        self.inner.last_dialog.lock().unwrap().clone()
    }

    /// Last evaluate preview line, if any this session.
    pub fn last_evaluate(&self) -> Option<String> {
        self.inner.last_evaluate.lock().unwrap().clone()
    }

    /// Last network summary line, if any this session.
    pub fn last_network(&self) -> Option<String> {
        self.inner.last_network.lock().unwrap().clone()
    }

    pub fn status_line(&self) -> String {
        match self.session() {
            BrowserSession::Closed => "未连接".into(),
            BrowserSession::Connected => {
                let url = self.inner.last_url.lock().unwrap().clone();
                if url.is_empty() {
                    "已连接".into()
                } else {
                    format!("已连接 — {url}")
                }
            }
        }
    }

    /// Full `/browser` cockpit body (text_overlay-friendly sections).
    /// Pass `None` for slash refresh; the live TUI overlay passes a pending
    /// `browser_*` permission line when `Permissions::front` matches.
    pub fn cockpit_body(&self) -> String {
        self.format_cockpit(None)
    }

    /// Build cockpit text with an optional live approval hint line.
    pub fn format_cockpit(&self, approval_line: Option<&str>) -> String {
        let kicker = match self.session() {
            BrowserSession::Closed => "未连接",
            BrowserSession::Connected => "已连接",
        };
        let status = self.status_line();
        let tabs = self.inner.last_tabs.lock().unwrap().clone();
        let shot = self.inner.last_screenshot.lock().unwrap().clone();

        let mut out = String::new();
        out.push_str(kicker);
        out.push('\n');
        out.push('\n');
        out.push_str("状态：\n");
        out.push_str(&format!("  {status}\n"));
        out.push('\n');
        out.push_str("标签页：\n");
        if tabs.is_empty() {
            out.push_str("  （无）\n");
        } else {
            for t in &tabs {
                let mark = if t.active { "*" } else { " " };
                let url = if t.url.is_empty() { "about:blank" } else { t.url.as_str() };
                out.push_str(&format!("  {mark} [{}] {url}\n", t.index));
            }
        }
        out.push('\n');
        out.push_str("最近截图：\n");
        match shot {
            Some(p) => {
                out.push_str(&format!("  {}\n", p.display()));
            }
            None => out.push_str("  （无）\n"),
        }
        out.push('\n');
        out.push_str("等待：\n");
        match self.inner.last_wait.lock().unwrap().clone() {
            Some(line) => out.push_str(&format!("  {line}\n")),
            None => out.push_str("  （无）\n"),
        }
        out.push('\n');
        out.push_str("对话框：\n");
        match self.inner.last_dialog.lock().unwrap().clone() {
            Some(line) => out.push_str(&format!("  {line}\n")),
            None => out.push_str("  （无）\n"),
        }
        out.push_str("  用 browser_handle_dialog 接受/拒绝；不渲染网页。\n");
        out.push('\n');
        out.push_str("能力：\n");
        out.push_str(
            "  P0 导航/后退 · 悬停 · 按键 · 下拉 · 填表 · 等待（text/selector）\n",
        );
        out.push_str(
            "  P1 拖拽 · 对话框 · 文件上传 · 视口 resize。不渲染网页。\n",
        );
        out.push_str(
            "  P2 evaluate（权限门）· console · network · 同域 iframe（frame_selector）。\n",
        );
        out.push('\n');
        out.push_str("最近 evaluate：\n");
        match self.inner.last_evaluate.lock().unwrap().clone() {
            Some(line) => out.push_str(&format!("  {line}\n")),
            None => out.push_str("  （无）\n"),
        }
        out.push_str(
            "  browser_evaluate 须过权限浮层（与 bash 同级）；计划模式会挡。\n",
        );
        out.push('\n');
        out.push_str("最近 network：\n");
        match self.inner.last_network.lock().unwrap().clone() {
            Some(line) => out.push_str(&format!("  {line}\n")),
            None => out.push_str("  （无）\n"),
        }
        out.push('\n');
        out.push_str("审批：\n");
        match approval_line {
            Some(line) if !line.is_empty() => {
                out.push_str(&format!("  {line}\n"));
            }
            _ => out.push_str("  （无）\n"),
        }
        out.push_str(
            "  真正批准/拒绝请用权限浮层（允许使用 …？）。\n",
        );
        out.push('\n');
        out.push_str("断开：\n");
        out.push_str(
            "  模型调用 browser_close 关闭 Chromium；卸载 tool-browser 会 dispose 会话。\n",
        );
        out.push_str("  Esc 关闭本面板。终端只显示驾驶舱状态，不渲染网页。\n");
        out.push('\n');
        out.push_str("工具：\n");
        out.push_str(
            "  search_tool / use_tool → browser_open · browser_navigate · browser_navigate_back · browser_snapshot · browser_click · browser_hover · browser_type · browser_press_key · browser_select_option · browser_fill_form · browser_wait_for · browser_drag · browser_handle_dialog · browser_file_upload · browser_resize · browser_evaluate · browser_console_messages · browser_network_requests · browser_screenshot · browser_tabs · browser_close\n",
        );
        out.push_str("  不进默认 sampler 工具表；无需先 /browser。\n");
        out.push('\n');
        out.push_str("配置：\n");
        out.push_str("  $DOCK_HOME/browser/user-data\n");
        out.push_str("  $DOCK_HOME/browser/screenshots\n");
        out
    }

    fn refresh_slash(&self) {
        let Some(slash) = self.inner.slash.lock().unwrap().clone() else {
            return;
        };
        let connected = self.session() == BrowserSession::Connected;
        let desc = if connected {
            "浏览器驾驶舱（已连接）"
        } else {
            "浏览器驾驶舱（未连接）"
        };
        let _ = slash.update_overlay("browser", desc, self.format_cockpit(None), "浏览器");
    }

    fn remember_tabs(&self, tabs: Vec<BrowserTabInfo>) {
        if let Some(active) = tabs.iter().find(|t| t.active) {
            *self.inner.last_url.lock().unwrap() = active.url.clone();
        }
        *self.inner.last_tabs.lock().unwrap() = tabs;
    }

    fn remember_screenshot(&self, path: PathBuf) {
        *self.inner.last_screenshot.lock().unwrap() = Some(path);
    }

    fn remember_wait(&self, line: String) {
        *self.inner.last_wait.lock().unwrap() = Some(line);
        self.refresh_slash();
    }

    fn remember_dialog(&self, line: String) {
        *self.inner.last_dialog.lock().unwrap() = Some(line);
        self.refresh_slash();
    }

    fn remember_evaluate(&self, line: String) {
        *self.inner.last_evaluate.lock().unwrap() = Some(line);
        self.refresh_slash();
    }

    fn remember_network(&self, line: String) {
        *self.inner.last_network.lock().unwrap() = Some(line);
        self.refresh_slash();
    }

    fn sync_pending_dialog(&self, session: &ConnectedSession) {
        if let Some(d) = session.pending_dialog() {
            let msg = d.message.replace("\n", " ");
            let short = if msg.chars().count() > 80 {
                format!("{}…", msg.chars().take(80).collect::<String>())
            } else {
                msg
            };
            let line = format!("待处理 {} — {}", d.dialog_type, short);
            *self.inner.last_dialog.lock().unwrap() = Some(line);
        }
    }

    fn mark_connected(&self, url: &str) {
        self.inner.connected.store(true, Ordering::SeqCst);
        *self.inner.last_url.lock().unwrap() = url.to_string();
        {
            let mut tabs = self.inner.last_tabs.lock().unwrap();
            if tabs.is_empty() {
                tabs.push(BrowserTabInfo {
                    index: 0,
                    url: url.to_string(),
                    active: true,
                });
            } else if let Some(t) = tabs.iter_mut().find(|t| t.active) {
                t.url = url.to_string();
            }
        }
        self.refresh_slash();
    }

    fn mark_closed(&self) {
        self.inner.connected.store(false, Ordering::SeqCst);
        self.inner.last_url.lock().unwrap().clear();
        self.inner.last_tabs.lock().unwrap().clear();
        // Keep last_screenshot so the cockpit can still show the path after close.
        self.refresh_slash();
    }

    async fn shutdown(&self) {
        let mut g = self.inner.live.lock().await;
        if let Some(session) = g.take() {
            session.shutdown().await;
        }
        self.mark_closed();
    }

    async fn ensure_open(&self, url: Option<&str>) -> Result<String, String> {
        let mut g = self.inner.live.lock().await;
        if let Some(session) = g.as_mut() {
            if let Some(u) = url {
                let cur = session.navigate(u).await?;
                let tabs = session.tab_infos().await;
                self.remember_tabs(tabs);
                self.mark_connected(&cur);
                return Ok(format!("navigated to {cur}"));
            }
            let tabs = session.tab_infos().await;
            let cur = tabs
                .iter()
                .find(|t| t.active)
                .map(|t| t.url.clone())
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| self.inner.last_url.lock().unwrap().clone());
            self.remember_tabs(tabs);
            self.mark_connected(&cur);
            return Ok(format!("already connected — {cur}"));
        }
        let session = ConnectedSession::launch(url).await?;
        let fallback = url.unwrap_or("about:blank").to_string();
        let url_now = match session.active_page() {
            Ok(p) => p.url().await.ok().flatten().unwrap_or(fallback),
            Err(_) => fallback,
        };
        let tabs = session.tab_infos().await;
        *g = Some(session);
        self.remember_tabs(tabs);
        self.mark_connected(&url_now);
        Ok(format!("opened {url_now}"))
    }

    /// Jump within an existing session. Errors if Closed (unlike [`Self::ensure_open`]).
    async fn navigate_existing(&self, url: &str) -> Result<String, String> {
        let mut g = self.inner.live.lock().await;
        let session = g.as_mut().ok_or_else(|| NEED_OPEN.to_string())?;
        let cur = session.navigate(url).await?;
        let tabs = session.tab_infos().await;
        drop(g);
        self.remember_tabs(tabs);
        self.mark_connected(&cur);
        Ok(format!("navigated to {cur}"))
    }

    async fn navigate_back_existing(&self) -> Result<String, String> {
        let mut g = self.inner.live.lock().await;
        let session = g.as_mut().ok_or_else(|| NEED_OPEN.to_string())?;
        let cur = session.navigate_back().await?;
        let tabs = session.tab_infos().await;
        drop(g);
        self.remember_tabs(tabs);
        if let Some(active) = self.inner.last_tabs.lock().unwrap().iter().find(|t| t.active) {
            self.mark_connected(&active.url);
        } else {
            self.refresh_slash();
        }
        Ok(cur)
    }
}

impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

pub fn tool_browser() -> Plugin {
    plugin(
        "tool-browser",
        Inject::from([TOOLS, SLASH]),
        |ctx, _: &()| {
            let browser = Browser::new();
            ctx.provide(BROWSER, browser.clone())?;
            let slash = ctx.require::<Slash>(SLASH)?;
            let tools = ctx.require::<Tools>(TOOLS)?;
            let body: ToolBody = {
                let ctx = ctx.clone();
                Arc::new(move |call| {
                    let ctx = ctx.clone();
                    Box::pin(async move { run_tool(&ctx, call).await })
                })
            };
            let mut disposers = Vec::with_capacity(BROWSER_TOOL_NAMES.len() + 2);
            for spec in browser_specs() {
                disposers.push(tools.register_deferred(spec, body.clone())?);
            }
            disposers.push(slash.register(SlashEntry {
                command: "browser".into(),
                description: "浏览器驾驶舱（未连接）".into(),
                kind: ExtraSlashKind::Overlay,
                text: browser.format_cockpit(None),
                title: "浏览器".into(),
                send: false,
            })?);
            browser.bind_slash(slash);

            let browser_drop = browser.clone();
            disposers.push(Disposable::from_async(move || async move {
                browser_drop.shutdown().await;
                Ok(())
            }));

            own_registered(ctx, disposers)?;
            Ok(None)
        },
    )
}

async fn run_tool(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let Some(browser) = ctx.get::<Browser>(BROWSER) else {
        return tool_result(call, NEED_OPEN);
    };
    let name = call.name.as_str();
    let result = match name {
        "browser_open" => match arg_str(&call.arguments, "url") {
            Some(url) if !url.is_empty() => browser.ensure_open(Some(&url)).await,
            _ => Err("Error: url is required".into()),
        },
        "browser_navigate" => {
            if browser.session() == BrowserSession::Closed {
                Err(NEED_OPEN.into())
            } else {
                match arg_str(&call.arguments, "url") {
                    Some(url) if !url.is_empty() => browser.navigate_existing(&url).await,
                    _ => Err("url is required".into()),
                }
            }
        }
        "browser_navigate_back" => {
            if browser.session() == BrowserSession::Closed {
                Err(NEED_OPEN.into())
            } else {
                browser.navigate_back_existing().await
            }
        }
        "browser_close" => {
            browser.shutdown().await;
            Ok("browser closed".into())
        }
        "browser_snapshot"
        | "browser_click"
        | "browser_hover"
        | "browser_type"
        | "browser_press_key"
        | "browser_select_option"
        | "browser_fill_form"
        | "browser_wait_for"
        | "browser_drag"
        | "browser_handle_dialog"
        | "browser_file_upload"
        | "browser_resize"
        | "browser_evaluate"
        | "browser_console_messages"
        | "browser_network_requests"
        | "browser_screenshot"
        | "browser_tabs" => {
            if browser.session() == BrowserSession::Closed {
                Err(NEED_OPEN.into())
            } else {
                dispatch_connected(&browser, name, &call.arguments).await
            }
        }
        other => Err(format!("unknown browser tool `{other}`")),
    };
    match result {
        Ok(content) => {
            if call.name == "browser_screenshot" {
                // Path stays in content for /browser cockpit; also load bytes.
                let path = content
                    .strip_prefix("saved ")
                    .unwrap_or(content.as_str())
                    .trim();
                let mut images = Vec::new();
                if let Some(img) = crate::tool_images::user_image_from_path(std::path::Path::new(path))
                {
                    images.push(img);
                }
                let body = if images.is_empty() {
                    content
                } else {
                    format!(
                        "{content}
{}",
                        crate::tool_images::IMAGE_INLINE_PLACEHOLDER
                    )
                };
                tool_result_with_images(call, body, images)
            } else {
                tool_result(call, content)
            }
        }
        Err(e) => {
            let msg = if e.starts_with("Error:") {
                e
            } else {
                format!("Error: {e}")
            };
            tool_result(call, msg)
        }
    }
}

async fn dispatch_connected(
    browser: &Browser,
    name: &str,
    args: &str,
) -> Result<String, String> {
    let mut g = browser.inner.live.lock().await;
    let session = g.as_mut().ok_or_else(|| NEED_OPEN.to_string())?;
    browser.sync_pending_dialog(session);
    match name {
        "browser_snapshot" => {
            let interactive = arg_bool(args, "interactive").unwrap_or(true);
            let frame = arg_str(args, "frame").or_else(|| arg_str(args, "frame_selector"));
            let snap = session.snapshot(interactive, frame.as_deref()).await?;
            let tabs = session.tab_infos().await;
            browser.remember_tabs(tabs);
            browser.refresh_slash();
            Ok(snap.text)
        }
        "browser_click" => {
            let r = arg_str(args, "ref").ok_or_else(|| "ref is required".to_string())?;
            // Optional frame/frame_selector documented for API symmetry; refs
            // must come from a snapshot taken in that frame.
            let _frame = arg_str(args, "frame").or_else(|| arg_str(args, "frame_selector"));
            session.click_ref(&r).await
        }
        "browser_type" => {
            let text = arg_str(args, "text").ok_or_else(|| "text is required".to_string())?;
            let ref_id = arg_str(args, "ref");
            let submit = arg_bool(args, "submit").unwrap_or(false);
            session
                .type_ref(ref_id.as_deref(), &text, submit)
                .await
        }
        "browser_screenshot" => {
            let full = arg_bool(args, "full_page").unwrap_or(false);
            let path = session.screenshot(full).await?;
            browser.remember_screenshot(path.clone());
            let tabs = session.tab_infos().await;
            browser.remember_tabs(tabs);
            browser.refresh_slash();
            Ok(format!("saved {}", path.display()))
        }
        "browser_tabs" => {
            let action = arg_str(args, "action").unwrap_or_else(|| "list".into());
            let index = arg_usize(args, "index");
            let url = arg_str(args, "url");
            let out = session
                .tabs(&action, index, url.as_deref())
                .await?;
            let tabs = session.tab_infos().await;
            browser.remember_tabs(tabs);
            browser.refresh_slash();
            Ok(out)
        }
        "browser_hover" => {
            let r = arg_str(args, "ref").ok_or_else(|| "ref is required".to_string())?;
            session.hover_ref(&r).await
        }
        "browser_press_key" => {
            let key = arg_str(args, "key").ok_or_else(|| "key is required".to_string())?;
            let ref_id = arg_str(args, "ref");
            session.press_key(&key, ref_id.as_deref()).await
        }
        "browser_select_option" => {
            let r = arg_str(args, "ref").ok_or_else(|| "ref is required".to_string())?;
            let value = arg_str(args, "value");
            let label = arg_str(args, "label");
            session
                .select_option(&r, value.as_deref(), label.as_deref())
                .await
        }
        "browser_fill_form" => {
            let fields = parse_fill_fields(args)?;
            session.fill_form(&fields).await
        }
        "browser_wait_for" => {
            let text = arg_str(args, "text");
            let selector = arg_str(args, "selector");
            let timeout_ms = arg_u64(args, "timeout_ms");
            let out = session
                .wait_for(text.as_deref(), selector.as_deref(), timeout_ms)
                .await;
            match &out {
                Ok(line) => browser.remember_wait(line.clone()),
                Err(e) => browser.remember_wait(format!("失败：{e}")),
            }
            out
        }
        "browser_drag" => {
            let source_ref = arg_str(args, "source_ref");
            let target_ref = arg_str(args, "target_ref");
            let start_x = arg_f64(args, "start_x");
            let start_y = arg_f64(args, "start_y");
            let end_x = arg_f64(args, "end_x");
            let end_y = arg_f64(args, "end_y");
            let steps = arg_u64(args, "steps").map(|n| n as u32);
            session
                .drag(
                    source_ref.as_deref(),
                    target_ref.as_deref(),
                    start_x,
                    start_y,
                    end_x,
                    end_y,
                    steps,
                )
                .await
        }
        "browser_handle_dialog" => {
            let accept = arg_bool(args, "accept").ok_or_else(|| {
                "accept boolean is required (true=accept, false=dismiss)".to_string()
            })?;
            let prompt_text = arg_str(args, "prompt_text");
            let out = session
                .handle_dialog(accept, prompt_text.as_deref())
                .await;
            match &out {
                Ok(line) => browser.remember_dialog(line.clone()),
                Err(e) => browser.remember_dialog(format!("失败：{e}")),
            }
            out
        }
        "browser_file_upload" => {
            let r = arg_str(args, "ref").ok_or_else(|| "ref is required".to_string())?;
            let paths = parse_paths(args)?;
            session.file_upload(&r, &paths).await
        }
        "browser_resize" => {
            let width = arg_i64(args, "width").ok_or_else(|| "width is required".to_string())?;
            let height = arg_i64(args, "height").ok_or_else(|| "height is required".to_string())?;
            session.resize(width, height).await
        }
        "browser_evaluate" => {
            let expression = arg_str(args, "expression")
                .or_else(|| arg_str(args, "code"))
                .ok_or_else(|| "expression is required".to_string())?;
            let frame = arg_str(args, "frame").or_else(|| arg_str(args, "frame_selector"));
            let out = session.evaluate(&expression, frame.as_deref()).await;
            match &out {
                Ok(line) => {
                    let preview = if line.chars().count() > 120 {
                        format!("{}…", line.chars().take(120).collect::<String>())
                    } else {
                        line.clone()
                    };
                    browser.remember_evaluate(preview);
                }
                Err(e) => browser.remember_evaluate(format!("失败：{e}")),
            }
            out
        }
        "browser_console_messages" => session.console_messages().await,
        "browser_network_requests" => {
            let out = session.network_requests().await;
            if let Ok(body) = &out {
                let summary = body.lines().next().unwrap_or("(empty)").to_string();
                let n = body.lines().count();
                browser.remember_network(format!("{n} entr(y/ies); last: {summary}"));
            }
            out
        }
        other => Err(format!("unknown browser tool `{other}`")),
    }
}

fn arg_str(raw: &str, key: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

fn arg_bool(raw: &str, key: &str) -> Option<bool> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get(key).and_then(|x| x.as_bool())
}

fn arg_usize(raw: &str, key: &str) -> Option<usize> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get(key).and_then(|x| {
        x.as_u64()
            .map(|n| n as usize)
            .or_else(|| x.as_i64().and_then(|n| usize::try_from(n).ok()))
    })
}

fn arg_u64(raw: &str, key: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get(key).and_then(|x| {
        x.as_u64()
            .or_else(|| x.as_i64().and_then(|n| u64::try_from(n).ok()))
    })
}

fn arg_i64(raw: &str, key: &str) -> Option<i64> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get(key).and_then(|x| {
        x.as_i64()
            .or_else(|| x.as_u64().and_then(|n| i64::try_from(n).ok()))
            .or_else(|| x.as_f64().map(|n| n as i64))
    })
}

fn arg_f64(raw: &str, key: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(raw).ok()?;
    v.get(key).and_then(|x| {
        x.as_f64()
            .or_else(|| x.as_i64().map(|n| n as f64))
            .or_else(|| x.as_u64().map(|n| n as f64))
    })
}

fn parse_paths(raw: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid JSON arguments: {e}"))?;
    if let Some(arr) = v.get("paths").and_then(|x| x.as_array()) {
        let mut out = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            let s = item
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("paths[{i}] must be a non-empty string"))?;
            out.push(s.to_string());
        }
        return Ok(out);
    }
    if let Some(p) = v.get("path").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
        return Ok(vec![p.to_string()]);
    }
    Err("paths array (or path string) is required".into())
}

fn parse_fill_fields(raw: &str) -> Result<Vec<(String, String)>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid JSON arguments: {e}"))?;
    let arr = v
        .get("fields")
        .and_then(|x| x.as_array())
        .ok_or_else(|| "fields array is required".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let r = item
            .get("ref")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("fields[{i}].ref is required"))?;
        let value = item.get("value").map(|x| match x {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }).unwrap_or_default();
        out.push((r.to_string(), value));
    }
    Ok(out)
}

fn browser_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "browser_open".into(),
            description: "Open a URL in Dock's isolated Chromium session (BUA cockpit via chromiumoxide CDP). Lazily launches Chromium under $DOCK_HOME/browser/user-data — no /browser required. Use browser_open for first connect; use browser_navigate to jump in an already-open session. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"url":{"type":"string","description":"URL to open."}},"required":["url"]}"#.into(),
        },
        ToolSpec {
            name: "browser_navigate".into(),
            description: "Navigate the current tab to a URL in an existing Chromium session. Errors if Closed — call browser_open first. Unlike browser_open, does not launch Chromium. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"url":{"type":"string","description":"URL to navigate to."}},"required":["url"]}"#.into(),
        },
        ToolSpec {
            name: "browser_navigate_back".into(),
            description: "Go back in history for the active tab (existing session only; errors if Closed). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{}}"#.into(),
        },
        ToolSpec {
            name: "browser_snapshot".into(),
            description: "Lean accessibility snapshot of the current page (or same-origin iframe via frame/frame_selector) with refs (@eN) for click/type/hover/select/fill. Prefer interactive=true. Cross-origin iframes fail clearly. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"interactive":{"type":"boolean","description":"If true (default), only interactive element refs."},"frame":{"type":"string","description":"Optional CSS selector for a same-origin iframe/frame."},"frame_selector":{"type":"string","description":"Alias of frame."}}}"#.into(),
        },
        ToolSpec {
            name: "browser_click".into(),
            description: "Click an element from browser_snapshot by ref (@eN). If the ref came from a framed snapshot, pass the same frame/frame_selector for clarity (refs already target that frame). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."},"frame":{"type":"string","description":"Optional CSS selector matching the iframe used for snapshot."},"frame_selector":{"type":"string","description":"Alias of frame."}},"required":["ref"]}"#.into(),
        },
        ToolSpec {
            name: "browser_hover".into(),
            description: "Hover an element from browser_snapshot by ref (@eN). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."}},"required":["ref"]}"#.into(),
        },
        ToolSpec {
            name: "browser_type".into(),
            description: "Type text into an element from browser_snapshot (optional ref focuses first). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."},"text":{"type":"string","description":"Text to type."},"submit":{"type":"boolean","description":"Press Enter after typing."}},"required":["text"]}"#.into(),
        },
        ToolSpec {
            name: "browser_press_key".into(),
            description: "Press a key or shortcut (Enter/Tab/Escape/ArrowDown or Control+a, Meta+Shift+t). Optional ref focuses that element first. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"key":{"type":"string","description":"Key or chord (e.g. Enter, Tab, Control+a)."},"ref":{"type":"string","description":"Optional snapshot ref to focus before pressing."}},"required":["key"]}"#.into(),
        },
        ToolSpec {
            name: "browser_select_option".into(),
            description: "Select an option on a <select> (or similar) by snapshot ref plus value and/or label. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."},"value":{"type":"string","description":"Option value attribute."},"label":{"type":"string","description":"Option visible label/text."}},"required":["ref"]}"#.into(),
        },
        ToolSpec {
            name: "browser_fill_form".into(),
            description: "Fill multiple form fields in one call. Pass fields: [{ref, value}, ...]. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"fields":{"type":"array","description":"Array of {ref, value} objects.","items":{"type":"object","properties":{"ref":{"type":"string"},"value":{}},"required":["ref"]}}},"required":["fields"]}"#.into(),
        },
        ToolSpec {
            name: "browser_wait_for".into(),
            description: "Wait for visible text and/or a CSS selector, or just sleep for timeout_ms when neither is set. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"text":{"type":"string","description":"Substring to wait for in document body text."},"selector":{"type":"string","description":"CSS selector to wait for."},"timeout_ms":{"type":"integer","description":"Max wait in milliseconds (default 30000)."}}}"#.into(),
        },
        ToolSpec {
            name: "browser_drag".into(),
            description: "Drag from a source snapshot ref (or start_x/start_y) to a target ref (or end_x/end_y) via CDP mouse events. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"source_ref":{"type":"string","description":"Snapshot ref to drag from."},"target_ref":{"type":"string","description":"Snapshot ref to drop on."},"start_x":{"type":"number"},"start_y":{"type":"number"},"end_x":{"type":"number"},"end_y":{"type":"number"},"steps":{"type":"integer","description":"Intermediate mouseMoved steps (default 10)."}}}"#.into(),
        },
        ToolSpec {
            name: "browser_handle_dialog".into(),
            description: "Accept or dismiss a JavaScript dialog (alert/confirm/prompt/beforeunload). Optional prompt_text for prompt dialogs. Wire Page.javascriptDialogOpening so agents are not stuck on native dialogs. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"accept":{"type":"boolean","description":"true to accept/OK, false to dismiss/Cancel."},"prompt_text":{"type":"string","description":"Text for prompt dialogs before accepting."}},"required":["accept"]}"#.into(),
        },
        ToolSpec {
            name: "browser_file_upload".into(),
            description: "Set files on a file input from browser_snapshot ref via DOM.setFileInputFiles. Pass paths: [\"/abs/path\", ...] or path for one file. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"ref":{"type":"string","description":"File input ref from browser_snapshot."},"paths":{"type":"array","items":{"type":"string"},"description":"Absolute or relative file paths."},"path":{"type":"string","description":"Single file path (alternative to paths)."}},"required":["ref"]}"#.into(),
        },
        ToolSpec {
            name: "browser_resize".into(),
            description: "Set the page viewport width/height via Emulation.setDeviceMetricsOverride. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"width":{"type":"integer","description":"Viewport width in CSS pixels."},"height":{"type":"integer","description":"Viewport height in CSS pixels."}},"required":["width","height"]}"#.into(),
        },
        ToolSpec {
            name: "browser_evaluate".into(),
            description: "Run JavaScript in the page (or same-origin iframe via frame/frame_selector) via chromiumoxide CDP Runtime.evaluate. Returns a truncated string/JSON result. Permissions-gated like bash (needs_permission + plan block). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"expression":{"type":"string","description":"JS expression or function to evaluate."},"code":{"type":"string","description":"Alias of expression."},"frame":{"type":"string","description":"Optional CSS selector for a same-origin iframe/frame."},"frame_selector":{"type":"string","description":"Alias of frame."}},"required":["expression"]}"#.into(),
        },
        ToolSpec {
            name: "browser_console_messages".into(),
            description: "Read-only recent console API messages captured since the session connected (truncated). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{}}"#.into(),
        },
        ToolSpec {
            name: "browser_network_requests".into(),
            description: "Read-only list of recent network requests (method/url/status/type). Truncated; never dumps response bodies. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{}}"#.into(),
        },
        ToolSpec {
            name: "browser_screenshot".into(),
            description: "Capture a screenshot into $DOCK_HOME/browser/screenshots/. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"full_page":{"type":"boolean","description":"Capture the full scrollable page."}}}"#.into(),
        },
        ToolSpec {
            name: "browser_tabs".into(),
            description: "List or switch Chromium tabs (list|new|switch|close). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"action":{"type":"string","enum":["list","new","switch","close"],"description":"Tab action. Default list."},"index":{"type":"integer","description":"Tab index for switch/close."},"url":{"type":"string","description":"URL when action is new."}}}"#.into(),
        },
        ToolSpec {
            name: "browser_close".into(),
            description: "Close the Dock Chromium session and return to 未连接. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{}}"#.into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use crate::slash::slash;
    use crate::tools::{tools, Tools};
    use cordis::Context;

    async fn boot_browser() -> (Context, cordis::Fiber) {
        let root = Context::new();
        root.plugin(tools(), ()).unwrap().wait().await.unwrap();
        root.plugin(slash(), ()).unwrap().wait().await.unwrap();
        let fiber = root.plugin(tool_browser(), ()).unwrap();
        fiber.wait().await.unwrap();
        (root, fiber)
    }

    #[test]
    fn cockpit_body_lists_sections_when_closed() {
        let browser = Browser::new();
        let body = browser.cockpit_body();
        assert!(body.starts_with("未连接"), "{body}");
        for needle in [
            "状态：",
            "标签页：",
            "最近截图：",
            "审批：",
            "断开：",
            "工具：",
            "配置：",
            "（无）",
            "允许使用 …？",
        ] {
            assert!(body.contains(needle), "missing {needle} in {body}");
        }
        assert_eq!(browser.status_line(), "未连接");
        assert!(body.contains("browser_hover"), "{body}");
        assert!(body.contains("browser_navigate"), "{body}");
        assert!(body.contains("browser_press_key"), "{body}");
        assert!(body.contains("browser_drag"), "{body}");
        assert!(body.contains("browser_handle_dialog"), "{body}");
        assert!(body.contains("browser_file_upload"), "{body}");
        assert!(body.contains("browser_resize"), "{body}");
        assert!(body.contains("browser_evaluate"), "{body}");
        assert!(body.contains("browser_network_requests"), "{body}");
        assert!(body.contains("P1"), "{body}");
        assert!(body.contains("P2"), "{body}");
        assert!(body.contains("最近 evaluate："), "{body}");
        assert!(body.contains("权限浮层"), "{body}");
        assert!(body.contains("最近 network："), "{body}");
        let with_approval = browser.format_cockpit(Some("browser_open — https://example.com/"));
        assert!(with_approval.contains("browser_open — https://example.com/"), "{with_approval}");
        assert!(!with_approval.contains("审批：\n  （无）"), "{with_approval}");
    }

    #[test]
    fn cockpit_body_shows_tabs_and_screenshot_path() {
        let browser = Browser::new();
        browser.inner.connected.store(true, Ordering::SeqCst);
        browser.remember_tabs(vec![
            BrowserTabInfo {
                index: 0,
                url: "https://example.com/".into(),
                active: true,
            },
            BrowserTabInfo {
                index: 1,
                url: "about:blank".into(),
                active: false,
            },
        ]);
        browser.remember_screenshot(PathBuf::from("/tmp/shot.png"));
        let body = browser.cockpit_body();
        assert!(body.starts_with("已连接"), "{body}");
        assert!(body.contains("已连接 — https://example.com/"), "{body}");
        assert!(body.contains("* [0] https://example.com/"), "{body}");
        assert!(body.contains("  [1] about:blank"), "{body}");
        assert!(body.contains("/tmp/shot.png"), "{body}");
        assert!(body.contains("审批："), "{body}");
        assert_eq!(
            browser.last_screenshot().as_deref(),
            Some(Path::new("/tmp/shot.png"))
        );
        assert_eq!(
            browser.status_line(),
            "已连接 — https://example.com/"
        );
    }

    #[tokio::test]
    async fn registers_deferred_named_service_slash_and_disposes() {
        let dock_home = tempfile::tempdir().unwrap();
        std::env::set_var("DOCK_HOME", dock_home.path());

        let (root, fiber) = boot_browser().await;
        let tools = root.require::<Tools>(TOOLS).unwrap();
        let browser = root.get::<Browser>(BROWSER).expect("named browser service");
        assert_eq!(browser.session(), BrowserSession::Closed);
        assert_eq!(browser.status_line(), "未连接");
        assert!(browser.cockpit_body().contains("未连接"));
        assert!(browser.cockpit_body().contains("标签页："));
        assert!(browser.cockpit_body().contains("最近截图："));
        assert!(browser.cockpit_body().contains("审批："));
        assert!(browser.cockpit_body().contains("等待："));
        assert!(browser.cockpit_body().contains("能力："));
        assert!(browser.last_wait().is_none());
        assert!(browser.cockpit_body().contains("对话框："));
        assert!(browser.last_dialog().is_none());
        assert!(browser.cockpit_body().contains("断开："));
        assert!(browser.tabs().is_empty());
        assert!(browser.last_screenshot().is_none());

        for name in BROWSER_TOOL_NAMES {
            assert!(
                tools.specs().iter().any(|s| s.name == *name),
                "missing {name}"
            );
            assert!(tools.is_deferred(name), "{name} must be deferred");
            assert!(tools.is_hidden(name), "{name} must be hidden from sampler");
        }
        let model: Vec<String> = tools
            .specs_for_model()
            .into_iter()
            .map(|s| s.name)
            .collect();
        for name in BROWSER_TOOL_NAMES {
            assert!(
                !model.iter().any(|n| n == name),
                "sampler must not see {name}: {model:?}"
            );
        }

        let extras = root.require::<Slash>(SLASH).unwrap().list();
        assert!(
            extras.iter().any(|e| e.command == "browser"),
            "missing /browser extra: {extras:?}"
        );

        let result = tools
            .execute(ToolCall {
                id: "1".into(),
                name: "browser_snapshot".into(),
                arguments: r#"{"interactive":true}"#.into(),
            })
            .await;
        assert!(
            result.content.contains("not connected") || result.content.contains("browser_open"),
            "{}",
            result.content
        );

        for need_open in [
            "browser_navigate",
            "browser_navigate_back",
            "browser_hover",
            "browser_press_key",
            "browser_select_option",
            "browser_fill_form",
            "browser_wait_for",
            "browser_drag",
            "browser_handle_dialog",
            "browser_file_upload",
            "browser_resize",
            "browser_evaluate",
            "browser_console_messages",
            "browser_network_requests",
        ] {
            let args = if need_open == "browser_navigate" {
                r#"{"url":"about:blank"}"#
            } else if need_open == "browser_press_key" {
                r#"{"key":"Enter"}"#
            } else if need_open == "browser_select_option" {
                r#"{"ref":"@e1","value":"x"}"#
            } else if need_open == "browser_fill_form" {
                r#"{"fields":[{"ref":"@e1","value":"x"}]}"#
            } else if need_open == "browser_wait_for" {
                r#"{"text":"hi","timeout_ms":10}"#
            } else if need_open == "browser_hover" {
                r#"{"ref":"@e1"}"#
            } else if need_open == "browser_drag" {
                r#"{"source_ref":"@e1","target_ref":"@e2"}"#
            } else if need_open == "browser_handle_dialog" {
                r#"{"accept":true}"#
            } else if need_open == "browser_file_upload" {
                r#"{"ref":"@e1","paths":["/tmp/x"]}"#
            } else if need_open == "browser_resize" {
                r#"{"width":800,"height":600}"#
            } else if need_open == "browser_evaluate" {
                r#"{"expression":"1+1"}"#
            } else {
                "{}"
            };
            let r = tools
                .execute(ToolCall {
                    id: need_open.into(),
                    name: need_open.into(),
                    arguments: args.into(),
                })
                .await;
            assert!(
                r.content.contains("not connected") || r.content.contains("browser_open"),
                "{need_open}: {}",
                r.content
            );
        }

        // Specs cover open vs navigate wording.
        let specs = tools.specs();
        let open = specs.iter().find(|s| s.name == "browser_open").unwrap();
        let nav = specs.iter().find(|s| s.name == "browser_navigate").unwrap();
        assert!(open.description.contains("browser_navigate") || open.description.contains("first"), "{}", open.description);
        assert!(nav.description.contains("existing") || nav.description.contains("Closed"), "{}", nav.description);

        fiber.dispose().await.unwrap();
        assert!(root.get::<Browser>(BROWSER).is_none());
        for name in BROWSER_TOOL_NAMES {
            assert!(
                !tools.specs().iter().any(|s| s.name == *name),
                "{name} survived dispose"
            );
        }
        assert!(root
            .require::<Slash>(SLASH)
            .unwrap()
            .list()
            .iter()
            .all(|e| e.command != "browser"));
    }

    #[tokio::test]
    async fn p0_navigate_press_wait_when_chrome_available() {
        let dock_home = tempfile::tempdir().unwrap();
        std::env::set_var("DOCK_HOME", dock_home.path());

        if session::discover_chrome().is_err() {
            eprintln!("skip p0 smoke: chrome not installed");
            return;
        }

        let (root, fiber) = boot_browser().await;
        let tools = root.require::<Tools>(TOOLS).unwrap();

        let open = tools
            .execute(ToolCall {
                id: "o".into(),
                name: "browser_open".into(),
                arguments: r#"{"url":"data:text/html,<html><body><h1>HelloBUA</h1><select id=s><option value=a>A</option><option value=b>B</option></select><input id=i /></body></html>"}"#.into(),
            })
            .await;
        assert!(!open.content.starts_with("Error:"), "open: {}", open.content);

        let nav = tools
            .execute(ToolCall {
                id: "n".into(),
                name: "browser_navigate".into(),
                arguments: r#"{"url":"data:text/html,<html><body><p>NavOK</p><input id=x /></body></html>"}"#.into(),
            })
            .await;
        assert!(!nav.content.starts_with("Error:"), "navigate: {}", nav.content);
        assert!(nav.content.contains("navigated"), "{}", nav.content);

        let wait = tools
            .execute(ToolCall {
                id: "w".into(),
                name: "browser_wait_for".into(),
                arguments: r#"{"text":"NavOK","timeout_ms":5000}"#.into(),
            })
            .await;
        assert!(!wait.content.starts_with("Error:"), "wait_for: {}", wait.content);

        let key = tools
            .execute(ToolCall {
                id: "k".into(),
                name: "browser_press_key".into(),
                arguments: r#"{"key":"Tab"}"#.into(),
            })
            .await;
        assert!(!key.content.starts_with("Error:"), "press_key: {}", key.content);

        let closed_nav = {
            let _ = tools
                .execute(ToolCall {
                    id: "c".into(),
                    name: "browser_close".into(),
                    arguments: "{}".into(),
                })
                .await;
            tools
                .execute(ToolCall {
                    id: "bad".into(),
                    name: "browser_navigate".into(),
                    arguments: r#"{"url":"about:blank"}"#.into(),
                })
                .await
        };
        assert!(
            closed_nav.content.contains("not connected")
                || closed_nav.content.contains("browser_open"),
            "{}",
            closed_nav.content
        );

        fiber.dispose().await.unwrap();
    }

    #[tokio::test]
    async fn p1_resize_dialog_upload_drag_when_chrome_available() {
        let dock_home = tempfile::tempdir().unwrap();
        std::env::set_var("DOCK_HOME", dock_home.path());

        if session::discover_chrome().is_err() {
            eprintln!("skip p1 smoke: chrome not installed");
            return;
        }

        let (root, fiber) = boot_browser().await;
        let tools = root.require::<Tools>(TOOLS).unwrap();

        let open = tools
            .execute(ToolCall {
                id: "o".into(),
                name: "browser_open".into(),
                arguments: r#"{"url":"data:text/html,<html><body><div id=a style='width:40px;height:40px'>A</div><div id=b style='width:40px;height:40px;margin-top:80px'>B</div><input id=f type=file /><script>setTimeout(function(){alert('BUA-D2');},50);</script></body></html>"}"#.into(),
            })
            .await;
        assert!(!open.content.starts_with("Error:"), "open: {}", open.content);

        let resize = tools
            .execute(ToolCall {
                id: "rz".into(),
                name: "browser_resize".into(),
                arguments: r#"{"width":1024,"height":768}"#.into(),
            })
            .await;
        assert!(!resize.content.starts_with("Error:"), "resize: {}", resize.content);
        assert!(resize.content.contains("1024x768"), "{}", resize.content);

        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let dlg = tools
            .execute(ToolCall {
                id: "d".into(),
                name: "browser_handle_dialog".into(),
                arguments: r#"{"accept":true}"#.into(),
            })
            .await;
        assert!(!dlg.content.starts_with("Error:"), "handle_dialog: {}", dlg.content);

        let drag = tools
            .execute(ToolCall {
                id: "dg".into(),
                name: "browser_drag".into(),
                arguments: r#"{"start_x":20,"start_y":20,"end_x":20,"end_y":120,"steps":5}"#.into(),
            })
            .await;
        assert!(!drag.content.starts_with("Error:"), "drag: {}", drag.content);

        let upload_path = dock_home.path().join("upload.txt");
        std::fs::write(&upload_path, b"hello").unwrap();
        let snap = tools
            .execute(ToolCall {
                id: "s".into(),
                name: "browser_snapshot".into(),
                arguments: r#"{"interactive":true}"#.into(),
            })
            .await;
        assert!(!snap.content.starts_with("Error:"), "snapshot: {}", snap.content);
        let file_ref = snap.content.lines().find_map(|l| {
            let lower = l.to_ascii_lowercase();
            if !(lower.contains("choose file")
                || lower.contains("file")
                || lower.contains("upload"))
            {
                return None;
            }
            // Prefer "[ref=eN]" then bare "@eN".
            if let Some(idx) = lower.find("ref=e") {
                let rest = &l[idx + 4..];
                let id: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                if id.starts_with('e') {
                    return Some(format!("@{id}"));
                }
            }
            l.split_whitespace()
                .find(|t| t.starts_with("@e") || t.contains("ref=e"))
                .map(|s| {
                    let s = s.trim_matches(|c| c == ',' || c == ')' || c == '(' || c == ']');
                    if let Some(r) = s.strip_prefix("ref=") {
                        format!("@{r}")
                    } else if s.starts_with('@') {
                        s.to_string()
                    } else {
                        format!("@{s}")
                    }
                })
        });
        let file_ref = file_ref.expect(&format!(
            "expected file input ref in snapshot:\n{}",
            snap.content
        ));
        let up = tools
            .execute(ToolCall {
                id: "up".into(),
                name: "browser_file_upload".into(),
                arguments: format!(
                    r#"{{"ref":"{}","paths":["{}"]}}"#,
                    file_ref,
                    upload_path.display()
                ),
            })
            .await;
        assert!(!up.content.starts_with("Error:"), "file_upload: {}", up.content);

        fiber.dispose().await.unwrap();
    }

    #[tokio::test]
    async fn open_close_launches_chromium_when_available() {
        let dock_home = tempfile::tempdir().unwrap();
        std::env::set_var("DOCK_HOME", dock_home.path());

        if session::discover_chrome().is_err() {
            eprintln!("skip open_close: chrome not installed");
            return;
        }

        let (root, fiber) = boot_browser().await;
        let tools = root.require::<Tools>(TOOLS).unwrap();
        let browser = root.get::<Browser>(BROWSER).unwrap();

        let open = tools
            .execute(ToolCall {
                id: "o".into(),
                name: "browser_open".into(),
                arguments: r#"{"url":"about:blank"}"#.into(),
            })
            .await;
        assert!(
            !open.content.starts_with("Error:"),
            "open failed: {}",
            open.content
        );
        assert_eq!(browser.session(), BrowserSession::Connected);
        assert!(
            browser.status_line().starts_with("已连接"),
            "{}",
            browser.status_line()
        );
        assert!(
            !browser.tabs().is_empty(),
            "cockpit should cache at least one tab"
        );
        let body = browser.cockpit_body();
        assert!(body.starts_with("已连接"), "{body}");
        assert!(body.contains("标签页："), "{body}");
        assert!(body.contains("审批："), "{body}");

        let snap = tools
            .execute(ToolCall {
                id: "s".into(),
                name: "browser_snapshot".into(),
                arguments: r#"{"interactive":true}"#.into(),
            })
            .await;
        assert!(
            !snap.content.starts_with("Error:"),
            "snapshot failed: {}",
            snap.content
        );

        let close = tools
            .execute(ToolCall {
                id: "c".into(),
                name: "browser_close".into(),
                arguments: "{}".into(),
            })
            .await;
        assert!(
            close.content.contains("closed"),
            "{}",
            close.content
        );
        assert_eq!(browser.session(), BrowserSession::Closed);

        // Dispose after close must still be clean.
        fiber.dispose().await.unwrap();
        assert!(root.get::<Browser>(BROWSER).is_none());
    }

    #[tokio::test]
    async fn p2_evaluate_network_iframe_when_chrome_available() {
        let dock_home = tempfile::tempdir().unwrap();
        std::env::set_var("DOCK_HOME", dock_home.path());

        if session::discover_chrome().is_err() {
            eprintln!("skip p2 smoke: chrome not installed");
            return;
        }

        let (root, fiber) = boot_browser().await;
        let tools = root.require::<Tools>(TOOLS).unwrap();
        let browser = root.get::<Browser>(BROWSER).unwrap();

        let open = tools
            .execute(ToolCall {
                id: "o2".into(),
                name: "browser_open".into(),
                arguments: r#"{"url":"about:blank"}"#.into(),
            })
            .await;
        assert!(!open.content.starts_with("Error:"), "open: {}", open.content);

        let setup = tools
            .execute(ToolCall {
                id: "ev0".into(),
                name: "browser_evaluate".into(),
                arguments: r#"{"expression":"(() => { document.body.innerHTML = '<h1 id=t>P2Main</h1><iframe id=f name=child src=\"data:text/html,<html><body><p id=p>InsideFrame</p></body></html>\"></iframe>'; console.log('BUA-P2-LOG'); return document.title = 'p2'; })()"}"#.into(),
            })
            .await;
        assert!(!setup.content.starts_with("Error:"), "setup evaluate: {}", setup.content);

        let ev = tools
            .execute(ToolCall {
                id: "ev".into(),
                name: "browser_evaluate".into(),
                arguments: r#"{"expression":"1+2"}"#.into(),
            })
            .await;
        assert!(!ev.content.starts_with("Error:"), "evaluate: {}", ev.content);
        assert!(ev.content.contains('3'), "evaluate result: {}", ev.content);
        assert!(browser.last_evaluate().is_some());

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let _ = tools
            .execute(ToolCall {
                id: "fetch".into(),
                name: "browser_evaluate".into(),
                arguments: r#"{"expression":"fetch('data:text/plain,hi').then(r => r.text()).catch(e => String(e))"}"#.into(),
            })
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;

        let console = tools
            .execute(ToolCall {
                id: "c".into(),
                name: "browser_console_messages".into(),
                arguments: "{}".into(),
            })
            .await;
        assert!(
            !console.content.starts_with("Error:"),
            "console: {}",
            console.content
        );
        assert!(
            console.content.contains("BUA-P2-LOG") || console.content.contains("log:"),
            "console: {}",
            console.content
        );

        let net = tools
            .execute(ToolCall {
                id: "n".into(),
                name: "browser_network_requests".into(),
                arguments: "{}".into(),
            })
            .await;
        assert!(!net.content.starts_with("Error:"), "network: {}", net.content);
        assert!(browser.last_network().is_some());

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let frame_ev = tools
            .execute(ToolCall {
                id: "fe".into(),
                name: "browser_evaluate".into(),
                arguments: r##"{"expression":"document.getElementById('p') && document.getElementById('p').textContent","frame_selector":"#f"}"##.into(),
            })
            .await;
        assert!(
            !frame_ev.content.starts_with("Error:"),
            "frame evaluate: {}",
            frame_ev.content
        );
        assert!(
            frame_ev.content.contains("InsideFrame"),
            "frame evaluate: {}",
            frame_ev.content
        );

        let cross = tools
            .execute(ToolCall {
                id: "xo".into(),
                name: "browser_evaluate".into(),
                arguments: r##"{"expression":"1","frame_selector":"#missing"}"##.into(),
            })
            .await;
        assert!(
            cross.content.contains("Error:")
                || cross.content.contains("no element")
                || cross.content.contains("matching"),
            "missing frame: {}",
            cross.content
        );

        fiber.dispose().await.unwrap();
    }
}
