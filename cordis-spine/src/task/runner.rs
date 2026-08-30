//! Dock `ChildRunner`: Grok coordinator seam; child body is isolate
//! `"sessions"`+`"turn"`+`"agentPresets"` + [`GrokStep`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use cordis::Context;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::agent_presets::AgentPresets;
use crate::names::{AGENT_PRESETS, SESSIONS, TURN};
use crate::runtime::{GrokStep, LoopHandle};
use crate::session::Sessions;
use crate::turn::TurnControl;
use crate::types::{LogEvent, TurnOutcome};

use super::coordinator::{
    ChildCompletion, ChildControl, ChildRunOutput, ChildRunRequest, ChildRunner, SendBoxFuture,
    StartedChild, SubagentProgress,
};
use super::interjection::format_interjection;
use super::store::ChildStore;
use super::types::{
    SubagentDescribeOutcome, SubagentOwner, SubagentResult, SubagentTypeSummary,
    SubagentValidateTypeOutcome,
};
use super::{DEPTH, SubagentLife, current_depth};

pub(super) const PARENT_SESSION_ID: &str = "dock";

pub(super) struct DockChildControl {
    turn: Arc<TurnControl>,
    cancelled: Arc<AtomicBool>,
    token: CancellationToken,
}

impl ChildControl for DockChildControl {
    type ProgressFuture = std::future::Ready<SubagentProgress>;

    fn progress(&self) -> Self::ProgressFuture {
        std::future::ready(SubagentProgress::default())
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.turn.cancel();
        self.token.cancel();
    }
}

pub(super) struct DockChildRunner {
    pub ctx: Context,
    pub store: ChildStore,
}

impl ChildRunner for DockChildRunner {
    type Control = DockChildControl;
    type CompletionData = ();
    type RunFuture = SendBoxFuture<ChildRunOutput<()>>;
    type ValidateFuture = SendBoxFuture<SubagentValidateTypeOutcome>;
    type DescribeFuture = SendBoxFuture<SubagentDescribeOutcome>;

    fn run(&self, run: ChildRunRequest<Self::Control>) -> Self::RunFuture {
        let parent = self.ctx.clone();
        let store = self.store.clone();
        Box::pin(async move { run_dock_child(parent, store, run).await })
    }

    fn validate_type(
        &self,
        subagent_type: String,
        _parent_session_id: String,
    ) -> Self::ValidateFuture {
        let ctx = self.ctx.clone();
        Box::pin(async move { validate_roster(&ctx, &subagent_type) })
    }

    fn describe_type(
        &self,
        subagent_type: String,
        _harness_agent_type: Option<String>,
        _parent_session_id: String,
    ) -> Self::DescribeFuture {
        let ctx = self.ctx.clone();
        Box::pin(async move {
            match validate_roster(&ctx, &subagent_type) {
                SubagentValidateTypeOutcome::Ok => {
                    SubagentDescribeOutcome::Ok(SubagentTypeSummary::default())
                }
                SubagentValidateTypeOutcome::Unknown { available } => {
                    SubagentDescribeOutcome::Unknown { available }
                }
                _ => SubagentDescribeOutcome::Unknown {
                    available: Vec::new(),
                },
            }
        })
    }

    fn on_completed(&self, completion: ChildCompletion<Self::CompletionData>) {
        if let Some(s) = self.store.snapshot(&completion.request.id) {
            if !s.done {
                // `run()` returns after the first turn; drive_child still owns
                // the slot and has already parked / continued.
                return;
            }
        }
        self.store
            .set_output(&completion.request.id, completion.result.output.to_string());
    }
}

fn validate_roster(ctx: &Context, subagent_type: &str) -> SubagentValidateTypeOutcome {
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        return SubagentValidateTypeOutcome::Unknown {
            available: Vec::new(),
        };
    };
    let roster = presets.current_roster();
    if roster.contains_key(subagent_type) {
        SubagentValidateTypeOutcome::Ok
    } else {
        SubagentValidateTypeOutcome::Unknown {
            available: roster.keys().cloned().collect(),
        }
    }
}

fn child_prompt(subagent_type: &str, description: &str, prompt: &str) -> String {
    format!("[{subagent_type}] {description}\n\n{prompt}")
}

