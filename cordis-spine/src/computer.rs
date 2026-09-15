//! Computer / CUA cockpit: named `"computer"` + `/computer` slash.
//!
//! Desktop control is **trycua cua-driver** via existing `mcp-client` (public
//! names `mcp_cua-driver__*`). This plugin does **not** implement key/mouse —
//! it live-looks `"mcp"` plus a cached local probe (driver 装没装、macOS 授权
//! 有没有给) and drives the two onboarding actions（安装 / 授权）。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cordis::{plugin, Inject, Plugin};

use crate::cua::{self, Perms};
use crate::mcp::Mcp;
use crate::names::{COMPUTER, MCP, PERMISSIONS, SLASH};
use crate::permissions::Permissions;
use crate::slash::{ExtraSlashKind, Slash, SlashEntry};

/// Config / MCP server key for trycua cua-driver (must match TOOLS.md).
pub use crate::cua::CUA_DRIVER_SERVER;

/// 进度区最多留多少行——安装脚本话很多，驾驶舱只要看得见「现在到哪了」。
const MAX_JOB_LINES: usize = 12;

/// 驾驶舱能发起的两个动作。都要先在确认态里看清楚要跑什么再按。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CuaAction {
    /// 下载并执行 trycua 官方安装脚本（装 / 重装 / 升级）。
    Install,
    /// macOS：跑 `cua-driver permissions grant` 去要 Accessibility / 屏幕录制。
    Grant,
}

impl CuaAction {
    pub fn title(self) -> &'static str {
        match self {
            CuaAction::Install => "安装 cua-driver",
            CuaAction::Grant => "授予系统权限",
        }
    }
}

/// `/computer` 的状态机。判定顺序：插件在不在 → MCP 行的连接状态 → 本机探测。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComputerState {
    /// `mcp-client` 不在树上（`install_app` 之外的裁剪树）。
    NotMounted,
    /// 本机找不到 driver 二进制。
    Missing,
    /// 装了、连上了，但 macOS 没给够授权——工具会在运行时失败。
    NeedsPermission {
        accessibility: bool,
        screen_recording: bool,
    },
    /// 配置里 `enabled = false`。
    Disabled,
    /// MCP 认证（cua-driver 走不到这条，保留给通用 MCP 面）。
    NeedsAuth { detail: String },
    /// 装了但连不上。
    Unreachable { detail: String },
    Connected {
        command: String,
        tools: usize,
        detail: String,
    },
}

impl ComputerState {
    /// 斜杠列表里那一行的后缀。
    pub fn kicker(&self) -> &'static str {
        match self {
            ComputerState::NotMounted => "未挂载",
            ComputerState::Missing => "未安装",
            ComputerState::NeedsPermission { .. } => "缺授权",
            ComputerState::Disabled => "已禁用",
            ComputerState::NeedsAuth { .. } => "需认证",
            ComputerState::Unreachable { .. } => "未连上",
            ComputerState::Connected { .. } => "已连接",
        }
    }
}

/// Named `"computer"` handle. Call sites live-lookup; do not capture the `Arc`.
/// Clone shares the same inner (slash binding + 探测缓存 + 正在跑的动作)。
#[derive(Clone)]
pub struct Computer {
    inner: Arc<ComputerInner>,
}

struct ComputerInner {
    ctx: cordis::Context,
    slash: Mutex<Option<Arc<Slash>>>,
    probe: Mutex<Probe>,
    job: Mutex<Option<Job>>,
}

/// 本机探测结果的缓存。`format_cockpit` 每帧都会被调用，那里**只读缓存**：
/// 逐帧扫 PATH 或起子进程问授权都是不可接受的。刷新点是开 overlay、Ctrl+R、
/// 以及每次安装 / 授权跑完。
#[derive(Clone, Debug, Default)]
struct Probe {
    driver: Option<PathBuf>,
    perms: Perms,
    /// 探过没有。没探过时驾驶舱不敢断言「未安装」。
    probed: bool,
}

struct Job {
    action: CuaAction,
    lines: VecDeque<String>,
    /// `None` = 还在跑。
    finished: Option<Result<String, String>>,
}

