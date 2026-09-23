//! TUI / mailbox / resume cache. Spawn still goes through [`ChannelBackend`].

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cordis::Disposable;
use tokio::sync::Notify;

use crate::agent::turn::TurnControl;
use crate::session::log::Sessions;
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
    /// 启动这个孩子的会话身份（`main` / `main#2`）。`send_message` 的相邻授权
    /// 看它：父只能发给自己的直接子，子只能发给它。
    pub parent: String,
    /// True if the child sent its parent a message during the current turn.
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
    /// 按 run id 攒的 workflow 子代理上报。与 `inbox` 分开就是为了**不**叫醒
    /// 主线程；由 workflow host 取走。
    workflow_inbox: Arc<Mutex<HashMap<String, Vec<WorkflowReport>>>>,
    parent_wake: Arc<Notify>,
}

struct ParentInbox {
    notices: VecDeque<ParentNotice>,
}

/// 一条 workflow 子代理的上报。不进父信箱：它是**过程**，run 还没结束就把它
/// 塞给主线程，等于让主线程在半份结果上开一轮。由 workflow host 取走，折进 run
/// 快照和滚动区 `Notice`；模型只在收尾拿到 `complete()` 的结果。
#[derive(Clone, Debug)]
pub struct WorkflowReport {
    pub agent_id: String,
    pub output: String,
}

/// Parent-facing notice, drained by the session actor into a hidden mailbox
/// turn (`LogEvent::SystemReminder`).
#[derive(Clone, Debug)]
pub(super) enum ParentNotice {
    /// Child sent its parent a message during its turn.
    Report { from: String, output: String },
    /// 一次 workflow run 收尾。整条 run 只有这一条通知——过程都在里面了。
    WorkflowDone {
        name: String,
        status: String,
        elapsed_ms: u64,
        /// `complete()` 的结果，或失败 / 暂停的原因。
        summary: String,
        /// 过程中各子代理的上报，按发生顺序。`agent_id` 已换成行上的 label。
        reports: Vec<WorkflowReport>,
        /// 攒不下而被丢掉的更早的上报条数。通知里如实说一句，别让主线程以为
        /// 这就是全部过程。
        dropped_reports: usize,
    },
    /// One per finished child turn: the child is idle and can be continued.
    TurnEnd {
        id: String,
        subagent_type: String,
        description: String,
        duration_ms: u64,
        cancelled: bool,
        /// The child's turn text, carried when it did not message the parent this turn so
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
            workflow_inbox: Arc::new(Mutex::new(HashMap::new())),
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
        self.ensure_owned(
            id,
            description,
            subagent_type,
            SubagentOwner::Task,
            crate::session::log::ROOT_IDENTITY,
        )
    }

    pub fn ensure_owned(
        &self,
        id: &str,
        description: String,
        subagent_type: String,
        owner: SubagentOwner,
        parent: &str,
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
            parent: parent.to_owned(),
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

    /// `parent` 启动的孩子（不论死活）。
    pub fn children_of(&self, parent: &str) -> Vec<SubagentSnap> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, s)| s.parent == parent)
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

    /// 在跑的孩子在下一步边界取走父级排队的消息（全部，按到达顺序）。
    ///
    /// 只在孩子 `Running` 时取：idle 的孩子由 `wait_next` 拿消息开下一轮，
    /// 这里抢走就会让那一轮起不来。
    pub fn take_queued_for_step(&self, id: &str) -> Vec<String> {
        let Some(slot) = self.get(id) else {
            return Vec::new();
        };
        if slot.life() != SubagentLife::Running || slot.disposed() {
            return Vec::new();
        }
        let drained = slot.queued.lock().unwrap().drain(..).collect();
        drained
    }

    /// 子代理发给父级的消息（`send_message` 子→父方向）的落点。
    ///
    /// workflow 的孩子**不进父信箱**：run 还在跑时把中间结论推给主线程，主线程
    /// 就会在半份结果上开一轮。它们攒进 run 自己的队列，由 workflow host 取走
    /// 折进快照和滚动区 `Notice`。
    pub fn push_report(&self, from: &str, output: &str) {
        let owner = self.get(from).map(|slot| {
            slot.reported_this_turn.store(true, Ordering::Relaxed);
            slot.owner.clone()
        });
        if let Some(run_id) = owner.as_ref().and_then(|o| o.workflow_run_id()) {
            self.workflow_inbox
                .lock()
                .unwrap()
                .entry(run_id.to_string())
                .or_default()
                .push(WorkflowReport {
                    agent_id: from.to_string(),
                    output: output.to_string(),
                });
            return;
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

    /// 取走某次 run 攒下的子代理上报。
    pub fn take_workflow_reports(&self, run_id: &str) -> Vec<WorkflowReport> {
        self.workflow_inbox
            .lock()
            .unwrap()
            .remove(run_id)
            .unwrap_or_default()
    }

    /// 一次 run 收尾：**整条 run 唯一**一条进父信箱的通知。
    pub fn push_workflow_done(
        &self,
        name: String,
        status: String,
        elapsed_ms: u64,
        summary: String,
        reports: Vec<WorkflowReport>,
        dropped_reports: usize,
    ) {
        self.inbox
            .lock()
            .unwrap()
            .notices
            .push_back(ParentNotice::WorkflowDone {
                name,
                status,
                elapsed_ms,
                summary,
                reports,
                dropped_reports,
            });
        self.notify_parent();
    }

    pub fn reset_reported(&self, id: &str) {
        if let Some(slot) = self.get(id) {
            slot.reported_this_turn.store(false, Ordering::Relaxed);
        }
    }

    /// Mailbox children that end a turn without messaging the parent still reach it.
    ///
    /// One notice per finished turn: the child is idle and continuable, plus
    /// its turn text when it did not message the parent (the parent cannot see assistant
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
    /// result (inline foreground spawn).
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
    /// `ttl`. Their slot and transcript survive, so `resume_from` keeps
    /// working.
    pub fn sweep_idle(&self, ttl: Duration) -> Vec<String> {
        let mut swept = Vec::new();
        for (id, slot) in self.inner.lock().unwrap().iter() {
            if slot.life() != SubagentLife::Idle || slot.disposed() {
                continue;
            }
            let parked = *slot.parked_at.lock().unwrap();
            if parked.is_none_or(|at| at.elapsed() < ttl) {
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
