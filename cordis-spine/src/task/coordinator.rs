//! Single-writer subagent coordinator actor.
//!
//! The actor owns the command receiver, the admission queue, pending/active/
//! completed state, concrete blocking waiters, foreground deadlines,
//! cancellation, and the terminal delivery disposition. All hosts drive it
//! through `ChannelBackend`; only their `ChildRunner` implementations differ.
//!
//! There is intentionally no shared mutable state in this module. A runner's
//! associated futures may be `Send` or non-`Send`; the resulting actor future
//! inherits that property naturally on stable Rust.

mod queue;
mod spawn;

use std::collections::{HashMap, HashSet, VecDeque};

use futures_util::stream::{FuturesUnordered, StreamExt};
use futures_util::FutureExt;
use tokio::sync::{mpsc, oneshot};

use super::admission::Admission;
use super::coordinator_state::{
    background_at_deadline, background_if_caller_gone, sleep_until, workflow_outstanding,
    ActiveChild, ChildRecord, CompletedChild, InternalEvent, PendingChild, ReplyFuture,
    TaggedFuture,
};
use super::types::{
    SubagentCancelOutcome, SubagentCancelTarget, SubagentEvent, SubagentRequest, SubagentResult,
    SubagentValidateTypeOutcome,
};

pub use super::coordinator_state::{
    ChildCompletion, ChildControl, ChildReporter, ChildRunOutput, ChildRunRequest, ChildRunner,
    CoordinatorConfig, SendBoxFuture, StartedChild, MAX_COMPLETED_ENTRIES,
};
use queue::{QueuedCaller, SpawnQueue, StartOrigin, QUEUED_REAP_INTERVAL};

/// Channel-owned subagent lifecycle actor.
pub struct SubagentCoordinator<R: ChildRunner> {
    commands: mpsc::UnboundedReceiver<SubagentEvent>,
    internal_tx: mpsc::UnboundedSender<InternalEvent<R::Control>>,
    internal_rx: mpsc::UnboundedReceiver<InternalEvent<R::Control>>,
    runner: R,
    config: CoordinatorConfig,
    admission: Admission,
    queued: SpawnQueue,
    /// When the queued sweep last ran; bounds how stale an out-of-band
    /// token cancel of a queued spawn can get (see [`Self::next_deadline`]).
    last_queued_reap: tokio::time::Instant,
    /// Re-entry latch: `finish_child` runs the queued sweep, and finishing a
    /// cancelled queued entry routes back through `finish_child`. The inner
    /// sweep is a no-op (a cancelled entry frees no running slot).
    draining_queued: bool,
    pending: HashMap<String, PendingChild>,
    active: HashMap<String, ActiveChild<R::Control>>,
    completed: HashMap<String, CompletedChild>,
    completed_order: VecDeque<String>,
    workflow_cancel_waiters: HashMap<String, Vec<oneshot::Sender<SubagentCancelOutcome>>>,
    /// Per-parent delete-path teardown drain, present only while a
    /// responder-bearing `TeardownSession` (`/delete`) waits for the session's
    /// children to finish. While an entry exists, spawn admission stays closed
    /// and [`SubagentEvent::OpenSpawnAdmission`] cannot reopen it (a racing
    /// next-turn open would reopen Task spawns mid-delete).
    teardown_drains: HashMap<String, TeardownDrain>,
    /// Parent sessions that received `ParentSession` cancel. Non-workflow spawns
    /// are rejected until [`SubagentEvent::OpenSpawnAdmission`] (next turn) or
    /// teardown drain completes, so a detached late `TaskTool` spawn cannot
    /// outrun Stop / delete.
    spawn_blocked_sessions: HashSet<String>,
    runs: FuturesUnordered<
        TaggedFuture<futures_util::future::CatchUnwind<std::panic::AssertUnwindSafe<R::RunFuture>>>,
    >,
    validations: FuturesUnordered<ReplyFuture<R::ValidateFuture, SubagentValidateTypeOutcome>>,
}

