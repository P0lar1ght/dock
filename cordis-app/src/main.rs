use std::sync::Arc;

use cordis_app::{cron_driver, session_actor, system_prompt, tab_mount, tab_mount_headless};
use cordis_gateway::{
    devices, gateway, gateway_remote, gateway_serve, ServeConfig, ServeControl, GATEWAY_SERVE,
};
use cordis_spine::{
    agent_loop, apply_restored_preset, install_app, AgentPresets, ApplyRestoredPreset, Sessions,
    AGENT_PRESETS, SESSIONS,
};
use cordis_tui::{tabs, tui, GatewayRef, GATEWAY};

enum ResumeArg {
    Off,
    Latest,
    Id(String),
}

/// `dock` 起 TUI；`dock serve` 起无头网关给父进程（桌面 GUI）用；
/// `dock serve --remote` 起给远程客户端用的常驻网关（设备令牌，前面挂反向代理）；
/// `dock mcp browser` 是内置浏览器 MCP 服务（由 Dock 自己按内置行拉起）；
/// `dock device …` 管设备令牌。
enum Mode {
    Tui,
    Serve(ServeArgs),
    Remote { bind: String },
    McpBrowser,
    Device(DeviceCmd),
}

enum DeviceCmd {
    Add(String),
    List,
    Revoke(String),
}

struct ServeArgs {
    config: ServeConfig,
    bind: String,
}

/// `dock serve` 默认让 OS 分配端口：GUI 从 ready 行里读实际地址，不和 TUI
/// `/pair` 的 18991 抢。
const SERVE_DEFAULT_BIND: &str = "127.0.0.1:0";
const SERVE_DEFAULT_APPLICATION: &str = "dock-gui";
/// `dock serve --remote` 的固定端口：反向代理照它配，占用就报错不顺延。
const REMOTE_DEFAULT_BIND: &str = "127.0.0.1:18990";

fn parse_args() -> (Mode, ResumeArg) {
    let mut args = std::env::args().skip(1).peekable();
    let mut resume = ResumeArg::Off;
    if args.peek().is_some_and(|a| a == "mcp") {
        args.next();
        let service = args.next();
        let extra = args.next();
        return match (service.as_deref(), extra) {
            (Some(cordis_browser::SERVER_NAME), None) => (Mode::McpBrowser, resume),
            (Some(cordis_browser::SERVER_NAME), Some(extra)) => {
                usage_error(&format!("dock mcp browser 不认识参数 {extra}"))
            }
            (Some(other), _) => {
                usage_error(&format!("dock mcp 不认识服务 {other}（目前只有 browser）"))
            }
            (None, _) => usage_error("dock mcp 需要服务名（目前只有 browser）"),
        };
    }
    if args.peek().is_some_and(|a| a == "device") {
        args.next();
        let action = args.next();
        let target = args.next();
        let cmd = match (action.as_deref(), target) {
            (Some("add"), Some(name)) => DeviceCmd::Add(name),
            (Some("list"), None) => DeviceCmd::List,
            (Some("revoke"), Some(which)) => DeviceCmd::Revoke(which),
            (Some("add"), None) => usage_error("dock device add 需要设备名"),
            (Some("revoke"), None) => usage_error("dock device revoke 需要设备名或 id"),
            _ => usage_error("用法：dock device add <名字> | list | revoke <名字或 id>"),
        };
        if let Some(extra) = args.next() {
            usage_error(&format!("dock device 不认识参数 {extra}"));
        }
        return (Mode::Device(cmd), resume);
    }
    let serve = args.peek().is_some_and(|a| a == "serve");
    if serve {
        args.next();
    }
    let mut remote = false;
    let mut bind_given = false;
    let mut origin: Option<String> = None;
    let mut application = SERVE_DEFAULT_APPLICATION.to_string();
    let mut bind = SERVE_DEFAULT_BIND.to_string();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "-V" | "--version" => {
                print_version();
                std::process::exit(0);
            }
            // 后面紧跟的是 id 才吃掉；是下一个开关就留给下一轮（`--resume --origin x`）。
            "-r" | "--resume" => {
                resume = match args.next_if(|next| !next.starts_with('-')) {
                    Some(id) => ResumeArg::Id(id),
                    None => ResumeArg::Latest,
                };
            }
            "--remote" if serve => remote = true,
            "--origin" | "--application" | "--bind" if serve => {
                let Some(value) = args.next() else {
                    usage_error(&format!("{arg} 需要一个值"));
                };
                match arg.as_str() {
                    "--origin" => origin = Some(value),
                    "--application" => application = value,
                    _ => {
                        bind = value;
                        bind_given = true;
                    }
                }
            }
            other if serve => usage_error(&format!("dock serve 不认识参数 {other}")),
            _ => {}
        }
    }
    if !serve {
        return (Mode::Tui, resume);
    }
    if remote {
        if origin.is_some() {
            usage_error("dock serve --remote 不用 --origin：远程客户端用设备令牌鉴权");
        }
        let bind = if bind_given {
            bind
        } else {
            REMOTE_DEFAULT_BIND.to_string()
        };
        return (Mode::Remote { bind }, resume);
    }
    let Some(origin) = origin else {
        usage_error("dock serve 需要 --origin（桌面 GUI webview 的 Origin，如 tauri://localhost）");
    };
    let mode = Mode::Serve(ServeArgs {
        config: ServeConfig {
            application,
            origin,
        },
        bind,
    });
    (mode, resume)
}

