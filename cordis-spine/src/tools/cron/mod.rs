//! Scheduled tasks. Grok's scheduler is ACP/workspace-heavy; this keeps the
//! prompt surface (`/loop` → `scheduler_create`) and leaves firing to the app's
//! ticker (`cordis-app` `cron_driver`).
//!
//! **When** ([`CronSchedule`]): a fixed interval (phase kept from creation, so a late
//! tick never drifts it), a 5-field cron expression in local time, one run at a
//! wall-clock time, a **watch** (a shell command checked on an interval; the prompt
//! fires only when its result meets a condition, so polling costs no tokens), **file
//! changes** (debounced), or **manual** only. Any task can also be run by hand
//! ([`Cron::trigger`]): the desktop app's “run now”, `dock schedule run`, gateway
//! `schedule/run`. Event sources (watch results, file changes, manual runs) all land
//! as a *pending* fire on the task, which the holder fires on its next tick.
//!
//! **Where**: into the session that owns the task (default), or a fresh session in
//! the same project per run ([`CronTarget::NewSession`]).
//!
//! **Durable**: the app mounts [`Cron::at`] on `$DOCK_HOME/schedules.json`, so tasks
//! survive a restart. Several Dock processes may share one `$DOCK_HOME` (a TUI and
//! the desktop app), so every task names the process that **holds** it: only the
//! holder fires it, runs its watch command, and watches its files. A task whose
//! holder has exited is adopted by the next live process on its next tick. Every
//! read-modify-write runs under an exclusive `flock` on `schedules.json.lock` and
//! lands with an atomic rename, so another process (the `dock schedule` CLI) can
//! request a run. File version 2; version 1 files are read and upgraded on the next
//! write. A Dock that only knows version 1 fails to parse a version 2 file and
//! refuses to touch it rather than rewriting it without the new fields.
//!
//! **Missed runs** (no Dock up, or the machine asleep — more than [`MISSED_GRACE`]
//! late) are never replayed one by one: a task with `catch_up` fires **once** and
//! realigns, one without just realigns to its next future slot. Defaults: cron and
//! one-shot catch up, intervals do not. A paused task skips its slots.
//!
//! **Expiry**: schedules that fire (or check) more often than daily expire
//! [`RECURRING_TASK_TTL_DAYS`] days after creation unless told otherwise (Grok parity
//! — a forgotten polling loop should not spend tokens forever); the rest do not.
//! `max_runs` caps the number of automatic fires. The last [`HISTORY_LEN`] runs are
//! kept on the task. [`Cron::new`] is the in-memory variant for tests and harnesses.

mod files;
mod schedule;
mod watch;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cordis::{plugin, Inject, Plugin};
use serde::{Deserialize, Serialize};

use crate::names::CRON;

pub use files::{FileWatches, FilesReady};
use schedule::{cron_next_after, DAY_MS};
pub use schedule::{
    parse_at, CronSchedule, FilesSpec, WatchSpec, WatchWhen, DEFAULT_CHECK_EVERY,
    DEFAULT_FILES_DEBOUNCE, MIN_CHECK_EVERY,
};
pub use watch::{run_check, WatchObservation, CHECK_TIMEOUT};

/// Grok `RECURRING_TASK_TTL_DAYS`. Schedules firing more often than daily drop after
/// this many days unless an expiry is given.
pub const RECURRING_TASK_TTL_DAYS: u64 = 7;

/// Grok `MAX_SCHEDULED_TASKS`.
pub const MAX_SCHEDULED_TASKS: usize = 50;

/// 每个任务留最近这么多次运行记录。
pub const HISTORY_LEN: usize = 20;

/// 落盘文件名（在 `$DOCK_HOME` 下）。
pub const SCHEDULES_FILE: &str = "schedules.json";
const FILE_VERSION: u32 = 2;

/// 晚了超过这么多才算「错过」。正常一秒一查，但送进关着的会话要等开页，晚几秒很常见；
/// 超过两分钟基本就是 Dock 没开或者电脑睡着了。
pub const MISSED_GRACE: Duration = Duration::from_secs(120);

pub fn recurring_task_ttl() -> Duration {
    Duration::from_secs(RECURRING_TASK_TTL_DAYS.saturating_mul(86_400))
}

/// 任务属于哪个会话：到点把提问提交到这里（或在它的工作目录开新会话）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CronOwner {
    /// 落盘会话 id（`Sessions::live_session_id`）。
    pub session: String,
    /// 那个会话的工作目录：会话关着时按它开页；条件命令、文件变化也以它为准。
    pub cwd: PathBuf,
}

/// 什么时候过期。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CronExpiry {
    /// 比一天还勤的（盯梢类循环、条件命令）[`RECURRING_TASK_TTL_DAYS`] 天后过期；
    /// 每天及更稀的、一次性的、文件变化、手动的不过期。
    #[default]
    Default,
    Never,
    /// 墙钟毫秒。
    At(u64),
}

/// 每次触发送到哪儿。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CronTarget {
    /// 发进建任务的那个会话（前几次的结果在上文里）。
    #[default]
    Session,
    /// 每次在同一个项目里新开一个会话（上下文干净，适合日报、周报）。
    NewSession,
}

impl CronTarget {
    fn is_session(&self) -> bool {
        *self == Self::Session
    }
}

/// 这一次为什么跑。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FireReason {
    /// 按时间到点。
    Schedule,
    /// 错过了，回来后补的那一次。
    CatchUp,
    /// 建任务时说了「马上跑一次」。
    Immediate,
    /// 手动运行（桌面端「立即运行」、`dock schedule run`、网关）。
    Manual,
    /// 条件命令的结果满足了条件。
    Watch,
    /// 监听的文件有改动。
    Files,
}

impl FireReason {
    /// 算不算进 `runs` / `max_runs`：手动和「马上跑一次」是额外的，不占次数。
    fn counts(self) -> bool {
        !matches!(self, Self::Manual | Self::Immediate)
    }
}

/// 一次运行的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunOutcome {
    /// 送进了会话（会话忙就排在队里）。
    Sent,
    /// 上一次还在排队 / 还在跑，这次就不再叠。
    Coalesced,
    /// 没送到（会话开不了之类），原因在 `error`。
    Failed,
}

/// 一次运行的记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronRun {
    pub at_ms: u64,
    pub reason: FireReason,
    pub outcome: RunOutcome,
    /// 送进了哪个会话（新开会话时是新的那个）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// 补跑时晚了多久。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 条件命令上一次检查的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronWatchState {
    pub checked_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// 输出的摘要（判断「变了没」用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// 上一次是不是满足条件（边沿触发：从不满足变成满足才触发）。
    #[serde(default)]
    pub matched: bool,
    /// 命令没跑起来 / 超时。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 等着下一轮触发的一次（手动、条件命令、文件变化）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingFire {
    reason: FireReason,
    /// 给模型的额外说明（命令输出、改了哪些文件、手动运行时附的话）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
    requested_at_ms: u64,
}

/// 建一个任务要的东西。`NewCronTask::new(schedule, prompt)` 之后按需改字段。
#[derive(Clone, Debug)]
pub struct NewCronTask {
    /// `None`：老调用方 / 测试，触发到第一页。
    pub owner: Option<CronOwner>,
    pub schedule: CronSchedule,
    pub prompt: String,
    /// 建好马上额外跑一次（不占次数，之后照常按 `schedule` 来）。
    pub fire_immediately: bool,
    pub expiry: CronExpiry,
    /// 自动触发跑满这么多次就结束（手动运行不算）。
    pub max_runs: Option<u32>,
    /// 错过了要不要补一次。`None` = 按种类的默认（cron / 一次性补，其余不补）。
    pub catch_up: Option<bool>,
    pub target: CronTarget,
    /// 新开会话时用的预设（`None` = 跟着开页时继承的）。
    pub preset: Option<String>,
    /// 建好先暂停着。
    pub paused: bool,
}

impl NewCronTask {
    pub fn new(schedule: CronSchedule, prompt: impl Into<String>) -> Self {
        Self {
            owner: None,
            schedule,
            prompt: prompt.into(),
            fire_immediately: false,
            expiry: CronExpiry::Default,
            max_runs: None,
            catch_up: None,
            target: CronTarget::Session,
            preset: None,
            paused: false,
        }
    }
}

/// 原地改一个任务。`None` 的字段不动。
#[derive(Clone, Debug, Default)]
pub struct CronPatch {
    /// 换触发方式。同为固定间隔时保持相位；换了种类就从现在起重新排。
    pub schedule: Option<CronSchedule>,
    pub prompt: Option<String>,
    pub expiry: Option<CronExpiry>,
    /// `Some(None)` = 不限次数。
    pub max_runs: Option<Option<u32>>,
    pub catch_up: Option<bool>,
    pub target: Option<CronTarget>,
    /// `Some(None)` = 不指定。
    pub preset: Option<Option<String>>,
    /// 暂停 / 继续。继续时跳过暂停期间的时刻（一次性的按错过处理）。
    pub paused: Option<bool>,
}

impl CronPatch {
    pub fn is_empty(&self) -> bool {
        self.schedule.is_none()
            && self.prompt.is_none()
            && self.expiry.is_none()
            && self.max_runs.is_none()
            && self.catch_up.is_none()
            && self.target.is_none()
            && self.preset.is_none()
            && self.paused.is_none()
    }
}