/// Backstop for a delete-path teardown hold: if a cancelled child never
/// finishes, force-reopen the session's spawn admission after this long (with a
/// warning) rather than blocking spawns for the process lifetime.
const TEARDOWN_DRAIN_MAX: std::time::Duration = std::time::Duration::from_secs(30);

/// In-flight delete-path teardown drain: responders to resolve once the last
/// child drains, and the backstop deadline that force-reopens admission.
struct TeardownDrain {
    waiters: Vec<oneshot::Sender<()>>,
    deadline: tokio::time::Instant,
}

impl<R: ChildRunner> SubagentCoordinator<R> {
    pub fn new(
        commands: mpsc::UnboundedReceiver<SubagentEvent>,
        runner: R,
        config: CoordinatorConfig,
    ) -> Self {
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        Self {
            commands,
            internal_tx,
            internal_rx,
            runner,
            admission: Admission::new(config.limits),
            queued: SpawnQueue::default(),
            last_queued_reap: tokio::time::Instant::now(),
            draining_queued: false,
            config,
            pending: HashMap::new(),
            active: HashMap::new(),
            completed: HashMap::new(),
            completed_order: VecDeque::new(),
            workflow_cancel_waiters: HashMap::new(),
            teardown_drains: HashMap::new(),
            spawn_blocked_sessions: HashSet::new(),
            runs: FuturesUnordered::new(),
            validations: FuturesUnordered::new(),
        }
    }

    pub async fn run(mut self) {
        let mut commands_open = true;
        loop {
            // Queued spawns need no exit check: queued non-empty means some
            // session is at capacity, so `runs` is non-empty, and the last
            // `finish_child` drains the queue before `runs` empties.
            if !commands_open && self.runs.is_empty() && self.validations.is_empty() {
                debug_assert!(
                    self.queued.is_empty(),
                    "actor exiting with spawns still queued"
                );
                break;
            }

            let deadline = self.next_deadline();
            tokio::select! {
                biased;
                Some(event) = self.internal_rx.recv() => self.handle_internal(event),
                Some((id, output)) = self.runs.next(), if !self.runs.is_empty() => {
                    match output {
                        Ok(output) => self.finish_child(&id, output),
                        Err(_) => self.finish_panicked_child(&id),
                    }
                }
                Some((respond_to, outcome)) = self.validations.next(), if !self.validations.is_empty() => {
                    let _ = respond_to.send(outcome);
                }
                command = self.commands.recv(), if commands_open => {
                    match command {
                        Some(command) => {
                            self.reap_abandoned_callers();
                            self.handle_command(command);
                        }
                        None => commands_open = false,
                    }
                }
                _ = sleep_until(deadline), if deadline.is_some() => self.process_deadlines(),
            }
            while self.completed.len() > MAX_COMPLETED_ENTRIES {
                let Some(id) = self.completed_order.pop_front() else {
                    break;
                };
                self.completed.remove(&id);
            }
        }

        self.cancel_all_children();
    }

    fn handle_command(&mut self, command: SubagentEvent) {
        match command {
            SubagentEvent::Spawn(command) => self.handle_spawn(command),
            SubagentEvent::Cancel(request) => match request.target {
                SubagentCancelTarget::SubagentId(id) => {
                    let outcome = self.cancel_one(&id, request.parent_session_id.as_deref(), true);
                    let _ = request.respond_to.send(outcome);
                }
                SubagentCancelTarget::ParentSession => {
                    let outcome = self.cancel_parent_session(request.parent_session_id.as_deref());
                    let _ = request.respond_to.send(outcome);
                }
                SubagentCancelTarget::WorkflowRunId(run_id) => {
                    self.cancel_workflow_children(&run_id, request.parent_session_id.as_deref());
                    let _ = request.respond_to.send(SubagentCancelOutcome::Cancelled);
                }
            },
            SubagentEvent::TeardownSession {
                parent_session_id,
                respond_to,
            } => {
                self.teardown_session_children(&parent_session_id);
                // Only the delete path (responder present) holds spawn
                // admission closed until children drain, so a next-turn
                // OpenSpawnAdmission cannot reopen spawns mid-delete.
                if let Some(respond_to) = respond_to {
                    if self.session_has_children(&parent_session_id) {
                        self.begin_teardown_drain(parent_session_id, respond_to);
                    } else {
                        let _ = respond_to.send(());
                    }
                }
            }
            SubagentEvent::OpenSpawnAdmission { parent_session_id } => {
                // Next-turn reopen after Stop is intentional even while cancelled
                // children finish — but not while a delete-path TeardownSession
                // is draining.
                if !self.teardown_drains.contains_key(&parent_session_id) {
                    self.spawn_blocked_sessions.remove(&parent_session_id);
                }
            }
            SubagentEvent::ValidateType(request) => {
                self.validations.push(ReplyFuture {
                    future: Box::pin(
                        self.runner
                            .validate_type(request.subagent_type, request.parent_session_id),
                    ),
                    respond_to: Some(request.respond_to),
                });
            }
        }
    }

