use cordis_app::session_actor;
use cordis_spine::{
    agent_loop, expired_task_notice, format_scheduled_task_reminder, install_app,
    interval_to_human, Cron, LlmOutput, LogEvent, Sessions, CRON, PROMPT_ASSEMBLE, SESSIONS,
};
use cordis_tui::{tui, SessionRef, SESSION_PORT};

const WORKSPACE_PROMPT: &str = "你是本地工作区里的编程助手。\
需要访问文件系统时使用 list_dir、read_file、grep、search_replace、glob、write_file、bash。\
长时间命令把 bash 的 is_background 设为 true（或 block_until_ms: 0），再用 get_task_output / wait_tasks / kill_task。\
查网上的近况用 web_search、web_fetch。\
多步进度用 todo_write。\
需要用户做选择时用 ask_user_question。\
做法不明确时用 enter_plan_mode / exit_plan_mode；定时任务用 scheduler_create / scheduler_list / scheduler_delete。\
子代理用 task；代码智能用 lsp；本地记忆用 memory_search / memory_get；盯长命令用 monitor；目标进度用 update_goal；多步编排用 workflow。\
会话内动态插件用 cordis_inspect / cordis_define / cordis_run / cordis_call / cordis_stop / cordis_undefine（先读 skills/cordis-plugin-development/SKILL.md）。自定义斜杠或只读 overlay 用 factory slash，不能替换内建命令。\
优先用工具，不要猜文件内容。回复尽量短。";

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
            let tick = cron.due();
            if let Some(sessions) = tick_ctx.get::<Sessions>(SESSIONS) {
                for job in &tick.expired {
                    let human = interval_to_human(job.every.as_secs());
                    sessions.append(LogEvent::LlmStream(LlmOutput {
                        text: expired_task_notice(&job.prompt, &human),
                        ..LlmOutput::default()
                    }));
                }
            }
            let Some(session) = tick_ctx.get::<SessionRef>(SESSION_PORT) else {
                continue;
            };
            for job in tick.fires {
                if let Some(sessions) = tick_ctx.get::<Sessions>(SESSIONS) {
                    let human = interval_to_human(job.every.as_secs());
                    sessions.arm_user_addon(
                        job.prompt.clone(),
                        format_scheduled_task_reminder(&job.id, &human),
                    );
                }
                session.submit(job.prompt, false);
            }
        }
    });

    root.plugin(tui(), ())?.wait().await?;
    Ok(())
}
