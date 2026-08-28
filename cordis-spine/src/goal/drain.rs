//! Drain copied from Grok `SessionActor::drain_goal_updates_with_extra`.
//! Classifier policy is disabled (dock has no classifier) → `CompletedWithoutClassifier`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use tokio::sync::mpsc::UnboundedReceiver;

use super::grok_tool::{
    RejectReason, UpdateGoalAck, UpdateGoalEnvelope, UpdateGoalInput,
};

pub struct GoalState {
    pub active: AtomicBool,
    pub title: Mutex<String>,
    pub blocked_streak: AtomicU32,
}

impl GoalState {
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            title: Mutex::new(String::new()),
            blocked_streak: AtomicU32::new(0),
        }
    }

    pub fn start(&self, title: impl Into<String>) {
        let title = title.into();
        *self.title.lock().unwrap() = if title.trim().is_empty() {
            "目标".into()
        } else {
            title
        };
        self.active.store(true, Ordering::SeqCst);
        self.blocked_streak.store(0, Ordering::SeqCst);
    }

    pub fn active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    fn harness_enabled(&self) -> bool {
        self.active()
    }
}

/// Copied from Grok `send_ack`.
fn send_ack(
    ack_tx: tokio::sync::oneshot::Sender<UpdateGoalAck>,
    ack: UpdateGoalAck,
) {
    let _ = ack_tx.send(ack);
}

/// Copied from Grok `drain_goal_updates_with_extra` (classifier `enabled: false`).
pub fn drain_one(state: &GoalState, input: UpdateGoalInput, ack_tx: tokio::sync::oneshot::Sender<UpdateGoalAck>) {
    if !state.harness_enabled() {
        send_ack(
            ack_tx,
            UpdateGoalAck::Rejected {
                reason: RejectReason::HarnessDisabled,
                detail: "Goal mode is not active for this session (no /goal run in \
                         progress); update_goal has no effect."
                    .to_string(),
            },
        );
        return;
    }
    if let Some(reason) = input.blocked_reason {
        let streak = state.blocked_streak.fetch_add(1, Ordering::Relaxed) + 1;
        if streak < 3 {
            send_ack(
                ack_tx,
                UpdateGoalAck::Accepted {
                    summary: format!(
                        "Blocked attempt {streak}/3 recorded. The harness needs \
                         3 consecutive blocked attempts before pausing the goal — \
                         continue retrying or refining the approach."
                    ),
                },
            );
            return;
        }
        state.active.store(false, Ordering::SeqCst);
        send_ack(
            ack_tx,
            UpdateGoalAck::Accepted {
                summary: format!("Goal blocked: {reason}."),
            },
        );
        return;
    }
    if input.completed != Some(true) {
        let summary = input
            .message
            .clone()
            .map(|m| format!("{m}."))
            .unwrap_or_else(|| "Goal updated.".to_string());
        send_ack(ack_tx, UpdateGoalAck::Accepted { summary });
        return;
    }
    state.blocked_streak.store(0, Ordering::Relaxed);
    // Grok: `if !policy.enabled { tracker.complete(); CompletedWithoutClassifier }`
    state.active.store(false, Ordering::SeqCst);
    send_ack(ack_tx, UpdateGoalAck::CompletedWithoutClassifier);
}

pub async fn drain_loop(state: std::sync::Arc<GoalState>, mut rx: UnboundedReceiver<UpdateGoalEnvelope>) {
    while let Some((input, ack_tx)) = rx.recv().await {
        drain_one(&state, input, ack_tx);
    }
}
