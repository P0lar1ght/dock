//! TUI / jobs / resume cache. Spawn still goes through [`ChannelBackend`].

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cordis::Disposable;
use tokio::sync::Notify;

use crate::session::Sessions;
use crate::turn::TurnControl;
use cordis_base::types::LogEvent;

use super::types::SubagentOwner;
use super::{SubagentLife, SubagentSnap};

pub(super) struct ChildSlot {
    pub description: String,
    pub subagent_type: String,
    pub life: Mutex<SubagentLife>,
    pub cancelled: AtomicBool,
    pub send_now: AtomicBool,
    pub interrupt: AtomicBool,
    pub(crate) dispose: AtomicBool,
    pub output: Mutex<String>,
    pub events: Mutex<Vec<LogEvent>>,
    /// Live child log so the parent TUI can inspect a running turn.
    pub sessions: Mutex<Option<Sessions>>,
    pub turn: Mutex<Option<Arc<TurnControl>>>,
    pub hold: Mutex<Vec<Disposable>>,
    pub queued: Mutex<VecDeque<String>>,
    pub urgent: Mutex<Option<String>>,
    pub(crate) wake: Notify,
    pub started_at: Instant,
    /// When the child last parked idle; `None` while it is running. Drives the
    /// idle sweep.
    pub parked_at: Mutex<Option<Instant>>,
    pub owner: SubagentOwner,
    /// True if `report` ran during the current child turn.
    pub reported_this_turn: AtomicBool,
}

impl ChildSlot {
    pub(crate) fn life(&self) -> SubagentLife {
        *self.life.lock().unwrap()
    }

    pub(crate) fn disposed(&self) -> bool {
        self.dispose.load(Ordering::Relaxed)
    }

    pub(crate) fn set_life(&self, life: SubagentLife) {
        if life == SubagentLife::Running {
            *self.parked_at.lock().unwrap() = None;
        }
        *self.life.lock().unwrap() = life;
    }
}

#[derive(Clone)]
pub(super) struct ChildStore {
    inner: Arc<Mutex<HashMap<String, Arc<ChildSlot>>>>,
    inbox: Arc<Mutex<ParentInbox>>,
    parent_wake: Arc<Notify>,
}

struct ParentInbox {
    notices: VecDeque<ParentNotice>,
}

/// Parent-facing notice, drained by the session actor into a hidden mailbox
/// turn (`LogEvent::SystemReminder`).
#[derive(Clone, Debug)]
pub(super) enum ParentNotice {
    /// Child called `report` during its turn.
    Report { from: String, output: String },
    /// One per finished child turn: the child is idle and can be continued.
    TurnEnd {
        id: String,
        subagent_type: String,
        description: String,
        duration_ms: u64,
        cancelled: bool,
        /// The child's turn text, carried when it did not `report` this turn so
        /// the parent is not left waiting on an empty notice.
        output: Option<String>,
    },
}

