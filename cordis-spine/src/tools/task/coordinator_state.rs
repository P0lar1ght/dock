use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::types::{SubagentRequest, SubagentResult, SubagentValidateTypeOutcome};

/// Cap on retained completed-subagent entries before the oldest are evicted.
pub const MAX_COMPLETED_ENTRIES: usize = 1024;

pub type SendBoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Runtime handle retained while a child is active.
pub trait ChildControl: 'static {
    fn cancel(&self);
}

/// Data reported when runtime initialization has produced a live child.
pub struct StartedChild<C> {
    pub child_session_id: String,
    pub control: C,
}

/// Input to one runtime-specific child run.
pub struct ChildRunRequest<C> {
    pub request: SubagentRequest,
    pub cancellation: CancellationToken,
    pub reporter: ChildReporter<C>,
}

/// Terminal output from one runtime-specific child run.
pub struct ChildRunOutput {
    pub result: SubagentResult,
}

/// Terminal event delivered to the runtime adapter after state is committed.
pub struct ChildCompletion {
    pub request: SubagentRequest,
    pub result: SubagentResult,
}

/// The only host-specific seam.
///
/// Associated future types intentionally carry no unconditional `Send` bound.
/// A local runner may return non-`Send` futures, while a multithreaded runner
/// may return `Send` futures.
pub trait ChildRunner: 'static {
    type Control: ChildControl;
    type RunFuture: Future<Output = ChildRunOutput> + 'static;
    type ValidateFuture: Future<Output = SubagentValidateTypeOutcome> + 'static;

    fn run(&self, request: ChildRunRequest<Self::Control>) -> Self::RunFuture;

    fn validate_type(
        &self,
        subagent_type: String,
        parent_session_id: String,
    ) -> Self::ValidateFuture;

    fn on_completed(&self, completion: ChildCompletion);

    fn running_count_changed(&self, _running: usize) {}
}

/// Host-configurable lifecycle policy. The transition logic remains shared.
#[derive(Clone)]
pub struct CoordinatorConfig {
    pub foreground_budget: std::time::Duration,
    /// Session-scoped spawn limits enforced by the coordinator.
    pub limits: super::admission::SubagentLimits,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            foreground_budget: std::time::Duration::from_secs(45),
            limits: super::admission::SubagentLimits::default(),
        }
    }
}

impl std::fmt::Debug for CoordinatorConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoordinatorConfig")
            .field("foreground_budget", &self.foreground_budget)
            .field("limits", &self.limits)
            .finish()
    }
}

/// Runner-side channel back into the actor.
pub struct ChildReporter<C> {
    pub(super) subagent_id: String,
    pub(super) tx: mpsc::UnboundedSender<InternalEvent<C>>,
}

impl<C> Clone for ChildReporter<C> {
    fn clone(&self) -> Self {
        Self {
            subagent_id: self.subagent_id.clone(),
            tx: self.tx.clone(),
        }
    }
}

impl<C: 'static> ChildReporter<C> {
    /// Promote the pending child to active. The acknowledgement closes the
    /// cancel-at-promote race: `false` means cancellation won and the adapter
    /// must tear down the half-initialized runtime.
    pub async fn started(&self, child: StartedChild<C>) -> bool {
        let (respond_to, response_rx) = oneshot::channel();
        if self
            .tx
            .send(InternalEvent::Started {
                subagent_id: self.subagent_id.clone(),
                child,
                respond_to,
            })
            .is_err()
        {
            return false;
        }
        response_rx.await.unwrap_or(false)
    }
}

pub(super) enum InternalEvent<C> {
    Started {
        subagent_id: String,
        child: StartedChild<C>,
        respond_to: oneshot::Sender<bool>,
    },
}

pub(super) struct PendingChild {
    pub(super) request: SubagentRequest,
    pub(super) cancellation: CancellationToken,
    pub(super) spawn_reply: Option<oneshot::Sender<SubagentResult>>,
    pub(super) foreground_deadline: Option<tokio::time::Instant>,
    pub(super) handle_only: bool,
    pub(super) explicitly_killed: bool,
}

pub(super) struct ActiveChild<C> {
    pub(super) request: SubagentRequest,
    pub(super) cancellation: CancellationToken,
    pub(super) spawn_reply: Option<oneshot::Sender<SubagentResult>>,
    pub(super) foreground_deadline: Option<tokio::time::Instant>,
    pub(super) handle_only: bool,
    pub(super) explicitly_killed: bool,
    pub(super) child_session_id: String,
    pub(super) control: C,
}

pub(super) struct CompletedChild {
    pub(super) request: SubagentRequest,
    pub(super) result: SubagentResult,
}

pub(super) struct TaggedFuture<F> {
    pub(super) subagent_id: String,
    pub(super) future: Pin<Box<F>>,
}

impl<F: Future> Future for TaggedFuture<F> {
    type Output = (String, F::Output);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.future
            .as_mut()
            .poll(cx)
            .map(|output| (this.subagent_id.clone(), output))
    }
}

pub(super) struct ReplyFuture<F, T> {
    pub(super) future: Pin<Box<F>>,
    pub(super) respond_to: Option<oneshot::Sender<T>>,
}

