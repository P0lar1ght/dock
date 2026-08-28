use cordis_app::session_actor;
use cordis_spine::{
    agent_loop, install_app, Cron, CRON, PROMPT_ASSEMBLE,
};
use cordis_tui::{tui, SessionRef, SESSION_PORT};

const WORKSPACE_PROMPT: &str = "You are a coding agent in a local workspace. \
Use list_dir, read_file, grep, search_replace, glob, write_file, and bash when you need filesystem access. \
Set bash is_background=true (or block_until_ms: 0) for long-running commands; then get_task_output / wait_tasks / kill_task. \
Use web_search and web_fetch for current web information. \
Use todo_write to show multi-step progress. \
Use ask_user_question when you need the user to choose. \
Use enter_plan_mode / exit_plan_mode for ambiguous work; scheduler_create / scheduler_list / scheduler_delete wrap cron. \
Prefer tools over guessing file contents. Keep replies concise.";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = cordis::Context::new();
    install_app(&root).await?;
    root.on_waterfall(PROMPT_ASSEMBLE, |_: String, _| WORKSPACE_PROMPT.to_string())?;
    root.plugin(agent_loop(), ())?.wait().await?;
    root.plugin(session_actor(), ())?.wait().await?;

    let tick_ctx = root.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            ticker.tick().await;
            let Some(cron) = tick_ctx.get::<Cron>(CRON) else {
                continue;
            };
            let due = cron.due();
            let Some(session) = tick_ctx.get::<SessionRef>(SESSION_PORT) else {
                continue;
            };
            for job in due {
                session.submit(format!("\u{21BB} {}", job.prompt), false);
            }
        }
    });

    root.plugin(tui(), ())?.wait().await?;
    Ok(())
}
