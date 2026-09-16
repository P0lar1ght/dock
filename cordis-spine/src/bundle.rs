use cordis::{Context, Fiber, Result};

use crate::agent_presets::agent_presets;
use crate::agents::agents;
use crate::ask_user::tool_ask_user;
use crate::browser::tool_browser;
use crate::compact::compact;
use crate::computer::tool_computer;
use crate::context_book::context;
use crate::cron::cron;
use crate::dynamic_runner::{dynamic_runner, DynamicRunner};
use crate::goal::tool_goal;
use crate::jobs::{jobs, tool_jobs};
use crate::llm::{llm, LlmConfig, LlmMode};
use crate::loop_plugin::agent_loop;
use crate::lsp::tool_lsp;
use crate::mcp::mcp_client;
use crate::memory::tool_memory;
use crate::monitor::tool_monitor;
use crate::names::DYNAMIC_CORDIS_RUNNER;
use crate::permissions::permissions;
use crate::plan_mode::plan_mode;
use crate::project_instructions::project_instructions;
use crate::prompt::system_prompt;
use crate::sched::tool_scheduler;
use crate::session::sessions;
use crate::settings::settings;
use crate::skills::{skills, tool_skills};
use crate::slash::slash;
use crate::task::{tool_task, TaskConfig};
use crate::todo_write::tool_todo;
use crate::tool_cordis::tool_cordis;
use crate::tools::{tools, workspace_tools};
use crate::tui_slots::tui_slots;
use crate::turn::turn;
use crate::web_fetch::tool_web;
use crate::workflow::tool_workflow;

/// Sessions / context / systemPrompt / agents. No `llm` or `tools` — the harness mounts those.
pub async fn install_core(ctx: &Context) -> Result<()> {
    let fibers = [
        ctx.plugin(sessions(), ())?,
        ctx.plugin(context(), ())?,
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
            ..LlmConfig::default()
        },
    )
    .await?;
    let fiber = ctx.plugin(agent_loop(), ())?;
    fiber.wait().await?;
    Ok(fiber)
}

/// App bundle: workspace tools + capability plugins that `register` into `"tools"`.
/// Settings + cron are live-looked-up; the loop is not included so the harness can swap it.
pub async fn install_app(ctx: &Context) -> Result<()> {
    // 工具落盘副本（`use_tool` / `grep` 的溢出、超预算 bash 的完整输出）没有
    // 自然的删除时机：写它们的那一轮结束后没人再负责。只在启动扫一次——此刻
    // 本进程一个文件都还没写，删不到正在用的；会话中途跑才有那个风险。
    tokio::task::spawn_blocking(crate::tool_output::gc_spill_dir);
    install_core(ctx).await?;
    ctx.plugin(settings(), ())?.wait().await?;
    ctx.plugin(turn(), ())?.wait().await?;
    ctx.plugin(permissions(), ())?.wait().await?;
    ctx.plugin(cron(), ())?.wait().await?;
    ctx.plugin(jobs(), ())?.wait().await?;
    ctx.plugin(slash(), ())?.wait().await?;
    ctx.plugin(skills(), ())?.wait().await?;
    ctx.plugin(project_instructions(), ())?.wait().await?;
    ctx.plugin(tui_slots(), ())?.wait().await?;
    ctx.plugin(agent_presets(), ())?.wait().await?;
    ctx.plugin(workspace_tools(), ())?.wait().await?;
    ctx.plugin(tool_web(), ())?.wait().await?;
    ctx.plugin(tool_browser(), ())?.wait().await?;
    ctx.plugin(tool_todo(), ())?.wait().await?;
    ctx.plugin(plan_mode(), ())?.wait().await?;
    ctx.plugin(tool_ask_user(), ())?.wait().await?;
    ctx.plugin(tool_jobs(), ())?.wait().await?;
    ctx.plugin(tool_scheduler(), ())?.wait().await?;
    ctx.plugin(tool_task(), TaskConfig::default())?
        .wait()
        .await?;
    ctx.plugin(tool_memory(), ())?.wait().await?;
    ctx.plugin(tool_monitor(), ())?.wait().await?;
    ctx.plugin(tool_goal(), ())?.wait().await?;
    ctx.plugin(tool_lsp(), ())?.wait().await?;
    ctx.plugin(tool_skills(), ())?.wait().await?;
    ctx.plugin(tool_workflow(), ())?.wait().await?;
    ctx.plugin(mcp_client(), ())?.wait().await?;
    ctx.plugin(tool_computer(), ())?.wait().await?;
    ctx.plugin(dynamic_runner(), ())?.wait().await?;
    if let Some(runner) = ctx.get::<DynamicRunner>(DYNAMIC_CORDIS_RUNNER) {
        tokio::spawn(async move {
            runner.boot_disk().await;
        });
    }
    ctx.plugin(tool_cordis(), ())?.wait().await?;
    ctx.plugin(llm(), LlmConfig::from_env())?.wait().await?;
    ctx.plugin(compact(), ())?.wait().await?;
    Ok(())
}
