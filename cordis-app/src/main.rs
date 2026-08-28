use cordis_app::session_actor;
use cordis_spine::{agent_loop, install_fakes};
use cordis_tui::tui;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = cordis::Context::new();
    install_fakes(&root).await?;
    root.plugin(agent_loop(), ())?.wait().await?;
    root.plugin(session_actor(), ())?.wait().await?;
    root.plugin(tui(), ())?.wait().await?;
    Ok(())
}