impl<F, T> Future for ReplyFuture<F, T>
where
    F: Future<Output = T>,
{
    type Output = (oneshot::Sender<T>, T);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.future.as_mut().poll(cx).map(|output| {
            let respond_to = match this.respond_to.take() {
                Some(respond_to) => respond_to,
                None => unreachable!("reply future polled after completion"),
            };
            (respond_to, output)
        })
    }
}

pub(super) enum ChildRecord<C> {
    Pending(PendingChild),
    Active(ActiveChild<C>),
}

impl<C> ChildRecord<C> {
    pub(super) fn request(&self) -> &SubagentRequest {
        match self {
            Self::Pending(child) => &child.request,
            Self::Active(child) => &child.request,
        }
    }
}

pub(super) trait ForegroundChild {
    fn id(&self) -> &str;
    fn child_session_id(&self) -> &str;
    fn deadline(&self) -> Option<tokio::time::Instant>;
    /// True when the spawn caller dropped its result receiver while this
    /// child was still treated as turn-blocking (old shell `ParentGone`).
    fn caller_gone(&self) -> bool;
    fn is_workflow(&self) -> bool;
    fn take_reply(&mut self) -> Option<oneshot::Sender<SubagentResult>>;
    fn mark_backgrounded(&mut self);
    /// Cancel the child's execution (token + active control where present).
    fn cancel(&mut self);
}

impl ForegroundChild for PendingChild {
    fn id(&self) -> &str {
        &self.request.id
    }

    fn child_session_id(&self) -> &str {
        &self.request.id
    }

    fn deadline(&self) -> Option<tokio::time::Instant> {
        self.foreground_deadline
    }

    fn caller_gone(&self) -> bool {
        !self.handle_only && self.spawn_reply.as_ref().is_some_and(|tx| tx.is_closed())
    }

    fn is_workflow(&self) -> bool {
        self.request.owner.is_workflow()
    }

    fn take_reply(&mut self) -> Option<oneshot::Sender<SubagentResult>> {
        self.spawn_reply.take()
    }

    fn mark_backgrounded(&mut self) {
        self.handle_only = true;
        self.foreground_deadline = None;
    }

    fn cancel(&mut self) {
        self.cancellation.cancel();
    }
}

impl<C: ChildControl> ForegroundChild for ActiveChild<C> {
    fn id(&self) -> &str {
        &self.request.id
    }

    fn child_session_id(&self) -> &str {
        &self.child_session_id
    }

    fn deadline(&self) -> Option<tokio::time::Instant> {
        self.foreground_deadline
    }

    fn caller_gone(&self) -> bool {
        !self.handle_only && self.spawn_reply.as_ref().is_some_and(|tx| tx.is_closed())
    }

    fn is_workflow(&self) -> bool {
        self.request.owner.is_workflow()
    }

    fn take_reply(&mut self) -> Option<oneshot::Sender<SubagentResult>> {
        self.spawn_reply.take()
    }

    fn mark_backgrounded(&mut self) {
        self.handle_only = true;
        self.foreground_deadline = None;
    }

    fn cancel(&mut self) {
        self.cancellation.cancel();
        self.control.cancel();
    }
}

pub(super) fn background_at_deadline(
    child: &mut impl ForegroundChild,
    now: tokio::time::Instant,
    budget: std::time::Duration,
) {
    if child.deadline().is_none_or(|deadline| deadline > now) {
        return;
    }
    tracing::warn!(
        subagent_id = child.id(),
        budget_ms = budget.as_millis() as u64,
        "foreground subagent exceeded await budget; auto-backgrounding (child keeps running)",
    );
    if let Some(respond_to) = child.take_reply() {
        // Interim handoff, not a completion: keep `success: false` (default)
        // so `SubagentResult::status()` consumers cannot record a completed
        // status for a still-running child. Callers branch on `backgrounded`.
        let _ = respond_to.send(SubagentResult {
            backgrounded: true,
            subagent_id: child.id().to_owned(),
            child_session_id: child.child_session_id().to_owned(),
            ..Default::default()
        });
    }
    child.mark_backgrounded();
}

/// Handle a foreground child whose spawn caller dropped the result channel
/// (parent turn stop / cancelled await). Task-owned children keep running and
/// just leave the turn-blocking `Outstanding` set — shell `ParentGone` parity.
/// Workflow-owned children are CANCELLED instead (old shell `ParentGone`
/// cancelled workflow children); `ChannelBackend`'s drop-cancel arming remains
/// defense in depth for hosts that go through it.
pub(super) fn background_if_caller_gone(child: &mut impl ForegroundChild) {
    if !child.caller_gone() {
        return;
    }
    let _ = child.take_reply();
    if child.is_workflow() {
        tracing::debug!(
            subagent_id = child.id(),
            "workflow subagent caller gone; cancelling child",
        );
        child.cancel();
        return;
    }
    tracing::debug!(
        subagent_id = child.id(),
        "foreground subagent caller gone; auto-backgrounding (child keeps running)",
    );
    child.mark_backgrounded();
}

pub(super) async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

pub(super) fn workflow_outstanding<C>(
    pending: &HashMap<String, PendingChild>,
    active: &HashMap<String, ActiveChild<C>>,
    run_id: &str,
) -> usize {
    pending
        .values()
        .filter(|child| child.request.owner.workflow_run_id() == Some(run_id))
        .count()
        + active
            .values()
            .filter(|child| child.request.owner.workflow_run_id() == Some(run_id))
            .count()
}
