use cordis_app::{cron_driver, session_actor, system_prompt};
use cordis_gateway::gateway;
use cordis_spine::{agent_loop, install_app};
use cordis_tui::tui;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = cordis::Context::new();
    install_app(&root).await?;
    root.plugin(system_prompt(), ())?.wait().await?;
    root.plugin(agent_loop(), ())?.wait().await?;
    root.plugin(session_actor(), ())?.wait().await?;
    root.plugin(gateway(), ())?.wait().await?;
    root.plugin(cron_driver(), ())?.wait().await?;
    root.plugin(tui(), ())?.wait().await?;
    Ok(())
}
