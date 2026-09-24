use std::sync::Arc;

use cordis_app::{cron_driver, session_actor, system_prompt, tab_mount, tab_mount_headless};
use cordis_gateway::{gateway, gateway_serve, ServeConfig, ServeControl, GATEWAY_SERVE};
use cordis_spine::{
    agent_loop, apply_restored_preset, install_app, AgentPresets, ApplyRestoredPreset, Sessions,
    AGENT_PRESETS, SESSIONS,
};
use cordis_tui::{tabs, tui};

enum ResumeArg {
    Off,
    Latest,
    Id(String),
}

/// `dock` 起 TUI；`dock serve` 起无头网关给父进程（桌面 GUI）用。
enum Mode {
    Tui,
    Serve(ServeArgs),
}

struct ServeArgs {
    config: ServeConfig,
    bind: String,
}

/// `dock serve` 默认让 OS 分配端口：GUI 从 ready 行里读实际地址，不和 TUI
/// `/pair` 的 18991 抢。
const SERVE_DEFAULT_BIND: &str = "127.0.0.1:0";
const SERVE_DEFAULT_APPLICATION: &str = "dock-gui";

fn parse_args() -> (Mode, ResumeArg) {
    let mut args = std::env::args().skip(1).peekable();
    let mut resume = ResumeArg::Off;
    let serve = args.peek().is_some_and(|a| a == "serve");
    if serve {
        args.next();
    }
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
            "--origin" | "--application" | "--bind" if serve => {
                let Some(value) = args.next() else {
                    usage_error(&format!("{arg} 需要一个值"));
                };
                match arg.as_str() {
                    "--origin" => origin = Some(value),
                    "--application" => application = value,
                    _ => bind = value,
                }
            }
            other if serve => usage_error(&format!("dock serve 不认识参数 {other}")),
            _ => {}
        }
    }
    if !serve {
        return (Mode::Tui, resume);
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
    }
    Ok(())
}