impl Computer {
    fn new(ctx: cordis::Context) -> Self {
        Self {
            inner: Arc::new(ComputerInner {
                ctx,
                slash: Mutex::new(None),
                probe: Mutex::new(Probe::default()),
                job: Mutex::new(None),
            }),
        }
    }

    fn bind_slash(&self, slash: Arc<Slash>) {
        *self.inner.slash.lock().unwrap() = Some(slash);
        self.refresh_slash();
    }

    /// 本机探测：找二进制 + （macOS）读一次 TCC 授权。跑在后台，跑完刷新斜杠条目。
    pub fn refresh(&self) {
        let this = self.clone();
        tokio::spawn(async move {
            let driver = cua::discover();
            let perms = match &driver {
                Some(path) => cua::probe_permissions(path).await,
                None => Perms::Unknown,
            };
            *this.inner.probe.lock().unwrap() = Probe {
                driver,
                perms,
                probed: true,
            };
            this.refresh_slash();
        });
    }

    /// 发现到的 driver 路径（探测过才有）。
    pub fn driver_path(&self) -> Option<PathBuf> {
        self.inner.probe.lock().unwrap().driver.clone()
    }

    /// 有动作在跑（TUI 靠它决定要不要持续重绘）。
    pub fn busy(&self) -> bool {
        self.inner
            .job
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|job| job.finished.is_none())
    }

    /// 底栏 hint 用：`i` 这次是装还是重装。
    pub fn install_label(&self) -> &'static str {
        if self.driver_path().is_some() {
            "reinstall"
        } else {
            "install"
        }
    }

    /// 底栏 hint 用：这台机器上 `p` 有没有意义。
    pub fn grant_available(&self) -> bool {
        cfg!(target_os = "macos")
            && self.driver_path().is_some()
            && !matches!(self.inner.probe.lock().unwrap().perms, Perms::NotRequired)
    }

    /// 起一个动作。已经有动作在跑、或这台机器上没意义时返回 Err（TUI flash 它）。
    pub fn start(&self, action: CuaAction) -> Result<(), String> {
        if self.busy() {
            return Err("上一个动作还在跑".into());
        }
        let driver = self.driver_path();
        if action == CuaAction::Grant {
            if !cfg!(target_os = "macos") {
                return Err("只有 macOS 需要 Accessibility / 屏幕录制授权".into());
            }
            if driver.is_none() {
                return Err("还没装 cua-driver，先按 i 安装".into());
            }
        }
        *self.inner.job.lock().unwrap() = Some(Job {
            action,
            lines: VecDeque::new(),
            finished: None,
        });
        self.refresh_slash();

        let this = self.clone();
        let sink: cua::LineSink = {
            let this = self.clone();
            Arc::new(move |line: String| this.push_line(line))
        };
        tokio::spawn(async move {
            let result = match action {
                CuaAction::Install => cua::install(sink).await,
                CuaAction::Grant => match driver {
                    Some(path) => cua::grant(&path, sink).await,
                    None => Err("还没装 cua-driver".into()),
                },
            };
            // 装完 / 授权完都要重新探测，再让 MCP 对一次账：不重启 dock 就能连上。
            let driver = cua::discover();
            let perms = match &driver {
                Some(path) => cua::probe_permissions(path).await,
                None => Perms::Unknown,
            };
            *this.inner.probe.lock().unwrap() = Probe {
                driver,
                perms,
                probed: true,
            };
            let done = match result {
                Ok(()) => {
                    let reloaded = this.reload_mcp().await;
                    Ok(match action {
                        CuaAction::Install => format!("cua-driver 已安装。{reloaded}"),
                        CuaAction::Grant => format!("授权流程已走完。{reloaded}"),
                    })
                }
                Err(e) => Err(format!("{}失败：{e}", action.title())),
            };
            if let Some(job) = this.inner.job.lock().unwrap().as_mut() {
                job.finished = Some(done);
            }
            this.refresh_slash();
        });
        Ok(())
    }

    /// 清掉已完成动作的进度区（关 overlay 时调）。跑着的不动。
    pub fn clear_finished(&self) {
        let mut job = self.inner.job.lock().unwrap();
        if job.as_ref().is_some_and(|j| j.finished.is_some()) {
            *job = None;
        }
    }

    async fn reload_mcp(&self) -> String {
        let Some(mcp) = self.inner.ctx.get::<Mcp>(MCP) else {
            return "MCP 未挂载。".into();
        };
        match mcp.reload().await {
            Ok(report) => report.summary(),
            Err(e) => format!("MCP 重载失败：{e}"),
        }
    }

    fn push_line(&self, line: String) {
        let mut job = self.inner.job.lock().unwrap();
        if let Some(job) = job.as_mut() {
            if job.lines.len() == MAX_JOB_LINES {
                job.lines.pop_front();
            }
            job.lines.push_back(line);
        }
    }

    /// 当前状态。每帧调用，只读缓存 + MCP 列表。
    pub fn state(&self) -> ComputerState {
        let Some(mcp) = self.inner.ctx.get::<Mcp>(MCP) else {
            return ComputerState::NotMounted;
        };
        let probe = self.inner.probe.lock().unwrap().clone();
        let list = mcp.list();
        let status = list.iter().find(|s| s.name == CUA_DRIVER_SERVER);
        match status {
            // 连上了还缺授权：工具调得动但会在 TCC 上失败，先催授权。
            Some(s) if s.enabled && s.ok => {
                if let Perms::Missing {
                    accessibility,
                    screen_recording,
                } = probe.perms
                {
                    return ComputerState::NeedsPermission {
                        accessibility,
                        screen_recording,
                    };
                }
                ComputerState::Connected {
                    command: s.command.clone(),
                    tools: s.tools.iter().filter(|t| t.enabled).count(),
                    detail: s.detail.clone(),
                }
            }
            Some(s) if !s.enabled => ComputerState::Disabled,
            Some(s) if s.needs_auth => ComputerState::NeedsAuth {
                detail: s.detail.clone(),
            },
            Some(s) => {
                if probe.probed && probe.driver.is_none() {
                    ComputerState::Missing
                } else {
                    ComputerState::Unreachable {
                        detail: s.detail.clone(),
                    }
                }
            }
            // 没有这条行：装了才会有内置行，所以没行基本等于没装。
            None => {
                if probe.probed && probe.driver.is_some() {
                    ComputerState::Unreachable {
                        detail: "config 里没有 [mcp_servers.cua-driver]；Ctrl+R 重载配置".into(),
                    }
                } else {
                    ComputerState::Missing
                }
            }
        }
    }

    /// Live-look MCP + optional permissions front for the cockpit body.
    pub fn format_cockpit(&self, approval_line: Option<&str>) -> String {
        self.format_cockpit_with(approval_line, None)
    }

    /// `pending` 非空时先渲染确认块：按下去会跑什么，一条条列出来。
    pub fn format_cockpit_with(
        &self,
        approval_line: Option<&str>,
        pending: Option<CuaAction>,
    ) -> String {
        let approval = approval_line.map(str::to_string).or_else(|| {
            self.inner
                .ctx
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

        let mut out = String::from("电脑 / CUA（cua-driver MCP）\n\n");
        self.append_status(&mut out);
        if let Some(action) = pending {
            self.append_confirm(&mut out, action);
        }
        self.append_job(&mut out);
        self.append_footer(&mut out, approval.as_deref());
        out
    }

    fn append_status(&self, out: &mut String) {
        let driver = self.driver_path();
        match self.state() {
            ComputerState::NotMounted => {
                out.push_str("状态：mcp-client 未挂载\n");
            }
            ComputerState::Missing => {
                out.push_str("状态：未安装 cua-driver\n");
                out.push_str("  Dock 不自带 driver：macOS 上它是 trycua 签名的 app，系统授权绑那份签名身份。\n");
                out.push_str("  按 i 从官方脚本安装（会先让你确认要跑什么）。\n");
                out.push_str(
                    "  已经装过：放进 PATH 或设 DOCK_CUA_DRIVER=<绝对路径>，再按 Ctrl+R 重新检测。\n",
                );
            }
            ComputerState::NeedsPermission {
                accessibility,
                screen_recording,
            } => {
                out.push_str("状态：已安装，缺系统授权\n");
                out.push_str(&format!(
                    "  辅助功能：{}　屏幕录制：{}\n",
                    mark(accessibility),
                    mark(screen_recording)
                ));
                out.push_str("  按 p 授权：由 CuaDriver 自己拉起系统弹窗（归属到 com.trycua.driver），再去系统设置打开开关。\n");
            }
            ComputerState::Disabled => {
                out.push_str("状态：已禁用\n");
                out.push_str("  /mcps 里 Space 启用 cua-driver，或改 config enabled=true。\n");
            }
            ComputerState::NeedsAuth { detail } => {
                out.push_str("状态：需认证\n");
                out.push_str(&format!("  {detail}\n"));
            }
            ComputerState::Unreachable { detail } => {
                out.push_str("状态：未连上\n");
                if !detail.is_empty() {
                    out.push_str(&format!("  {detail}\n"));
                }
                if let Some(path) = &driver {
                    out.push_str(&format!("  driver：{}\n", path.display()));
                }
                out.push_str("  Ctrl+R 重新检测并重载 MCP 配置；按 i 可以重装 / 升级。\n");
                out.push_str("  Linux 还要 X11/XWayland 与 at-spi2-core（见 TOOLS.md）。\n");
            }
            ComputerState::Connected {
                command,
                tools,
                detail,
            } => {
                out.push_str("状态：已连接\n");
                out.push_str(&format!("  command：{command}\n"));
                out.push_str(&format!(
                    "  工具：{tools} 个已启用（公名 mcp_cua-driver__*）\n"
                ));
                if !detail.is_empty() {
                    out.push_str(&format!("  详情：{detail}\n"));
                }
            }
        }
        out.push('\n');
    }

    fn append_confirm(&self, out: &mut String, action: CuaAction) {
        out.push_str(&format!("确认：{}\n", action.title()));
        let lines = match action {
            CuaAction::Install => cua::install_plan_lines(),
            CuaAction::Grant => vec![
                format!(
                    "{} permissions grant",
                    self.driver_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "cua-driver".into())
                ),
                "会拉起 CuaDriver 并弹系统授权对话框，随后你要去系统设置里打开开关。".into(),
            ],
        };
        for line in lines {
            out.push_str(&format!("  {line}\n"));
        }
        out.push_str("  Enter 确认执行，Esc 取消。\n\n");
    }

    fn append_job(&self, out: &mut String) {
        let job = self.inner.job.lock().unwrap();
        let Some(job) = job.as_ref() else {
            return;
        };
        match &job.finished {
            None => out.push_str(&format!("{}（进行中）：\n", job.action.title())),
            Some(Ok(msg)) => out.push_str(&format!("{}：完成 — {msg}\n", job.action.title())),
            Some(Err(msg)) => out.push_str(&format!("{}：{msg}\n", job.action.title())),
        }
        for line in &job.lines {
            out.push_str(&format!("  {line}\n"));
        }
        out.push('\n');
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
        let desc = format!("电脑驾驶舱（{}）", self.state().kicker());
        let _ = slash.update_overlay("computer", &desc, body, "电脑");
    }
}