async fn run_dock_child(
    parent: Context,
    store: ChildStore,
    run: ChildRunRequest<DockChildControl>,
) -> ChildRunOutput<()> {
    let wall = Instant::now();
    let id = run.request.id.clone();
    let typ = run.request.subagent_type.clone();
    let desc = run.request.description.clone();
    store.ensure_owned(&id, desc.clone(), typ.clone(), run.request.owner.clone());

    let resume = run
        .request
        .resume_from
        .as_deref()
        .map(|src| store.events(src))
        .unwrap_or_default();
    let prompt = child_prompt(&typ, &desc, &run.request.prompt);

    let def = parent
        .get::<AgentPresets>(AGENT_PRESETS)
        .and_then(|p| p.subagent(&typ));
    let Some(def) = def else {
        return failed(
            &id,
            &store,
            wall,
            format!("unknown subagent type {typ}"),
            false,
        );
    };

    let child = parent
        .isolate("sessions")
        .isolate("turn")
        .isolate("agentPresets");
    let sessions = Sessions::isolated_as(child.clone(), id.clone());
    if !resume.is_empty() {
        sessions.seed(resume);
    }
    let mut hold = Vec::new();
    match child.provide(SESSIONS, sessions.clone()) {
        Ok(d) => hold.push(d),
        Err(e) => {
            return failed(&id, &store, wall, format!("child sessions: {e}"), false);
        }
    }
    match child.provide(TURN, TurnControl::new()) {
        Ok(d) => hold.push(d),
        Err(e) => {
            return failed(&id, &store, wall, format!("child turn: {e}"), false);
        }
    }
    let mut preset = def.to_preset(&typ);
    if matches!(run.request.owner, SubagentOwner::Subagent) {
        super::format::append_mailbox_report_duty(&mut preset.persona);
    }
    match child.provide(AGENT_PRESETS, AgentPresets::overlay(preset)) {
        Ok(d) => hold.push(d),
        Err(e) => {
            return failed(&id, &store, wall, format!("child agentPresets: {e}"), false);
        }
    }
    let Some(turn) = child.get::<TurnControl>(TURN) else {
        return failed(&id, &store, wall, "child turn missing".into(), false);
    };
    store.set_turn(&id, turn.clone());
    store.set_hold(&id, hold);
    store.set_sessions(&id, sessions.clone());

    let cancelled = Arc::new(AtomicBool::new(false));
    let promoted = run
        .reporter
        .started(StartedChild {
            child_session_id: id.clone(),
            persona: None,
            resumed_from: run.request.resume_from.clone(),
            child_cwd: String::new(),
            worktree_path: None,
            effective_model_id: String::new(),
            definition_background: false,
            control: DockChildControl {
                turn: turn.clone(),
                cancelled: cancelled.clone(),
                token: run.cancellation.clone(),
            },
        })
        .await;
    if !promoted {
        turn.cancel();
        return failed(&id, &store, wall, "cancelled before start".into(), true);
    }

    let (first_tx, first_rx) = oneshot::channel();
    let store_w = store.clone();
    let child_w = child.clone();
    let id_w = id.clone();
    let parent_w = parent.clone();
    let coord_cancel = run.cancellation.clone();
    let notify_idle = matches!(run.request.owner, SubagentOwner::Subagent);
    tokio::spawn(async move {
        drive_child(
            parent_w,
            child_w,
            store_w,
            id_w,
            prompt,
            cancelled,
            coord_cancel,
            first_tx,
            notify_idle,
        )
        .await;
    });

    match first_rx.await {
        Ok(out) => out,
        Err(_) => failed(&id, &store, wall, "child worker dropped".into(), false),
    }
}