impl ChildStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            inbox: Arc::new(Mutex::new(ParentInbox {
                notices: VecDeque::new(),
            })),
            parent_wake: Arc::new(Notify::new()),
        }
    }

    pub fn parent_wake(&self) -> Arc<Notify> {
        self.parent_wake.clone()
    }

    pub fn has_parent_notices(&self) -> bool {
        !self.inbox.lock().unwrap().notices.is_empty()
    }

    pub fn ensure(&self, id: &str, description: String, subagent_type: String) -> Arc<ChildSlot> {
        self.ensure_owned(id, description, subagent_type, SubagentOwner::Task)
    }

    pub fn ensure_owned(
        &self,
        id: &str,
        description: String,
        subagent_type: String,
        owner: SubagentOwner,
    ) -> Arc<ChildSlot> {
        let mut map = self.inner.lock().unwrap();
        if let Some(slot) = map.get(id) {
            return slot.clone();
        }
        let slot = Arc::new(ChildSlot {
            description,
            subagent_type,
            life: Mutex::new(SubagentLife::Running),
            cancelled: AtomicBool::new(false),
            send_now: AtomicBool::new(false),
            interrupt: AtomicBool::new(false),
            dispose: AtomicBool::new(false),
            output: Mutex::new(String::new()),
            events: Mutex::new(Vec::new()),
            sessions: Mutex::new(None),
            turn: Mutex::new(None),
            hold: Mutex::new(Vec::new()),
            queued: Mutex::new(VecDeque::new()),
            urgent: Mutex::new(None),
            wake: Notify::new(),
            started_at: Instant::now(),
            parked_at: Mutex::new(None),
            owner,
            reported_this_turn: AtomicBool::new(false),
        });
        map.insert(id.to_owned(), slot.clone());
        slot
    }

    pub fn get(&self, id: &str) -> Option<Arc<ChildSlot>> {
        self.inner.lock().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<SubagentSnap> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .map(|(id, s)| snap(id, s))
            .collect()
    }

    pub fn snapshot(&self, id: &str) -> Option<SubagentSnap> {
        self.inner.lock().unwrap().get(id).map(|s| snap(id, s))
    }

    pub fn events(&self, id: &str) -> Vec<LogEvent> {
        let Some(slot) = self.get(id) else {
            return Vec::new();
        };
        if let Some(sessions) = slot.sessions.lock().unwrap().as_ref() {
            return sessions.events();
        }
        let cloned = slot.events.lock().unwrap().clone();
        cloned
    }

    pub fn set_sessions(&self, id: &str, sessions: Sessions) {
        if let Some(slot) = self.get(id) {
            *slot.sessions.lock().unwrap() = Some(sessions);
        }
    }

    pub fn set_turn(&self, id: &str, turn: Arc<TurnControl>) {
        if let Some(slot) = self.get(id) {
            *slot.turn.lock().unwrap() = Some(turn);
        }
    }

    pub fn set_hold(&self, id: &str, hold: Vec<Disposable>) {
        if let Some(slot) = self.get(id) {
            *slot.hold.lock().unwrap() = hold;
        }
    }

    pub fn set_events(&self, id: &str, events: Vec<LogEvent>) {
        if let Some(slot) = self.get(id) {
            *slot.events.lock().unwrap() = events;
        }
    }

    pub fn set_output(&self, id: &str, output: String) {
        if let Some(slot) = self.get(id) {
            *slot.output.lock().unwrap() = output;
        }
    }

    pub fn park_idle(&self, id: &str, output: String, cancelled: bool) {
        let Some(slot) = self.get(id) else {
            return;
        };
        if slot.dispose.load(Ordering::Relaxed) {
            self.dispose(id, output, cancelled);
            return;
        }
        if cancelled {
            slot.cancelled.store(true, Ordering::Relaxed);
        }
        *slot.output.lock().unwrap() = output;
        *slot.parked_at.lock().unwrap() = Some(Instant::now());
        slot.set_life(SubagentLife::Idle);
        slot.wake.notify_waiters();
    }

    pub fn finish(&self, id: &str, output: String, cancelled: bool) {
        self.dispose(id, output, cancelled);
    }

    pub fn dispose(&self, id: &str, output: String, cancelled: bool) {
        let Some(slot) = self.get(id) else {
            return;
        };
        slot.dispose.store(true, Ordering::Relaxed);
        if cancelled {
            slot.cancelled.store(true, Ordering::Relaxed);
        }
        *slot.output.lock().unwrap() = output;
        slot.set_life(SubagentLife::Done);
        if let Some(turn) = slot.turn.lock().unwrap().as_ref() {
            turn.cancel();
        }
        slot.hold.lock().unwrap().clear();
        slot.wake.notify_waiters();
    }

    pub fn mark_cancelled(&self, id: &str) -> bool {
        self.request_dispose(id)
    }

    pub fn request_dispose(&self, id: &str) -> bool {
        let Some(slot) = self.get(id) else {
            return false;
        };
        if slot.life() == SubagentLife::Done {
            return false;
        }
        slot.dispose.store(true, Ordering::Relaxed);
        slot.cancelled.store(true, Ordering::Relaxed);
        if let Some(turn) = slot.turn.lock().unwrap().as_ref() {
            turn.cancel();
        }
        slot.wake.notify_waiters();
        true
    }

    pub fn request_interrupt(&self, id: &str) -> InterruptOutcome {
        let Some(slot) = self.get(id) else {
            return InterruptOutcome::Missing;
        };
        match slot.life() {
            SubagentLife::Done => InterruptOutcome::Settled,
            SubagentLife::Idle => InterruptOutcome::Settled,
            SubagentLife::Running => {
                slot.interrupt.store(true, Ordering::Relaxed);
                if let Some(turn) = slot.turn.lock().unwrap().as_ref() {
                    turn.cancel();
                }
                slot.wake.notify_waiters();
                InterruptOutcome::Requested
            }
        }
    }

    /// Returns `true` if the child was already running (message waits for
    /// this turn). `false` means it was idle: life flips to running now.
    pub fn enqueue_queued(&self, id: &str, message: String) -> Result<bool, String> {
        let slot = self
            .get(id)
            .ok_or_else(|| format!("unknown subagent {id}"))?;
        if slot.life() == SubagentLife::Done || slot.dispose.load(Ordering::Relaxed) {
            return Err(format!("subagent {id} is done; message was not delivered"));
        }
        let running = slot.life() == SubagentLife::Running;
        slot.queued.lock().unwrap().push_back(message);
        if !running {
            slot.set_life(SubagentLife::Running);
        }
        notify_inbox(&slot);
        Ok(running)
    }

    /// Returns `true` if the child was already running (send-now). `false`
    /// means it was idle: life flips to running and the next turn starts.
    pub fn push_urgent(&self, id: &str, message: String) -> Result<bool, String> {
        let slot = self
            .get(id)
            .ok_or_else(|| format!("unknown subagent {id}"))?;
        if slot.life() == SubagentLife::Done || slot.dispose.load(Ordering::Relaxed) {
            return Err(format!("subagent {id} is done; message was not delivered"));
        }
        let running = slot.life() == SubagentLife::Running;
        *slot.urgent.lock().unwrap() = Some(message);
        if running {
            slot.send_now.store(true, Ordering::Relaxed);
            if let Some(turn) = slot.turn.lock().unwrap().as_ref() {
                turn.cancel();
            }
        } else {
            slot.set_life(SubagentLife::Running);
        }
        notify_inbox(&slot);
        Ok(running)
    }

    /// Queued count and whether an urgent payload is waiting.
    pub fn inbox_pending(&self, id: &str) -> (usize, bool) {
        let Some(slot) = self.get(id) else {
            return (0, false);
        };
        let queued = slot.queued.lock().unwrap().len();
        let urgent = slot.urgent.lock().unwrap().is_some();
        (queued, urgent)
    }

    pub fn take_urgent(&self, id: &str) -> Option<String> {
        self.get(id).and_then(|s| s.urgent.lock().unwrap().take())
    }

    pub fn take_queued(&self, id: &str) -> Option<String> {
        self.get(id)
            .and_then(|s| s.queued.lock().unwrap().pop_front())
    }

    pub fn push_report(&self, from: &str, output: &str) {
        if let Some(slot) = self.get(from) {
            slot.reported_this_turn.store(true, Ordering::Relaxed);
        }
        self.inbox
            .lock()
            .unwrap()
            .notices
            .push_back(ParentNotice::Report {
                from: from.to_string(),
                output: output.to_string(),
            });
        self.notify_parent();
    }

    pub fn reset_reported(&self, id: &str) {
        if let Some(slot) = self.get(id) {
            slot.reported_this_turn.store(false, Ordering::Relaxed);
        }
    }

    /// Mailbox children that end a turn without `report` still reach the parent.
    ///
    /// One notice per finished turn: the child is idle and continuable, plus
    /// its turn text when it did not `report` (the parent cannot see assistant
    /// text otherwise). Callers that already delivered the result inline
    /// ([`Self::consume_completion`]) drop it again.
    pub fn push_turn_end(&self, id: &str, output: Option<String>, cancelled: bool) {
        let Some(slot) = self.get(id) else {
            return;
        };
        let notice = ParentNotice::TurnEnd {
            id: id.to_string(),
            subagent_type: slot.subagent_type.clone(),
            description: slot.description.clone(),
            duration_ms: slot.started_at.elapsed().as_millis() as u64,
            cancelled,
            output,
        };
        self.inbox.lock().unwrap().notices.push_back(notice);
        self.notify_parent();
    }

    /// Drop the queued turn-end notice for `id`: the caller already has the
    /// result (inline foreground spawn, or a `get_task_output` poll).
    pub fn consume_completion(&self, id: &str) {
        self.inbox
            .lock()
            .unwrap()
            .notices
            .retain(|n| !matches!(n, ParentNotice::TurnEnd { id: queued, .. } if queued == id));
    }

    pub fn drain_notices(&self) -> Vec<ParentNotice> {
        self.inbox.lock().unwrap().notices.drain(..).collect()
    }

    /// Dispose children that have been idle and unaddressed for longer than
    /// `ttl`. Their slot and transcript survive, so `get_task_output` and
    /// `resume_from` keep working.
    pub fn sweep_idle(&self, ttl: Duration) -> Vec<String> {
        let mut swept = Vec::new();
        for (id, slot) in self.inner.lock().unwrap().iter() {
            if slot.life() != SubagentLife::Idle || slot.disposed() {
                continue;
            }
            let parked = *slot.parked_at.lock().unwrap();
            if !parked.is_some_and(|at| at.elapsed() >= ttl) {
                continue;
            }
            if slot.queued.lock().unwrap().is_empty() && slot.urgent.lock().unwrap().is_none() {
                swept.push(id.clone());
            }
        }
        for id in &swept {
            let output = self
                .get(id)
                .map(|s| s.output.lock().unwrap().clone())
                .unwrap_or_default();
            self.dispose(id, output, false);
        }
        swept
    }

    /// Wake the parent actor. `notify_one` stores a permit so a notice pushed
    /// in the window between the actor's `has_parent_notices` check and its
    /// `notified()` registration is not lost.
    fn notify_parent(&self) {
        self.parent_wake.notify_waiters();
        self.parent_wake.notify_one();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InterruptOutcome {
    Requested,
    Settled,
    Missing,
}

fn notify_inbox(slot: &ChildSlot) {
    slot.wake.notify_waiters();
    // `notify_waiters` does not store a permit. `notify_one` covers the
    // window where `wait_next` has not registered yet.
    slot.wake.notify_one();
}

pub(super) fn snap(id: &str, s: &ChildSlot) -> SubagentSnap {
    let life = s.life();
    SubagentSnap {
        id: id.into(),
        description: s.description.clone(),
        subagent_type: s.subagent_type.clone(),
        done: life == SubagentLife::Done,
        idle: life == SubagentLife::Idle,
        cancelled: s.cancelled.load(Ordering::Relaxed),
        output: s.output.lock().unwrap().clone(),
        started_at: s.started_at,
    }
}
