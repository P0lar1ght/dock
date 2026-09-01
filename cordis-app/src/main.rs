use cordis_app::{cron_driver, session_actor, system_prompt};
use cordis_gateway::gateway;
use cordis_spine::{agent_loop, install_app, Sessions, SESSIONS};
use cordis_tui::tui;

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
            "-r" | "--resume" => {
                resume = match args.next() {
                    Some(id) if !id.starts_with('-') => ResumeArg::Id(id),
                    Some(flag) if matches!(flag.as_str(), "-h" | "--help") => {
                        print_help();
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

fn print_help() {
    eprintln!(
        "\
Dock — Grok-shaped TUI on a Cordis plugin tree.

Usage:
  cargo run -p cordis-app -- [options]

Options:
  -r, --resume [id]  Restore the latest session for this cwd, or a specific id
  -h, --help         Show this help

Sessions are stored in $DOCK_HOME/sessions/<cwd>/ (default ~/.dock/sessions/).
"
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let resume = parse_args();
    let root = cordis::Context::new();
    install_app(&root).await?;
    if let Some(sessions) = root.get::<Sessions>(SESSIONS) {
        sessions.attach_disk();
        match resume {
            ResumeArg::Off => {}
            ResumeArg::Latest => {
                let _ = sessions.resume_latest();
            }
            ResumeArg::Id(id) => {
                let _ = sessions.resume_id(&id);
            }
        }
    }
    root.plugin(system_prompt(), ())?.wait().await?;
    root.plugin(agent_loop(), ())?.wait().await?;
    root.plugin(session_actor(), ())?.wait().await?;
    root.plugin(gateway(), ())?.wait().await?;
    root.plugin(cron_driver(), ())?.wait().await?;
    root.plugin(tui(), ())?.wait().await?;
    Ok(())
}