#[derive(Clone, Debug)]
pub struct CronJob {
    pub id: String,
    pub schedule: CronSchedule,
    pub prompt: String,
    pub created_at: Instant,
    /// 下一次到点（条件命令是下一次检查）。文件变化、手动、跑完了的没有。
    pub next: Option<Instant>,
    /// `None`：老调用方建的，触发到第一页。
    pub owner: Option<CronOwner>,
    pub created_at_ms: u64,
    pub next_at_ms: Option<u64>,
    /// `None`：不过期。
    pub expires_at_ms: Option<u64>,
    pub last_fired_at_ms: Option<u64>,
    /// 上次触发失败的原因（会话开不了之类）；下次成功触发时清掉。
    pub last_error: Option<String>,
    /// 自动触发过几次（手动运行不算）。
    pub runs: u32,
    pub max_runs: Option<u32>,
    /// 错过了补一次。
    pub catch_up: bool,
    pub target: CronTarget,
    pub preset: Option<String>,
    pub paused: bool,
    /// 跑完了（一次性的跑过了、跑满了次数），不会再自己触发。
    pub done: bool,
    /// 有一次等着下一轮触发（手动运行 / 条件满足 / 文件改了）。
    pub pending: bool,
    /// 最近的运行，旧的在前。
    pub history: Vec<CronRun>,
    /// 条件命令上一次检查的结果。
    pub watch: Option<CronWatchState>,
    /// 这个进程持有它（只有持有者会触发）。
    pub held_here: bool,
    /// 有活着的 Dock 进程持有它（本进程或别的）。`false` = 现在没有 Dock 会去跑它。
    pub holder_live: bool,
}

impl CronJob {
    /// 跑完了，不会再触发（还留在表上 = 最后一次没送到，留给人看）。
    pub fn finished(&self) -> bool {
        self.done
    }

    /// 最近一次运行。
    pub fn last_run(&self) -> Option<&CronRun> {
        self.history.last()
    }
}

/// 一次触发。
#[derive(Clone, Debug)]
pub struct CronFire {
    /// 触发之后的样子（`runs` 已加一，`next` 已排到下一次）。
    pub job: CronJob,
    pub reason: FireReason,
    /// 按时间触发时原定的时刻（墙钟毫秒）。
    pub scheduled_at_ms: Option<u64>,
    /// 错过了、这是补的那一次：晚了多久。
    pub late_by: Option<Duration>,
    /// 给模型的额外说明（命令输出、改了哪些文件、手动运行附的话）。
    pub context: Option<String>,
}

/// 到点该查一次的条件命令。查完用 [`Cron::observe`] 交回结果。
#[derive(Clone, Debug)]
pub struct CronCheck {
    pub job: CronJob,
    pub spec: WatchSpec,
}

/// One ticker pass, only tasks this process holds: fires to deliver, watch commands
/// to run, tasks that hit their expiry (removed without firing, matching Grok
/// expiry), and one-shot tasks whose time passed while no Dock was up and that do
/// not catch up (removed).
#[derive(Clone, Debug, Default)]
pub struct CronTick {
    pub fires: Vec<CronFire>,
    pub checks: Vec<CronCheck>,
    pub expired: Vec<CronJob>,
    pub missed: Vec<CronJob>,
}

/// 一次触发送得怎么样（驱动送完交回来，记进历史）。
#[derive(Clone, Debug)]
pub struct FireReport {
    pub reason: FireReason,
    pub outcome: RunOutcome,
    pub session: Option<String>,
    pub late_by: Option<Duration>,
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("maximum {0} scheduled tasks")]
    LimitReached(usize),
    #[error("prompt is required")]
    EmptyPrompt,
    #[error("unknown task id {0}")]
    UnknownId(String),
    #[error("{0}")]
    InvalidSchedule(String),
    #[error("定时任务读写失败：{0}")]
    Store(String),
}

/// 持有任务的进程。pid 会被复用，所以带上它启动的时刻。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Holder {
    pid: u32,
    started_at_ms: u64,
}

/// 盘上的触发方式。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Sched {
    #[serde(rename_all = "camelCase")]
    Every {
        every_ms: u64,
    },
    Cron {
        expr: String,
    },
    #[serde(rename_all = "camelCase")]
    At {
        at_ms: u64,
    },
    #[serde(rename_all = "camelCase")]
    Watch {
        command: String,
        every_ms: u64,
        when: WatchWhen,
    },
    #[serde(rename_all = "camelCase")]
    Files {
        paths: Vec<String>,
        debounce_ms: u64,
    },
    Manual,
}

impl From<&CronSchedule> for Sched {
    fn from(s: &CronSchedule) -> Self {
        match s {
            CronSchedule::Every(every) => Sched::Every {
                every_ms: ms(clamp_every(*every)),
            },
            CronSchedule::Cron(expr) => Sched::Cron { expr: expr.clone() },
            CronSchedule::At(at_ms) => Sched::At { at_ms: *at_ms },
            CronSchedule::Watch(spec) => Sched::Watch {
                command: spec.command.clone(),
                every_ms: ms(spec.every.max(MIN_CHECK_EVERY)),
                when: spec.when.clone(),
            },
            CronSchedule::Files(spec) => Sched::Files {
                paths: spec.paths.clone(),
                debounce_ms: ms(spec.debounce),
            },
            CronSchedule::Manual => Sched::Manual,
        }
    }
}

impl From<&Sched> for CronSchedule {
    fn from(s: &Sched) -> Self {
        match s {
            Sched::Every { every_ms } => CronSchedule::Every(Duration::from_millis(*every_ms)),
            Sched::Cron { expr } => CronSchedule::Cron(expr.clone()),
            Sched::At { at_ms } => CronSchedule::At(*at_ms),
            Sched::Watch {
                command,
                every_ms,
                when,
            } => CronSchedule::Watch(WatchSpec {
                command: command.clone(),
                every: Duration::from_millis(*every_ms),
                when: when.clone(),
            }),
            Sched::Files { paths, debounce_ms } => CronSchedule::Files(FilesSpec {
                paths: paths.clone(),
                debounce: Duration::from_millis(*debounce_ms),
            }),
            Sched::Manual => CronSchedule::Manual,
        }
    }
}

impl Sched {
    /// 建任务（或换触发方式）后第一次到点的时刻。条件命令马上查一次（定基线），
    /// 文件变化、手动没有时刻。`Ok(None)` 就是没有；排不出来（时间已过）是 `Err`。
    fn first_after(&self, now: u64) -> Result<Option<u64>, CronError> {
        match self {
            Sched::Every { every_ms } => Ok(Some(now.saturating_add(*every_ms))),
            Sched::Cron { expr } => cron_next_after(expr, now)
                .map(Some)
                .ok_or_else(|| never_fires(self)),
            Sched::At { at_ms } if *at_ms > now => Ok(Some(*at_ms)),
            Sched::At { .. } => Err(never_fires(self)),
            Sched::Watch { .. } => Ok(Some(now)),
            Sched::Files { .. } | Sched::Manual => Ok(None),
        }
    }

    /// 原定在 `slot` 的那次（已经到了）之后的下一次，严格晚于 `now`。固定间隔、
    /// 条件命令保持相位。
    fn next_after(&self, slot: u64, now: u64) -> Option<u64> {
        match self {
            Sched::Every { every_ms } | Sched::Watch { every_ms, .. } => {
                Some(next_future(slot, *every_ms, now))
            }
            Sched::Cron { expr } => cron_next_after(expr, now),
            Sched::At { .. } | Sched::Files { .. } | Sched::Manual => None,
        }
    }

    fn default_catch_up(&self) -> bool {
        matches!(self, Sched::Cron { .. } | Sched::At { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "TaskOnDisk")]
struct Task {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
    schedule: Sched,
    prompt: String,
    created_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at_ms: Option<u64>,
    /// 下一次到点（条件命令是下一次检查）。文件变化、手动、跑完了的没有。
    #[serde(skip_serializing_if = "Option::is_none")]
    next_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "is_zero")]
    runs: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_runs: Option<u32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    catch_up: bool,
    #[serde(skip_serializing_if = "CronTarget::is_session")]
    target: CronTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    preset: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    paused: bool,
    /// 跑完了。下一轮 `due()` 收掉；最后一次没送到（有 `last_error`）的留着给人看。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending: Option<PendingFire>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    history: Vec<CronRun>,
    #[serde(skip_serializing_if = "Option::is_none")]
    watch: Option<CronWatchState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_fired_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    holder: Option<Holder>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// 盘上读到的样子。除了 v2 还认 v1：v1 的 `everyMs` 在顶层、没有 `schedule`，
/// `expiresAtMs` / `nextAtMs` 必有，后来加的字段都没有（v1 的循环都不补跑）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskOnDisk {
    id: String,
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    schedule: Option<Sched>,
    #[serde(default)]
    every_ms: Option<u64>,
    prompt: String,
    created_at_ms: u64,
    #[serde(default)]
    expires_at_ms: Option<u64>,
    #[serde(default)]
    next_at_ms: Option<u64>,
    #[serde(default)]
    runs: u32,
    #[serde(default)]
    max_runs: Option<u32>,
    #[serde(default)]
    catch_up: bool,
    #[serde(default)]
    target: CronTarget,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    pending: Option<PendingFire>,
    #[serde(default)]
    history: Vec<CronRun>,
    #[serde(default)]
    watch: Option<CronWatchState>,
    #[serde(default)]
    last_fired_at_ms: Option<u64>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    holder: Option<Holder>,
}

impl TryFrom<TaskOnDisk> for Task {
    type Error = String;

