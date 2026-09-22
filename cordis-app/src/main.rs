use cordis_app::{cron_driver, session_actor, system_prompt, tab_mount};
use cordis_gateway::gateway;
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

fn parse_args() -> ResumeArg {
    let mut args = std::env::args().skip(1);
    let mut resume = ResumeArg::Off;
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
            "-r" | "--resume" => {
                resume = match args.next() {
                    Some(id) if !id.starts_with('-') => ResumeArg::Id(id),
                    Some(flag) if matches!(flag.as_str(), "-h" | "--help") => {
                        print_help();
                        std::process::exit(0);
                    }
                    Some(flag) if matches!(flag.as_str(), "-V" | "--version") => {
                        print_version();
                        std::process::exit(0);
                    }
                    Some(_) | None => ResumeArg::Latest,
                };
            }
            _ => {}
        }
    }
    resume
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

Options:
  -r, --resume [id]  Restore the latest session for this cwd, or a specific id
  -V, --version      Print version and exit
  -h, --help         Show this help

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
    let resume = parse_args();
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
    root.plugin(gateway(), ())?.wait().await?;
    root.plugin(cron_driver(), ())?.wait().await?;
    // 分页服务要在 TUI 之前挂上：事件循环第一帧就会问它当前是哪一页。
    root.plugin(tabs(), tab_mount())?.wait().await?;
    root.plugin(tui(), ())?.wait().await?;
    Ok(())
}
