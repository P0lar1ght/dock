//! `update_goal` — model-driven goal progress reporting.
//!
//! Each invocation is paired with an `oneshot::Sender<UpdateGoalAck>`
//! over the channel to `SessionActor`; the tool blocks on that ack so
//! the model's tool reply reflects the real outcome (classifier
//! verdict / transition / rejection), not a misleading instant success.

pub const UPDATE_GOAL_TOOL_NAME: &str = "update_goal";

/// Copied from `xai-grok-tools-api::slash_commands`.
pub const GOAL_RESERVED_SUBCOMMANDS: &[&str] = &["status", "pause", "resume", "clear", "edit"];

/// 斜杠 `/goal` 注入给模型的说明（工具名保持英文）。
pub fn goal_usage_message() -> &'static str {
    "用法: /goal <目标>\n\
     设定一个目标，一直做到完成为止。"
}

/// Bare `/goal` 回车后留在输入框里：用法可见，光标在 `/goal ` 后。
pub fn goal_composer_fill() -> String {
    format!("{}\n/goal ", goal_usage_message())
}

/// 斜杠 `/goal` 注入给模型的说明（工具名保持英文）。
pub fn goal_instruction(objective: &str) -> String {
    format!(
        "# /goal — 追求目标\n\n\
         已设定目标：{objective}\n\n\
         请直接推进这个目标，能做多远做多远。用户要的东西由你自己交付：不要追问、不要把手工步骤留给用户。如果对话继续，就一直做到目标完成为止。\n\n\
         跟踪：把目标拆成具体步骤，有 todo 工具就用来跟踪，做完一项标完成一项。\n\n\
         边做边验证：每改一处都要在真实路径上测过再往下走。声称完成必须有本会话里产出的证据，不能靠假设。\n\n\
         只有目标真正达成时才调用 update_goal(completed: true, message: \"摘要\")。只有同一问题连续失败 3 次以上、确实卡住时才调用 update_goal(blocked_reason: \"原因\")。过程中可用 update_goal(message: \"进展\") 记一笔。如果 update_goal 返回错误，继续做目标，并在回复里报告状态。\n\n\
         现在开始。"
    )
}

/// Injected when no goal is present so the model can start one itself.
pub fn goal_offer_addon() -> &'static str {
    "多步、需要持续推进直到完成的任务，先调用 update_goal(objective: \"…\") 设定目标。\
     设定后用 update_goal(message) 记进度，全部完成时 update_goal(completed: true)。\
     一次性问答或简单查询不要设目标。"
}

/// Grok `GOAL_CONTINUATION_SENTINEL` + slim Chinese body. Hidden from the
/// pager (`LogEvent::SystemReminder`); the HTTP mapper sends it as the next
/// user-role message so an active goal does not end on the first text sample.
pub const GOAL_CONTINUATION_SENTINEL: &str = "Goal NOT complete — continue working. Next step:";

pub fn goal_continuation_directive(objective: &str) -> String {
    format!(
        "<system-reminder>\n\
         <goal-state>\n\
         Objective: {objective}\n\
         Status: Active\n\
         </goal-state>\n\n\
         {GOAL_CONTINUATION_SENTINEL}\n\
         继续推进这个目标，不要停下来向用户确认。有 todo 就看列表做下一步。只有真正完成时才调用 update_goal(completed: true)。\n\
         </system-reminder>"
    )
}

use super::serde_lenient::deserialize_lenient_option_bool;

#[derive(Debug)]
pub struct GoalToolError {
    pub kind: &'static str,
    pub detail: String,
}

impl GoalToolError {
    pub fn custom(kind: &'static str, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Input schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateGoalInput {
    #[serde(default)]
    #[schemars(
        description = "Set or replace the goal objective. Call this when the user's task is multi-step and should run until complete. Omit for a one-shot question. With no active goal this starts goal mode; with an active goal this retitles it."
    )]
    pub objective: Option<String>,

    #[serde(default, deserialize_with = "deserialize_lenient_option_bool")]
    #[schemars(
        description = "Set to true ONLY when the goal is fully achieved. This ends goal mode. Use together with `message` to include a completion summary."
    )]
    pub completed: Option<bool>,

    #[serde(default)]
    #[schemars(
        description = "Optional short message logged as progress (visible in tool response, not surfaced to the pager dashboard). Use with `completed: true` for a completion summary."
    )]
    pub message: Option<String>,

    #[serde(default)]
    #[schemars(
        description = "Set only when truly stuck after 3+ consecutive failed attempts at the same problem. If set, the goal is paused as blocked. This is a FAILURE signal — never put success text here. For success, use `completed: true` with `message`."
    )]
    pub blocked_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Channel types — inserted into Resources, read by SessionActor