    fn try_from(d: TaskOnDisk) -> Result<Self, String> {
        let schedule = match (d.schedule, d.every_ms) {
            (Some(schedule), _) => schedule,
            (None, Some(every_ms)) => Sched::Every { every_ms },
            (None, None) => return Err(format!("任务 {} 没有触发时刻", d.id)),
        };
        Ok(Task {
            id: d.id,
            session: d.session,
            cwd: d.cwd,
            schedule,
            prompt: d.prompt,
            created_at_ms: d.created_at_ms,
            expires_at_ms: d.expires_at_ms,
            next_at_ms: d.next_at_ms,
            runs: d.runs,
            max_runs: d.max_runs,
            catch_up: d.catch_up,
            target: d.target,
            preset: d.preset,
            paused: d.paused,
            done: d.done,
            pending: d.pending,
            history: d.history,
            watch: d.watch,
            last_fired_at_ms: d.last_fired_at_ms,
            last_error: d.last_error,
            holder: d.holder,
        })
    }
}

impl Task {
    fn exhausted(&self) -> bool {
        self.max_runs.is_some_and(|m| self.runs >= m)
    }

    /// 这次触发之后：记次数、排下一次、跑满 / 一次性的标成跑完。
    fn after_fire(&mut self, reason: FireReason, slot: Option<u64>, now: u64) {
        self.last_fired_at_ms = Some(now);
        if !reason.counts() {
            return;
        }
        self.runs = self.runs.saturating_add(1);
        if let Some(slot) = slot {
            self.next_at_ms = self.schedule.next_after(slot, now);
        }
        let one_shot = matches!(self.schedule, Sched::At { .. });
        if self.exhausted() || one_shot {
            self.done = true;
            self.next_at_ms = None;
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct FileBody {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    tasks: Vec<Task>,
}

/// 先只读版本号：比本进程认识的新，就别往下解析（更不能写回去）。
#[derive(Deserialize)]
struct FileVersion {
    #[serde(default)]
    version: u32,
}

/// 上次读到的文件（mtime + 长度）。没变就不重读。
type Stamp = Option<(SystemTime, u64)>;

#[derive(Default)]
struct State {
    tasks: Vec<Task>,
    stamp: Stamp,
}

/// Named `"cron"` service. Live-lookup from the ticker; do not capture the Arc.
/// 墙钟（毫秒），见 [`Cron`] 的 `clock` 字段。
type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub struct Cron {
    /// `None` = 只在内存里（测试 / 没挂盘的 harness）。
    path: Option<PathBuf>,
    me: Holder,
    state: Mutex<State>,
    seq: AtomicU64,
    /// 内容变了就 +1（本进程改的，或从盘上读到别的进程改的）。网关拿它推 `schedule/changed`。
    revision: AtomicU64,
    /// 把墙钟毫秒换回 `Instant` 的固定基准：同一个毫秒数永远换出同一个 `Instant`。
    base: (Instant, u64),
    /// 墙钟（毫秒）。正式用 [`now_ms`]；测试换成手动拨的，免得「两次 `due()` 之间过了
    /// 多久」取决于机器忙不忙。
    clock: Clock,
}

impl std::fmt::Debug for Cron {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cron").field("path", &self.path).finish()
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ms(d: Duration) -> u64 {
    d.as_millis().min(u128::from(u64::MAX)) as u64
}

/// `session` 给了就只认它名下的任务（模型工具不能动别的会话的）。
fn scoped<'a>(
    tasks: &'a mut [Task],
    id: &str,
    session: Option<&str>,
) -> Result<&'a mut Task, CronError> {
    tasks
        .iter_mut()
        .find(|t| t.id == id && session.is_none_or(|s| t.session.as_deref() == Some(s)))
        .ok_or_else(|| CronError::UnknownId(id.to_string()))
}

impl Cron {
    /// 只在内存里的一份（测试、harness）。所有任务都算本进程持有。
    pub fn new() -> Self {
        Self::build(None, Arc::new(now_ms))
    }

    /// 内存版，墙钟由测试拨（见 [`Self::clock`] 字段）。
    #[cfg(test)]
    fn with_clock(clock: Clock) -> Self {
        Self::build(None, clock)
    }

    /// 落在 `path`（一般是 `$DOCK_HOME/schedules.json`）的一份。
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self::build(Some(path.into()), Arc::new(now_ms))
    }

    fn build(path: Option<PathBuf>, clock: Clock) -> Self {
        let now = clock();
        Self {
            path,
            me: Holder {
                pid: std::process::id(),
                started_at_ms: now,
            },
            state: Mutex::new(State::default()),
            seq: AtomicU64::new(1),
            revision: AtomicU64::new(0),
            base: (Instant::now(), now),
            clock,
        }
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    /// 落盘路径；内存版为 `None`。
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// 内容版本：任务增删改、触发、或读到别的进程的改动都会变。
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// 固定间隔、不属于任何会话的任务（测试 / 老调用方）。
    pub fn add(&self, every: Duration, prompt: impl Into<String>) -> Result<String, CronError> {
        self.add_ex(every, prompt, false)
    }

    pub fn add_ex(
        &self,
        every: Duration,
        prompt: impl Into<String>,
        fire_immediately: bool,
    ) -> Result<String, CronError> {
        self.add_owned(None, every, prompt, fire_immediately)
    }

    /// 建一个属于 `owner` 会话的固定间隔任务，本进程持有。
    pub fn add_owned(
        &self,
        owner: Option<CronOwner>,
        every: Duration,
        prompt: impl Into<String>,
        fire_immediately: bool,
    ) -> Result<String, CronError> {
        self.create(NewCronTask {
            owner,
            fire_immediately,
            ..NewCronTask::new(CronSchedule::Every(every), prompt)
        })
    }

    /// 建一个任务，本进程持有。
    pub fn create(&self, new: NewCronTask) -> Result<String, CronError> {
        let NewCronTask {
            owner,
            schedule,
            prompt,
            fire_immediately,
            expiry,
            max_runs,
            catch_up,
            target,
            preset,
            paused,
        } = new;
        if prompt.trim().is_empty() {
            return Err(CronError::EmptyPrompt);
        }
        check_max_runs(max_runs)?;
        let sched = Sched::from(&schedule);
        let me = self.me;
        let seq = &self.seq;
        self.transact(|tasks| {
            if tasks.len() >= MAX_SCHEDULED_TASKS {
                return Err(CronError::LimitReached(MAX_SCHEDULED_TASKS));
            }
            let now = self.now();
            let next = sched.first_after(now)?;
            let id = loop {
                let id = format!("cron-{:x}{:x}", now, seq.fetch_add(1, Ordering::Relaxed));
                if !tasks.iter().any(|t| t.id == id) {
                    break id;
                }
            };
            tasks.push(Task {
                id: id.clone(),
                session: owner.as_ref().map(|o| o.session.clone()),
                cwd: owner.as_ref().map(|o| o.cwd.clone()),
                expires_at_ms: resolve_expiry(expiry, &sched, now),
                catch_up: catch_up.unwrap_or_else(|| sched.default_catch_up()),
                schedule: sched.clone(),
                prompt,
                created_at_ms: now,
                next_at_ms: next,
                runs: 0,
                max_runs,
                target,
                preset,
                paused,
                done: false,
                pending: fire_immediately.then_some(PendingFire {
                    reason: FireReason::Immediate,
                    context: None,
                    requested_at_ms: now,
                }),
                history: Vec::new(),
                watch: None,
                last_fired_at_ms: None,
                last_error: None,
                holder: Some(me),
            });
            Ok(id)
        })?
    }

    /// Update in place. Omitted fields stay unchanged; interval changes keep
    /// phase from `last_fired_at` (or `created_at`). TTL is not reset.
    pub fn update(
        &self,
        id: &str,
        every: Option<Duration>,
        prompt: Option<&str>,
    ) -> Result<CronJob, CronError> {
        self.patch(
            id,
            None,
            CronPatch {
                schedule: every.map(CronSchedule::Every),
                prompt: prompt.map(str::to_string),
                ..CronPatch::default()
            },
        )
    }

    /// 原地改。`session` 给了就只认它名下的任务（模型工具不能改别的会话的）。
    /// 过期时刻只在 `patch.expiry` 给了时才动（改触发方式不续期）。
    pub fn patch(
        &self,
        id: &str,
        session: Option<&str>,
        patch: CronPatch,
    ) -> Result<CronJob, CronError> {
        if patch.prompt.as_deref().is_some_and(|p| p.trim().is_empty()) {
            return Err(CronError::EmptyPrompt);
        }
        if let Some(max_runs) = patch.max_runs {
            check_max_runs(max_runs)?;
        }
        let sched = patch.schedule.as_ref().map(Sched::from);
        let updated = self.transact(|tasks| {
            let task = scoped(tasks, id, session)?;
            let now = self.now();
            let mut rearm = patch.max_runs.is_some();
            if let Some(sched) = sched.filter(|s| *s != task.schedule) {
                let next = match (&task.schedule, &sched) {
                    (Sched::Every { .. }, Sched::Every { every_ms }) => {
                        let anchor = task.last_fired_at_ms.unwrap_or(task.created_at_ms);
                        Some(anchor.saturating_add(*every_ms).max(now))
                    }
                    _ => sched.first_after(now)?,
                };
                // 换了检查命令 / 条件，旧的基线就不作数了。
                if !matches!(sched, Sched::Watch { .. }) || !same_watch(&task.schedule, &sched) {
                    task.watch = None;
                }
                task.schedule = sched;
                task.next_at_ms = next;
                rearm = true;
            }
            if let Some(text) = &patch.prompt {
                task.prompt = text.clone();
            }
            if let Some(expiry) = patch.expiry {
                task.expires_at_ms = resolve_expiry(expiry, &task.schedule, now);
            }
            if let Some(max_runs) = patch.max_runs {
                task.max_runs = max_runs;
            }
            if let Some(catch_up) = patch.catch_up {
                task.catch_up = catch_up;
            }
            if let Some(target) = patch.target {
                task.target = target;
            }
            if let Some(preset) = &patch.preset {
                task.preset = preset.clone();
            }
            if let Some(paused) = patch.paused {
                if task.paused && !paused {
                    // 继续：暂停期间的时刻跳过，不补（一次性的照「错过」处理，见 `due`）。
                    if let Some(slot) = task.next_at_ms.filter(|slot| *slot <= now) {
                        if !matches!(task.schedule, Sched::At { .. }) {
                            task.next_at_ms = task.schedule.next_after(slot, now);
                        }
                    }
                }
                task.paused = paused;
            }
            // 跑满了就停着；跑完了的改了次数或触发方式后还能跑，就重新排上。
            if task.exhausted() {
                task.done = true;
                task.next_at_ms = None;
            } else if rearm && task.done {
                let next = task.schedule.first_after(now).ok().flatten();
                let event_driven = matches!(task.schedule, Sched::Files { .. } | Sched::Manual);
                if next.is_some() || event_driven {
                    task.done = false;
                    task.next_at_ms = next;
                }
            }
            Ok(task.clone())
        })??;
        Ok(self.snap(&updated))
    }

    /// 手动运行（或事件触发）：记一次「等着触发」，持有它的 Dock 下一轮（一秒内）就送。
    /// 没有活着的持有者时一直等着，下一个打开的 Dock 接管后马上送。`context` 是给模型
    /// 的额外说明。暂停、跑完了的任务只认手动运行；事件触发直接忽略（回 `Ok(false)`）。
    pub fn trigger(
        &self,
        id: &str,
        session: Option<&str>,
        reason: FireReason,
        context: Option<String>,
    ) -> Result<bool, CronError> {
        self.transact(|tasks| {
            let task = scoped(tasks, id, session)?;
            if reason != FireReason::Manual && (task.paused || task.done) {
                return Ok(false);
            }
            let context = match (task.pending.take(), context) {
                // 上一次还没送：两段说明都留着（文件改动攒在一起）。
                (
                    Some(PendingFire {
                        context: Some(old), ..
                    }),
                    Some(new),
                ) if old != new => Some(format!("{old}\n\n{new}")),
                (Some(old), None) => old.context,
                (_, new) => new,
            };
            task.pending = Some(PendingFire {
                reason,
                context,
                requested_at_ms: self.now(),
            });
            Ok(true)
        })?
    }

    /// 交回一次条件命令的检查结果：更新基线，满足条件就记一次「等着触发」。
    /// `may_fire = false`（这个任务自己那一轮还在跑）时只更新基线——它自己改出来的
    /// 变化不该再叫醒它。回 `Ok(true)` = 这次会触发。
    pub fn observe(
        &self,
        id: &str,
        observation: &WatchObservation,
        may_fire: bool,
    ) -> Result<bool, CronError> {
        self.transact(|tasks| {
            let task = scoped(tasks, id, None)?;
            let Sched::Watch { command, when, .. } = &task.schedule else {
                return Ok(false);
            };
            let now = self.now();
            let prev = task.watch.clone().unwrap_or_default();
            if let Some(error) = &observation.error {
                task.watch = Some(CronWatchState {
                    checked_at_ms: now,
                    error: Some(error.clone()),
                    ..prev
                });
                return Ok(false);
            }
            let hash = observation.digest();
            let (fires, matched) = match when {
                WatchWhen::Changed => (prev.hash.as_ref().is_some_and(|h| *h != hash), false),
                other => {
                    let now_matched = observation.satisfies(other);
                    (now_matched && !prev.matched, now_matched)
                }
            };
            let fires = fires && may_fire && !task.paused && !task.done;
            if fires {
                let context = observation.context(command, when);
                task.pending = Some(PendingFire {
                    reason: FireReason::Watch,
                    context: Some(context),
                    requested_at_ms: now,
                });
            }
            task.watch = Some(CronWatchState {
                checked_at_ms: now,
                exit: observation.exit,
                hash: Some(hash),
                matched,
                error: None,
            });
            Ok(fires)
        })?
    }

    /// 驱动送完一次，交回结果：记进历史（留最近 [`HISTORY_LEN`] 次），更新 `last_error`。
    pub fn report(&self, id: &str, report: FireReport) {
        let now = self.now();
        let _ = self.transact(|tasks| {
            if let Some(task) = tasks.iter_mut().find(|t| t.id == id) {
                task.last_error = match report.outcome {
                    RunOutcome::Failed => report.error.clone().or(Some("没送到".into())),
                    _ => None,
                };
                task.history.push(CronRun {
                    at_ms: now,
                    reason: report.reason,
                    outcome: report.outcome,
                    session: report.session.clone(),
                    late_ms: report.late_by.map(ms),
                    error: report.error.clone(),
                });
                let extra = task.history.len().saturating_sub(HISTORY_LEN);
                task.history.drain(..extra);
            }
        });
    }

    pub fn list(&self) -> Vec<CronJob> {
        self.refresh();
        let state = self.state.lock().unwrap();
        state.tasks.iter().map(|t| self.snap(t)).collect()
    }

    /// `session` 名下的任务。
    pub fn list_for(&self, session: &str) -> Vec<CronJob> {
        self.list()
            .into_iter()
            .filter(|j| j.owner.as_ref().is_some_and(|o| o.session == session))
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<CronJob> {
        self.list().into_iter().find(|j| j.id == id)
    }

    /// 删掉一个任务。`Ok(false)` 是真没有这个 id；文件读写失败是 `Err(Store)`，
    /// 不能混成「没有」——否则文件坏了时模型 / GUI 只会以为 id 打错了。
    pub fn cancel(&self, id: &str) -> Result<bool, CronError> {
        self.cancel_where(id, None)
    }

    /// 同 [`Self::cancel`]，但只认 `session` 名下的任务。
    pub fn cancel_where(&self, id: &str, session: Option<&str>) -> Result<bool, CronError> {
        self.transact(|tasks| {
            let before = tasks.len();
            tasks.retain(|t| {
                !(t.id == id && session.is_none_or(|s| t.session.as_deref() == Some(s)))
            });
            tasks.len() != before
        })
    }

    /// 记下（或清掉）任务的错误：这次触发为什么没成、文件监听为什么挂不上。任务留着，
    /// 下次照常再试；跑完了的因此留在表上给人看，不被收掉。
    pub fn set_error(&self, id: &str, error: Option<String>) {
        let _ = self.transact(|tasks| {
            if let Some(task) = tasks.iter_mut().find(|t| t.id == id) {
                task.last_error = error;
            }
        });
    }

    /// Jobs this process holds that are due: pending fires (manual, watch, files),
    /// elapsed time slots, and watch checks; plus expiry drops and missed one-shots.
    /// Adopts tasks whose holder has exited first. Finished tasks (one-shot fired,
    /// `max_runs` reached) are dropped on the pass after their last fire unless that
    /// delivery failed. Paused tasks only fire manual runs.
    ///
    /// 文件读写失败回 `Err(Store)`，不当成「这一秒没有要触发的」——那样文件坏了
    /// 时驱动会一直静默不触发，没有任何线索。
    pub fn due(&self) -> Result<CronTick, CronError> {
        let me = self.me;
        let persistent = self.path.is_some();
        let grace = ms(MISSED_GRACE);
        let tick = self.transact(|tasks| {
            let now = self.now();
            let mut fires = Vec::new();
            let mut checks = Vec::new();
            let mut expired = Vec::new();
            let mut missed = Vec::new();
            tasks.retain_mut(|task| {
                let mine = !persistent || task.holder == Some(me);
                if !mine {
                    if holder_alive(task.holder, me) {
                        return true;
                    }
                    // 持有它的进程没了：接过来。错过的怎么办看下面的 catch_up。
                    task.holder = Some(me);
                }
                if task.expires_at_ms.is_some_and(|at| now >= at) {
                    expired.push(task.clone());
                    return false;
                }
                if let Some(pending) = task.pending.take() {
                    task.after_fire(pending.reason, None, now);
                    fires.push(RawFire {
                        task: task.clone(),
                        reason: pending.reason,
                        slot: None,
                        late: None,
                        context: pending.context,
                    });
                    return true;
                }
                if task.done {
                    // 最后一次没送到的留着给人看，其余收掉。
                    return task.last_error.is_some();
                }
                if task.paused {
                    return true;
                }
                let Some(slot) = task.next_at_ms.filter(|slot| *slot <= now) else {
                    return true;
                };
                if matches!(task.schedule, Sched::Watch { .. }) {
                    // 到点查一次命令（不补查，晚了就查这一次）。
                    task.next_at_ms = task.schedule.next_after(slot, now);
                    checks.push(task.clone());
                    return true;
                }
                let late = now - slot;
                let was_missed = late > grace;
                if was_missed && !task.catch_up {
                    // 错过了、不补：对齐到下一个未来时刻。一次性的就此作废。
                    task.next_at_ms = task.schedule.next_after(slot, now);
                    if task.next_at_ms.is_none() {
                        missed.push(task.clone());
                        return false;
                    }
                    return true;
                }
                let reason = if was_missed {
                    FireReason::CatchUp
                } else {
                    FireReason::Schedule
                };
                task.after_fire(reason, Some(slot), now);
                fires.push(RawFire {
                    task: task.clone(),
                    reason,
                    slot: Some(slot),
                    late: was_missed.then_some(late),
                    context: None,
                });
                true
            });
            (fires, checks, expired, missed)
        });
        let (fires, checks, expired, missed) = tick?;
        Ok(CronTick {
            fires: fires
                .into_iter()
                .map(|f| CronFire {
                    job: self.snap(&f.task),
                    reason: f.reason,
                    scheduled_at_ms: f.slot,
                    late_by: f.late.map(Duration::from_millis),
                    context: f.context,
                })
                .collect(),
            checks: checks
                .iter()
                .filter_map(|t| {
                    let job = self.snap(t);
                    match &job.schedule {
                        CronSchedule::Watch(spec) => Some(CronCheck {
                            spec: spec.clone(),
                            job,
                        }),
                        _ => None,
                    }
                })
                .collect(),
            expired: expired.iter().map(|t| self.snap(t)).collect(),
            missed: missed.iter().map(|t| self.snap(t)).collect(),
        })
    }

    fn snap(&self, task: &Task) -> CronJob {
        let held_here = self.path.is_none() || task.holder == Some(self.me);
        CronJob {
            id: task.id.clone(),
            schedule: CronSchedule::from(&task.schedule),
            prompt: task.prompt.clone(),
            created_at: self.instant_of(task.created_at_ms),
            next: task.next_at_ms.map(|at| self.instant_of(at)),
            owner: task.session.clone().map(|session| CronOwner {
                session,
                cwd: task.cwd.clone().unwrap_or_default(),
            }),
            created_at_ms: task.created_at_ms,
            next_at_ms: task.next_at_ms,
            expires_at_ms: task.expires_at_ms,
            last_fired_at_ms: task.last_fired_at_ms,
            last_error: task.last_error.clone(),
            runs: task.runs,
            max_runs: task.max_runs,
            catch_up: task.catch_up,
            target: task.target,
            preset: task.preset.clone(),
            paused: task.paused,
            done: task.done,
            pending: task.pending.is_some(),
            history: task.history.clone(),
            watch: task.watch.clone(),
            held_here,
            holder_live: held_here || holder_alive(task.holder, self.me),
        }
    }

    fn instant_of(&self, at_ms: u64) -> Instant {
        let (base, base_ms) = self.base;
        if at_ms >= base_ms {
            base + Duration::from_millis(at_ms - base_ms)
        } else {
            base.checked_sub(Duration::from_millis(base_ms - at_ms))
                .unwrap_or(base)
        }
    }

    /// 在锁里读、改、写。内存版只改缓存；落盘版先 `flock`，再读盘、改、原子写回。
    fn transact<R>(&self, f: impl FnOnce(&mut Vec<Task>) -> R) -> Result<R, CronError> {
        let Some(path) = &self.path else {
            let mut state = self.state.lock().unwrap();
            let before = state.tasks.clone();
            let out = f(&mut state.tasks);
            if state.tasks != before {
                self.revision.fetch_add(1, Ordering::Relaxed);
            }
            return Ok(out);
        };
        let _lock = FileLock::acquire(path).map_err(CronError::Store)?;
        let (mut tasks, _) = read_file(path).map_err(CronError::Store)?;
        let read = tasks.clone();
        let out = f(&mut tasks);
        if tasks != read {
            write_file(path, &tasks).map_err(CronError::Store)?;
        }
        let stamp = stamp_of(path);
        let mut state = self.state.lock().unwrap();
        if state.tasks != tasks {
            self.revision.fetch_add(1, Ordering::Relaxed);
        }
        state.tasks = tasks;
        state.stamp = stamp;
        Ok(out)
    }

    /// 盘上的文件变了（别的进程改的）就重读一遍。读不出来留着旧缓存。
    fn refresh(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let stamp = stamp_of(path);
        {
            let state = self.state.lock().unwrap();
            if state.stamp == stamp && stamp.is_some() {
                return;
            }
        }
        let Ok((tasks, stamp)) = read_file(path) else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if state.tasks != tasks {
            self.revision.fetch_add(1, Ordering::Relaxed);
        }
        state.tasks = tasks;
        state.stamp = stamp;
    }
}

/// `due()` 里攒的一次触发（锁外再换成 [`CronFire`]）。
struct RawFire {
    task: Task,
    reason: FireReason,
    slot: Option<u64>,
    late: Option<u64>,
    context: Option<String>,
}

/// 两个都是条件命令，且命令和条件都没变（只改了检查间隔）。
fn same_watch(a: &Sched, b: &Sched) -> bool {
    match (a, b) {
        (
            Sched::Watch {
                command: ca,
                when: wa,
                ..
            },
            Sched::Watch {
                command: cb,
                when: wb,
                ..
            },
        ) => ca == cb && wa == wb,
        _ => false,
    }
}

fn check_max_runs(max_runs: Option<u32>) -> Result<(), CronError> {
    if max_runs == Some(0) {
        return Err(CronError::InvalidSchedule("max_runs 至少是 1".into()));
    }
    Ok(())
}

fn never_fires(sched: &Sched) -> CronError {
    CronError::InvalidSchedule(match sched {
        Sched::At { .. } => "这个时间已经过了".into(),
        Sched::Cron { expr } => format!("cron 表达式 {expr:?} 以后不会再触发"),
        _ => "排不出下一次".into(),
    })
}

/// 过期时刻。默认：比一天还勤的 7 天后过期，其余不过期。
fn resolve_expiry(expiry: CronExpiry, sched: &Sched, now: u64) -> Option<u64> {
    match expiry {
        CronExpiry::Never => None,
        CronExpiry::At(at) => Some(at),
        CronExpiry::Default => {
            let period = CronSchedule::from(sched).period_hint(now)?;
            (ms(period) < DAY_MS).then(|| now.saturating_add(ms(recurring_task_ttl())))
        }
    }
}

/// `from` 之后第一个严格晚于 `now` 的 `from + k * every`。
fn next_future(from: u64, every: u64, now: u64) -> u64 {
    let every = every.max(1);
    if from > now {
        return from;
    }
    let behind = now - from;
    from.saturating_add((behind / every + 1).saturating_mul(every))
}

/// 持有者还活着吗。自己算活着；同 pid 但启动时刻不同 = 以前那个进程，已经没了。
fn holder_alive(holder: Option<Holder>, me: Holder) -> bool {
    let Some(holder) = holder else {
        return false;
    };
    if holder == me {
        return true;
    }
    if holder.pid == me.pid {
        return false;
    }
    pid_alive(holder.pid)
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 只做存在 / 权限检查，不发信号。
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// 非 unix 上查不了：保守地当它还活着，不抢。
#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    true
}

fn stamp_of(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// 读盘。文件不在 = 没有任务；写坏了、或是更新版本的 Dock 写的，就报错，不拿空表盖掉它。
fn read_file(path: &Path) -> Result<(Vec<Task>, Stamp), String> {
    let stamp = stamp_of(path);
    match std::fs::read(path) {
        Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Ok((Vec::new(), stamp)),
        Ok(bytes) => {
            let parse_err = |e: serde_json::Error| format!("{} 解析失败：{e}", path.display());
            let FileVersion { version } = serde_json::from_slice(&bytes).map_err(parse_err)?;
            if version > FILE_VERSION {
                return Err(format!(
                    "{} 是更新版本的 Dock 写的（version {version}），这个 Dock 只认到 {FILE_VERSION}；升级后再用",
                    path.display()
                ));
            }
            serde_json::from_slice::<FileBody>(&bytes)
                .map(|body| (body.tasks, stamp))
                .map_err(parse_err)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Vec::new(), None)),
        Err(e) => Err(format!("读 {} 失败：{e}", path.display())),
    }
}

fn write_file(path: &Path, tasks: &[Task]) -> Result<(), String> {
    let body = FileBody {
        version: FILE_VERSION,
        tasks: tasks.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&body).map_err(|e| e.to_string())?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("写 {} 失败：{e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("写 {} 失败：{e}", path.display())
    })
}

/// `schedules.json.lock` 上的排他锁，drop 时释放。
struct FileLock {
    _file: std::fs::File,
}

impl FileLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("建 {} 失败：{e}", dir.display()))?;
        }
        let lock = path.with_extension("json.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock)
            .map_err(|e| format!("打开 {} 失败：{e}", lock.display()))?;
        // 非 Unix 不加锁：产品主路径是 Unix（持有者存活也靠 `kill(pid, 0)`）。在
        // 非 Unix 上多个 Dock 共用一个 `$DOCK_HOME` 时，读改写没有互斥，可能
        // 互相覆盖。
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: fd 在 `file` 存活期间有效；LOCK_EX 阻塞到拿到为止。
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(format!(
                    "锁 {} 失败：{}",
                    lock.display(),
                    std::io::Error::last_os_error()
                ));
            }
        }
        Ok(Self { _file: file })
    }
}

