//! Drain copied from Grok `SessionActor::drain_goal_updates_with_extra`.
//! Classifier policy is disabled (dock has no classifier) → `CompletedWithoutClassifier`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedReceiver;

use super::grok_tool::{RejectReason, UpdateGoalAck, UpdateGoalEnvelope, UpdateGoalInput};

pub struct GoalState {
    pub active: AtomicBool,
    pub paused: AtomicBool,
    pub awaiting_composer: AtomicBool,
    pub instruction_pending: AtomicBool,
    pub title: Mutex<String>,
    /// Last `update_goal` message / blocked note for TUI chrome.
    pub status: Mutex<String>,
    pub blocked_streak: AtomicU32,
    /// 模型报了 `update_goal(completed)`：目标不再跑，但标题和进展留着给客户端
    /// 显示「已完成」，直到下一次 `start` / `clear`。
    completed: AtomicBool,
    timer: Mutex<GoalTimer>,
}

/// 目标真正在跑的时长：暂停的那段不算。
#[derive(Default)]
struct GoalTimer {
    running_since: Option<Instant>,
    banked: Duration,
}

impl GoalTimer {
    fn run(&mut self) {
        self.running_since.get_or_insert_with(Instant::now);
    }

    fn stop(&mut self) {
        if let Some(since) = self.running_since.take() {
            self.banked += since.elapsed();
        }
    }

    fn elapsed(&self) -> Duration {
        self.banked + self.running_since.map_or(Duration::ZERO, |s| s.elapsed())
    }
}

