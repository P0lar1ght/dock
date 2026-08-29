//! In-process cron. Grok's scheduler is ACP/workspace-heavy; this keeps the
//! interval + prompt surface (`/loop 5m …`) and fires due jobs to the session.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cordis::{plugin, Inject, Plugin};

use crate::names::CRON;

#[derive(Clone, Debug)]
pub struct CronJob {
    pub id: String,
    pub every: Duration,
    pub prompt: String,
    pub created_at: Instant,
    pub next: Instant,
}

#[derive(Debug)]
struct Inner {
    id: String,
    every: Duration,
    prompt: String,
    next: Instant,
    created_at: Instant,
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

    pub fn add(&self, every: Duration, prompt: impl Into<String>) -> String {
        let id = format!("cron-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        let every = if every.is_zero() {
            Duration::from_secs(1)
        } else {
            every
        };
        let now = Instant::now();
        self.jobs.lock().unwrap().push(Inner {
            id: id.clone(),
            every,
            prompt: prompt.into(),
            next: now + every,
            created_at: now,
        });
        id
    }

    pub fn list(&self) -> Vec<CronJob> {
        self.jobs
            .lock()
            .unwrap()
            .iter()
            .map(|j| CronJob {
                id: j.id.clone(),
                every: j.every,
                prompt: j.prompt.clone(),
                created_at: j.created_at,
                next: j.next,
            })
            .collect()
    }

    pub fn cancel(&self, id: &str) -> bool {
        let mut jobs = self.jobs.lock().unwrap();
        let before = jobs.len();
        jobs.retain(|j| j.id != id);
        jobs.len() != before
    }

    /// Jobs whose `next` has elapsed. Each is rescheduled by `every`.
    pub fn due(&self) -> Vec<CronJob> {
        let now = Instant::now();
        let mut jobs = self.jobs.lock().unwrap();
        let mut out = Vec::new();
        for job in jobs.iter_mut() {
            if job.next <= now {
                out.push(CronJob {
                    id: job.id.clone(),
                    every: job.every,
                    prompt: job.prompt.clone(),
                    created_at: job.created_at,
                    next: job.next,
                });
                job.next = now + job.every;
            }
        }
        out
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
        cron.add(Duration::from_millis(1), "tick");
        std::thread::sleep(Duration::from_millis(5));
        let first = cron.due();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].prompt, "tick");
        assert!(cron.due().is_empty());
    }
}