    fn handle_internal(&mut self, event: InternalEvent<R::Control>) {
        match event {
            InternalEvent::Started {
                subagent_id,
                child,
                respond_to,
            } => {
                let Some(pending) = self.pending.remove(&subagent_id) else {
                    let _ = respond_to.send(false);
                    return;
                };
                if pending.cancellation.is_cancelled() {
                    self.pending.insert(subagent_id, pending);
                    let _ = respond_to.send(false);
                    return;
                }
                self.active.insert(
                    subagent_id,
                    ActiveChild {
                        request: pending.request,
                        cancellation: pending.cancellation,
                        spawn_reply: pending.spawn_reply,
                        foreground_deadline: pending.foreground_deadline,
                        handle_only: pending.handle_only,
                        explicitly_killed: pending.explicitly_killed,
                        child_session_id: child.child_session_id,
                        control: child.control,
                    },
                );
                let _ = respond_to.send(true);
            }
        }
    }

    fn start_child(
        &mut self,
        request: SubagentRequest,
        spawn_reply: Option<oneshot::Sender<SubagentResult>>,
        origin: StartOrigin,
    ) {
        let id = request.id.clone();
        let cancellation = request.cancel_token.clone();
        // `spawn_reply: None`: the caller was auto-backgrounded while queued.
        let handle_only = request.run_in_background || spawn_reply.is_none();
        let foreground_deadline = match origin {
            StartOrigin::Direct => (spawn_reply.is_some() && request.awaits_in_foreground())
                .then(|| tokio::time::Instant::now() + self.config.foreground_budget),
            StartOrigin::Dequeued { deadline, .. } => deadline,
        };
        self.pending.insert(
            id.clone(),
            PendingChild {
                request: request.clone(),
                cancellation: cancellation.clone(),
                spawn_reply,
                foreground_deadline,
                handle_only,
                explicitly_killed: false,
            },
        );
        self.running_count_changed();
        let reporter = ChildReporter {
            subagent_id: id.clone(),
            tx: self.internal_tx.clone(),
        };
        self.runs.push(TaggedFuture {
            subagent_id: id,
            future: Box::pin(
                std::panic::AssertUnwindSafe(self.runner.run(ChildRunRequest {
                    request,
                    cancellation,
                    reporter,
                }))
                .catch_unwind(),
            ),
        });
    }

    /// Workflow agents draw from their run's own pool, not session slots.
    fn session_running_count(&self, parent_session_id: &str) -> usize {
        self.pending
            .values()
            .map(|child| &child.request)
            .chain(self.active.values().map(|child| &child.request))
            .filter(|request| {
                request.parent_session_id == parent_session_id && !request.owner.is_workflow()
            })
            .count()
    }

