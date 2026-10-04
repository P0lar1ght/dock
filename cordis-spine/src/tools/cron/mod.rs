//! Scheduled tasks. Grok's scheduler is ACP/workspace-heavy; this keeps the
//! interval + prompt surface (`/loop` → `scheduler_create`) and leaves firing to
//! the app's ticker (`cordis-app` `cron_driver`).
//!
//! **Durable**: the app mounts [`Cron::at`] on `$DOCK_HOME/schedules.json`, so tasks
//! survive a restart. Several Dock processes may share one `$DOCK_HOME` (a TUI and
//! the desktop app), so every task names the process that **holds** it: only the
//! holder fires it. A task whose holder has exited is adopted by the next live
//! process on its next tick. Every read-modify-write runs under an exclusive
//! `flock` on `schedules.json.lock` and lands with an atomic rename.
//!
//! Each task also names the **session** it belongs to (and that session's cwd):
//! the ticker submits the prompt there, opening the session as a page when it is
//! closed. Tasks without an owner (old callers, tests) fire into the first page.
//!
//! Runs missed while no Dock was up are not replayed: an adopted task realigns to
//! its next future slot. Tasks still expire [`RECURRING_TASK_TTL_DAYS`] days after
//! creation (Grok parity — a forgotten loop should not spend tokens forever).
//! [`Cron::new`] is the in-memory variant for tests and harnesses.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cordis::{plugin, Inject, Plugin};
use serde::{Deserialize, Serialize};

use crate::names::CRON;

/// Grok `RECURRING_TASK_TTL_DAYS`. Recurring tasks drop after this many days.
pub const RECURRING_TASK_TTL_DAYS: u64 = 7;

/// Grok `MAX_SCHEDULED_TASKS`.
pub const MAX_SCHEDULED_TASKS: usize = 50;

/// 落盘文件名（在 `$DOCK_HOME` 下）。
pub const SCHEDULES_FILE: &str = "schedules.json";
const FILE_VERSION: u32 = 1;

pub fn recurring_task_ttl() -> Duration {
    Duration::from_secs(RECURRING_TASK_TTL_DAYS.saturating_mul(86_400))
}

/// 任务属于哪个会话：到点把提问提交到这里。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CronOwner {
    /// 落盘会话 id（`Sessions::live_session_id`）。
    pub session: String,
    /// 那个会话的工作目录：会话关着时按它开页。
    pub cwd: PathBuf,
}

#[derive(Clone, Debug)]
pub struct CronJob {
    pub id: String,
    pub every: Duration,
    pub prompt: String,
    pub created_at: Instant,
    pub next: Instant,
    /// `None`：老调用方建的，触发到第一页。
    pub owner: Option<CronOwner>,
    pub created_at_ms: u64,
    pub next_at_ms: u64,
    pub expires_at_ms: u64,
    pub last_fired_at_ms: Option<u64>,
    /// 上次触发失败的原因（会话开不了之类）；下次成功触发时清掉。
    pub last_error: Option<String>,
    /// 这个进程持有它（只有持有者会触发）。
    pub held_here: bool,
}

/// One ticker pass: fires to run, plus tasks that hit the 7-day TTL (removed
/// without firing, matching Grok expiry). Only tasks this process holds.
#[derive(Clone, Debug, Default)]
pub struct CronTick {
    pub fires: Vec<CronJob>,
    pub expired: Vec<CronJob>,
}

#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("maximum {0} scheduled tasks")]
    LimitReached(usize),
    #[error("prompt is required")]
    EmptyPrompt,
    #[error("unknown task id {0}")]
    UnknownId(String),
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Task {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
    every_ms: u64,
    prompt: String,
    created_at_ms: u64,
    expires_at_ms: u64,
    next_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_fired_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    holder: Option<Holder>,
}

#[derive(Default, Serialize, Deserialize)]
struct FileBody {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    tasks: Vec<Task>,
}

/// 上次读到的文件（mtime + 长度）。没变就不重读。
type Stamp = Option<(SystemTime, u64)>;

#[derive(Default)]
struct State {
    tasks: Vec<Task>,
    stamp: Stamp,
}

/// Named `"cron"` service. Live-lookup from the ticker; do not capture the Arc.
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
}

impl std::fmt::Debug for Cron {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cron").field("path", &self.path).finish()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ms(d: Duration) -> u64 {
    d.as_millis().min(u128::from(u64::MAX)) as u64
}