impl GoalState {
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            awaiting_composer: AtomicBool::new(false),
            instruction_pending: AtomicBool::new(false),
            title: Mutex::new(String::new()),
            status: Mutex::new(String::new()),
            blocked_streak: AtomicU32::new(0),
            completed: AtomicBool::new(false),
            timer: Mutex::new(GoalTimer::default()),
        }
    }

    pub fn start(&self, title: impl Into<String>) {
        let title = title.into();
        *self.title.lock().unwrap() = if title.trim().is_empty() {
            "目标".into()
        } else {
            title
        };
        self.status.lock().unwrap().clear();
        self.active.store(true, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
        self.awaiting_composer.store(false, Ordering::SeqCst);
        self.instruction_pending.store(true, Ordering::SeqCst);
        self.blocked_streak.store(0, Ordering::SeqCst);
        self.completed.store(false, Ordering::SeqCst);
        let mut timer = self.timer.lock().unwrap();
        *timer = GoalTimer::default();
        timer.run();
    }

    pub fn take_instruction(&self) -> Option<String> {
        if !self.instruction_pending.swap(false, Ordering::SeqCst) {
            return None;
        }
        if !self.active() {
            return None;
        }
        Some(super::grok_tool::goal_instruction(
            &self.title.lock().unwrap(),
        ))
    }

    pub fn active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Running or paused — chrome / overlay still have a goal to show.
    pub fn present(&self) -> bool {
        self.active() || self.paused()
    }

    pub fn pause(&self) -> bool {
        if !self.active() {
            return false;
        }
        self.active.store(false, Ordering::SeqCst);
        self.paused.store(true, Ordering::SeqCst);
        self.instruction_pending.store(false, Ordering::SeqCst);
        self.timer.lock().unwrap().stop();
        true
    }

    pub fn resume(&self) -> bool {
        if !self.paused() {
            return false;
        }
        self.paused.store(false, Ordering::SeqCst);
        self.active.store(true, Ordering::SeqCst);
        self.instruction_pending.store(true, Ordering::SeqCst);
        self.timer.lock().unwrap().run();
        true
    }

    pub fn clear(&self) {
        self.active.store(false, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
        self.awaiting_composer.store(false, Ordering::SeqCst);
        self.instruction_pending.store(false, Ordering::SeqCst);
        self.blocked_streak.store(0, Ordering::SeqCst);
        self.title.lock().unwrap().clear();
        self.status.lock().unwrap().clear();
        self.completed.store(false, Ordering::SeqCst);
        *self.timer.lock().unwrap() = GoalTimer::default();
    }

    /// 模型报完成：停表、停跑，标题留着（见 `completed` 字段）。
    fn complete(&self) {
        self.active.store(false, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
        self.completed.store(true, Ordering::SeqCst);
        self.timer.lock().unwrap().stop();
    }

    /// 上一个目标是模型报完成收尾的（不是被清掉的）。
    pub fn completed(&self) -> bool {
        self.completed.load(Ordering::SeqCst)
    }

    /// 目标跑了多久（暂停的时段不算）；没有目标为 0。
    pub fn elapsed(&self) -> Duration {
        self.timer.lock().unwrap().elapsed()
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }

    fn record_status(&self, text: impl Into<String>) {
        *self.status.lock().unwrap() = text.into();
    }

    pub fn set_title(&self, title: impl Into<String>) {
        let title = title.into();
        *self.title.lock().unwrap() = if title.trim().is_empty() {
            "目标".into()
        } else {
            title
        };
        if self.present() {
            self.instruction_pending.store(true, Ordering::SeqCst);
        }
    }

    pub fn arm_composer(&self) {
        self.awaiting_composer.store(true, Ordering::SeqCst);
    }

    pub fn disarm_composer(&self) {
        self.awaiting_composer.store(false, Ordering::SeqCst);
    }

    pub fn awaiting_composer(&self) -> bool {
        self.awaiting_composer.load(Ordering::SeqCst)
    }

    fn harness_enabled(&self) -> bool {
        self.active()
    }
}

/// Copied from Grok `send_ack`.
fn send_ack(ack_tx: tokio::sync::oneshot::Sender<UpdateGoalAck>, ack: UpdateGoalAck) {
    let _ = ack_tx.send(ack);
}

/// Copied from Grok `drain_goal_updates_with_extra` (classifier `enabled: false`).
pub fn drain_one(
    state: &GoalState,
    input: UpdateGoalInput,
    ack_tx: tokio::sync::oneshot::Sender<UpdateGoalAck>,
) {
    let objective = input
        .objective
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(title) = objective {
        if !state.present() {
            state.start(title.clone());
            if let Some(message) = input
                .message
                .as_ref()
                .map(|m| m.trim())
                .filter(|m| !m.is_empty())
            {
                state.record_status(message.to_string());
            }
            send_ack(
                ack_tx,
                UpdateGoalAck::Accepted {
                    summary: format!("Goal set: {title}."),
                },
            );
            return;
        }
        state.set_title(&title);
        if !state.harness_enabled() {
            send_ack(
                ack_tx,
                UpdateGoalAck::Accepted {
                    summary: format!("Goal retitled: {title}."),
                },
            );
            return;
        }
    }
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
        state.record_status(format!("受阻 {streak}/3：{reason}"));
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
        // 受阻三次是**暂停**，不是清掉。直接写 `active = false` 会让 `paused`
        // 仍是 false，于是 `present()` 变假：目标从 TUI 里整个消失，`resume()`
        // 也拒绝（它要求 `paused()`），用户再没有办法把它捡回来。
        state.pause();
        send_ack(
            ack_tx,
            UpdateGoalAck::Accepted {
                summary: format!("Goal blocked: {reason}. 目标已暂停，用 /goal resume 继续。"),
            },
        );
        return;
    }
    if input.completed != Some(true) {
        if let Some(message) = input
            .message
            .as_ref()
            .map(|m| m.trim())
            .filter(|m| !m.is_empty())
        {
            state.record_status(message.to_string());
        }
        let summary = input
            .message
            .clone()
            .map(|m| format!("{m}."))
            .unwrap_or_else(|| "Goal updated.".to_string());
        send_ack(ack_tx, UpdateGoalAck::Accepted { summary });
        return;
    }
    state.blocked_streak.store(0, Ordering::Relaxed);
    // 完成时附带的总结就是最后一条进展：客户端的「已完成」条显示它。
    if let Some(message) = input
        .message
        .as_ref()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
    {
        state.record_status(message.to_string());
    }
    // Grok: `if !policy.enabled { tracker.complete(); CompletedWithoutClassifier }`
    state.complete();
    send_ack(ack_tx, UpdateGoalAck::CompletedWithoutClassifier);
}

pub async fn drain_loop(
    state: std::sync::Arc<GoalState>,
    mut rx: UnboundedReceiver<UpdateGoalEnvelope>,
) {
    while let Some((input, ack_tx)) = rx.recv().await {
        drain_one(&state, input, ack_tx);
    }
}

#[cfg(test)]
mod tests {
    use super::super::grok_tool::{RejectReason, UpdateGoalAck, UpdateGoalInput};
    use super::*;

    #[test]
    fn pause_disables_harness_resume_restores() {
        let state = GoalState::new();
        state.start("ship");
        assert!(state.active());
        assert!(state.present());
        assert!(state.pause());
        assert!(!state.active());
        assert!(state.paused());
        assert!(state.present());
        assert_eq!(state.title.lock().unwrap().as_str(), "ship");
        assert!(state.resume());
        assert!(state.active());
        assert!(!state.paused());
    }

    /// 受阻三次要**暂停**目标，不是让它凭空消失。
    ///
    /// 改动前这里直接写 `active = false`，`paused` 仍是 false：`present()` 变假
    /// → TUI 里没有目标了，`resume()` 又要求 `paused()` → 用户捡不回来。
    #[test]
    fn three_blocked_attempts_pause_instead_of_vanishing() {
        let state = GoalState::new();
        state.start("ship");
        for _ in 0..3 {
            let (tx, rx) = tokio::sync::oneshot::channel();
            drain_one(
                &state,
                UpdateGoalInput {
                    blocked_reason: Some("上游挂了".into()),
                    ..Default::default()
                },
                tx,
            );
            let _ = rx.blocking_recv().unwrap();
        }
        assert!(!state.active(), "受阻三次后不该还在跑");
        assert!(state.paused(), "应当是暂停");
        assert!(state.present(), "TUI 仍要看得到这个目标");
        assert!(state.resume(), "用户要能把它捡回来");
        assert!(state.active());
    }

    #[test]
    fn pause_rejects_update_goal() {
        let state = GoalState::new();
        state.start("ship");
        state.pause();
        let (tx, rx) = tokio::sync::oneshot::channel();
        drain_one(
            &state,
            UpdateGoalInput {
                message: Some("hi".into()),
                ..Default::default()
            },
            tx,
        );
        let ack = rx.blocking_recv().unwrap();
        assert!(matches!(
            ack,
            UpdateGoalAck::Rejected {
                reason: RejectReason::HarnessDisabled,
                ..
            }
        ));
    }

    #[test]
    fn clear_drops_present() {
        let state = GoalState::new();
        state.start("ship");
        state.clear();
        assert!(!state.present());
        assert!(state.title.lock().unwrap().is_empty());
    }

    /// 模型报完成：不再跑、也不算暂停，但客户端要能显示「已完成 · 用时」。
    #[test]
    fn completion_keeps_title_and_stops_the_clock() {
        let state = GoalState::new();
        state.start("ship");
        let (tx, rx) = tokio::sync::oneshot::channel();
        drain_one(
            &state,
            UpdateGoalInput {
                completed: Some(true),
                ..Default::default()
            },
            tx,
        );
        let _ = rx.blocking_recv().unwrap();
        assert!(!state.present());
        assert!(state.completed());
        assert_eq!(state.title.lock().unwrap().as_str(), "ship");
        let stopped = state.elapsed();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(state.elapsed(), stopped, "完成后不该再计时");
        state.start("next");
        assert!(!state.completed(), "新目标不带上一个的完成标记");
        state.clear();
        assert!(!state.completed());
        assert_eq!(state.elapsed(), Duration::ZERO);
    }

    #[test]
    fn paused_time_is_not_counted() {
        let state = GoalState::new();
        state.start("ship");
        std::thread::sleep(Duration::from_millis(20));
        assert!(state.pause());
        let at_pause = state.elapsed();
        assert!(at_pause >= Duration::from_millis(20));
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(state.elapsed(), at_pause, "暂停期间不计时");
        assert!(state.resume());
        std::thread::sleep(Duration::from_millis(5));
        assert!(state.elapsed() > at_pause);
    }

    #[test]
    fn continuation_directive_keeps_grok_sentinel() {
        let d = super::super::grok_tool::goal_continuation_directive("理解并分析 TUI");
        assert!(
            d.contains(super::super::grok_tool::GOAL_CONTINUATION_SENTINEL),
            "{d}"
        );
        assert!(d.contains("理解并分析 TUI"), "{d}");
        assert!(d.contains("<system-reminder>"), "{d}");
    }

    #[test]
    fn update_stores_status_message() {
        let state = GoalState::new();
        state.start("ship");
        let (tx, rx) = tokio::sync::oneshot::channel();
        drain_one(
            &state,
            UpdateGoalInput {
                message: Some("  正在读 TUI  ".into()),
                ..Default::default()
            },
            tx,
        );
        let _ = rx.blocking_recv().unwrap();
        assert_eq!(state.status(), "正在读 TUI");
        state.clear();
        assert!(state.status().is_empty());
    }

    #[test]
    fn objective_starts_goal_without_prior_slash() {
        let state = GoalState::new();
        let (tx, rx) = tokio::sync::oneshot::channel();
        drain_one(
            &state,
            UpdateGoalInput {
                objective: Some("  理解 TUI  ".into()),
                message: Some("开干".into()),
                ..Default::default()
            },
            tx,
        );
        let ack = rx.blocking_recv().unwrap();
        assert!(matches!(ack, UpdateGoalAck::Accepted { summary } if summary.contains("理解 TUI")));
        assert!(state.active());
        assert_eq!(state.title.lock().unwrap().as_str(), "理解 TUI");
        assert_eq!(state.status(), "开干");
    }
}
