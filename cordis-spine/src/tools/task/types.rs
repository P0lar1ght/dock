//! Copied from grok-build xai-grok-tools grok_build/task/types.rs.
//! `register_resource!` / `educe` / `xai_tool_types` stripped; local enums kept.

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::sync::Arc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub fn is_not_sentinel(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && !t.eq_ignore_ascii_case("null")
        && !t.eq_ignore_ascii_case("none")
        && !t.eq_ignore_ascii_case("undefined")
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SubagentOwner {
    #[default]
    Task,
    Workflow {
        run_id: String,
    },
}

impl SubagentOwner {
    pub fn workflow(run_id: impl Into<String>) -> Self {
        Self::Workflow {
            run_id: run_id.into(),
        }
    }

    pub fn workflow_run_id(&self) -> Option<&str> {
        match self {
            Self::Task => None,
            Self::Workflow { run_id } => Some(run_id),
        }
    }

    pub fn is_workflow(&self) -> bool {
        matches!(self, Self::Workflow { .. })
    }
}

// Request / Response

/// Plain spawn request emitted by `TaskTool`.
#[derive(Debug, Clone)]
pub struct SubagentRequest {
    /// Subagent ID (UUID v7). Same as `TaskToolInput.task_id`; becomes the child session ID.
    pub id: String,
    pub prompt: String,
    pub description: String,
    pub subagent_type: String,
    pub parent_session_id: String,
    /// Resume from a previously completed subagent's conversation.
    /// Inherits the raw transcript from the in-process child store. System
    /// prompt is freshly rendered.
    pub resume_from: Option<String>,
    /// Runtime overrides for the child agent.
    pub runtime_overrides: SubagentRuntimeOverrides,
    /// Whether this subagent was launched with `run_in_background: true`.
    ///
    /// Controls immediate handle delivery: a background child returns a
    /// `subagent_id` right away and pushes a turn-end notice when its turn
    /// finishes. A foreground child is awaited inline, and the caller drops
    /// the queued notice once it has the result.
    pub run_in_background: bool,
    /// When false, the subagent's turn-end notice is NOT queued for the
    /// parent — used by harness-internal subagents like the goal planner that
    /// the model must never see.
    pub surface_completion: bool,
    pub await_to_completion: bool,
    pub owner: SubagentOwner,
    pub cancel_token: CancellationToken,
}

impl SubagentRequest {
    pub fn is_scheduler_loop(&self) -> bool {
        self.runtime_overrides.loop_task_id.is_some()
    }

    /// The caller blocks on the foreground await budget (neither backgrounded
    /// nor awaiting to completion).
    pub fn awaits_in_foreground(&self) -> bool {
        !self.run_in_background && !self.await_to_completion
    }
}

/// Spawn command envelope owned by the coordinator mailbox.
pub struct SubagentSpawnRequest {
    pub request: Box<SubagentRequest>,
    pub result_tx: oneshot::Sender<SubagentResult>,
}

impl std::ops::Deref for SubagentSpawnRequest {
    type Target = SubagentRequest;

    fn deref(&self) -> &Self::Target {
        &self.request
    }
}

impl SubagentSpawnRequest {
    /// Build and send a reply while the plain request remains borrowable.
    ///
    /// Primarily useful for channel adapters and deterministic test harnesses;
    /// production lifecycle replies are owned by `SubagentCoordinator`.
    #[allow(clippy::result_large_err)] // Err 是领域对象，调用点按值消费；box 会改签名与所有匹配点，单独决策
    pub fn respond_with(
        self,
        build: impl FnOnce(&SubagentRequest) -> SubagentResult,
    ) -> Result<(), SubagentResult> {
        let result = build(&self.request);
        self.result_tx.send(result)
    }
}

/// Per-spawn dynamic runtime overrides for a subagent.
#[derive(Debug, Clone, Default)]
pub struct SubagentRuntimeOverrides {
    /// Scheduler-loop lineage: marks the spawn as belonging to a loop task so
    /// the coordinator can account for it and the workflow host can inherit it.
    pub loop_task_id: Option<String>,
    /// 这次委派允许到哪一层。`None` = 跟角色预设走，不额外收窄。
    ///
    /// 挂进子会话 ctx 的 `"capability"`，与预设工具集取交集，**MCP / 动态包工具
    /// 也受它管**。
    pub capability_mode: Option<crate::tools::capability::CapabilityMode>,
}

// Re-export of [`xai_tool_types::is_not_sentinel`] for existing call sites.

/// Returns `true` if the string looks like a real subagent ID rather than a
/// model-emitted placeholder (`""`, `"null"`, `"none"`, `"undefined"`, whitespace).
pub fn is_valid_resume_id(s: &str) -> bool {
    is_not_sentinel(s)
}

/// Result returned by a completed subagent.
#[derive(Debug, Clone)]
pub struct SubagentResult {
    pub success: bool,
    /// The subagent's final output text.
    ///
    /// Stored as `Arc<str>` so cloning into per-consumer summaries
    /// (`SubagentCompletionSummary`, snapshot status, etc.) is a refcount
    /// bump rather than a full copy. Subagent outputs can be arbitrarily
    /// large (entire transcript), so this matters at scale.
    pub output: Arc<str>,
    /// Error message if the subagent failed.
    pub error: Option<String>,
    /// True if the subagent was cancelled (by user or model).
    /// Distinct from failure — cancellation is intentional.
    pub cancelled: bool,
    pub subagent_id: String,
    /// The child session ID (same as subagent_id for MVP).
    pub child_session_id: String,
    pub tool_calls: u32,
    pub turns: u32,
    pub duration_ms: u64,
    pub tokens_used: u64,
    pub output_tokens_used: u64,
    pub total_tokens_used: u64,
    pub output_usage_incomplete: bool,
    /// Path to the isolated worktree if one was created.
    pub worktree_path: Option<String>,
    /// Set when a blocking subagent exceeded its await budget and was
    /// auto-backgrounded: the child is still running (result via auto-wake /
    /// `get_command_or_subagent_output`), so the tool returns a `task_id` notice
    /// instead of a completion. Never set for natively backgrounded subagents.
    pub backgrounded: bool,
}

impl Default for SubagentResult {
    fn default() -> Self {
        Self {
            success: false,
            output: Arc::from(""),
            error: None,
            cancelled: false,
            subagent_id: String::new(),
            child_session_id: String::new(),
            tool_calls: 0,
            turns: 0,
            duration_ms: 0,
            tokens_used: 0,
            output_tokens_used: 0,
            total_tokens_used: 0,
            output_usage_incomplete: false,
            worktree_path: None,
            backgrounded: false,
        }
    }
}

impl SubagentResult {
    /// Terminal status string: `"cancelled"`, `"completed"`, or `"failed"`.
    pub fn status(&self) -> &'static str {
        if self.cancelled {
            "cancelled"
        } else if self.success {
            "completed"
        } else {
            "failed"
        }
    }
}