// ---------------------------------------------------------------------------

/// Outcome of an `update_goal` call as delivered by the session actor.
#[derive(Debug)]
pub enum UpdateGoalAck {
    /// `message`-only or `blocked_reason` update accepted.
    Accepted { summary: String },
    /// Classifier judged the goal achieved.
    ClassifierAchieved { details_path: String },
    /// Classifier could not produce a verdict (infra failure); the
    /// harness fails open and treats the goal as achieved.
    ClassifierFailOpenAchieved { reason: &'static str },
    /// Classifier rejected the completion; `attempt < max_runs` so
    /// another attempt is still available.
    ClassifierNotAchieved {
        details_path: String,
        attempt: u32,
        max_runs: u32,
    },
    /// Classifier rejected the completion AND the per-goal cap was
    /// reached; the goal has been auto-paused with `BackOff`.
    ClassifierCapReached { details_path: String, attempt: u32 },
    /// Classifier rejected the completion with the same flagged gaps as
    /// the prior attempt (no progress); the goal auto-paused early
    /// before the cap.
    ClassifierStalled { details_path: String, attempt: u32 },
    /// Verification found no model-fixable path (every refuter flagged a
    /// contradiction or environment-unverifiable blocker); the goal
    /// paused for a user decision.
    ClassifierBlocked { details_path: String },
    /// Classifier disabled by policy; goal marked complete directly.
    CompletedWithoutClassifier,
    /// Second `update_goal(completed: true)` arrived while a
    /// classifier was already verifying the previous attempt; routed
    /// through the synthetic-NotAchieved accounting.
    ClassifierConcurrentInFlight {
        details_path: String,
        attempt: u32,
        max_runs: u32,
    },
    /// Mid-turn `completed: true` was queued for classifier
    /// verification at turn-end. The verdict arrives as a system
    /// reminder in the next user turn; the model must NOT call
    /// `update_goal(completed: true)` again until then.
    ///
    /// Invariant: the ack is resolved IMMEDIATELY at defer time, NOT
    /// parked — parking deadlocks the single-task actor.
    DeferredToTurnEnd { pending_depth: u32 },
    /// Update was rejected; `reason` discriminates the cause and
    /// drives the tool-error code, `detail` is the model-facing
    /// message.
    Rejected {
        reason: RejectReason,
        detail: String,
    },
}

/// Structured cause for `UpdateGoalAck::Rejected`; each variant maps
/// to a stable `error_code()` the model sees as the `ToolError` kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// A prior cmd in this drain transitioned the goal to Blocked.
    BlockSeenInDrain,
    /// `blocked_reason` set but the goal was not Active.
    BlockedAgainstNonActive,
    /// `completed: true` arrived after the classifier cap auto-paused
    /// the goal — model must wait for user resume.
    PostCap,
    /// `completed: true` against a non-Active goal for reasons OTHER
    /// than the classifier cap.
    NonActive,
    /// The goal harness is not enabled for this session (no `/goal`
    /// run in progress), so there is no orchestration to update. The
    /// `update_goal` tool and its `GoalUpdateHandle` are always
    /// exposed, so a model can call the tool outside goal mode; the
    /// drain rejects cleanly with this reason instead of dropping the
    /// ack oneshot (which would surface as the misleading
    /// `harness_no_ack` "dropped the response channel" error).
    HarnessDisabled,
    /// Reserved for strict-mode eviction surfacing; not currently
    /// constructed (the new design acks evicted entries as
    /// `DeferredToTurnEnd` at their own defer time).
    PendingQueueEvicted,
    /// The goal auto-paused mid-drain (cap, stall/no_progress, or
    /// blocked); this strictly-later entry was dropped without
    /// re-verification.
    DroppedAfterPauseInDrain,
    /// `GoalOrchestration` snapshot vanished between guard and reserve.
    OrchestrationVanished,
    /// Goal transitioned out of Active while the classifier awaited
    /// a verdict (user paused mid-fire).
    StatusChangedDuringClassifier,
    /// In-flight short-circuit but the orchestration snapshot
    /// vanished mid-flight.
    InFlightOrchestrationVanished,
}

