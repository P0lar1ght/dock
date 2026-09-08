//! BUA browser cockpit: named `"browser"` + deferred `browser_*` via chromiumoxide CDP.
//!
//! TUI is cockpit-only (full overlay is PR C). Tools land on the `"tools"` table via
//! [`Tools::register_deferred`] so the sampler never sees them; the model discovers
//! them with `search_tool` and calls them with `use_tool`. Profile lives under
//! `$DOCK_HOME/browser/…`, not the user's daily Chrome profile. Lean a11y refs are
//! agent-browser-shaped (reference only — no agent-browser binary / Node runtime).

mod session;
mod snapshot;
mod wait;

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

    pub fn status_line(&self) -> String {
        match self.session() {
            BrowserSession::Closed => "未连接".into(),
            BrowserSession::Connected => {
                let url = self.inner.last_url.lock().unwrap().clone();
                if url.is_empty() {
                    "Connected".into()
                } else {
                    format!("Connected — {url}")
                }
            }
        }
    }

    fn slash_body(&self) -> String {
        let status = self.status_line();
        format!(
            "{status}\n\
chromiumoxide CDP（独立 profile：$DOCK_HOME/browser/user-data）。\n\
模型可直接 browser_open，无需先 /browser。工具经 search_tool / use_tool：\
browser_open、browser_snapshot、browser_click、browser_type、browser_screenshot、browser_tabs、browser_close；不进默认 sampler 工具表。\n\
截图：$DOCK_HOME/browser/screenshots/。"
        )
    }

    fn refresh_slash(&self) {
        let Some(slash) = self.inner.slash.lock().unwrap().clone() else {
            return;
        };
        let connected = self.session() == BrowserSession::Connected;
        let desc = if connected {
            "浏览器驾驶舱（Connected）"
        } else {
            "浏览器驾驶舱（未连接）"
        };
        let _ = slash.update_overlay("browser", desc, self.slash_body(), "浏览器");
    }

    fn mark_connected(&self, url: &str) {
        self.inner.connected.store(true, Ordering::SeqCst);
        *self.inner.last_url.lock().unwrap() = url.to_string();
        self.refresh_slash();
    }

    fn mark_closed(&self) {
        self.inner.connected.store(false, Ordering::SeqCst);
        self.inner.last_url.lock().unwrap().clear();
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
                self.mark_connected(&cur);
                return Ok(format!("navigated to {cur}"));
            }
            let cur = session
                .active_page()?
                .url()
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| self.inner.last_url.lock().unwrap().clone());
            self.mark_connected(&cur);
            return Ok(format!("already connected — {cur}"));
        }
        let session = ConnectedSession::launch(url).await?;
        let fallback = url.unwrap_or("about:blank").to_string();
        let url_now = match session.active_page() {
            Ok(p) => p.url().await.ok().flatten().unwrap_or(fallback),
            Err(_) => fallback,
        };
        *g = Some(session);
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
                text: browser.slash_body(),
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
            if let Ok(p) = session.active_page() {
                if let Ok(Some(u)) = p.url().await {
                    *browser.inner.last_url.lock().unwrap() = u;
                    browser.refresh_slash();
                }
            }
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
            Ok(format!("saved {}", path.display()))
        }
        "browser_tabs" => {
            let action = arg_str(args, "action").unwrap_or_else(|| "list".into());
            let index = arg_usize(args, "index");
            let url = arg_str(args, "url");
            let out = session
                .tabs(&action, index, url.as_deref())
                .await?;
            if let Ok(p) = session.active_page() {
                if let Ok(Some(u)) = p.url().await {
                    *browser.inner.last_url.lock().unwrap() = u;
                    browser.refresh_slash();
                }
            }
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

    #[tokio::test]
    async fn registers_deferred_named_service_slash_and_disposes() {
        let dock_home = tempfile::tempdir().unwrap();
        std::env::set_var("DOCK_HOME", dock_home.path());

        let (root, fiber) = boot_browser().await;
        let tools = root.require::<Tools>(TOOLS).unwrap();
        let browser = root.get::<Browser>(BROWSER).expect("named browser service");
        assert_eq!(browser.session(), BrowserSession::Closed);
        assert_eq!(browser.status_line(), "未连接");

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
            browser.status_line().starts_with("Connected"),
            "{}",
            browser.status_line()
        );

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
