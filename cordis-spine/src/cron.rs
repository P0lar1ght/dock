//! In-process cron. Grok's scheduler is ACP/workspace-heavy; this keeps the
//! interval + prompt surface (`/loop` → `scheduler_create`) and fires due jobs
//! to the session.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cordis::{Inject, Plugin, plugin};

use crate::names::CRON;

/// Grok `RECURRING_TASK_TTL_DAYS`. Recurring tasks drop after this many days.
pub const RECURRING_TASK_TTL_DAYS: u64 = 7;

/// Grok `MAX_SCHEDULED_TASKS`.
pub const MAX_SCHEDULED_TASKS: usize = 50;

pub fn recurring_task_ttl() -> Duration {
    Duration::from_secs(RECURRING_TASK_TTL_DAYS.saturating_mul(86_400))
}

#[derive(Clone, Debug)]
pub struct CronJob {
    pub id: String,
    pub every: Duration,
    pub prompt: String,
    pub created_at: Instant,
    pub next: Instant,
}

/// One ticker pass: fires to run, plus tasks that hit the 7-day TTL (removed
/// without firing, matching Grok non-durable expiry).
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
}

#[derive(Debug)]
struct Inner {
    id: String,
    every: Duration,
    prompt: String,
    next: Instant,
    created_at: Instant,
    expires_at: Instant,
    last_fired_at: Option<Instant>,
}

/// Named `"cron"` service. Live-lookup from the ticker; do not capture the Arc.
#[derive(Debug)]
pub struct Cron {
    jobs: Mutex<Vec<Inner>>,
    seq: AtomicU64,
}

impl Cron {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(Vec::new()),
            seq: AtomicU64::new(1),
        }
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
        let prompt = prompt.into();
        if prompt.trim().is_empty() {
            return Err(CronError::EmptyPrompt);
        }
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.len() >= MAX_SCHEDULED_TASKS {
            return Err(CronError::LimitReached(MAX_SCHEDULED_TASKS));
        }
        let every = clamp_every(every);
        let now = Instant::now();
        let id = format!("cron-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        let next = if fire_immediately { now } else { now + every };
        jobs.push(Inner {
            id: id.clone(),
            every,
            prompt,
            next,
            created_at: now,
            expires_at: now + recurring_task_ttl(),
            last_fired_at: None,
        });
        Ok(id)
    }

    /// Update in place. Omitted fields stay unchanged; interval changes keep
    /// phase from `last_fired_at` (or `created_at`). TTL is not reset.
    pub fn update(
        &self,
        id: &str,
        every: Option<Duration>,
        prompt: Option<&str>,
    ) -> Result<CronJob, CronError> {
        let mut jobs = self.jobs.lock().unwrap();
        let job = jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| CronError::UnknownId(id.to_string()))?;
        if let Some(text) = prompt {
            if text.trim().is_empty() {
                return Err(CronError::EmptyPrompt);
            }
            job.prompt = text.to_string();
        }
        if let Some(every) = every {
            let every = clamp_every(every);
            if every != job.every {
                let anchor = job.last_fired_at.unwrap_or(job.created_at);
                job.every = every;
                let mut next = anchor + every;
                let now = Instant::now();
                if next <= now {
                    next = now;
                }
                job.next = next;
            }
        }
        Ok(snap(job))
    }

    pub fn list(&self) -> Vec<CronJob> {
        self.jobs.lock().unwrap().iter().map(snap).collect()
    }

    pub fn cancel(&self, id: &str) -> bool {
        let mut jobs = self.jobs.lock().unwrap();
        let before = jobs.len();
        jobs.retain(|j| j.id != id);
        jobs.len() != before
    }

    /// Jobs whose `next` has elapsed (rescheduled by `every`), plus TTL drops.
    pub fn due(&self) -> CronTick {
        let now = Instant::now();
        let mut jobs = self.jobs.lock().unwrap();
        let mut fires = Vec::new();
        let mut expired = Vec::new();
        jobs.retain_mut(|job| {
            if now >= job.expires_at {
                expired.push(snap(job));
                return false;
            }
            if job.next <= now {
                fires.push(snap(job));
                job.last_fired_at = Some(now);
                job.next = now + job.every;
            }
            true
        });
        CronTick { fires, expired }
    }
}

fn clamp_every(every: Duration) -> Duration {
    if every.is_zero() {
        Duration::from_secs(1)
    } else {
        every
    }
}

fn snap(job: &Inner) -> CronJob {
    CronJob {
        id: job.id.clone(),
        every: job.every,
        prompt: job.prompt.clone(),
        created_at: job.created_at,
        next: job.next,
    }
}

impl Default for Cron {
    fn default() -> Self {
        Self::new()
    }
}

pub fn cron() -> Plugin {
    plugin("cron", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(CRON, Cron::new())?))
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
        let first = cron.due();
        assert_eq!(first.fires.len(), 1);
        assert_eq!(first.fires[0].prompt, "tick");
        assert!(first.expired.is_empty());
        let second = cron.due();
        assert!(second.fires.is_empty());
        assert!(second.expired.is_empty());
    }

    #[test]
    fn fire_immediately_is_due_now() {
        let cron = Cron::new();
        cron.add_ex(Duration::from_secs(60), "now", true).unwrap();
        let tick = cron.due();
        assert_eq!(tick.fires.len(), 1);
        assert_eq!(tick.fires[0].prompt, "now");
        assert!(cron.due().fires.is_empty());
    }

    #[test]
    fn ttl_expires_without_firing() {
        let cron = Cron::new();
        let id = cron.add(Duration::from_secs(60), "tick").unwrap();
        {
            let mut jobs = cron.jobs.lock().unwrap();
            jobs[0].expires_at = Instant::now() - Duration::from_secs(1);
        }
        let tick = cron.due();
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
}