    fn finish_child(&mut self, id: &str, output: ChildRunOutput) {
        let record = if let Some(child) = self.active.remove(id) {
            ChildRecord::Active(child)
        } else if let Some(child) = self.pending.remove(id) {
            ChildRecord::Pending(child)
        } else {
            return;
        };

        let request = record.request().clone();
        let mut spawn_reply = match record {
            ChildRecord::Pending(child) => child.spawn_reply,
            ChildRecord::Active(child) => child.spawn_reply,
        };

        // Hand the result to a caller that is still waiting. A handle-only
        // spawn (background, or already auto-backgrounded) has none.
        if let Some(respond_to) = spawn_reply.take() {
            let _ = respond_to.send(output.result.clone());
        }

        let completed = CompletedChild {
            request: request.clone(),
            result: output.result.clone(),
        };
        self.completed.insert(id.to_owned(), completed);
        self.completed_order.push_back(id.to_owned());
        self.running_count_changed();
        let workflow_run_id = request.owner.workflow_run_id().map(str::to_owned);
        let parent_session_id = request.parent_session_id.clone();
        self.runner.on_completed(ChildCompletion {
            request,
            result: output.result,
        });
        if let Some(run_id) = workflow_run_id {
            self.resolve_workflow_cancel_waiters(&run_id);
        }
        self.resolve_teardown_drain_waiters(&parent_session_id);
        self.start_queued_within_capacity();
    }

    fn finish_panicked_child(&mut self, id: &str) {
        let request = self
            .active
            .get(id)
            .map(|child| child.request.clone())
            .or_else(|| self.pending.get(id).map(|child| child.request.clone()));
        let Some(request) = request else {
            return;
        };
        tracing::error!(subagent_id = id, "subagent child runner panicked");
        self.finish_child(
            id,
            ChildRunOutput {
                result: SubagentResult {
                    success: false,
                    error: Some("Subagent runtime panicked".to_owned()),
                    subagent_id: request.id.clone(),
                    child_session_id: request.id,
                    ..Default::default()
                },
            },
        );
    }

    fn cancel_one(
        &mut self,
        id: &str,
        parent_session_id: Option<&str>,
        explicit: bool,
    ) -> SubagentCancelOutcome {
        if let Some(child) = self.active.get_mut(id) {
            if belongs_to_session(&child.request, parent_session_id) {
                child.explicitly_killed |= explicit;
                child.cancellation.cancel();
                child.control.cancel();
                return SubagentCancelOutcome::Cancelled;
            }
        }
        if let Some(child) = self.pending.get_mut(id) {
            if belongs_to_session(&child.request, parent_session_id) {
                child.explicitly_killed |= explicit;
                child.cancellation.cancel();
                return SubagentCancelOutcome::Cancelled;
            }
        }
        if self.remove_queued(|request| {
            request.id == id && belongs_to_session(request, parent_session_id)
        }) > 0
        {
            return SubagentCancelOutcome::Cancelled;
        }
        if let Some(child) = self.completed.get(id) {
            if belongs_to_session(&child.request, parent_session_id) {
                return SubagentCancelOutcome::AlreadyFinished {
                    status: child.result.status().to_owned(),
                };
            }
        }
        SubagentCancelOutcome::NotFound
    }

    fn teardown_session_children(&mut self, parent_session_id: &str) {
        let mut cancelled = 0;
        for child in self.active.values_mut() {
            if child.request.parent_session_id == parent_session_id {
                // Parent is gone: do not rebuffer this completion for a later
                // resume of the same session id.
                child.request.surface_completion = false;
                child.cancellation.cancel();
                child.control.cancel();
                cancelled += 1;
            }
        }
        for child in self.pending.values_mut() {
            if child.request.parent_session_id == parent_session_id {
                child.request.surface_completion = false;
                child.cancellation.cancel();
                cancelled += 1;
            }
        }
        // Parent is gone here too: a queued spawn's cancelled completion must
        // not be rebuffered for a later resume of the same session id.
        for queued in self.queued.iter_mut() {
            if queued.request.parent_session_id == parent_session_id {
                queued.request.surface_completion = false;
            }
        }
        cancelled += self.remove_queued(|request| request.parent_session_id == parent_session_id);
        if cancelled > 0 {
            tracing::info!(
                parent_session_id,
                cancelled,
                "cancelled subagents on session teardown"
            );
        }
    }

