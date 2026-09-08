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
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

use session::ConnectedSession;

/// Planned whitelist. Registered deferred; not in `specs_for_model`.
pub const BROWSER_TOOL_NAMES: &[&str] = &[
    "browser_open",
    "browser_snapshot",
    "browser_click",
    "browser_type",
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
            "  search_tool / use_tool → browser_open · browser_snapshot · browser_click · browser_type · browser_screenshot · browser_tabs · browser_close\n",
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
        "browser_close" => {
            browser.shutdown().await;
            Ok("browser closed".into())
        }
        "browser_snapshot"
        | "browser_click"
        | "browser_type"
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
        Ok(content) => tool_result(call, content),
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
    match name {
        "browser_snapshot" => {
            let interactive = arg_bool(args, "interactive").unwrap_or(true);
            let snap = session.snapshot(interactive).await?;
            let tabs = session.tab_infos().await;
            browser.remember_tabs(tabs);
            browser.refresh_slash();
            Ok(snap.text)
        }
        "browser_click" => {
            let r = arg_str(args, "ref").ok_or_else(|| "ref is required".to_string())?;
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

fn browser_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "browser_open".into(),
            description: "Open a URL in Dock's isolated Chromium session (BUA cockpit via chromiumoxide CDP). Lazily launches Chromium under $DOCK_HOME/browser/user-data — no /browser required. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"url":{"type":"string","description":"URL to open."}},"required":["url"]}"#.into(),
        },
        ToolSpec {
            name: "browser_snapshot".into(),
            description: "Lean accessibility snapshot of the current page with refs (@eN) for click/type. Prefer interactive=true. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"interactive":{"type":"boolean","description":"If true (default), only interactive element refs."}}}"#.into(),
        },
        ToolSpec {
            name: "browser_click".into(),
            description: "Click an element from browser_snapshot by ref (@eN). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."}},"required":["ref"]}"#.into(),
        },
        ToolSpec {
            name: "browser_type".into(),
            description: "Type text into an element from browser_snapshot (optional ref focuses first). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."},"text":{"type":"string","description":"Text to type."},"submit":{"type":"boolean","description":"Press Enter after typing."}},"required":["text"]}"#.into(),
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
}