async fn drive_child(
    parent: Context,
    child: Context,
    store: ChildStore,
    id: String,
    mut prompt: String,
    cancelled: Arc<AtomicBool>,
    coord_cancel: CancellationToken,
    first_tx: oneshot::Sender<ChildRunOutput<()>>,
    notify_idle: bool,
) {
    let wall = Instant::now();
    let handle = LoopHandle::new(child.clone(), Arc::new(GrokStep));
    let mut first_tx = Some(first_tx);

    loop {
        if store
            .get(&id)
            .is_some_and(|s| s.dispose.load(Ordering::Relaxed))
            || (first_tx.is_some() && coord_cancel.is_cancelled())
        {
            let out = failed(&id, &store, wall, "cancelled".into(), true);
            if let Some(tx) = first_tx.take() {
                let _ = tx.send(out);
            }
            store.dispose(&id, "cancelled".into(), true);
            return;
        }

        if let Some(turn) = child.get::<TurnControl>(TURN) {
            turn.reset();
        }
        store.reset_reported(&id);
        store
            .get(&id)
            .inspect(|s| s.set_life(SubagentLife::Running));
        if notify_idle {
            if let Some(s) = child.get::<Sessions>(SESSIONS) {
                s.append(LogEvent::SystemReminder(super::format::wrap_reminder(
                    super::format::MAILBOX_REPORT_TURN_REMINDER,
                )));
            }
        }

        let outcome = if first_tx.is_some() {
            tokio::select! {
                _ = coord_cancel.cancelled() => Err("cancelled".into()),
                out = DEPTH.scope(current_depth() + 1, handle.run(prompt.clone())) => match out {
                    Ok(TurnOutcome::Text(t)) => Ok(t),
                    Err(e) => Err(format!("{e}")),
                },
            }
        } else {
            match DEPTH
                .scope(current_depth() + 1, handle.run(prompt.clone()))
                .await
            {
                Ok(TurnOutcome::Text(t)) => Ok(t),
                Err(e) => Err(format!("{e}")),
            }
        };
        if let Some(s) = child.get::<Sessions>(SESSIONS) {
            store.set_events(&id, s.events());
        }
        if let (Some(child_s), Some(parent_s)) = (
            child.get::<Sessions>(SESSIONS),
            parent.get::<Sessions>(SESSIONS),
        ) {
            let mut child_ledger = child_s.ledger();
            if cancelled.load(Ordering::Relaxed) && child_ledger.totals.model_calls > 0 {
                child_ledger.mark_incomplete();
            }
            parent_s.fold_subagent_ledger(&child_ledger);
        }

        let slot = store.get(&id);
        let dispose = slot
            .as_ref()
            .is_some_and(|s| s.dispose.load(Ordering::Relaxed))
            || (first_tx.is_some() && coord_cancel.is_cancelled());
        let send_now = slot
            .as_ref()
            .is_some_and(|s| s.send_now.swap(false, Ordering::Relaxed));
        let interrupt = slot
            .as_ref()
            .is_some_and(|s| s.interrupt.swap(false, Ordering::Relaxed));

        if dispose {
            let msg = match &outcome {
                Ok(t) => t.clone(),
                Err(e) => e.clone(),
            };
            let out = failed(&id, &store, wall, msg.clone(), true);
            if let Some(tx) = first_tx.take() {
                let _ = tx.send(out);
            }
            store.dispose(&id, msg, true);
            return;
        }

        if send_now {
            if let Some(msg) = store.take_urgent(&id) {
                prompt = format_interjection(&msg);
                continue;
            }
        }

        let duration_ms = wall.elapsed().as_millis() as u64;
        let was_cancelled = cancelled.load(Ordering::Relaxed)
            || interrupt
            || matches!(&outcome, Err(e) if e.contains("cancel"));
        let (success, output) = match &outcome {
            Ok(text) => (true, text.clone()),
            Err(e) => (false, format!("failed: {e}")),
        };
        if notify_idle {
            store.forward_unreported_turn(&id, &output);
        }
        let next_ready = take_inbox(&store, &id);
        if next_ready.is_none() {
            store.park_idle(&id, output.clone(), was_cancelled && interrupt);
        } else {
            store.set_output(&id, output.clone());
        }
        if first_tx.is_some() && notify_idle {
            store.queue_first_idle(&id);
        }
        let first_out = ChildRunOutput {
            result: SubagentResult {
                success: success && !interrupt,
                output: Arc::from(output.clone()),
                error: if success { None } else { Some(output.clone()) },
                cancelled: interrupt,
                subagent_id: id.clone(),
                child_session_id: id.clone(),
                duration_ms,
                ..Default::default()
            },
            completion_data: (),
            snapshot_ref: None,
        };
        if let Some(tx) = first_tx.take() {
            let _ = tx.send(first_out);
        }

        let next = match next_ready {
            Some(msg) => Some(msg),
            None => wait_next(&store, &id).await,
        };
        match next {
            None => {
                store.dispose(&id, output, false);
                return;
            }
            // Idle + urgent is a plain next turn. Envelope is only for send-now
            // while a turn is running (the `send_now` branch above).
            Some(NextMsg::Urgent(m)) => prompt = m,
            Some(NextMsg::Queued(m)) => prompt = m,
        }
    }
}

#[derive(Debug)]
enum NextMsg {
    Queued(String),
    Urgent(String),
}

async fn wait_next(store: &ChildStore, id: &str) -> Option<NextMsg> {
    loop {
        let Some(slot) = store.get(id) else {
            return None;
        };
        if slot.dispose.load(Ordering::Relaxed) {
            return None;
        }
        if let Some(msg) = take_inbox(store, id) {
            return Some(msg);
        }
        let notified = slot.wake.notified();
        tokio::pin!(notified);
        // `notified()` is lazy: enable() inserts the waiter before the
        // second inbox check so enqueue+notify cannot be lost.
        if notified.as_mut().enable() {
            continue;
        }
        if slot.dispose.load(Ordering::Relaxed) {
            return None;
        }
        if let Some(msg) = take_inbox(store, id) {
            return Some(msg);
        }
        notified.await;
    }
}

