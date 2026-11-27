//! Scheduled-task driver. `"cron"` holds the jobs (durable, shared by every Dock
//! process on this `$DOCK_HOME`); this plugin is the ticker that, for the jobs this
//! process holds:
//!
//! - delivers due fires (time slots, manual runs, watch / file triggers) into **their
//!   own session**, or a fresh session in the same project ([`CronTarget::NewSession`]),
//!   and reports each run back to the job's history;
//! - runs due **watch** commands in the background and hands the result to
//!   [`Cron::observe`], which turns a met condition into a pending fire;
//! - keeps notify watchers on **file-change** jobs and triggers them once the changes
//!   settle.
//!
//! Only `"cron"` is injected: each page's `sessions` / `session.port` are looked up at
//! fire time.
//!
//! A job's session is found among the open pages; a closed one is reopened as a
//! page in its own cwd ([`Tabs::open_session`]). Jobs without an owner (older
//! callers) fire into the first page, as before. A failed delivery is recorded on
//! the job (`last_error` + history) and retried at the next fire.
//!
//! Pages the driver opened (a fresh session per run, or a closed owner session it
//! reopened) are closed again once they have sat unused for [`CLOSE_IDLE_AFTER`]
//! (not the page the terminal is showing, not while anything still runs or waits
//! there): otherwise a new-session job adds a tab per run. The session stays on disk;
//! the gateway tells the desktop app the thread closed, and sending to it reopens it.
//!
//! A busy session queues the fire behind its current turn. If the previous fire of
//! the same prompt is **still queued** (or, for a new-session job, the previous run's
//! session is still working), this one is dropped and recorded as coalesced: a turn
//! that outlasts the interval must not leave a pile of identical prompts to run back
//! to back. While a job's own run is going, its file changes and watch results do
//! not trigger it again (it would otherwise wake itself with its own edits).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use cordis_spine::{
    expired_task_notice, format_fire_note, missed_task_notice, run_check, session_home, Cron,
    CronApproval, CronCheck, CronFire, CronJob, CronSchedule, CronTarget, FileWatches, FireNote,
    FireReason, FireReport, LlmOutput, LogEvent, PermissionMode, RunApproval, RunOutcome,
    SessionRef, Sessions, Tabs, CHECK_TIMEOUT, CRON, SCHEDULE_CHANGED, SESSIONS, SESSION_PORT,
    TABS,
};

/// Grok polls scheduled prompts once a second.
const TICK: Duration = Duration::from_secs(1);

/// 驱动替定时任务开出来的页，闲置这么久就关掉（会话留在盘上）。
pub const CLOSE_IDLE_AFTER: Duration = Duration::from_secs(600);

/// 正在跑的条件命令（按任务 id），同一个任务的检查不叠。
type Checking = Arc<Mutex<HashSet<String>>>;

/// 驱动开出来的页：会话 id → 最后一次还在用的时刻。
type Opened = HashMap<String, Instant>;

pub fn cron_driver() -> Plugin {
    cron_driver_closing_after(CLOSE_IDLE_AFTER)
}

