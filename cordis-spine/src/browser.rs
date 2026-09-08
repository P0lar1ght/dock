//! BUA cockpit skeleton: named `"browser"` + deferred `browser_*` stubs.
//!
//! TUI is cockpit-only. A real Chromium/CDP session is PR B — this plugin
//! does not depend on chromiumoxide, Node, or Playwright. Tools land on the
//! `"tools"` table via [`Tools::register_deferred`] so the sampler never sees
//! them; the model discovers them with `search_tool` and calls them with
//! `use_tool`.

use std::sync::Mutex;

use cordis::{plugin, Inject, Plugin};

use crate::names::{BROWSER, SLASH, TOOLS};
use crate::slash::{ExtraSlashKind, Slash, SlashEntry};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

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

const NOT_CONNECTED: &str =
    "Error: browser is not connected. Chromium session is not wired yet (PR B). TUI /browser reports 未连接.";

const SLASH_BODY: &str = "未连接 — Chromium 会话尚未接入（骨架，PR B 才驱动真浏览器）。\n\
模型经 search_tool / use_tool 发现 browser_open、browser_snapshot、browser_click、browser_type、browser_screenshot、browser_tabs、browser_close；它们不进默认 sampler 工具表。";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserSession {
    /// No CDP session. PR B will add a connected variant.
    Closed,
}

/// Named `"browser"` handle. Call sites live-lookup; do not capture the `Arc`.
pub struct Browser {
    session: Mutex<BrowserSession>,
}

impl Browser {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(BrowserSession::Closed),
        }
    }

    pub fn session(&self) -> BrowserSession {
        *self.session.lock().unwrap()
    }

    pub fn status_line(&self) -> &'static str {
        match self.session() {
            BrowserSession::Closed => "未连接",
        }
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
            ctx.provide(BROWSER, Browser::new())?;
            let tools = ctx.require::<Tools>(TOOLS)?;
            let body: ToolBody = {
                let ctx = ctx.clone();
                std::sync::Arc::new(move |call| {
                    let ctx = ctx.clone();
                    Box::pin(async move { run_stub(&ctx, call) })
                })
            };
            let mut disposers = Vec::with_capacity(BROWSER_TOOL_NAMES.len() + 1);
            for spec in browser_specs() {
                disposers.push(tools.register_deferred(spec, body.clone())?);
            }
            let slash = ctx.require::<Slash>(SLASH)?;
            disposers.push(slash.register(SlashEntry {
                command: "browser".into(),
                description: "浏览器驾驶舱（未连接）".into(),
                kind: ExtraSlashKind::Overlay,
                text: SLASH_BODY.into(),
                title: "浏览器".into(),
                send: false,
            })?);
            own_registered(ctx, disposers)?;
            Ok(None)
        },
    )
}

fn run_stub(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let Some(browser) = ctx.get::<Browser>(BROWSER) else {
        return tool_result(call, NOT_CONNECTED);
    };
    match browser.session() {
        BrowserSession::Closed => tool_result(call, NOT_CONNECTED),
    }
}

fn browser_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "browser_open".into(),
            description: "Open a URL in the external Chromium session (BUA cockpit). Skeleton: not connected until PR B wires CDP. Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"url":{"type":"string","description":"URL to open."}},"required":["url"]}"#.into(),
        },
        ToolSpec {
            name: "browser_snapshot".into(),
            description: "Accessibility / DOM snapshot of the current page for click/type refs. Skeleton: browser not connected (PR B). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"interactive":{"type":"boolean","description":"If true, include interactive element refs."}}}"#.into(),
        },
        ToolSpec {
            name: "browser_click".into(),
            description: "Click an element from browser_snapshot. Skeleton: browser not connected (PR B). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."}},"required":["ref"]}"#.into(),
        },
        ToolSpec {
            name: "browser_type".into(),
            description: "Type text into an element from browser_snapshot. Skeleton: browser not connected (PR B). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"ref":{"type":"string","description":"Element ref from browser_snapshot."},"text":{"type":"string","description":"Text to type."},"submit":{"type":"boolean","description":"Press Enter after typing."}},"required":["text"]}"#.into(),
        },
        ToolSpec {
            name: "browser_screenshot".into(),
            description: "Capture a screenshot of the current page. Skeleton: browser not connected (PR B). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"full_page":{"type":"boolean","description":"Capture the full scrollable page."}}}"#.into(),
        },
        ToolSpec {
            name: "browser_tabs".into(),
            description: "List or switch Chromium tabs. Skeleton: browser not connected (PR B). Discover via search_tool; call with use_tool.".into(),
            parameters_json: r#"{"type":"object","properties":{"action":{"type":"string","enum":["list","new","switch","close"],"description":"Tab action. Default list."},"index":{"type":"integer","description":"Tab index for switch/close."},"url":{"type":"string","description":"URL when action is new."}}}"#.into(),
        },
        ToolSpec {
            name: "browser_close".into(),
            description: "Close the external Chromium session. Skeleton: browser not connected (PR B). Discover via search_tool; call with use_tool.".into(),
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
                name: "browser_open".into(),
                arguments: r#"{"url":"https://example.com"}"#.into(),
            })
            .await;
        assert!(
            result.content.contains("not connected"),
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
}