    /// Whether any child (active, pending, or queued) still belongs to the
    /// session. Unlike [`Self::session_running_count`] it counts every owner,
    /// including workflow, since teardown drains all children.
    fn session_has_children(&self, parent_session_id: &str) -> bool {
        self.active
            .values()
            .map(|child| &child.request)
            .chain(self.pending.values().map(|child| &child.request))
            .chain(self.queued.iter().map(|queued| queued.request.as_ref()))
            .any(|request| request.parent_session_id == parent_session_id)
    }

    /// Latch spawn admission closed for a delete-path teardown and park the
    /// responder until the last child drains (or the backstop deadline fires).
    fn begin_teardown_drain(&mut self, parent_session_id: String, respond_to: oneshot::Sender<()>) {
        let deadline = tokio::time::Instant::now() + TEARDOWN_DRAIN_MAX;
        self.spawn_blocked_sessions
            .insert(parent_session_id.clone());
        self.teardown_drains
            .entry(parent_session_id)
            .or_insert_with(|| TeardownDrain {
                waiters: Vec::new(),
                deadline,
            })
            .waiters
            .push(respond_to);
    }

    /// Clear a delete-path hold: reopen the session's spawn admission and
    /// resolve every parked drain responder.
    fn clear_teardown_drain(&mut self, parent_session_id: &str) {
        self.spawn_blocked_sessions.remove(parent_session_id);
        if let Some(drain) = self.teardown_drains.remove(parent_session_id) {
            for respond_to in drain.waiters {
                let _ = respond_to.send(());
            }
        }
    }

    fn resolve_teardown_drain_waiters(&mut self, parent_session_id: &str) {
        // Cheap precondition (one lookup) before the three-collection scan on
        // every child completion: only a delete-path teardown holds a drain.
        if !self.teardown_drains.contains_key(parent_session_id) {
            return;
        }
        if self.session_has_children(parent_session_id) {
            return;
        }
        self.clear_teardown_drain(parent_session_id);
    }

    /// All non-workflow children for the parent session (user Stop / Esc).
    ///
    /// Requires a concrete session id — unbound (`None`) is rejected so a
    /// wildcard cannot cancel every session on a shared coordinator.
    fn cancel_parent_session(&mut self, parent_session_id: Option<&str>) -> SubagentCancelOutcome {
        let Some(parent_session_id) = parent_session_id else {
            return SubagentCancelOutcome::NotFound;
        };
        self.spawn_blocked_sessions
            .insert(parent_session_id.to_owned());
        for child in self.active.values() {
            if child.request.parent_session_id == parent_session_id
                && !child.request.owner.is_workflow()
            {
                child.cancellation.cancel();
                child.control.cancel();
            }
        }
        for child in self.pending.values() {
            if child.request.parent_session_id == parent_session_id
                && !child.request.owner.is_workflow()
            {
                child.cancellation.cancel();
            }
        }
        self.remove_queued(|request| {
            request.parent_session_id == parent_session_id && !request.owner.is_workflow()
        });
        SubagentCancelOutcome::Cancelled
    }

    fn cancel_workflow_children(&mut self, run_id: &str, parent_session_id: Option<&str>) {
        for child in self.active.values() {
            if child.request.owner.workflow_run_id() == Some(run_id)
                && belongs_to_session(&child.request, parent_session_id)
            {
                child.cancellation.cancel();
                child.control.cancel();
            }
        }
        for child in self.pending.values() {
            if child.request.owner.workflow_run_id() == Some(run_id)
                && belongs_to_session(&child.request, parent_session_id)
            {
                child.cancellation.cancel();
            }
        }
    }

    fn resolve_workflow_cancel_waiters(&mut self, run_id: &str) {
        if workflow_outstanding(&self.pending, &self.active, run_id) != 0 {
            return;
        }
        for respond_to in self
            .workflow_cancel_waiters
            .remove(run_id)
            .unwrap_or_default()
        {
            let _ = respond_to.send(SubagentCancelOutcome::Cancelled);
        }
    }