/// 同 [`cron_driver`]，自己开出来的页闲置多久关掉可调（测试用）。
pub fn cron_driver_closing_after(close_after: Duration) -> Plugin {
    plugin("cron-driver", Inject::from([CRON]), move |ctx, _: &()| {
        let tick_ctx = ctx.clone();
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(TICK);
            let mut seen = None;
            let mut files = FileWatches::new();
            let checking = Checking::default();
            let mut opened = Opened::new();
            loop {
                ticker.tick().await;
                tick_once(&tick_ctx, &mut files, &checking, &mut opened).await;
                close_idle_pages(&tick_ctx, &mut opened, close_after).await;
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

/// 上一次 `due()` 失败的原因，用来只报一回。
fn last_store_error() -> &'static std::sync::Mutex<Option<String>> {
    static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    &LAST
}

/// One pass. Services are live-looked here, not captured by the spawned task.
async fn tick_once(
    ctx: &Context,
    files: &mut FileWatches,
    checking: &Checking,
    opened: &mut Opened,
) {
    let Some(cron) = ctx.get::<Cron>(CRON) else {
        return;
    };
    let tick = match cron.due() {
        Ok(tick) => {
            *last_store_error().lock().unwrap() = None;
            tick
        }
        Err(e) => {
            // 每秒都会再失败一次：同一个错误只报一回，恢复后再坏再报。
            let msg = e.to_string();
            let mut last = last_store_error().lock().unwrap();
            if last.as_deref() != Some(msg.as_str()) {
                eprintln!("dock: 定时任务这一轮没法触发：{msg}");
                *last = Some(msg);
            }
            return;
        }
    };
    for job in &tick.expired {
        notice(
            ctx,
            job,
            expired_task_notice(&job.prompt, &job.schedule.label()),
        );
    }
    for job in &tick.missed {
        notice(
            ctx,
            job,
            missed_task_notice(&job.prompt, &job.schedule.label()),
        );
    }
    for fire in tick.fires {
        let (report, opened_page) = deliver(ctx, &fire).await;
        if let Some(e) = &report.error {
            eprintln!("dock: 定时任务 {} 没送到会话：{e}", fire.job.id);
        }
        if let Some(session) = opened_page {
            opened.insert(session, Instant::now());
        }
        cron.report(&fire.job.id, report);
    }
    for check in tick.checks {
        spawn_check(ctx, check, checking);
    }
    watch_files(ctx, &cron, files);
}

/// 后台跑一次条件命令，结果交给 [`Cron::observe`]。同一个任务上一次还没跑完就跳过这次。
fn spawn_check(ctx: &Context, check: CronCheck, checking: &Checking) {
    let id = check.job.id.clone();
    if !checking.lock().unwrap().insert(id.clone()) {
        return;
    }
    let ctx = ctx.clone();
    let checking = checking.clone();
    tokio::spawn(async move {
        let cwd = job_cwd(&ctx, &check.job);
        let observation = run_check(&check.spec.command, &cwd, CHECK_TIMEOUT).await;
        // 它自己那一轮还在跑：只更新基线，不拿它自己造成的变化叫醒它。
        let may_fire = !run_in_progress(&ctx, &check.job);
        if let Some(cron) = ctx.get::<Cron>(CRON) {
            if let Err(e) = cron.observe(&id, &observation, may_fire) {
                eprintln!("dock: 定时任务 {id} 的检查结果没记上：{e}");
            }
        }
        checking.lock().unwrap().remove(&id);
    });
}

/// 文件变化任务：跟着任务表挂 / 摘监听；任务自己那一轮在跑时的改动不算；改动安静
/// 下来就记一次触发。
fn watch_files(ctx: &Context, cron: &Cron, files: &mut FileWatches) {
    let jobs = cron.list();
    let files_job = |j: &&CronJob| j.held_here && matches!(j.schedule, CronSchedule::Files(_));
    if files.is_empty() && !jobs.iter().any(|j| files_job(&j)) {
        return;
    }
    let (errors, attached) = files.reconcile(&jobs);
    for (id, error) in errors {
        cron.set_error(&id, Some(error));
    }
    for id in attached {
        cron.set_error(&id, None);
    }
    for job in jobs.iter().filter(files_job) {
        if run_in_progress(ctx, job) {
            files.hold(&job.id);
        }
    }
    for ready in files.ready() {
        let _ = cron.trigger(&ready.id, None, FireReason::Files, Some(ready.context()));
    }
}

/// 关掉驱动自己开出来、已经闲置够久的页。还在用（当前页、在跑、在等你、有旁问页 / 子代理 /
/// 目标）就从头计时；已经被别人关掉的不再管。
async fn close_idle_pages(ctx: &Context, opened: &mut Opened, close_after: Duration) {
    if opened.is_empty() {
        return;
    }
    let Some(tabs) = ctx.get::<Tabs>(TABS) else {
        return;
    };
    let mut idle = Vec::new();
    opened.retain(|session, last_used| {
        if page_of_session(ctx, session).is_none() {
            return false;
        }
        if tabs.session_in_use(session) {
            *last_used = Instant::now();
            return true;
        }
        if last_used.elapsed() < close_after {
            return true;
        }
        idle.push(session.clone());
        false
    });
    for session in idle {
        if let Err(e) = tabs.close_session(&session).await {
            eprintln!("dock: 定时任务开的会话 {session} 没关掉：{e}");
        }
    }
}

/// 给任务所属的会话留一句说明（过期 / 错过）。只发给开着的那一页：为一句提示重开
/// 一个会话不值得。
fn notice(ctx: &Context, job: &CronJob, text: String) {
    if let Some(sessions) = open_page_of(ctx, job).and_then(|p| p.get::<Sessions>(SESSIONS)) {
        sessions.append(LogEvent::LlmStream(LlmOutput {
            text,
            ..LlmOutput::default()
        }));
    }
}

/// 把这次触发送出去，交回结果（记进任务的历史）和驱动为它开出来的页（会话 id）。
async fn deliver(ctx: &Context, fire: &CronFire) -> (FireReport, Option<String>) {
    let mut report = FireReport {
        reason: fire.reason,
        outcome: RunOutcome::Sent,
        session: None,
        late_by: fire.late_by,
        error: None,
    };
    let mut opened = None;
    match send(ctx, fire).await {
        Ok(sent) => {
            report.session = sent.session.clone();
            if sent.coalesced {
                report.outcome = RunOutcome::Coalesced;
            }
            if sent.opened_page {
                opened = sent.session;
            }
        }
        Err(e) => {
            report.outcome = RunOutcome::Failed;
            report.error = Some(e);
        }
    }
    (report, opened)
}

/// 一次送达的结果。
struct Sent {
    /// 送进了哪个会话。
    session: Option<String>,
    /// 上一次还没跑完，这次跳过了。
    coalesced: bool,
    /// 那一页是驱动这次开出来的（闲置够久要关掉）。
    opened_page: bool,
}

/// 送进任务所属的会话（找到那一页，没开着就开出来），或在它的项目里开个新会话，再提交。
async fn send(ctx: &Context, fire: &CronFire) -> Result<Sent, String> {
    let job = &fire.job;
    let fresh = job.target == CronTarget::NewSession;
    let mut opened_page = false;
    let page = if fresh {
        if let Some(prev) = job.last_run().and_then(|r| r.session.clone()) {
            if page_of_session(ctx, &prev).is_some_and(|page| page_busy(&page)) {
                return Ok(Sent {
                    session: Some(prev),
                    coalesced: true,
                    opened_page: false,
                });
            }
        }
        let tabs = ctx
            .get::<Tabs>(TABS)
            .ok_or_else(|| "没有分页服务，开不了新会话".to_string())?;
        opened_page = true;
        tabs.open_new_session(&job_cwd(ctx, job), job.preset.as_deref())
            .await?
    } else {
        match open_page_of(ctx, job) {
            Some(page) => page,
            None => {
                let owner = job
                    .owner
                    .as_ref()
                    .ok_or_else(|| "没有会话可送".to_string())?;
                let tabs = ctx
                    .get::<Tabs>(TABS)
                    .ok_or_else(|| format!("会话 {} 没开着，也没有分页服务", owner.session))?;
                opened_page = true;
                tabs.open_session(&owner.session, &owner.cwd).await?
            }
        }
    };
    let port = page
        .get::<SessionRef>(SESSION_PORT)
        .ok_or_else(|| "那一页没有会话入口".to_string())?;
    let session = page
        .get::<Sessions>(SESSIONS)
        .map(|s| s.live_session_id())
        .filter(|id| !id.is_empty());
    // 上一次触发还在队里没轮到（那一页一直在忙）：不再叠一条。
    if port.queued_prompts().iter().any(|q| q.text == job.prompt) {
        return Ok(Sent {
            session,
            coalesced: true,
            opened_page,
        });
    }
    if let Some(sessions) = page.get::<Sessions>(SESSIONS) {
        let schedule = job.schedule.human();
        sessions.arm_user_addon(
            job.prompt.clone(),
            format_fire_note(&FireNote {
                task_id: &job.id,
                schedule: &schedule,
                recurring: job.schedule.is_recurring(),
                reason: fire.reason,
                late_by: fire.late_by,
                context: fire.context.as_deref(),
                fresh_session: fresh,
            }),
        );
    }
    if let Some(sessions) = page.get::<Sessions>(SESSIONS) {
        // 这一轮多半没人看着：按任务自己的档位批，权限请求等不到人就按拒绝处理，
        // 这一轮才收得了尾（不然那一页一直算忙，之后每次到点都只记成「合并掉」）。
        sessions.arm_run_approval(
            job.prompt.clone(),
            RunApproval {
                mode: match job.approval {
                    CronApproval::Ask => PermissionMode::Ask,
                    CronApproval::Smart => PermissionMode::Smart,
                    CronApproval::Allow => PermissionMode::Allow,
                },
                timeout: job.approval_timeout,
                task_id: job.id.clone(),
            },
        );
    }
    port.submit(job.prompt.clone(), false);
    Ok(Sent {
        session,
        coalesced: false,
        opened_page,
    })
}

/// 这一页在跑一轮，或者还有排着的消息。
fn page_busy(page: &Context) -> bool {
    page.get::<SessionRef>(SESSION_PORT)
        .is_some_and(|port| port.working() || port.has_queued())
}

/// 这个任务自己那一轮是不是还在跑：送进原会话的看那个会话在不在忙，新开会话的看上一次
/// 开的那个。
fn run_in_progress(ctx: &Context, job: &CronJob) -> bool {
    let page = match job.target {
        CronTarget::Session => open_page_of(ctx, job),
        CronTarget::NewSession => job
            .last_run()
            .and_then(|r| r.session.as_deref())
            .and_then(|id| page_of_session(ctx, id)),
    };
    page.is_some_and(|page| page_busy(&page))
}

/// 任务的项目目录：记在任务上的，没有就用这一页归档的项目（不是它的 cwd）。
fn job_cwd(ctx: &Context, job: &CronJob) -> PathBuf {
    job.owner
        .as_ref()
        .map(|o| o.cwd.clone())
        .filter(|cwd| !cwd.as_os_str().is_empty())
        .unwrap_or_else(|| session_home(ctx))
}

/// 正在写 `session` 的那一页。
fn page_of_session(ctx: &Context, session: &str) -> Option<Context> {
    let pages = ctx
        .get::<Tabs>(TABS)
        .map(|tabs| tabs.contexts())
        .unwrap_or_else(|| vec![ctx.clone()]);
    pages.into_iter().find(|page| {
        page.get::<Sessions>(SESSIONS)
            .is_some_and(|s| s.live_session_id() == session)
    })
}

/// 任务所属会话开着的那一页。没记归属的老任务算第一页。
fn open_page_of(ctx: &Context, job: &CronJob) -> Option<Context> {
    match &job.owner {
        None => Some(ctx.clone()),
        Some(owner) => page_of_session(ctx, &owner.session),
    }
}