fn clamp_every(every: Duration) -> Duration {
    if every.is_zero() {
        Duration::from_secs(1)
    } else {
        every
    }
}

impl Default for Cron {
    fn default() -> Self {
        Self::new()
    }
}

/// 挂在 `$DOCK_HOME/schedules.json` 上的 `"cron"`。
pub fn cron() -> Plugin {
    plugin("cron", Inject::new(), |ctx, _: &()| {
        let path = cordis_base::config::dock_home().join(SCHEDULES_FILE);
        Ok(Some(ctx.provide(CRON, Cron::at(path))?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手动拨的墙钟，从真墙钟附近起步（cron 按真实的本机日历算）。
    fn manual_clock() -> (Arc<AtomicU64>, Cron) {
        let now = Arc::new(AtomicU64::new(now_ms()));
        let clock = now.clone();
        let cron = Cron::with_clock(Arc::new(move || clock.load(Ordering::SeqCst)));
        (now, cron)
    }

    /// 以前用真墙钟、1ms 周期：两次 `due()` 之间只要过了 1ms（并行跑测试、机器忙时常见）
    /// 第二次就又触发，偶发失败。现在手动拨钟：同一时刻再查不触发，拨过一个周期才再触发。
    #[test]
    fn due_reschedules() {
        let (now, cron) = manual_clock();
        cron.add(Duration::from_millis(1), "tick").unwrap();
        // 周期按 clamp_every 规整过，按规整后的拨。
        let every = ms(clamp_every(Duration::from_millis(1)));
        now.fetch_add(every, Ordering::SeqCst);
        let first = cron.due().unwrap();
        assert_eq!(first.fires.len(), 1);
        assert_eq!(first.fires[0].job.prompt, "tick");
        assert!(first.expired.is_empty());
        let second = cron.due().unwrap();
        assert!(second.fires.is_empty(), "同一时刻不该再触发");
        assert!(second.expired.is_empty());
        now.fetch_add(every, Ordering::SeqCst);
        assert_eq!(cron.due().unwrap().fires.len(), 1, "过了一个周期再触发");
    }

    /// 回归：触发晚了一点（一秒一查、开页要等），下一次仍按原来的相位排，不往后漂。
    /// 旧实现是 `now + every`，每次晚多少就漂多少，「每天」的任务会越跑越晚。
    #[test]
    fn a_late_tick_does_not_drift_the_phase() {
        let (now, cron) = manual_clock();
        let t0 = now.load(Ordering::SeqCst);
        let id = cron.add(Duration::from_secs(60), "tick").unwrap();
        now.store(t0 + 60_700, Ordering::SeqCst);
        let tick = cron.due().unwrap();
        assert_eq!(tick.fires.len(), 1);
        assert_eq!(tick.fires[0].scheduled_at_ms, Some(t0 + 60_000));
        assert_eq!(tick.fires[0].late_by, None, "晚 0.7 秒不算错过");
        assert_eq!(cron.get(&id).unwrap().next_at_ms, Some(t0 + 120_000));
    }

    #[test]
    fn fire_immediately_is_due_now() {
        let cron = Cron::new();
        cron.add_ex(Duration::from_secs(60), "now", true).unwrap();
        let tick = cron.due().unwrap();
        assert_eq!(tick.fires.len(), 1);
        assert_eq!(tick.fires[0].job.prompt, "now");
        assert!(cron.due().unwrap().fires.is_empty());
    }

    #[test]
    fn ttl_expires_without_firing() {
        let cron = Cron::new();
        let id = cron.add(Duration::from_secs(60), "tick").unwrap();
        cron.transact(|tasks| tasks[0].expires_at_ms = Some(now_ms() - 1_000))
            .unwrap();
        let tick = cron.due().unwrap();
        assert!(tick.fires.is_empty(), "{tick:?}");
        assert_eq!(tick.expired.len(), 1);
        assert_eq!(tick.expired[0].id, id);
        assert!(cron.list().is_empty());
    }

    /// 默认过期看节奏：比一天还勤的 7 天，每天及更稀的、一次性的不过期；给了就照给的。
    #[test]
    fn default_expiry_follows_the_cadence() {
        let (now, cron) = manual_clock();
        let t0 = now.load(Ordering::SeqCst);
        let week = ms(recurring_task_ttl());
        let expiry_of = |schedule: CronSchedule, expiry: CronExpiry| {
            let id = cron
                .create(NewCronTask {
                    expiry,
                    ..NewCronTask::new(schedule, "x")
                })
                .unwrap();
            cron.get(&id).unwrap().expires_at_ms
        };
        let every = |secs| CronSchedule::Every(Duration::from_secs(secs));
        let d = CronExpiry::Default;
        assert_eq!(expiry_of(every(300), d), Some(t0 + week));
        assert_eq!(expiry_of(every(86_400), d), None);
        assert_eq!(
            expiry_of(CronSchedule::cron("*/10 * * * *").unwrap(), d),
            Some(t0 + week)
        );
        assert_eq!(
            expiry_of(CronSchedule::cron("0 9 * * 1-5").unwrap(), d),
            None
        );
        assert_eq!(expiry_of(CronSchedule::At(t0 + 60_000), d), None);
        assert_eq!(expiry_of(every(300), CronExpiry::Never), None);
        assert_eq!(
            expiry_of(every(86_400), CronExpiry::At(t0 + 5)),
            Some(t0 + 5)
        );
    }

    /// 错过了（Dock 没开 / 电脑睡着）：固定间隔默认不补，对齐到下一个未来时刻；
    /// cron 默认补一次，只补一次，并说晚了多久。
    #[test]
    fn missed_runs_skip_for_intervals_and_catch_up_once_for_cron() {
        let (now, cron) = manual_clock();
        let t0 = now.load(Ordering::SeqCst);
        let every = cron.add(Duration::from_secs(60), "loop").unwrap();
        let daily = cron
            .create(NewCronTask::new(
                CronSchedule::cron("0 9 * * *").unwrap(),
                "日报",
            ))
            .unwrap();
        // 把 cron 的那一次挪到 10 分钟前，再拨钟让间隔任务也落后 10 分钟。
        cron.transact(|tasks| {
            let task = tasks.iter_mut().find(|t| t.id == daily).unwrap();
            task.next_at_ms = Some(t0 + 60_000);
        })
        .unwrap();
        let late = 10 * 60_000;
        now.store(t0 + 60_000 + late, Ordering::SeqCst);

        let tick = cron.due().unwrap();
        assert_eq!(tick.fires.len(), 1, "{tick:?}");
        let fire = &tick.fires[0];
        assert_eq!(fire.job.id, daily);
        assert_eq!(fire.late_by, Some(Duration::from_millis(late)));
        let after = now.load(Ordering::SeqCst);
        assert!(fire.job.next_at_ms.unwrap() > after, "补完排到下一个 9 点");

        let job = cron.get(&every).unwrap();
        assert_eq!(job.runs, 0, "间隔任务错过了不补");
        let next = job.next_at_ms.unwrap();
        assert!(next > after && next <= after + 60_000, "{job:?}");
        assert_eq!((next - t0) % 60_000, 0, "相位不变");
        assert!(cron.due().unwrap().fires.is_empty(), "补过一次就不再补");
    }

    /// 只跑一次：到点触发一次，下一轮从表上收掉；最后一次没送到就留着给人看。
    #[test]
    fn a_one_shot_fires_once_and_then_leaves() {
        let (now, cron) = manual_clock();
        let t0 = now.load(Ordering::SeqCst);
        let ok = cron
            .create(NewCronTask::new(CronSchedule::At(t0 + 5_000), "提醒"))
            .unwrap();
        let failed = cron
            .create(NewCronTask::new(CronSchedule::At(t0 + 5_000), "提醒 2"))
            .unwrap();
        assert!(cron.due().unwrap().fires.is_empty(), "还没到");
        now.store(t0 + 5_000, Ordering::SeqCst);
        let tick = cron.due().unwrap();
        assert_eq!(tick.fires.len(), 2);
        assert!(tick.fires.iter().all(|f| f.job.finished()));
        cron.set_error(&failed, Some("会话开不了".into()));
        now.store(t0 + 6_000, Ordering::SeqCst);
        assert!(cron.due().unwrap().fires.is_empty(), "不再触发");
        assert!(cron.get(&ok).is_none(), "送到了的收掉");
        assert!(
            cron.get(&failed).is_some_and(|j| j.finished()),
            "没送到的留着"
        );
    }

    /// 一次性任务错过了、又说了不补：作废，并报出来（驱动据此给会话留一句说明）。
    #[test]
    fn a_missed_one_shot_without_catch_up_is_reported() {
        let (now, cron) = manual_clock();
        let t0 = now.load(Ordering::SeqCst);
        let id = cron
            .create(NewCronTask {
                catch_up: Some(false),
                ..NewCronTask::new(CronSchedule::At(t0 + 1_000), "开会")
            })
            .unwrap();
        now.store(t0 + 1_000 + ms(MISSED_GRACE) + 1, Ordering::SeqCst);
        let tick = cron.due().unwrap();
        assert!(tick.fires.is_empty(), "{tick:?}");
        assert_eq!(tick.missed.len(), 1);
        assert_eq!(tick.missed[0].id, id);
        assert!(cron.list().is_empty());
    }

    #[test]
    fn max_runs_ends_the_task() {
        let (now, cron) = manual_clock();
        let t0 = now.load(Ordering::SeqCst);
        let id = cron
            .create(NewCronTask {
                max_runs: Some(2),
                ..NewCronTask::new(CronSchedule::Every(Duration::from_secs(60)), "两次")
            })
            .unwrap();
        for k in 1..=2 {
            now.store(t0 + k * 60_000, Ordering::SeqCst);
            assert_eq!(cron.due().unwrap().fires.len(), 1);
        }
        assert!(cron.get(&id).unwrap().finished());
        now.store(t0 + 3 * 60_000, Ordering::SeqCst);
        assert!(cron.due().unwrap().fires.is_empty());
        assert!(cron.get(&id).is_none(), "跑满收掉");
        assert!(matches!(
            cron.create(NewCronTask {
                max_runs: Some(0),
                ..NewCronTask::new(CronSchedule::Every(Duration::from_secs(60)), "x")
            }),
            Err(CronError::InvalidSchedule(_))
        ));
    }

    #[test]
    fn a_time_in_the_past_is_rejected() {
        let cron = Cron::new();
        assert!(matches!(
            cron.create(NewCronTask::new(CronSchedule::At(now_ms() - 1), "x")),
            Err(CronError::InvalidSchedule(m)) if m.contains("已经过了")
        ));
        assert!(cron.list().is_empty());
    }

    /// 换触发方式：间隔 → cron 从现在起重排；跑完的一次性任务换个新时刻就重新排上。
    #[test]
    fn patch_switches_the_schedule_kind() {
        let (now, cron) = manual_clock();
        let t0 = now.load(Ordering::SeqCst);
        let id = cron.add(Duration::from_secs(60), "x").unwrap();
        let daily = CronSchedule::cron("0 9 * * *").unwrap();
        let job = cron
            .patch(
                &id,
                None,
                CronPatch {
                    schedule: Some(daily.clone()),
                    ..CronPatch::default()
                },
            )
            .unwrap();
        assert_eq!(job.schedule, daily);
        assert_eq!(job.next_at_ms, cron_next_after("0 9 * * *", t0));

        let once = cron
            .create(NewCronTask::new(CronSchedule::At(t0 + 1_000), "一次"))
            .unwrap();
        now.store(t0 + 1_000, Ordering::SeqCst);
        cron.due().unwrap();
        cron.set_error(&once, Some("没送到".into()));
        let again = cron
            .patch(
                &once,
                None,
                CronPatch {
                    schedule: Some(CronSchedule::At(t0 + 90_000)),
                    ..CronPatch::default()
                },
            )
            .unwrap();
        assert_eq!(again.next_at_ms, Some(t0 + 90_000));
        assert!(matches!(
            cron.patch(
                &once,
                None,
                CronPatch {
                    schedule: Some(CronSchedule::At(t0)),
                    ..CronPatch::default()
                },
            ),
            Err(CronError::InvalidSchedule(_))
        ));
    }

    #[test]
    fn max_50_rejects() {
        let cron = Cron::new();
        for i in 0..MAX_SCHEDULED_TASKS {
            cron.add(Duration::from_secs(60), format!("p{i}")).unwrap();
        }
        match cron.add(Duration::from_secs(60), "overflow") {
            Err(CronError::LimitReached(n)) => assert_eq!(n, MAX_SCHEDULED_TASKS),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn update_keeps_phase_when_interval_unchanged() {
        let cron = Cron::new();
        let id = cron.add(Duration::from_secs(60), "old").unwrap();
        let next_before = cron.list()[0].next;
        cron.update(&id, Some(Duration::from_secs(60)), Some("new"))
            .unwrap();
        let jobs = cron.list();
        assert_eq!(jobs[0].prompt, "new");
        assert_eq!(
            jobs[0].schedule,
            CronSchedule::Every(Duration::from_secs(60))
        );
        assert_eq!(jobs[0].next, next_before);
        assert_eq!(jobs[0].id, id);
    }

    #[test]
    fn update_unknown_id_errors() {
        let cron = Cron::new();
        match cron.update("cron-missing", None, Some("x")) {
            Err(CronError::UnknownId(id)) => assert_eq!(id, "cron-missing"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn empty_prompt_rejected() {
        let cron = Cron::new();
        assert!(matches!(
            cron.add(Duration::from_secs(60), "  "),
            Err(CronError::EmptyPrompt)
        ));
    }

    fn owner(session: &str) -> CronOwner {
        CronOwner {
            session: session.into(),
            cwd: PathBuf::from("/tmp/proj"),
        }
    }

    /// 一个已经退出的进程的 pid：起一个马上结束的子进程，等它退出。
    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    /// 把盘上任务的持有者换成别的进程（模拟另一个 Dock）。
    fn hand_to(path: &Path, pid: u32) {
        let (mut tasks, _) = read_file(path).unwrap();
        for t in &mut tasks {
            t.holder = Some(Holder {
                pid,
                started_at_ms: 1,
            });
        }
        write_file(path, &tasks).unwrap();
    }

    #[test]
    fn durable_tasks_survive_a_new_instance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCHEDULES_FILE);
        let id = Cron::at(&path)
            .add_owned(Some(owner("s-1")), Duration::from_secs(600), "check", false)
            .unwrap();
        let again = Cron::at(&path);
        let jobs = again.list();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, id);
        assert_eq!(jobs[0].owner, Some(owner("s-1")));
        assert_eq!(
            jobs[0].schedule,
            CronSchedule::Every(Duration::from_secs(600))
        );
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"version\": 2"), "{raw}");
        assert!(raw.contains("\"kind\": \"every\""), "{raw}");
    }

    /// v1 的文件（`everyMs` 在顶层）照读；下一次写回时升到 v2，原来的任务不丢字段。
    #[test]
    fn version_1_files_are_read_and_upgraded_on_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCHEDULES_FILE);
        let now = now_ms();
        let v1 = serde_json::json!({
            "version": 1,
            "tasks": [{
                "id": "cron-old",
                "session": "s-1",
                "cwd": "/tmp/proj",
                "everyMs": 600_000,
                "prompt": "老任务",
                "createdAtMs": now,
                "expiresAtMs": now + 1_000_000,
                "nextAtMs": now + 600_000,
                "lastFiredAtMs": now - 1,
            }]
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&v1).unwrap()).unwrap();
        let cron = Cron::at(&path);
        let old = cron.get("cron-old").unwrap();
        assert_eq!(old.schedule, CronSchedule::Every(Duration::from_secs(600)));
        assert_eq!(old.expires_at_ms, Some(now + 1_000_000));
        assert_eq!(old.next_at_ms, Some(now + 600_000));
        assert!(!old.catch_up, "v1 的循环不补跑");
        assert_eq!(old.owner, Some(owner("s-1")));

        cron.add(Duration::from_secs(60), "新任务").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"version\": 2"), "{raw}");
        assert!(
            !raw.contains("\"everyMs\": 600000,\n      \"prompt\""),
            "{raw}"
        );
        let again = Cron::at(&path).get("cron-old").unwrap();
        assert_eq!(again.schedule, old.schedule);
        assert_eq!(again.last_fired_at_ms, Some(now - 1));
    }

    /// 更新版本的 Dock 写的文件：不认识就报错，绝不按自己的格式写回去（会丢字段）。
    #[test]
    fn a_newer_file_version_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCHEDULES_FILE);
        let body = r#"{ "version": 3, "tasks": [] }"#;
        std::fs::write(&path, body).unwrap();
        let cron = Cron::at(&path);
        assert!(matches!(
            cron.add(Duration::from_secs(60), "x"),
            Err(CronError::Store(m)) if m.contains("更新版本")
        ));
        assert!(matches!(cron.due(), Err(CronError::Store(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    }

    /// 两个 Dock 共用一个 `$DOCK_HOME`：别的活进程持有的任务，这边不触发。
    #[test]
    fn a_task_held_by_a_live_process_is_not_fired_here() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCHEDULES_FILE);
        let cron = Cron::at(&path);
        cron.add_owned(Some(owner("s-1")), Duration::from_secs(60), "x", true)
            .unwrap();
        // pid 1 一直活着（launchd / init）。
        hand_to(&path, 1);
        let tick = cron.due().unwrap();
        assert!(tick.fires.is_empty(), "{tick:?}");
        assert!(!cron.list()[0].held_here);
    }

    /// 持有者退出了：下一个活进程接过来，停机期间错过的不补跑，对齐到下一个未来时刻。
    #[test]
    fn orphaned_tasks_are_adopted_without_replaying_missed_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCHEDULES_FILE);
        let cron = Cron::at(&path);
        cron.add_owned(Some(owner("s-1")), Duration::from_secs(60), "x", false)
            .unwrap();
        let (mut tasks, _) = read_file(&path).unwrap();
        let stale = now_ms() - 10 * 60_000 - 5_000;
        tasks[0].next_at_ms = Some(stale);
        write_file(&path, &tasks).unwrap();
        hand_to(&path, dead_pid());

        let tick = cron.due().unwrap();
        assert!(tick.fires.is_empty(), "错过的不补跑：{tick:?}");
        let job = &cron.list()[0];
        assert!(job.held_here, "接管了");
        let now = now_ms();
        let next = job.next_at_ms.unwrap();
        assert!(next > now && next <= now + 60_000, "{job:?}");
        assert_eq!((next - stale) % 60_000, 0, "相位不变");
    }

    /// 文件写坏了：读写都报错，不拿空表把它盖掉。
    #[test]
    fn a_corrupt_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCHEDULES_FILE);
        std::fs::write(&path, "{ not json").unwrap();
        let cron = Cron::at(&path);
        assert!(matches!(
            cron.add(Duration::from_secs(60), "x"),
            Err(CronError::Store(_))
        ));
        // 读写失败要报出来，不能混成「这一秒没有要触发的」「没有这个 id」。
        assert!(matches!(cron.due(), Err(CronError::Store(_))));
        assert!(matches!(
            cron.cancel_where("cron-1", Some("s-a")),
            Err(CronError::Store(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    /// 模型工具只能改 / 删自己会话的任务。
    #[test]
    fn scoped_update_and_cancel_only_touch_that_session() {
        let cron = Cron::new();
        let a = cron
            .add_owned(Some(owner("s-a")), Duration::from_secs(60), "a", false)
            .unwrap();
        let hijack = CronPatch {
            prompt: Some("hijack".into()),
            ..CronPatch::default()
        };
        assert!(matches!(
            cron.patch(&a, Some("s-b"), hijack),
            Err(CronError::UnknownId(_))
        ));
        assert!(!cron.cancel_where(&a, Some("s-b")).unwrap());
        assert_eq!(cron.list_for("s-a").len(), 1);
        assert!(cron.list_for("s-b").is_empty());
        assert!(cron.cancel_where(&a, Some("s-a")).unwrap());
        assert!(cron.list().is_empty());
    }

    #[test]
    fn revision_moves_on_every_change() {
        let cron = Cron::new();
        let r0 = cron.revision();
        let id = cron.add(Duration::from_secs(60), "x").unwrap();
        let r1 = cron.revision();
        assert!(r1 > r0);
        assert!(cron.due().unwrap().fires.is_empty());
        assert_eq!(cron.revision(), r1, "没变就不动");
        cron.cancel(&id).unwrap();
        assert!(cron.revision() > r1);
    }

    fn watch_task(cron: &Cron, when: WatchWhen) -> String {
        cron.create(NewCronTask::new(
            CronSchedule::watch("gh pr checks 12", Duration::from_secs(60), when).unwrap(),
            "CI 有动静了，看一下",
        ))
        .unwrap()
    }

    fn seen(exit: i32, output: &str) -> WatchObservation {
        WatchObservation {
            exit: Some(exit),
            output: output.into(),
            error: None,
        }
    }

    /// 条件命令「结果变了」：建好马上查一次定基线（不触发），之后按间隔查；变了才触发，
    /// 带着命令的输出；查不成（超时 / 没跑起来）不算数。
    #[test]
    fn a_watch_sets_a_baseline_then_fires_when_the_output_changes() {
        let (now, cron) = manual_clock();
        let t0 = now.load(Ordering::SeqCst);
        let id = watch_task(&cron, WatchWhen::Changed);
        let tick = cron.due().unwrap();
        assert_eq!(tick.checks.len(), 1, "马上查第一次");
        assert!(tick.fires.is_empty());
        assert_eq!(tick.checks[0].spec.command, "gh pr checks 12");
        assert!(
            !cron.observe(&id, &seen(0, "pending"), true).unwrap(),
            "第一次只记基线"
        );
        assert!(
            cron.due().unwrap().checks.is_empty(),
            "下一次检查在一分钟后"
        );

        now.store(t0 + 60_000, Ordering::SeqCst);
        assert_eq!(cron.due().unwrap().checks.len(), 1);
        assert!(!cron
            .observe(&id, &WatchObservation::failed("超时"), true)
            .unwrap());
        assert!(
            !cron.observe(&id, &seen(0, "pending"), true).unwrap(),
            "没变"
        );
        assert!(
            cron.observe(&id, &seen(1, "fail: test_x"), true).unwrap(),
            "变了"
        );
        let tick = cron.due().unwrap();
        assert_eq!(tick.fires.len(), 1);
        let fire = &tick.fires[0];
        assert_eq!(fire.reason, FireReason::Watch);
        let context = fire.context.as_deref().unwrap();
        assert!(context.contains("fail: test_x"), "{context}");
        assert!(context.contains("exit code 1"), "{context}");
        assert_eq!(fire.job.runs, 1, "条件触发算次数");
        assert!(!fire.job.done, "条件命令一直盯着");
    }

    /// 「成功时」是边沿触发：从失败变成功触发一次，一直成功不重复叫。
    #[test]
    fn a_watch_condition_fires_on_the_edge_only() {
        let cron = Cron::new();
        let id = watch_task(&cron, WatchWhen::Succeeds);
        let fired: Vec<bool> = [1, 0, 0, 1, 0]
            .into_iter()
            .map(|exit| cron.observe(&id, &seen(exit, ""), true).unwrap())
            .collect();
        assert_eq!(fired, vec![false, true, false, false, true]);
    }

    /// 这个任务自己那一轮还在跑（可能就是它改出来的变化）：只更新基线，不叫醒它。
    #[test]
    fn a_watch_does_not_fire_while_its_own_run_is_going() {
        let cron = Cron::new();
        let id = watch_task(&cron, WatchWhen::Changed);
        cron.observe(&id, &seen(0, "a"), true).unwrap();
        assert!(!cron.observe(&id, &seen(0, "b"), false).unwrap());
        assert!(
            !cron.observe(&id, &seen(0, "b"), true).unwrap(),
            "基线已经是 b 了"
        );
        assert!(cron.due().unwrap().fires.is_empty());
    }

    /// 手动运行：只记一次「等着」，下一轮送出；暂停、跑完的也能手动跑；事件触发碰到暂停就算了。
    /// 手动的不占次数、不动排期。
    #[test]
    fn manual_runs_work_on_paused_tasks_and_do_not_count() {
        let (now, cron) = manual_clock();
        let t0 = now.load(Ordering::SeqCst);
        let id = cron
            .create(NewCronTask {
                max_runs: Some(1),
                paused: true,
                ..NewCronTask::new(CronSchedule::Every(Duration::from_secs(60)), "x")
            })
            .unwrap();
        assert!(!cron
            .trigger(&id, None, FireReason::Files, Some("a.md".into()))
            .unwrap());
        assert!(cron
            .trigger(&id, None, FireReason::Manual, Some("Input: v1.2".into()))
            .unwrap());
        let tick = cron.due().unwrap();
        assert_eq!(tick.fires.len(), 1);
        assert_eq!(tick.fires[0].reason, FireReason::Manual);
        assert_eq!(tick.fires[0].context.as_deref(), Some("Input: v1.2"));
        let job = cron.get(&id).unwrap();
        assert_eq!(job.runs, 0, "手动的不算次数");
        assert_eq!(job.next_at_ms, Some(t0 + 60_000), "不动排期");
        assert!(matches!(
            cron.trigger("cron-nope", None, FireReason::Manual, None),
            Err(CronError::UnknownId(_))
        ));
    }

    /// 事件接连来、上一次还没送：两段说明都留着，只送一次。
    #[test]
    fn pending_triggers_merge_their_context() {
        let cron = Cron::new();
        let id = cron
            .create(NewCronTask::new(
                CronSchedule::files(vec!["docs".into()], DEFAULT_FILES_DEBOUNCE).unwrap(),
                "同步翻译",
            ))
            .unwrap();
        cron.trigger(&id, None, FireReason::Files, Some("- a.md".into()))
            .unwrap();
        cron.trigger(&id, None, FireReason::Files, Some("- b.md".into()))
            .unwrap();
        let tick = cron.due().unwrap();
        assert_eq!(tick.fires.len(), 1);
        assert_eq!(tick.fires[0].context.as_deref(), Some("- a.md\n\n- b.md"));
        assert!(cron.due().unwrap().fires.is_empty());
        assert!(!cron.get(&id).unwrap().done, "文件变化一直盯着");
    }

    /// 暂停期间的时刻不跑；继续以后跳过它们、按原相位排下一次，不补。
    #[test]
    fn a_resumed_task_skips_the_slots_it_missed_while_paused() {
        let (now, cron) = manual_clock();
        let t0 = now.load(Ordering::SeqCst);
        let id = cron.add(Duration::from_secs(60), "x").unwrap();
        let pause = |paused| CronPatch {
            paused: Some(paused),
            ..CronPatch::default()
        };
        cron.patch(&id, None, pause(true)).unwrap();
        now.store(t0 + 5 * 60_000 + 30_000, Ordering::SeqCst);
        assert!(cron.due().unwrap().fires.is_empty(), "暂停着不跑");
        let job = cron.patch(&id, None, pause(false)).unwrap();
        assert_eq!(
            job.next_at_ms,
            Some(t0 + 6 * 60_000),
            "跳过暂停期间的，相位不变"
        );
        assert!(cron.due().unwrap().fires.is_empty());
    }

    /// 每次送完记进历史（最近 `HISTORY_LEN` 次），没送到的记下原因、送到了就清掉。
    #[test]
    fn reports_build_a_bounded_history() {
        let cron = Cron::new();
        let id = cron.add(Duration::from_secs(60), "x").unwrap();
        let report = |outcome, error: Option<&str>| FireReport {
            reason: FireReason::Schedule,
            outcome,
            session: Some("s-1".into()),
            late_by: None,
            error: error.map(str::to_string),
        };
        cron.report(&id, report(RunOutcome::Failed, Some("会话开不了")));
        assert_eq!(
            cron.get(&id).unwrap().last_error.as_deref(),
            Some("会话开不了")
        );
        for _ in 0..HISTORY_LEN + 3 {
            cron.report(&id, report(RunOutcome::Sent, None));
        }
        let job = cron.get(&id).unwrap();
        assert_eq!(job.history.len(), HISTORY_LEN);
        assert!(job.history.iter().all(|r| r.outcome == RunOutcome::Sent));
        assert_eq!(job.last_error, None);
        assert_eq!(job.last_run().unwrap().session.as_deref(), Some("s-1"));
    }

    /// 新字段（条件命令、目标、暂停、历史、基线、等着的触发）都落盘，换个实例读得回来。
    #[test]
    fn every_new_field_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCHEDULES_FILE);
        let cron = Cron::at(&path);
        let id = cron
            .create(NewCronTask {
                owner: Some(owner("s-1")),
                target: CronTarget::NewSession,
                preset: Some("code".into()),
                paused: true,
                ..NewCronTask::new(
                    CronSchedule::watch(
                        "curl -s example.com",
                        Duration::from_secs(120),
                        WatchWhen::Matches {
                            pattern: "sold out".into(),
                        },
                    )
                    .unwrap(),
                    "有货了告诉我",
                )
            })
            .unwrap();
        cron.observe(&id, &seen(0, "in stock"), true).unwrap();
        cron.trigger(&id, None, FireReason::Manual, Some("看一眼".into()))
            .unwrap();
        cron.report(
            &id,
            FireReport {
                reason: FireReason::Manual,
                outcome: RunOutcome::Coalesced,
                session: Some("s-2".into()),
                late_by: None,
                error: None,
            },
        );
        let before = cron.get(&id).unwrap();
        let after = Cron::at(&path).get(&id).unwrap();
        assert_eq!(after.schedule, before.schedule);
        assert_eq!(after.target, CronTarget::NewSession);
        assert_eq!(after.preset.as_deref(), Some("code"));
        assert!(after.paused && after.pending);
        assert_eq!(after.history, before.history);
        assert_eq!(after.watch, before.watch);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"kind\": \"watch\""), "{raw}");
        assert!(raw.contains("\"target\": \"newSession\""), "{raw}");
    }

    #[test]
    fn next_future_keeps_phase() {
        assert_eq!(next_future(100, 10, 95), 100);
        assert_eq!(next_future(100, 10, 100), 110);
        assert_eq!(next_future(100, 10, 125), 130);
    }
}
