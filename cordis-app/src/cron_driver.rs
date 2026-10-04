//! Scheduled-task driver. `"cron"` holds the jobs (durable, shared by every Dock
//! process on this `$DOCK_HOME`); this plugin is the ticker that fires the ones
//! this process holds into **their own session**. Only `"cron"` is injected:
//! each page's `sessions` / `session.port` are looked up at fire time.
//!
//! A job's session is found among the open pages; a closed one is reopened as a
//! page in its own cwd ([`Tabs::open_session`]). Jobs without an owner (older
//! callers) fire into the first page, as before. A failed delivery is recorded on
//! the job (`last_error`) and retried at the next fire.

use std::time::Duration;

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use cordis_spine::{
    expired_task_notice, format_scheduled_task_reminder, interval_to_human, Cron, CronJob,
    LlmOutput, LogEvent, Sessions, CRON, SCHEDULE_CHANGED, SESSIONS,
};
use cordis_tui::{SessionRef, Tabs, SESSION_PORT, TUI_TABS};

/// Grok polls scheduled prompts once a second.
const TICK: Duration = Duration::from_secs(1);

pub fn cron_driver() -> Plugin {
    plugin("cron-driver", Inject::from([CRON]), |ctx, _: &()| {
        let tick_ctx = ctx.clone();
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(TICK);
            let mut seen = None;
            loop {
                ticker.tick().await;
                tick_once(&tick_ctx).await;
                // 内容变了（本进程改的、别的进程改的、刚触发过的）就告诉监听者。
                let revision = tick_ctx.get::<Cron>(CRON).map(|c| c.revision());
                if revision != seen {
                    if seen.is_some() {
                        tick_ctx.emit(SCHEDULE_CHANGED, ());
                    }
                    seen = revision;
                }
            }
        });
        ctx.effect("cron-driver-task", move |scope| {
            scope.own(Disposable::from_fn(move || task.abort()));
            Ok(())
        })?;
        Ok(None)
    })
}

/// One pass. Services are live-looked here, not captured by the spawned task.
async fn tick_once(ctx: &Context) {
    let Some(cron) = ctx.get::<Cron>(CRON) else {
        return;
    };
    let tick = cron.due();
    for job in &tick.expired {
        // 过期提示只发给开着的那一页：为一句提示重开一个会话不值得。
        if let Some(page) = open_page_of(ctx, job) {
            if let Some(sessions) = page.get::<Sessions>(SESSIONS) {
                let human = interval_to_human(job.every.as_secs());
                sessions.append(LogEvent::LlmStream(LlmOutput {
                    text: expired_task_notice(&job.prompt, &human),
                    ..LlmOutput::default()
                }));
            }
        }
    }
    for job in tick.fires {
        let outcome = deliver(ctx, &job).await;
        let error = outcome.err();
        if error.is_some() || job.last_error.is_some() {
            if let Some(e) = &error {
                eprintln!("dock: 定时任务 {} 没送到会话：{e}", job.id);
            }
            cron.set_error(&job.id, error);
        }
    }
}

/// 把这次触发送进任务所属的会话：找到那一页，没开着就开出来，再提交。
async fn deliver(ctx: &Context, job: &CronJob) -> Result<(), String> {
    let page = match open_page_of(ctx, job) {
        Some(page) => page,
        None => {
            let owner = job
                .owner
                .as_ref()
                .ok_or_else(|| "没有会话可送".to_string())?;
            let tabs = ctx
                .get::<Tabs>(TUI_TABS)
                .ok_or_else(|| format!("会话 {} 没开着，也没有分页服务", owner.session))?;
            tabs.open_session(&owner.session, &owner.cwd).await?
        }
    };
    let port = page
        .get::<SessionRef>(SESSION_PORT)
        .ok_or_else(|| "那一页没有会话入口".to_string())?;
    if let Some(sessions) = page.get::<Sessions>(SESSIONS) {
        let human = interval_to_human(job.every.as_secs());
        sessions.arm_user_addon(
            job.prompt.clone(),
            format_scheduled_task_reminder(&job.id, &human),
        );
    }
    port.submit(job.prompt.clone(), false);
    Ok(())
}

/// 任务所属会话开着的那一页。没记归属的老任务算第一页。
fn open_page_of(ctx: &Context, job: &CronJob) -> Option<Context> {
    let Some(owner) = &job.owner else {
        return Some(ctx.clone());
    };
    let pages = ctx
        .get::<Tabs>(TUI_TABS)
        .map(|tabs| tabs.contexts())
        .unwrap_or_else(|| vec![ctx.clone()]);
    pages.into_iter().find(|page| {
        page.get::<Sessions>(SESSIONS)
            .is_some_and(|s| s.live_session_id() == owner.session)
    })
}