// Cancel protocol

#[derive(Debug, Clone)]
pub enum SubagentCancelTarget {
    SubagentId(String),
    /// User Stop / Esc: cancel every live child of the session.
    ParentSession,
    WorkflowRunId(String),
}

/// Cancel request sent by `KillTaskTool` or session cancellation paths.
pub struct SubagentCancelRequest {
    pub parent_session_id: Option<String>,
    pub target: SubagentCancelTarget,
    pub respond_to: oneshot::Sender<SubagentCancelOutcome>,
}

#[derive(Debug, Clone)]
pub enum SubagentCancelOutcome {
    Cancelled,
    AlreadyFinished { status: String },
    NotFound,
}

// Validate-type protocol

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SubagentValidateTypeOutcome {
    Ok,
    /// `available` is the live roster's ids.
    Unknown {
        available: Vec<String>,
    },
    /// Coordinator unreachable; distinct from `Unknown` (the type may be valid).
    ValidationUnavailable,
}

pub struct SubagentValidateTypeRequest {
    pub subagent_type: String,
    pub parent_session_id: String,
    pub respond_to: oneshot::Sender<SubagentValidateTypeOutcome>,
}

// Describe-type protocol

/// Coordinator message enum. Kept exhaustive so every actor command is handled.
pub enum SubagentEvent {
    Spawn(SubagentSpawnRequest),
    Cancel(SubagentCancelRequest),
    /// Cancel a session's children and drain them. `respond_to`, if set,
    /// resolves when no children remain (caller should time-bound the wait).
    TeardownSession {
        parent_session_id: String,
        respond_to: Option<oneshot::Sender<()>>,
    },
    /// Re-open spawns for a parent session after a prior ParentSession stop.
    /// Emitted at the start of each user turn so Stop's late-spawn gate does not
    /// permanently block the next prompt.
    OpenSpawnAdmission {
        parent_session_id: String,
    },
    ValidateType(SubagentValidateTypeRequest),
}