fn take_inbox(store: &ChildStore, id: &str) -> Option<NextMsg> {
    if let Some(u) = store.take_urgent(id) {
        return Some(NextMsg::Urgent(u));
    }
    store.take_queued(id).map(NextMsg::Queued)
}

fn failed(
    id: &str,
    store: &ChildStore,
    wall: Instant,
    msg: String,
    cancelled: bool,
) -> ChildRunOutput<()> {
    store.dispose(id, msg.clone(), cancelled);
    ChildRunOutput {
        result: SubagentResult {
            success: false,
            cancelled,
            output: Arc::from(msg.clone()),
            error: Some(msg),
            subagent_id: id.to_owned(),
            child_session_id: id.to_owned(),
            duration_ms: wall.elapsed().as_millis() as u64,
            ..Default::default()
        },
        completion_data: (),
        snapshot_ref: None,
    }
}

#[cfg(test)]
mod inbox_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn wait_next_sees_message_enqueued_before_wait() {
        let store = ChildStore::new();
        store.ensure("x", "d".into(), "t".into());
        store.enqueue_queued("x", "hello".into()).unwrap();
        store.park_idle("x", "out".into(), false);
        let got = tokio::time::timeout(Duration::from_millis(200), wait_next(&store, "x")).await;
        assert!(
            matches!(got, Ok(Some(NextMsg::Queued(ref s))) if s == "hello"),
            "{got:?}"
        );
    }

    #[tokio::test]
    async fn wait_next_wakes_on_enqueue_while_idle() {
        let store = ChildStore::new();
        store.ensure("x", "d".into(), "t".into());
        store.park_idle("x", "out".into(), false);
        let store2 = store.clone();
        let join = tokio::spawn(async move { wait_next(&store2, "x").await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        store.enqueue_queued("x", "later".into()).unwrap();
        let got = tokio::time::timeout(Duration::from_millis(200), join)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(got, Some(NextMsg::Queued(ref s)) if s == "later"),
            "{got:?}"
        );
    }

    #[test]
    fn enqueue_while_idle_marks_running_before_return() {
        let store = ChildStore::new();
        store.ensure("x", "d".into(), "t".into());
        store.park_idle("x", "out".into(), false);
        assert!(store.snapshot("x").unwrap().idle);
        let was_running = store.enqueue_queued("x", "hello".into()).unwrap();
        assert!(!was_running);
        assert!(store.snapshot("x").unwrap().running());
        assert_eq!(store.inbox_pending("x"), (1, false));
    }

    #[test]
    fn urgent_while_idle_marks_running_before_return() {
        let store = ChildStore::new();
        store.ensure("x", "d".into(), "t".into());
        store.park_idle("x", "out".into(), false);
        let was_running = store.push_urgent("x", "now".into()).unwrap();
        assert!(!was_running);
        assert!(store.snapshot("x").unwrap().running());
        assert_eq!(store.inbox_pending("x"), (0, true));
    }

    #[test]
    fn mailbox_idle_without_report_forwards() {
        let store = ChildStore::new();
        store.ensure_owned(
            "x",
            "d".into(),
            "t".into(),
            super::super::types::SubagentOwner::Subagent,
        );
        store.forward_unreported_turn("x", "FINDINGS");
        let text = format!("{:?}", store.drain_notices());
        assert!(text.contains("FINDINGS"), "{text}");
        assert!(
            text.contains("未调用 report") || text.contains("report"),
            "{text}"
        );
    }

    #[test]
    fn mailbox_can_report_many_times_then_skip_auto_forward() {
        let store = ChildStore::new();
        store.ensure_owned(
            "x",
            "d".into(),
            "t".into(),
            super::super::types::SubagentOwner::Subagent,
        );
        store.push_report("x", "progress-1");
        store.push_report("x", "progress-2");
        store.forward_unreported_turn("x", "should-not-forward");
        let text = format!("{:?}", store.drain_notices());
        assert!(text.contains("progress-1"), "{text}");
        assert!(text.contains("progress-2"), "{text}");
        assert!(!text.contains("should-not-forward"), "{text}");
    }

    #[test]
    fn task_child_does_not_auto_forward() {
        let store = ChildStore::new();
        store.ensure("x", "d".into(), "t".into());
        store.forward_unreported_turn("x", "FINDINGS");
        assert!(store.drain_notices().is_empty());
    }
}
