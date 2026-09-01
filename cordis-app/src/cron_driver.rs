//! Scheduled-task driver. `"cron"` holds the jobs; this plugin is the ticker
//! that fires them into the session. Mount after `session_actor` — it needs
//! `session.port` to submit.

use std::time::Duration;

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use cordis_spine::{
    expired_task_notice, format_scheduled_task_reminder, interval_to_human, Cron, LlmOutput,
    LogEvent, Sessions, CRON, SESSIONS,
};
use cordis_tui::{SessionRef, SESSION_PORT};

/// Grok polls scheduled prompts once a second.
const TICK: Duration = Duration::from_secs(1);

pub fn cron_driver() -> Plugin {
    plugin(
        "cron-driver",
        Inject::from([CRON, SESSIONS, SESSION_PORT]),
        |ctx, _: &()| {
            let tick_ctx = ctx.clone();
            let task = tokio::spawn(async move {
                let mut ticker = tokio::time::interval(TICK);
                loop {
                    ticker.tick().await;
                    tick_once(&tick_ctx);
                }
            });
            ctx.effect("cron-driver-task", move |scope| {
                scope.own(Disposable::from_fn(move || task.abort()));
                Ok(())
            })?;
            Ok(None)
        },
    )
}

/// One pass. Services are live-looked here, not captured by the spawned task.
fn tick_once(ctx: &Context) {
    let Some(cron) = ctx.get::<Cron>(CRON) else {
        return;
    };
    let tick = cron.due();
    if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
        for job in &tick.expired {
            let human = interval_to_human(job.every.as_secs());
            sessions.append(LogEvent::LlmStream(LlmOutput {
                text: expired_task_notice(&job.prompt, &human),
                ..LlmOutput::default()
            }));
        }
    }
    let Some(session) = ctx.get::<SessionRef>(SESSION_PORT) else {
        return;
    };
    for job in tick.fires {
        if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
            let human = interval_to_human(job.every.as_secs());
            sessions.arm_user_addon(
                job.prompt.clone(),
                format_scheduled_task_reminder(&job.id, &human),
            );
        }
        session.submit(job.prompt, false);
    }
}