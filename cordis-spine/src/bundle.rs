use cordis::{Context, Fiber, Result};

use crate::agent::agents::agents;
use crate::agent::loop_plugin::agent_loop;
use crate::agent::presets::agent_presets;
use crate::agent::turn::turn;
use crate::host::permissions::permissions;
use crate::host::settings::settings;
use crate::host::slash::slash;
use crate::host::tui_slots::tui_slots;
use crate::llm::compact::compact;
use crate::llm::sampler::{llm, LlmConfig, LlmMode};
use crate::names::DYNAMIC_CORDIS_RUNNER;
use crate::prompt::assemble::system_prompt;
use crate::prompt::context_book::context;
use crate::prompt::project_instructions::project_instructions;
use crate::session::log::sessions;
use crate::session::roster::roster;
use crate::tools::ask_user::tool_ask_user;
use crate::tools::browser::tool_browser;
use crate::tools::computer::tool_computer;
use crate::tools::cron::cron;
use crate::tools::dynamic_runner::{dynamic_runner, DynamicRunner};
use crate::tools::goal::{goal_service, goal_tool_registration};
use crate::tools::jobs::{jobs, tool_jobs};
use crate::tools::lsp::tool_lsp;
use crate::tools::mcp::mcp_client;
use crate::tools::memory::tool_memory;
use crate::tools::monitor::tool_monitor;
use crate::tools::plan_mode::{plan_mode_service, plan_mode_tool_registration};
use crate::tools::registry::{tools, workspace_tools};
use crate::tools::sched::tool_scheduler;
use crate::tools::skills::{skills, tool_skills};
use crate::tools::task::{tool_task, TaskConfig};
use crate::tools::todo_write::{todo_service, todo_tool_registration};
use crate::tools::tool_cordis::tool_cordis;
use crate::tools::web_fetch::{tool_web, web_fetch_params};
use crate::tools::workflow::tool_workflow;

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
    tokio::task::spawn_blocking(cordis_base::tool_output::gc_spill_dir);
    install_core(ctx).await?;
    ctx.plugin(settings(), ())?.wait().await?;
    ctx.plugin(turn(), ())?.wait().await?;
    ctx.plugin(permissions(), ())?.wait().await?;
    ctx.plugin(cron(), ())?.wait().await?;
    ctx.plugin(roster(), ())?.wait().await?;
    ctx.plugin(jobs(), ())?.wait().await?;
    ctx.plugin(slash(), ())?.wait().await?;
    ctx.plugin(skills(), ())?.wait().await?;
    ctx.plugin(project_instructions(), ())?.wait().await?;
    ctx.plugin(tui_slots(), ())?.wait().await?;
    ctx.plugin(agent_presets(), ())?.wait().await?;
    ctx.plugin(workspace_tools(), ())?.wait().await?;
    ctx.plugin(tool_web(), web_fetch_params())?.wait().await?;
    ctx.plugin(tool_browser(), ())?.wait().await?;
    // 根会话的 todos 服务（`TODOS` 按页隔离：分页各自挂 todo_service）。
    ctx.plugin(todo_service(), ())?.wait().await?;
    // todo_write 工具只在全局工具表注册一份，靠执行期 ctx 派发到调用页。
    ctx.plugin(todo_tool_registration(), ())?.wait().await?;
    ctx.plugin(tool_ask_user(), ())?.wait().await?;
    ctx.plugin(tool_jobs(), ())?.wait().await?;
    ctx.plugin(tool_scheduler(), ())?.wait().await?;
    ctx.plugin(tool_task(), TaskConfig::default())?
        .wait()
        .await?;
    ctx.plugin(tool_memory(), ())?.wait().await?;
    ctx.plugin(tool_monitor(), ())?.wait().await?;
    // 根会话的 goal 服务（`GOAL` 按页隔离：分页各自挂 goal_service）。
    ctx.plugin(goal_service(), ())?.wait().await?;
    // update_goal 工具只在全局工具表注册一份，靠执行期 ctx 派发到调用页。
    ctx.plugin(goal_tool_registration(), ())?.wait().await?;
    // 根会话的 plan-mode 服务（`PLAN_MODE` 按页隔离：分页各自挂 plan_mode_service）。
    ctx.plugin(plan_mode_service(), ())?.wait().await?;
    // enter_plan_mode / exit_plan_mode 工具只在全局工具表注册一份，靠执行期 ctx 派发到调用页。
    ctx.plugin(plan_mode_tool_registration(), ())?
        .wait()
        .await?;
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