impl RejectReason {
    /// Stable error code surfaced as the tool's `ToolError.kind`.
    pub fn error_code(self) -> &'static str {
        match self {
            Self::BlockSeenInDrain => "goal_update_block_seen",
            Self::BlockedAgainstNonActive => "goal_update_blocked_against_non_active",
            Self::PostCap => "goal_update_post_cap",
            Self::NonActive => "goal_update_non_active",
            Self::HarnessDisabled => "goal_update_harness_disabled",
            Self::PendingQueueEvicted => "goal_update_evicted",
            Self::DroppedAfterPauseInDrain => "goal_update_dropped_after_pause",
            Self::OrchestrationVanished => "goal_update_no_orchestration",
            Self::StatusChangedDuringClassifier => "goal_update_status_changed",
            Self::InFlightOrchestrationVanished => "goal_update_in_flight_orchestration_vanished",
        }
    }
}

/// Item posted across the goal-update channel: the model's input
/// paired with the oneshot the tool will await for its reply.
pub type UpdateGoalEnvelope = (UpdateGoalInput, tokio::sync::oneshot::Sender<UpdateGoalAck>);

/// Wrap an `UpdateGoalInput` in an envelope whose ack receiver is
/// discarded. Test-only helper; `pub` is needed for cross-crate test
/// access from `xai-grok-shell`.
#[doc(hidden)]
pub fn envelope_for_test(input: UpdateGoalInput) -> UpdateGoalEnvelope {
    let (ack_tx, _ack_rx) = tokio::sync::oneshot::channel();
    (input, ack_tx)
}

/// Handle for the `update_goal` tool to send commands to the session.
/// Inserted into Resources as an ephemeral (non-serialized) resource.
pub struct GoalUpdateHandle(pub tokio::sync::mpsc::UnboundedSender<UpdateGoalEnvelope>);

impl std::fmt::Debug for GoalUpdateHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalUpdateHandle").finish()
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateGoalOutput {
    pub success: bool,
    pub summary: String,
}

/// Map an [`UpdateGoalAck`] to the model-facing tool result. Public
/// so host session tests can assert the same model-facing strings.
pub fn render_ack_into_output(ack: UpdateGoalAck) -> Result<UpdateGoalOutput, GoalToolError> {
    match ack {
        UpdateGoalAck::Accepted { summary } => Ok(UpdateGoalOutput {
            success: true,
            summary,
        }),
        UpdateGoalAck::CompletedWithoutClassifier => Ok(UpdateGoalOutput {
            success: true,
            summary: "Goal marked complete.".to_string(),
        }),
        UpdateGoalAck::ClassifierAchieved { details_path } => Ok(UpdateGoalOutput {
            success: true,
            summary: format!(
                "Goal classifier verdict: Achieved. Goal complete. See {details_path}"
            ),
        }),
        UpdateGoalAck::ClassifierFailOpenAchieved { reason } => Ok(UpdateGoalOutput {
            success: true,
            summary: format!(
                "Goal marked complete via fail-open (reason: {reason}). No classifier verdict \
                 was produced."
            ),
        }),
        UpdateGoalAck::ClassifierNotAchieved {
            details_path,
            attempt,
            max_runs,
        } => Err(GoalToolError::custom(
            "goal_classifier_not_achieved",
            format!(
                "Goal classifier rejected this completion attempt ({attempt}/{max_runs}). \
                 Review {details_path} and continue working; another attempt is available."
            ),
        )),
        UpdateGoalAck::ClassifierCapReached {
            details_path,
            attempt,
        } => {
            // Empty path ⇒ the harness wrote no synthetic details (e.g. a
            // squatted scratch root); omit the "See …" pointer entirely.
            let pointer = if details_path.trim().is_empty() {
                String::new()
            } else {
                format!(" See {details_path}")
            };
            Err(GoalToolError::custom(
                "goal_classifier_cap_reached",
                format!(
                    "Goal classifier rejected completion {attempt} times — goal auto-paused.{pointer}"
                ),
            ))
        }
        UpdateGoalAck::ClassifierStalled {
            details_path,
            attempt,
        } => Err(GoalToolError::custom(
            "goal_classifier_stalled",
            format!(
                "Goal verification saw no change in the flagged gaps across {attempt} attempts \
                 — goal auto-paused. Review {details_path}; the user must resume."
            ),
        )),
        UpdateGoalAck::ClassifierBlocked { details_path } => Err(GoalToolError::custom(
            "goal_classifier_blocked",
            format!(
                "Goal verification found no model-fixable path (objective/plan contradiction or \
                 evidence that cannot be captured here) — goal paused for your decision. \
                 See {details_path}"
            ),
        )),
        UpdateGoalAck::ClassifierConcurrentInFlight {
            details_path,
            attempt,
            max_runs,
        } => {
            // Empty path ⇒ no harness-written details to point at; omit the
            // "; see …" pointer (never reference content we didn't write).
            let pointer = if details_path.trim().is_empty() {
                String::new()
            } else {
                format!("; see {details_path}")
            };
            Err(GoalToolError::custom(
                "goal_classifier_in_flight",
                format!(
                    "Goal classifier is still verifying a previous completion — do NOT call \
                     update_goal(completed: true) again until you receive a verdict reminder. \
                     This attempt was recorded as Not Achieved ({attempt}/{max_runs}){pointer}"
                ),
            ))
        }
        UpdateGoalAck::DeferredToTurnEnd { pending_depth } => Ok(UpdateGoalOutput {
            success: true,
            summary: format!(
                "Goal completion queued for classifier verification at end of turn \
                 (pending_depth={pending_depth}). The verdict will be delivered as a \
                 system reminder before your next reply; do NOT call update_goal \
                 again until you see it."
            ),
        }),
        UpdateGoalAck::Rejected { reason, detail } => {
            Err(GoalToolError::custom(reason.error_code(), detail))
        }
    }
}

