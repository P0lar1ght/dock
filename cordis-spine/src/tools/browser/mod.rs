//! 浏览器驾驶舱：named `"browser"` + `/browser`。
//!
//! 浏览器工具本身不在进程里：它们是内置 MCP 服务 `browser`（`dock mcp browser`，
//! crate `cordis-browser`）暴露的 `mcp_browser__browser_*`，和其它 MCP 一样经
//! `search_tool` / `use_tool`。这里只做两件事：看这台服务器连没连上，以及管
//! 有头 / 无头偏好（`[browser].headed`，服务器下次拉起 Chromium 时读）。
//! 和 `computer` 驾驶舱同一个形状；TUI 只渲染、只路由按键。

use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Inject, Plugin};

use crate::host::slash::{ExtraSlashKind, Slash, SlashEntry};
use crate::names::{BROWSER, MCP, SLASH};
use crate::tools::mcp::Mcp;
use cordis_base::config::{self, BROWSER_MCP_SERVER};

/// 浏览器 MCP 工具的公名前缀（`mcp_browser__browser_open` …）。
pub const BROWSER_MCP_PREFIX: &str = "mcp_browser__";

/// 驾驶舱状态，每帧从 MCP 列表现算。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserState {
    /// 没挂 `mcp-client`。
    NotMounted,
    /// 没有 `[mcp_servers.browser]` 这条行（不是从 `dock` 二进制起的，或
    /// `DOCK_BROWSER_MCP=off`）。
    Missing,
    /// 在 `/mcps` 里关掉了。
    Disabled,
    Connected {
        tools: usize,
    },
    Unreachable {
        detail: String,
    },
}

impl BrowserState {
    pub fn title(&self) -> &'static str {
        match self {
            Self::NotMounted => "未挂载",
            Self::Missing => "未启用",
            Self::Disabled => "已禁用",
            Self::Connected { .. } => "已连接",
            Self::Unreachable { .. } => "未连上",
        }
    }
}

/// Named `"browser"` handle. Call sites live-lookup; do not capture the `Arc`.
#[derive(Clone)]
pub struct Browser {
    inner: Arc<Inner>,
}

struct Inner {
    ctx: Context,
    slash: Mutex<Option<Arc<Slash>>>,
}

impl Browser {
    fn new(ctx: Context) -> Self {
        Self {
            inner: Arc::new(Inner {
                ctx,
                slash: Mutex::new(None),
            }),
        }
    }

    fn bind_slash(&self, slash: Arc<Slash>) {
        *self.inner.slash.lock().unwrap() = Some(slash);
    }

    pub fn state(&self) -> BrowserState {
        let Some(mcp) = self.inner.ctx.get::<Mcp>(MCP) else {
            return BrowserState::NotMounted;
        };
        match mcp
            .list()
            .into_iter()
            .find(|s| s.name == BROWSER_MCP_SERVER)
        {
            None => BrowserState::Missing,
            Some(s) if !s.enabled => BrowserState::Disabled,
            Some(s) if s.ok => BrowserState::Connected {
                tools: s.tools.iter().filter(|t| t.enabled).count(),
            },
            Some(s) => BrowserState::Unreachable { detail: s.detail },
        }
    }

    pub fn status_line(&self) -> String {
        match self.state() {
            BrowserState::Connected { tools } => format!("已连接（{tools} 个工具）"),
            BrowserState::Unreachable { detail } if !detail.is_empty() => {
                format!("未连上 — {detail}")
            }
            other => other.title().into(),
        }
    }

    /// Persisted `[browser].headed` preference (ignores env override).
    pub fn headed_pref(&self) -> bool {
        config::load_browser_headed()
    }

    /// Effective headed at next launch: env `DOCK_BROWSER_HEADED` (any non-empty) OR pref.
    pub fn headed_effective(&self) -> bool {
        config::effective_browser_headed()
    }