    fn next_deadline(&self) -> Option<tokio::time::Instant> {
        self.pending
            .values()
            .filter_map(|child| child.foreground_deadline)
            .chain(
                self.active
                    .values()
                    .filter_map(|child| child.foreground_deadline),
            )
            .chain(
                self.queued
                    .iter()
                    .filter_map(|queued| queued.caller.deadline()),
            )
            .chain(
                // Anchored to the last sweep, not `now`: a stream of other
                // wakes must not keep pushing the next sweep further out.
                (!self.queued.is_empty()).then(|| self.last_queued_reap + QUEUED_REAP_INTERVAL),
            )
            .chain(self.teardown_drains.values().map(|drain| drain.deadline))
            .min()
    }

    fn reap_abandoned_callers(&mut self) {
        self.last_queued_reap = tokio::time::Instant::now();
        for child in self.pending.values_mut() {
            background_if_caller_gone(child);
        }
        for child in self.active.values_mut() {
            background_if_caller_gone(child);
        }
        // The queued leg of the same sweep.
        for queued in self.queued.iter_mut() {
            if let QueuedCaller::Awaiting { result_tx, .. } = &queued.caller {
                if result_tx.is_closed() {
                    queued.caller = QueuedCaller::Backgrounded;
                }
            }
        }
        self.remove_queued(|request| request.cancel_token.is_cancelled());
    }

    fn process_deadlines(&mut self) {
        self.reap_abandoned_callers();
        let now = tokio::time::Instant::now();
        // Backstop: a delete-path hold whose drain deadline elapsed force-clears
        // so a child that never finishes cannot block spawns forever.
        let stale: Vec<String> = self
            .teardown_drains
            .iter()
            .filter(|(_, drain)| drain.deadline <= now)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            tracing::warn!(
                parent_session_id = %id,
                still_draining = self.session_has_children(&id),
                "teardown drain deadline elapsed; force-reopening spawn admission",
            );
            self.clear_teardown_drain(&id);
        }
        for child in self.pending.values_mut() {
            background_at_deadline(child, now, self.config.foreground_budget);
        }
        for child in self.active.values_mut() {
            background_at_deadline(child, now, self.config.foreground_budget);
        }
        // The spawn stays queued; only its caller is handed off.
        for queued in self.queued.iter_mut() {
            if queued
                .caller
                .deadline()
                .is_none_or(|deadline| deadline > now)
            {
                continue;
            }
            let caller = std::mem::replace(&mut queued.caller, QueuedCaller::Backgrounded);
            let Some(result_tx) = caller.into_spawn_reply() else {
                continue;
            };
            tracing::warn!(
                subagent_id = %queued.request.id,
                budget_ms = self.config.foreground_budget.as_millis() as u64,
                "queued subagent exceeded await budget; auto-backgrounding (spawn stays queued)",
            );
            let _ = result_tx.send(SubagentResult {
                backgrounded: true,
                subagent_id: queued.request.id.clone(),
                child_session_id: queued.request.id.clone(),
                ..Default::default()
            });
        }
    }

    fn running_count_changed(&self) {
        self.runner
            .running_count_changed(self.pending.len() + self.active.len());
    }

    fn cancel_all_children(&self) {
        for child in self.active.values() {
            child.cancellation.cancel();
            child.control.cancel();
        }
        for child in self.pending.values() {
            child.cancellation.cancel();
        }
    }
}

fn belongs_to_session(request: &SubagentRequest, parent_session_id: Option<&str>) -> bool {
    parent_session_id.is_none_or(|id| request.parent_session_id == id)
}

impl<R: ChildRunner> Drop for SubagentCoordinator<R> {
    fn drop(&mut self) {
        // Not `remove_queued`: that routes through `finish_child`, which runs
        // host completion callbacks — off-limits from a destructor (the
        // host's storage may already be tearing down).
        self.resolve_queued_at_drop();
        self.cancel_all_children();
    }
}
