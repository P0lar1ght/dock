use cordis::{Context, Fiber, Result};

use crate::agents::agents;
use crate::llm::{llm, LlmConfig, LlmMode};
use crate::loop_plugin::agent_loop;
use crate::prompt::system_prompt;
use crate::session::sessions;
use crate::tools::tools;

/// Sessions / systemPrompt / agents. No `llm` or `tools` — the harness mounts those.
pub async fn install_core(ctx: &Context) -> Result<()> {
    let fibers = [
        ctx.plugin(sessions(), ())?,
        ctx.plugin(system_prompt(), ())?,
        ctx.plugin(agents(), ())?,
    ];
    for fiber in fibers {
        fiber.wait().await?;
    }
    Ok(())
}

/// Core + echo `"tools"`. No `llm` — the harness mounts that plugin.
pub async fn install_without_llm(ctx: &Context) -> Result<()> {
    install_core(ctx).await?;
    ctx.plugin(tools(), ())?.wait().await?;
    Ok(())
}

async fn mount_five(ctx: &Context, llm_cfg: LlmConfig) -> Result<()> {
    install_without_llm(ctx).await?;
    ctx.plugin(llm(), llm_cfg)?.wait().await?;
    Ok(())
}

/// Mount the five fake services and wait until they have provided.
/// The loop is not included so it can be swapped.
pub async fn install_fakes(ctx: &Context) -> Result<()> {
    mount_five(ctx, LlmConfig::default()).await
}

/// DSH spine-demo style bundle: fakes + the default loop.
pub async fn install_spine(ctx: &Context) -> Result<Fiber> {
    install_fakes(ctx).await?;
    let fiber = ctx.plugin(agent_loop(), ())?;
    fiber.wait().await?;
    Ok(fiber)
}

/// Foundation harness: stub `llm` (no tools) + the five seams + loop.
/// Swap `"tools"` later; do not fork the loop.
pub async fn install_foundation(ctx: &Context) -> Result<Fiber> {
    mount_five(
        ctx,
        LlmConfig {
            mode: LlmMode::Text,
        },
    )
    .await?;
    let fiber = ctx.plugin(agent_loop(), ())?;
    fiber.wait().await?;
    Ok(fiber)
}