    /// Persist headed preference. Does **not** restart Chromium.
    pub fn set_headed(&self, headed: bool) -> Result<(), String> {
        config::persist_browser_headed(headed)?;
        self.refresh_slash();
        Ok(())
    }

    /// Toggle persisted headed preference. Returns the new pref value.
    /// 已经在跑的 Chromium 保持原样，等所有会话都 browser_close 之后下一次拉起才生效。
    pub fn toggle_headed(&self) -> Result<bool, String> {
        let next = !self.headed_pref();
        self.set_headed(next)?;
        Ok(next)
    }

    pub fn cockpit_body(&self) -> String {
        self.format_cockpit(None)
    }

    /// `/browser` 驾驶舱正文。`approval_line` 是 TUI 传进来的待批 `mcp_browser__*` 调用。
    pub fn format_cockpit(&self, approval_line: Option<&str>) -> String {
        let state = self.state();
        let mut out = String::new();
        out.push_str(state.title());
        out.push_str("\n\n状态：\n");
        out.push_str(&format!("  {}\n", self.status_line()));
        match state {
            BrowserState::Missing => out.push_str(
                "  没有 [mcp_servers.browser]。从 dock 启动会自动带上这条内置行；\n  DOCK_BROWSER_MCP=off 会关掉它。\n",
            ),
            BrowserState::Disabled => out.push_str("  在 /mcps 里按 Space 重新启用。\n"),
            _ => {}
        }
        out.push_str("\n显示：\n");
        let pref = if self.headed_pref() {
            "有头"
        } else {
            "无头"
        };
        let effective = if self.headed_effective() {
            "有头"
        } else {
            "无头"
        };
        out.push_str(&format!("  偏好：{pref}（默认无头）\n"));
        if config::dock_browser_headed_env_override() {
            out.push_str(&format!(
                "  生效：{effective}（DOCK_BROWSER_HEADED 覆盖 → 有头）\n"
            ));
        } else {
            out.push_str(&format!("  生效：{effective}\n"));
        }
        out.push_str("  按 h 切换有头/无头（写入 config.toml [browser].headed）。\n");
        out.push_str("  已开的 Chromium 不重启；所有会话都 browser_close 之后下次拉起才生效。\n");
        out.push_str("\n会话：\n");
        out.push_str("  每个会话自己的标签页；登录态（profile）所有会话共用。\n");
        out.push_str("  另一个 Dock 进程已经开着 Chromium 时直接连上它，不再开第二个。\n");
        out.push_str("\n审批：\n");
        match approval_line {
            Some(line) if !line.is_empty() => out.push_str(&format!("  {line}\n")),
            _ => out.push_str("  （无）\n"),
        }
        out.push_str(
            "  mcp_browser__browser_evaluate 须过权限浮层（与 bash 同级）；计划模式会挡。\n",
        );
        out.push_str("\n工具：\n");
        out.push_str(
            "  search_tool / use_tool → mcp_browser__browser_open · navigate · snapshot · click · type · …（21 个）\n",
        );
        out.push_str("\n配置：\n");
        out.push_str("  $DOCK_HOME/browser/user-data\n");
        out.push_str("  $DOCK_HOME/browser/screenshots\n");
        out.push_str("\nEsc 关闭本面板。终端不渲染网页。\n");
        out
    }

    fn refresh_slash(&self) {
        let Some(slash) = self.inner.slash.lock().unwrap().clone() else {
            return;
        };
        let _ = slash.update_overlay(
            "browser",
            "浏览器驾驶舱",
            self.format_cockpit(None),
            "浏览器",
        );
    }
}