fn mark(ok: bool) -> &'static str {
    if ok {
        "已授权"
    } else {
        "未授权"
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
            // 挂上就探一次：斜杠列表里的「未安装 / 已连接」第一帧就得是真的。
            computer.refresh();
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

    /// 单测不该真去连本机 driver：内置 MCP 行一旦注入，`mcp-client` 会 spawn
    /// 真的 cua-driver 守护进程。
    fn no_driver() -> crate::test_env::EnvScope {
        crate::test_env::scoped()
            .home()
            .set(crate::cua::DRIVER_ENV, "off")
    }

    #[tokio::test]
    async fn registers_slash_named_service_and_disposes() {
        let _env = no_driver();
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

        let _env = no_driver();
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
            slash.list().iter().any(|e| e.command == "computer"),
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

    /// 零配置活体冒烟：本机装了 driver、`config.toml` 里一个字都没写，也该连上。
    /// 默认不跑 —— 它会真的 spawn 一个 `cua-driver mcp` 子进程。
    #[tokio::test]
    #[ignore = "needs cua-driver installed on this machine; spawns the real driver over stdio"]
    async fn zero_config_connects_to_a_real_driver() {
        use crate::mcp::mcp_client;
        use crate::tools::tools;

        // HOME / cwd 都指向空目录：内置行是唯一来源，项目 .dock/config.toml
        // 不能掺进来；同时放开 DOCK_CUA_DRIVER 走真实发现。
        let cwd = tempfile::tempdir().unwrap();
        let _env = crate::test_env::scoped()
            .home()
            .cwd(cwd.path())
            .remove(crate::cua::DRIVER_ENV);

        let root = Context::new();
        root.plugin(slash(), ()).unwrap().wait().await.unwrap();
        root.plugin(tools(), ()).unwrap().wait().await.unwrap();
        root.plugin(mcp_client(), ()).unwrap().wait().await.unwrap();
        root.plugin(tool_computer(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let computer = root.get::<Computer>(COMPUTER).unwrap();

        let mut state = computer.state();
        for _ in 0..150 {
            if matches!(state, ComputerState::Connected { .. }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            state = computer.state();
        }
        let ComputerState::Connected { command, tools, .. } = state else {
            panic!(
                "零配置没连上：{:?}\n{}",
                state,
                computer.format_cockpit(None)
            );
        };
        assert!(command.ends_with("cua-driver"), "{command}");
        // 0.28.1 是 56 个；给个下限，免得「连上了但工具表是空的」也算过。
        assert!(tools >= 10, "只有 {tools} 个工具，八成没真连上");
    }

    /// 没装 driver 时驾驶舱要给出下一步，而不是只说「未配置」。
    #[tokio::test]
    async fn missing_driver_cockpit_guides_install() {
        use crate::mcp::mcp_client;
        use crate::tools::tools;

        let _env = no_driver();
        let root = Context::new();
        root.plugin(slash(), ()).unwrap().wait().await.unwrap();
        root.plugin(tools(), ()).unwrap().wait().await.unwrap();
        root.plugin(mcp_client(), ()).unwrap().wait().await.unwrap();
        root.plugin(tool_computer(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let computer = root.get::<Computer>(COMPUTER).unwrap();
        // 挂载时那次探测是后台的，等它落地。
        for _ in 0..50 {
            if matches!(computer.state(), ComputerState::Missing) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(computer.state(), ComputerState::Missing);
        let body = computer.format_cockpit(None);
        assert!(body.contains("未安装 cua-driver"), "{body}");
        assert!(body.contains("按 i"), "{body}");
        assert_eq!(computer.install_label(), "install");
        assert!(!computer.busy());
    }

    /// 确认块必须把要执行的东西写出来——按 i 不能直接开跑外部脚本。
    #[tokio::test]
    async fn confirm_block_shows_what_will_run() {
        use crate::mcp::mcp_client;
        use crate::tools::tools;

        let _env = no_driver();
        let root = Context::new();
        root.plugin(slash(), ()).unwrap().wait().await.unwrap();
        root.plugin(tools(), ()).unwrap().wait().await.unwrap();
        root.plugin(mcp_client(), ()).unwrap().wait().await.unwrap();
        root.plugin(tool_computer(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let computer = root.get::<Computer>(COMPUTER).unwrap();
        let body = computer.format_cockpit_with(None, Some(CuaAction::Install));
        assert!(body.contains("确认：安装 cua-driver"), "{body}");
        assert!(body.contains(crate::cua::INSTALL_SCRIPT_URL), "{body}");
        assert!(body.contains("Enter 确认执行"), "{body}");
    }

    /// 没装 driver 时按 p 要说人话，而不是起一个注定失败的子进程。
    #[tokio::test]
    async fn grant_without_driver_is_refused() {
        use crate::mcp::mcp_client;
        use crate::tools::tools;

        let _env = no_driver();
        let root = Context::new();
        root.plugin(slash(), ()).unwrap().wait().await.unwrap();
        root.plugin(tools(), ()).unwrap().wait().await.unwrap();
        root.plugin(mcp_client(), ()).unwrap().wait().await.unwrap();
        root.plugin(tool_computer(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let computer = root.get::<Computer>(COMPUTER).unwrap();
        let err = computer.start(CuaAction::Grant).unwrap_err();
        assert!(!err.is_empty(), "拒绝要带原因");
        assert!(!computer.busy(), "被拒的动作不该占住驾驶舱");
        assert!(!computer.grant_available());
    }
}
