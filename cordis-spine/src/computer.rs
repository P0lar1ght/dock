//! Thin Computer / CUA cockpit: named `"computer"` + `/computer` slash.
//!
//! Desktop control is **trycua cua-driver** via existing `mcp-client` (public
//! names `mcp_cua-driver__*`). This plugin does **not** implement key/mouse —
//! it live-looks `"mcp"` for cua-driver connection status and registers a
//! reversible slash extra for the TUI thin cockpit.

use std::sync::{Arc, Mutex};

use cordis::{plugin, Inject, Plugin};

use crate::mcp::Mcp;
use crate::names::{COMPUTER, MCP, PERMISSIONS, SLASH};
use crate::permissions::Permissions;
use crate::slash::{ExtraSlashKind, Slash, SlashEntry};

/// Config / MCP server key for trycua cua-driver (must match TOOLS.md).
pub const CUA_DRIVER_SERVER: &str = "cua-driver";

/// Named `"computer"` handle. Call sites live-lookup; do not capture the `Arc`.
/// Clone shares the same inner (slash binding).
#[derive(Clone)]
pub struct Computer {
    inner: Arc<ComputerInner>,
}

struct ComputerInner {
    ctx: cordis::Context,
    slash: Mutex<Option<Arc<Slash>>>,
}

impl Computer {
    fn new(ctx: cordis::Context) -> Self {
        Self {
            inner: Arc::new(ComputerInner {
                ctx,
                slash: Mutex::new(None),
            }),
        }
    }

    fn bind_slash(&self, slash: Arc<Slash>) {
        *self.inner.slash.lock().unwrap() = Some(slash);
        self.refresh_slash();
    }

    /// Live-look MCP + optional permissions front for the cockpit body.
    pub fn format_cockpit(&self, approval_line: Option<&str>) -> String {
        let approval = approval_line
            .map(str::to_string)
            .or_else(|| {
                self.inner.ctx
                    .get::<Permissions>(PERMISSIONS)
                    .and_then(|p| p.front())
                    .filter(|prompt| prompt.tool.starts_with("mcp_cua-driver__"))
                    .map(|prompt| {
                        if prompt.summary.is_empty() {
                            prompt.tool.clone()
                        } else {
                            format!("{} — {}", prompt.tool, prompt.summary)
                        }
                    })
            });
        self.format_cockpit_inner(approval.as_deref())
    }

    fn format_cockpit_inner(&self, approval_line: Option<&str>) -> String {
        let mut out = String::from("电脑 / CUA（cua-driver MCP）\n\n");

        match self.inner.ctx.get::<Mcp>(MCP) {
            None => {
                out.push_str("状态：mcp-client 未挂载\n");
            }
            Some(mcp) => {
                let list = mcp.list();
                let Some(status) = list.iter().find(|s| s.name == CUA_DRIVER_SERVER) else {
                    out.push_str("状态：未配置\n");
                    out.push_str(
                        "  config.toml 启用 [mcp_servers.cua-driver]（见 config.toml.example）。\n",
                    );
                    out.push_str("  本机安装：cua-driver --version / cua-driver doctor\n");
                    out.push('\n');
                    self.append_footer(&mut out, approval_line);
                    return out;
                };

                if !status.enabled {
                    out.push_str("状态：已禁用\n");
                    out.push_str("  /mcps 里 Space 启用 cua-driver，或改 config enabled=true。\n");
                } else if status.ok {
                    out.push_str("状态：已连接\n");
                    out.push_str(&format!("  command：{}\n", status.command));
                    let n = status.tools.iter().filter(|t| t.enabled).count();
                    out.push_str(&format!(
                        "  工具：{} 个已启用（公名 mcp_cua-driver__*）\n",
                        n
                    ));
                    if !status.detail.is_empty() {
                        out.push_str(&format!("  详情：{}\n", status.detail));
                    }
                } else if status.needs_auth {
                    out.push_str("状态：需认证\n");
                    out.push_str(&format!("  {}\n", status.detail));
                } else {
                    out.push_str("状态：未装 / 连不上\n");
                    if status.detail.is_empty() {
                        out.push_str(
                            "  检查 PATH 上的 cua-driver、DISPLAY/X11、AT-SPI（见 TOOLS.md）。\n",
                        );
                    } else {
                        out.push_str(&format!("  {}\n", status.detail));
                    }
                }
            }
        }

        out.push('\n');
        self.append_footer(&mut out, approval_line);
        out
    }