impl Cron {
    /// 只在内存里的一份（测试、harness）。所有任务都算本进程持有。
    pub fn new() -> Self {
        Self::build(None)
    }

    /// 落在 `path`（一般是 `$DOCK_HOME/schedules.json`）的一份。
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self::build(Some(path.into()))
    }

    fn build(path: Option<PathBuf>) -> Self {
        let now = now_ms();
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
        }
    }

    /// 落盘路径；内存版为 `None`。
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// 内容版本：任务增删改、触发、或读到别的进程的改动都会变。
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

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

    /// 建一个属于 `owner` 会话的任务，本进程持有。
    pub fn add_owned(
        &self,
        owner: Option<CronOwner>,
        every: Duration,
        prompt: impl Into<String>,
        fire_immediately: bool,
    ) -> Result<String, CronError> {
        let prompt = prompt.into();
        if prompt.trim().is_empty() {
            return Err(CronError::EmptyPrompt);
        }
        let every = clamp_every(every);
        let me = self.me;
        let seq = &self.seq;
        self.transact(|tasks| {
            if tasks.len() >= MAX_SCHEDULED_TASKS {
                return Err(CronError::LimitReached(MAX_SCHEDULED_TASKS));
            }
            let now = now_ms();
            let id = loop {
                let id = format!("cron-{:x}{:x}", now, seq.fetch_add(1, Ordering::Relaxed));
                if !tasks.iter().any(|t| t.id == id) {
                    break id;
                }
            };
            let every_ms = ms(every);
            tasks.push(Task {
                id: id.clone(),
                session: owner.as_ref().map(|o| o.session.clone()),
                cwd: owner.as_ref().map(|o| o.cwd.clone()),
                every_ms,
                prompt,
                created_at_ms: now,
                expires_at_ms: now.saturating_add(ms(recurring_task_ttl())),
                next_at_ms: if fire_immediately {
                    now
                } else {
                    now.saturating_add(every_ms)
                },
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
        self.update_where(id, None, every, prompt)
    }

    /// 同 [`Self::update`]，但只认 `session` 名下的任务（模型工具不能改别的会话的）。
    pub fn update_where(
        &self,
        id: &str,
        session: Option<&str>,
        every: Option<Duration>,
        prompt: Option<&str>,
    ) -> Result<CronJob, CronError> {
        if prompt.is_some_and(|p| p.trim().is_empty()) {
            return Err(CronError::EmptyPrompt);
        }
        let updated = self.transact(|tasks| {
            let task = tasks
                .iter_mut()
                .find(|t| t.id == id && session.is_none_or(|s| t.session.as_deref() == Some(s)))
                .ok_or_else(|| CronError::UnknownId(id.to_string()))?;
            if let Some(text) = prompt {
                task.prompt = text.to_string();
            }
            if let Some(every) = every {
                let every_ms = ms(clamp_every(every));
                if every_ms != task.every_ms {
                    let anchor = task.last_fired_at_ms.unwrap_or(task.created_at_ms);
                    task.every_ms = every_ms;
                    task.next_at_ms = anchor.saturating_add(every_ms).max(now_ms());
                }
            }
            Ok(task.clone())
        })??;
        Ok(self.snap(&updated))
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

    /// 记下这次触发为什么没成（会话开不了之类）。任务留着，下次照常再试。
    pub fn set_error(&self, id: &str, error: Option<String>) {
        let _ = self.transact(|tasks| {
            if let Some(task) = tasks.iter_mut().find(|t| t.id == id) {
                task.last_error = error;
            }
        });
    }

    /// Jobs this process holds whose `next` has elapsed (rescheduled by
    /// `every`), plus TTL drops. Adopts tasks whose holder has exited first.
    ///
    /// 文件读写失败回 `Err(Store)`，不当成「这一秒没有要触发的」——那样文件坏了
    /// 时驱动会一直静默不触发，没有任何线索。
    pub fn due(&self) -> Result<CronTick, CronError> {
        let me = self.me;
        let persistent = self.path.is_some();
        let tick = self.transact(|tasks| {
            let now = now_ms();
            let mut fires = Vec::new();
            let mut expired = Vec::new();
            tasks.retain_mut(|task| {
                let mine = !persistent || task.holder == Some(me);
                if !mine {
                    if holder_alive(task.holder, me) {
                        return true;
                    }
                    // 持有它的进程没了：接过来。停机期间错过的不补跑。
                    task.holder = Some(me);
                    if task.next_at_ms <= now {
                        task.next_at_ms = next_future(task.next_at_ms, task.every_ms, now);
                    }
                }
                if now >= task.expires_at_ms {
                    expired.push(task.clone());
                    return false;
                }
                if task.next_at_ms <= now {
                    fires.push(task.clone());
                    task.last_fired_at_ms = Some(now);
                    task.next_at_ms = now.saturating_add(task.every_ms);
                }
                true
            });
            (fires, expired)
        });
        let (fires, expired) = tick?;
        Ok(CronTick {
            fires: fires.iter().map(|t| self.snap(t)).collect(),
            expired: expired.iter().map(|t| self.snap(t)).collect(),
        })
    }

    fn snap(&self, task: &Task) -> CronJob {
        CronJob {
            id: task.id.clone(),
            every: Duration::from_millis(task.every_ms),
            prompt: task.prompt.clone(),
            created_at: self.instant_of(task.created_at_ms),
            next: self.instant_of(task.next_at_ms),
            owner: task.session.clone().map(|session| CronOwner {
                session,
                cwd: task.cwd.clone().unwrap_or_default(),
            }),
            created_at_ms: task.created_at_ms,
            next_at_ms: task.next_at_ms,
            expires_at_ms: task.expires_at_ms,
            last_fired_at_ms: task.last_fired_at_ms,
            last_error: task.last_error.clone(),
            held_here: self.path.is_none() || task.holder == Some(self.me),
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

/// 读盘。文件不在 = 没有任务；写坏了就报错，不拿空表盖掉它。
fn read_file(path: &Path) -> Result<(Vec<Task>, Stamp), String> {
    let stamp = stamp_of(path);
    match std::fs::read(path) {
        Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Ok((Vec::new(), stamp)),
        Ok(bytes) => serde_json::from_slice::<FileBody>(&bytes)
            .map(|body| (body.tasks, stamp))
            .map_err(|e| format!("{} 解析失败：{e}", path.display())),
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

    #[test]
    fn due_reschedules() {
        let cron = Cron::new();
        cron.add(Duration::from_millis(1), "tick").unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let first = cron.due().unwrap();
        assert_eq!(first.fires.len(), 1);
        assert_eq!(first.fires[0].prompt, "tick");
        assert!(first.expired.is_empty());
        let second = cron.due().unwrap();
        assert!(second.fires.is_empty());
        assert!(second.expired.is_empty());
    }

    #[test]
    fn fire_immediately_is_due_now() {
        let cron = Cron::new();
        cron.add_ex(Duration::from_secs(60), "now", true).unwrap();
        let tick = cron.due().unwrap();
        assert_eq!(tick.fires.len(), 1);
        assert_eq!(tick.fires[0].prompt, "now");
        assert!(cron.due().unwrap().fires.is_empty());
    }

    #[test]
    fn ttl_expires_without_firing() {
        let cron = Cron::new();
        let id = cron.add(Duration::from_secs(60), "tick").unwrap();
        cron.transact(|tasks| tasks[0].expires_at_ms = now_ms() - 1_000)
            .unwrap();
        let tick = cron.due().unwrap();
        assert!(tick.fires.is_empty(), "{tick:?}");
        assert_eq!(tick.expired.len(), 1);
        assert_eq!(tick.expired[0].id, id);
        assert!(cron.list().is_empty());
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
        assert_eq!(jobs[0].every, Duration::from_secs(60));
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
        assert_eq!(jobs[0].every, Duration::from_secs(600));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"version\": 1"), "{raw}");
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
        tasks[0].next_at_ms = stale;
        write_file(&path, &tasks).unwrap();
        hand_to(&path, dead_pid());

        let tick = cron.due().unwrap();
        assert!(tick.fires.is_empty(), "错过的不补跑：{tick:?}");
        let job = &cron.list()[0];
        assert!(job.held_here, "接管了");
        let now = now_ms();
        assert!(
            job.next_at_ms > now && job.next_at_ms <= now + 60_000,
            "{job:?}"
        );
        assert_eq!((job.next_at_ms - stale) % 60_000, 0, "相位不变");
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
        assert!(matches!(
            cron.update_where(&a, Some("s-b"), None, Some("hijack")),
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

    #[test]
    fn next_future_keeps_phase() {
        assert_eq!(next_future(100, 10, 95), 100);
        assert_eq!(next_future(100, 10, 100), 110);
        assert_eq!(next_future(100, 10, 125), 130);
    }
}