fn usage_error(message: &str) -> ! {
    eprintln!("dock: {message}（dock --help 查看用法）");
    std::process::exit(2);
}

fn print_version() {
    println!("dock {}", env!("CARGO_PKG_VERSION"));
}

fn print_help() {
    eprintln!(
        "\
Dock — Grok-shaped TUI on a Cordis plugin tree.

Usage:
  dock [options]
  dock serve --origin <origin> [--application <id>] [--bind <addr>] [options]
  dock serve --remote [--bind <addr>] [options]
  dock device add <name> | list | revoke <name|id>
  dock mcp browser

Options:
  -r, --resume [id]  Restore the latest session for this cwd, or a specific id
  -V, --version      Print version and exit
  -h, --help         Show this help

serve: run headless for a parent process (the desktop GUI). The gateway listens
on loopback right away; stdout carries one JSON line per event: first
{{\"event\":\"ready\",\"ws\":…,\"ticket\":…}}, then a fresh ticket for every
{{\"cmd\":\"ticket\"}} written to stdin. Dock exits when stdin closes.
  --origin <origin>     Origin the tickets are bound to (e.g. tauri://localhost)
  --application <id>    Application id (default dock-gui)
  --bind <addr>         Loopback address (default 127.0.0.1:0, OS-assigned port)

serve --remote: long-running gateway for remote clients (desktop GUI or web UI
on another machine). Still binds loopback only (default 127.0.0.1:18990, never
walks to another port); put a TLS reverse proxy (Caddy / nginx) or Tailscale in
front of it. Clients authenticate with a device token, never a pairing ticket;
pairing HTTP routes are not served. Runs until SIGINT / SIGTERM.

device: manage device tokens for --remote. `add` prints the token once (only its
sha256 is stored in $DOCK_HOME/devices.json); `revoke` takes effect immediately
and drops that device's open connections.

mcp browser: Dock's browser MCP server on stdio (NDJSON JSON-RPC). Dock starts it
itself as the built-in [mcp_servers.browser] row; DOCK_BROWSER_MCP=off disables it.

Sessions are stored in $DOCK_HOME/sessions/<cwd>/ (default ~/.dock/sessions/).
"
    );
}

fn notify_preset_apply(outcome: ApplyRestoredPreset) {
    if let ApplyRestoredPreset::Failed { id, error } = outcome {
        eprintln!("dock: resume preset `{id}` not applied: {error}");
    }
}

fn resume_and_apply_preset(
    ctx: &cordis::Context,
    sessions: &Sessions,
    presets: &AgentPresets,
    id: &str,
) -> bool {
    let stamped = sessions.archived_preset_id(id);
    if !sessions.restore(id) {
        return false;
    }
    cordis_spine::clear_plan_for_session_switch(ctx);
    notify_preset_apply(apply_restored_preset(presets, stamped.as_deref()));
    true
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mode, resume) = parse_args();
    if let Mode::McpBrowser = mode {
        // stdout 是协议通道：这里什么都不能往 stdout 打。
        cordis_browser::serve_stdio().await?;
        return Ok(());
    }
    if let Mode::Device(cmd) = &mode {
        run_device(cmd);
        return Ok(());
    }
    // 内置浏览器行拉起的就是自己这个二进制（`dock mcp browser`）。
    if let Ok(exe) = std::env::current_exe() {
        cordis_spine::register_builtin_browser_mcp(exe);
    }
    let root = cordis::Context::new();
    install_app(&root).await?;
    if let Some(sessions) = root.get::<Sessions>(SESSIONS) {
        sessions.attach_disk();
        if let Some(presets) = root.get::<AgentPresets>(AGENT_PRESETS) {
            // Stamp the live preset on new archives even before `/preset`.
            sessions.seed_preset_if_unset(&presets.current_id());
        }
        match resume {
            ResumeArg::Off => {}
            ResumeArg::Latest => {
                if let Some(presets) = root.get::<AgentPresets>(AGENT_PRESETS) {
                    if let Some(id) = sessions.archived().into_iter().next().map(|s| s.id) {
                        let _ = resume_and_apply_preset(&root, &sessions, &presets, &id);
                    }
                } else if sessions.resume_latest() {
                    cordis_spine::clear_plan_for_session_switch(&root);
                }
            }
            ResumeArg::Id(id) => {
                if let Some(presets) = root.get::<AgentPresets>(AGENT_PRESETS) {
                    let _ = resume_and_apply_preset(&root, &sessions, &presets, &id);
                } else if sessions.resume_id(&id) {
                    cordis_spine::clear_plan_for_session_switch(&root);
                }
            }
        }
    }
    root.plugin(system_prompt(), ())?.wait().await?;
    root.plugin(agent_loop(), ())?.wait().await?;
    root.plugin(session_actor(), ())?.wait().await?;
    match mode {
        Mode::Tui => {
            root.plugin(gateway(), ())?.wait().await?;
            root.plugin(cron_driver(), ())?.wait().await?;
            // 分页服务要在 TUI 之前挂上：事件循环第一帧就会问它当前是哪一页。
            root.plugin(tabs(), tab_mount())?.wait().await?;
            root.plugin(tui(), ())?.wait().await?;
        }
        Mode::Serve(args) => {
            // 无头：不挂 TUI。分页服务照挂（不带视图），网关按线程开页；
            // 网关挂载即监听，stdout 只写控制行。
            root.plugin(gateway_serve(args.bind), args.config)?
                .wait()
                .await?;
            root.plugin(cron_driver(), ())?.wait().await?;
            root.plugin(tabs(), tab_mount_headless())?.wait().await?;
            let control: Arc<ServeControl> = root.require::<ServeControl>(GATEWAY_SERVE)?;
            let stdin = tokio::io::BufReader::new(tokio::io::stdin());
            // 父进程关了 stdin（退出或崩了）就收尾：会话是逐条实时落盘的，不用额外保存。
            control.run(stdin, tokio::io::stdout()).await?;
        }
        Mode::Remote { bind } => {
            root.plugin(gateway_remote(bind), ())?.wait().await?;
            root.plugin(cron_driver(), ())?.wait().await?;
            root.plugin(tabs(), tab_mount_headless())?.wait().await?;
            let addr = root.require::<GatewayRef>(GATEWAY)?.local_addr();
            eprintln!("dock serve --remote：监听 ws://{addr}/api/ws（只绑回环）");
            eprintln!("  对外请在前面挂 TLS 反向代理或 Tailscale，见 docs/REMOTE.md。");
            match devices::list() {
                Ok(list) if list.is_empty() => {
                    eprintln!("  还没有设备令牌：dock device add <名字> 签一枚，客户端才连得上。")
                }
                Ok(list) => eprintln!("  已登记 {} 台设备（dock device list）。", list.len()),
                Err(e) => eprintln!("  读设备名单失败：{e}"),
            }
            wait_for_shutdown().await;
        }
        Mode::McpBrowser | Mode::Device(_) => unreachable!("handled before install_app"),
    }
    Ok(())
}

/// `dock device …`：只动 `$DOCK_HOME/devices.json`，不起 Dock。
fn run_device(cmd: &DeviceCmd) {
    match cmd {
        DeviceCmd::Add(name) => match devices::add(name) {
            Ok((device, token)) => {
                println!(
                    "已添加设备「{}」（id {}）。令牌只显示这一次：",
                    device.name, device.id
                );
                println!();
                println!("{token}");
                println!();
                println!("客户端连接时填：wss://<你的域名>/api/ws 和这个令牌。");
                println!("丢了就撤销重签：dock device revoke {}", device.name);
            }
            Err(e) => fail(&e),
        },
        DeviceCmd::List => match devices::list() {
            Ok(list) if list.is_empty() => println!("还没有设备令牌（dock device add <名字>）。"),
            Ok(list) => {
                for d in list {
                    let used = d
                        .last_used_ms
                        .map(|ms| format!("最后使用 {}", when(ms)))
                        .unwrap_or_else(|| "从未使用".into());
                    println!(
                        "{}  {}  创建于 {}  {}",
                        d.id,
                        d.name,
                        when(d.created_ms),
                        used
                    );
                }
            }
            Err(e) => fail(&e),
        },
        DeviceCmd::Revoke(which) => match devices::revoke(which) {
            Ok(device) => println!(
                "已撤销设备「{}」（id {}）；它连着的连接会在几秒内断开。",
                device.name, device.id
            ),
            Err(e) => fail(&e),
        },
    }
}

fn fail(message: &str) -> ! {
    eprintln!("dock: {message}");
    std::process::exit(1);
}

/// 毫秒时间戳 → 距今多久（不引日期库）。
fn when(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(ms);
    let secs = now.saturating_sub(ms) / 1000;
    match secs {
        0..=59 => "刚刚".into(),
        60..=3599 => format!("{} 分钟前", secs / 60),
        3600..=86_399 => format!("{} 小时前", secs / 3600),
        _ => format!("{} 天前", secs / 86_400),
    }
}

/// 常驻网关等 Ctrl-C 或 SIGTERM（systemd 停服务发的就是它）。会话逐条实时落盘，不用额外保存。
async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    eprintln!("dock serve --remote：收到退出信号，停止。");
}