    fn append_footer(&self, out: &mut String, approval_line: Option<&str>) {
        out.push_str("能力：\n");
        out.push_str(
            "  桌面键鼠 / 开应用走 search_tool → use_tool(\"mcp_cua-driver__…\")；全部与 bash 同级 permissions。\n",
        );
        out.push_str(
            "  勿与 Dock browser_*（chromiumoxide）混淆；cua-driver 自带 browser_* 也不是 BUA。\n",
        );
        out.push_str("  不嵌真桌面；无 Docker / 不自研键鼠。\n");
        if let Some(line) = approval_line {
            out.push('\n');
            out.push_str("审批：\n");
            out.push_str(&format!("  {line}\n"));
        } else {
            out.push('\n');
            out.push_str("审批：无挂起（cua-driver 工具会走通用 permissions 浮层）\n");
        }
    }

    fn refresh_slash(&self) {
        let Some(slash) = self.inner.slash.lock().unwrap().clone() else {
            return;
        };
        let body = self.format_cockpit(None);
        let desc = match self.inner.ctx.get::<Mcp>(MCP).and_then(|m| {
            m.list()
                .into_iter()
                .find(|s| s.name == CUA_DRIVER_SERVER)
        }) {
            Some(s) if s.enabled && s.ok => "电脑驾驶舱（已连接）",
            Some(s) if !s.enabled => "电脑驾驶舱（已禁用）",
            Some(_) => "电脑驾驶舱（未连上）",
            None => "电脑驾驶舱（未配置）",
        };
        let _ = slash.update_overlay("computer", desc, body, "电脑");
    }
}

/// Mount named `"computer"` + `/computer` slash. Injects MCP + SLASH.
pub fn tool_computer() -> Plugin {
    plugin(
        "tool-computer",
        Inject::from([MCP, SLASH]),
        |ctx, _: &()| {
            let computer = Computer::new(ctx.clone());
            ctx.provide(COMPUTER, computer.clone())?;
            let slash = ctx.require::<Slash>(SLASH)?;
            let body = computer.format_cockpit(None);
            let disposer = slash.register(SlashEntry {
                command: "computer".into(),
                description: "电脑驾驶舱（cua-driver）".into(),
                kind: ExtraSlashKind::Overlay,
                text: body,
                title: "电脑".into(),
                send: false,
            })?;
            computer.bind_slash(slash);
            // own_registered registers the disposer on the fiber; provide is also an effect.
            crate::tools::own_registered(ctx, vec![disposer])?;
            Ok(None)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis::Context;

    use crate::slash::slash;

    #[tokio::test]
    async fn registers_slash_named_service_and_disposes() {
        let root = Context::new();
        root.plugin(slash(), ()).unwrap().wait().await.unwrap();
        // MCP missing → plugin stays Pending until MCP exists; provide a stub via empty mcp? 
        // Inject requires MCP — mount without MCP should leave fiber pending.
        let pending = root.plugin(tool_computer(), ()).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            root.get::<Computer>(COMPUTER).is_none(),
            "computer must not provide before MCP"
        );
        pending.dispose().await.unwrap();
    }

    #[tokio::test]
    async fn with_mcp_registers_and_disposes() {
        use crate::mcp::mcp_client;
        use crate::tools::tools;

        let root = Context::new();
        root.plugin(slash(), ()).unwrap().wait().await.unwrap();
        root.plugin(tools(), ()).unwrap().wait().await.unwrap();
        root.plugin(mcp_client(), ()).unwrap().wait().await.unwrap();
        let fiber = root.plugin(tool_computer(), ()).unwrap();
        fiber.wait().await.unwrap();

        let computer = root.get::<Computer>(COMPUTER).expect("computer");
        let body = computer.format_cockpit(None);
        assert!(body.contains("cua-driver"), "{body}");
        assert!(body.contains("mcp_cua-driver__"), "{body}");

        let slash = root.get::<Slash>(SLASH).unwrap();
        assert!(
            slash
                .list()
                .iter()
                .any(|e| e.command == "computer"),
            "slash missing computer"
        );

        fiber.dispose().await.unwrap();
        assert!(root.get::<Computer>(COMPUTER).is_none());
        let slash = root.get::<Slash>(SLASH).unwrap();
        assert!(
            !slash.list().iter().any(|e| e.command == "computer"),
            "computer slash survived dispose"
        );
    }
}