pub fn tool_browser() -> Plugin {
    plugin("tool-browser", Inject::from([MCP, SLASH]), |ctx, _: &()| {
        let browser = Browser::new(ctx.clone());
        ctx.provide(BROWSER, browser.clone())?;
        let slash = ctx.require::<Slash>(SLASH)?;
        let disposer = slash.register(SlashEntry {
            command: "browser".into(),
            description: "浏览器驾驶舱".into(),
            kind: ExtraSlashKind::Overlay,
            text: browser.format_cockpit(None),
            title: "浏览器".into(),
            send: false,
        })?;
        browser.bind_slash(slash);
        crate::tools::registry::own_registered(ctx, vec![disposer])?;
        Ok(None)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::slash::slash;
    use crate::tools::mcp::mcp_client;
    use crate::tools::registry::tools;

    fn no_builtin() -> cordis_base::test_env::EnvScope {
        cordis_base::test_env::scoped()
            .set("DOCK_CUA_DRIVER", "off")
            .set(config::BROWSER_MCP_ENV, "off")
    }

    async fn boot() -> (Context, cordis::Fiber) {
        let root = Context::new();
        root.plugin(tools(), ()).unwrap().wait().await.unwrap();
        root.plugin(slash(), ()).unwrap().wait().await.unwrap();
        root.plugin(mcp_client(), ()).unwrap().wait().await.unwrap();
        let fiber = root.plugin(tool_browser(), ()).unwrap();
        fiber.wait().await.unwrap();
        (root, fiber)
    }

    #[tokio::test]
    async fn registers_slash_named_service_and_disposes() {
        let home = tempfile::tempdir().unwrap();
        let _env = no_builtin().set("DOCK_HOME", home.path());
        let (root, fiber) = boot().await;
        let browser = root.get::<Browser>(BROWSER).unwrap();
        assert_eq!(browser.state(), BrowserState::Missing);
        let slash = root.require::<Slash>(SLASH).unwrap();
        assert!(slash.list().iter().any(|e| e.command == "browser"));
        // 进程内不再注册任何 browser_* 工具。
        let tools = root
            .require::<crate::tools::registry::Tools>(crate::names::TOOLS)
            .unwrap();
        assert!(!tools.specs().iter().any(|s| s.name.starts_with("browser_")));

        fiber.dispose().await.unwrap();
        assert!(root.get::<Browser>(BROWSER).is_none());
        assert!(slash.list().iter().all(|e| e.command != "browser"));
    }

    #[tokio::test]
    async fn cockpit_without_row_explains_how_to_enable() {
        let home = tempfile::tempdir().unwrap();
        let _env = no_builtin()
            .set("DOCK_HOME", home.path())
            .remove("DOCK_BROWSER_HEADED");
        let (root, fiber) = boot().await;
        let browser = root.get::<Browser>(BROWSER).unwrap();
        let body = browser.cockpit_body();
        assert!(body.starts_with("未启用"), "{body}");
        for needle in [
            "[mcp_servers.browser]",
            "DOCK_BROWSER_MCP=off",
            "偏好：无头",
            "按 h 切换",
            "每个会话自己的标签页",
            "mcp_browser__browser_evaluate",
            "审批：\n  （无）",
        ] {
            assert!(body.contains(needle), "missing {needle} in {body}");
        }
        let with_approval = browser.format_cockpit(Some("mcp_browser__browser_evaluate — 1+1"));
        assert!(with_approval.contains("mcp_browser__browser_evaluate — 1+1"));
        fiber.dispose().await.unwrap();
    }

    #[tokio::test]
    async fn headed_toggle_persists_and_env_overrides() {
        let home = tempfile::tempdir().unwrap();
        let _env = no_builtin()
            .set("DOCK_HOME", home.path())
            .remove("DOCK_BROWSER_HEADED");
        let (root, fiber) = boot().await;
        let browser = root.get::<Browser>(BROWSER).unwrap();
        assert!(!browser.headed_pref());
        assert!(browser.toggle_headed().unwrap());
        assert!(browser.headed_pref());
        assert!(browser.cockpit_body().contains("偏好：有头"));

        std::env::set_var("DOCK_BROWSER_HEADED", "1");
        browser.set_headed(false).unwrap();
        assert!(!browser.headed_pref());
        assert!(browser.headed_effective());
        assert!(browser.cockpit_body().contains("DOCK_BROWSER_HEADED"));
        fiber.dispose().await.unwrap();
    }
}