pub fn build_summary(input: &UpdateGoalInput) -> String {
    let mut parts = Vec::new();
    if let Some(obj) = input
        .objective
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("Goal set: {obj}"));
    }
    if input.completed == Some(true) {
        parts.push("Goal marked complete".to_string());
    }
    if let Some(ref reason) = input.blocked_reason {
        parts.push(format!("Goal blocked: {reason}"));
    }
    if let Some(ref msg) = input.message {
        parts.push(msg.clone());
    }
    if parts.is_empty() {
        "Goal updated.".to_string()
    } else {
        parts.join(". ") + "."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_input() -> UpdateGoalInput {
        UpdateGoalInput {
            objective: None,
            completed: None,
            message: None,
            blocked_reason: None,
        }
    }

    #[test]
    fn build_summary_empty_input() {
        assert_eq!(build_summary(&empty_input()), "Goal updated.");
    }

    #[test]
    fn build_summary_completed() {
        let input = UpdateGoalInput {
            completed: Some(true),
            ..empty_input()
        };
        assert_eq!(build_summary(&input), "Goal marked complete.");
    }

    #[test]
    fn build_summary_message_only() {
        let input = UpdateGoalInput {
            message: Some("Working on it".into()),
            ..empty_input()
        };
        assert_eq!(build_summary(&input), "Working on it.");
    }

    #[test]
    fn build_summary_blocked_reason_only() {
        let input = UpdateGoalInput {
            blocked_reason: Some("no windows sdk".into()),
            ..empty_input()
        };
        assert_eq!(build_summary(&input), "Goal blocked: no windows sdk.");
    }

    #[test]
    fn build_summary_blocked_reason_with_message() {
        let input = UpdateGoalInput {
            blocked_reason: Some("X".into()),
            message: Some("longer body".into()),
            ..empty_input()
        };
        let summary = build_summary(&input);
        assert!(summary.contains("Goal blocked: X"));
        assert!(summary.contains("longer body"));
    }

    #[test]
    fn build_summary_completed_with_message() {
        let input = UpdateGoalInput {
            completed: Some(true),
            message: Some("All done".into()),
            ..empty_input()
        };
        let summary = build_summary(&input);
        assert!(summary.contains("Goal marked complete"));
        assert!(summary.contains("All done"));
    }

    #[test]
    fn build_summary_objective_only() {
        let input = UpdateGoalInput {
            objective: Some("ship lsp".into()),
            ..empty_input()
        };
        assert_eq!(build_summary(&input), "Goal set: ship lsp.");
    }

    #[test]
    fn build_summary_completed_false_treated_as_noop() {
        let input = UpdateGoalInput {
            completed: Some(false),
            ..empty_input()
        };
        assert_eq!(build_summary(&input), "Goal updated.");
    }

    #[test]
    fn composer_fill_keeps_usage_and_slash() {
        let fill = goal_composer_fill();
        assert!(fill.contains("用法: /goal <目标>"), "{fill}");
        assert!(fill.contains('\n'), "{fill}");
        assert!(fill.ends_with("/goal "), "{fill}");
    }
}
