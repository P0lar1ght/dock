//! Copied from grok-build xai-grok-tools grok_build/task/types.rs.
//! `register_resource!` / `educe` / `xai_tool_types` stripped; local enums kept.

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    ListDir,
    Write,
    Move,
    Search,
    Lsp,
    Execute,
    Plan,
    WebSearch,
    WebFetch,
    BackgroundTaskAction,
    WaitTasksAction,
    KillTaskAction,
    List,
    Skill,
    MemorySearch,
    MemoryGet,
    Task,
    EnterPlan,
    ExitPlan,
    AskUser,
    ImageGen,
    VideoGen,
    ImageToVideo,
    ReferenceToVideo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentCapabilityMode {
    ReadOnly,
    ReadWrite,
    Execute,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubagentIsolationMode {
    #[default]
    None,
    Worktree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitMode {
    WaitAny,
    WaitAll,
}

pub fn is_not_sentinel(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && !t.eq_ignore_ascii_case("null")
        && !t.eq_ignore_ascii_case("none")
        && !t.eq_ignore_ascii_case("undefined")
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        return home.join(rest).to_string_lossy().into_owned();
    }
    if path == "~" {
        return std::env::var_os("HOME")
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".into());
    }
    path.to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SubagentOwner {
    #[default]
    Task,
    /// Continuable spawn from the independent `subagent` tool.
    Subagent,
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
            Self::Task | Self::Subagent => None,
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
    /// Parent turn/prompt ID that launched this subagent.
    ///
    /// Used to cancel only the subagents spawned by the currently-cancelled turn,
    /// without affecting background subagents from earlier turns.
    pub parent_prompt_id: Option<String>,
    /// Resume from a previously completed subagent's conversation.
    /// Inherits raw transcript, tool state, and model. System prompt is
    /// freshly rendered.
    pub resume_from: Option<String>,
    /// Explicit working directory for the child session.
    /// Validated at spawn time by the injected child runner.
    pub cwd: Option<String>,
    /// Runtime overrides for the child agent.
    pub runtime_overrides: SubagentRuntimeOverrides,
    /// Whether this subagent was launched with `run_in_background: true`.
    ///
    /// Controls immediate handle delivery and completion surfacing. A
    /// background child still auto-surfaces its completion to the model
    /// (buffered reminder / auto-wake) when `surface_completion` is set —
    /// background does not mean fire-and-forget. Prompt cancellation still
    /// cancels every child owned by that prompt.
    pub run_in_background: bool,
    /// When false, the subagent's completion is NOT buffered for the
    /// between-turn "idle completion" reminder — used by harness-internal
    /// subagents like the goal planner/classifier that the model must never see.
    pub surface_completion: bool,
    pub await_to_completion: bool,
    /// Harness-only: seed child with normalized parent conversation, then append
    /// `prompt`. Not on TaskToolInput. Successful `resume_from` takes precedence.
    pub fork_context: bool,
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
///
/// Optional values inherit from the parent or role default. Explicit values take
/// precedence over role defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModelOverrideProvenance {
    /// Internal harness, role, persona, or config resolution.
    #[default]
    Harness,
    /// A model-facing `Task.model` argument.
    Tool,
}

#[derive(Debug, Clone, Default)]
pub struct SubagentRuntimeOverrides {
    /// Override the model (e.g. "test-model").
    pub model: Option<String>,
    /// Whether `model` came from a model-facing Task call or internal harness logic.
    pub model_override_provenance: ModelOverrideProvenance,
    /// Override reasoning effort (e.g. "low", "medium", "high").
    pub reasoning_effort: Option<String>,
    /// Named persona/SOUL template to apply.
    pub persona: Option<String>,
    /// Capability mode controlling tool access.
    pub capability_mode: Option<SubagentCapabilityMode>,
    /// Isolation mode for child execution environment.
    /// `None` means "use role/persona default" (which itself defaults to `None`/shared workspace).
    pub isolation: Option<SubagentIsolationMode>,
    /// `/goal`-only harness override: the `agent_type` (e.g. `"cursor"`,
    /// `"grok-build-plan"`) whose `AgentDefinition` decides the child's harness
    /// flavor — system prompt + toolset — applied
    /// REGARDLESS of the parent agent (so a session can pin a
    /// compat-harness verifier and vice versa).
    /// Orthogonal to `subagent_type`, which still selects the toolset-role
    /// (implementer vs explorer). `None` for every non-goal spawn ⇒ the parent
    /// agent decides the flavor (unchanged behavior).
    pub harness_agent_type: Option<String>,
    pub completion_output_cap: Option<usize>,
    pub spawn_depth: Option<u32>,
    pub output_token_budget: Option<u64>,
    pub output_schema: Option<serde_json::Value>,
    pub loop_task_id: Option<String>,
}

// Re-export of [`xai_tool_types::is_not_sentinel`] for existing call sites.

/// Sanitize a model-emitted `cwd` argument for the `task` tool.
///
/// Strips stray surrounding quote/backtick characters (matched or unmatched),
/// trims whitespace, expands a leading `~` to the user's home directory, and
/// rejects sentinel placeholders (`""`, `"null"`, `"none"`, `"undefined"`).
///
/// Returns `Some(cleaned)` for a usable path, `None` if the value should be
/// treated as absent. Shared by the tool layer (`task::mod`) and the
/// defense-in-depth check in `xai-grok-shell`'s subagent coordinator.
pub fn sanitize_cwd_value(s: &str) -> Option<String> {
    let unquoted = s.trim().trim_matches(['"', '\'', '`']);
    // Re-trim after stripping quotes: this trim flows into the returned
    // value; the trim inside `is_not_sentinel` does not.
    let cleaned = unquoted.trim();
    if !is_not_sentinel(cleaned) {
        return None;
    }
    Some(expand_tilde(cleaned))
}

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

// Query protocol

/// Query sent by `TaskOutputTool` to the shared coordinator actor.
pub struct SubagentQueryRequest {
    /// The subagent ID to look up.
    pub subagent_id: String,
    /// Restrict the lookup to children owned by this parent session.
    pub parent_session_id: Option<String>,
    /// If true, coordinator waits for completion (up to timeout) before responding.
    pub block: bool,
    /// Max wait time in ms when blocking. Default 30s.
    pub timeout_ms: Option<u64>,
    /// Oneshot for the coordinator to send back the snapshot.
    pub respond_to: oneshot::Sender<Option<SubagentSnapshot>>,
}

pub struct SubagentLoopUnitActiveRequest {
    pub task_id: String,
    pub respond_to: oneshot::Sender<bool>,
}

/// Point-in-time snapshot of a subagent's state.
/// Returned by the coordinator in response to a `SubagentQueryRequest`.
#[derive(Debug, Clone)]
pub struct SubagentSnapshot {
    pub subagent_id: String,
    pub description: String,
    pub subagent_type: String,
    pub status: SubagentSnapshotStatus,
    /// Wall-clock start time (epoch ms).
    pub started_at_epoch_ms: u64,
    /// Elapsed wall-clock time in milliseconds.
    pub duration_ms: u64,
    /// Persona used by this subagent, if any.
    pub persona: Option<String>,
}

/// Lifecycle metadata returned to shell presentation and extension callers.
#[derive(Debug, Clone)]
pub struct SubagentInspection {
    pub snapshot: SubagentSnapshot,
    pub parent_session_id: String,
    pub child_session_id: String,
    pub fork_parent_prompt_id: Option<String>,
    pub resumed_from: Option<String>,
}

impl SubagentSnapshot {
    /// Whether the child is still in flight (initializing or running) — the
    /// shared liveness rule every driver's blocking query loops on.
    pub fn is_running(&self) -> bool {
        matches!(
            self.status,
            SubagentSnapshotStatus::Running { .. } | SubagentSnapshotStatus::Initializing
        )
    }
}

/// Status of a subagent snapshot.
#[derive(Debug, Clone)]
pub enum SubagentSnapshotStatus {
    /// Subagent is being set up (creating worktree, resolving config, spawning
    /// session). Queries during this phase should report the subagent as
    /// initializing rather than "not found".
    Initializing,
    /// Child session is still running. Fields are populated from the child
    /// session's `SessionSignals` snapshot at query time (pull-based).
    Running {
        /// Number of completed turns so far.
        turn_count: u32,
        /// Total tool calls executed so far.
        tool_call_count: u32,
        /// Current tokens used in the context window.
        tokens_used: u64,
        /// Total context window capacity (tokens).
        context_window_tokens: u64,
        /// Context window usage as a percentage (0–100).
        context_usage_pct: u8,
        /// Distinct tool names called so far (e.g. `["bash", "read_file"]`).
        tools_used: Vec<String>,
        /// Number of errors encountered so far.
        error_count: u32,
    },
    /// Child session completed successfully.
    Completed {
        output: String,
        tool_calls: u32,
        turns: u32,
        worktree_path: Option<String>,
    },
    /// Child session failed or crashed.
    Failed { error: String },
    /// Child session was cancelled (by user or model).
    Cancelled { reason: Option<String> },
}

impl SubagentSnapshotStatus {
    /// Returns `true` for terminal states: `Completed`, `Failed`, `Cancelled`.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled { .. }
        )
    }
}

// Cancel protocol

#[derive(Debug, Clone)]
pub enum SubagentCancelTarget {
    SubagentId(String),
    /// Turn-scoped cancel (soft cancel / max-turns).
    ParentPromptId(String),
    /// User Stop / Esc with cancel_subagents — prior-turn background too.
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

/// Summary of a completed subagent, used for between-turn delivery.
/// Session ownership lives on the coordinator's `BufferedCompletion` wrapper;
/// drains are scoped there, so delivered summaries carry no owner field.
#[derive(Debug, Clone)]
pub struct SubagentCompletionSummary {
    pub subagent_id: String,
    pub subagent_type: String,
    pub description: String,
    pub success: bool,
    pub duration_ms: u64,
    pub tool_calls: u32,
    pub turns: u32,
    /// The subagent's final output text. Refcount-shared with
    /// `SubagentResult.output` (no allocation on the path from coordinator
    /// to between-turn drain).
    ///
    /// Surfaced inline in completion notifications when the parent agent's
    /// toolset has no `BackgroundTaskAction` tool. Toolsets
    /// that DO have a polling tool keep the existing metadata-only line +
    /// "Use get_task_output(...)" pointer.
    pub output: Arc<str>,
}

/// Multi-wait request: block until one or all of the listed subagents finish.
pub struct SubagentMultiWaitRequest {
    pub subagent_ids: Vec<String>,
    pub mode: WaitMode,
    pub timeout_ms: Option<u64>,
    pub respond_to: oneshot::Sender<Vec<Option<SubagentSnapshot>>>,
}

/// Request to drain buffered completion summaries.
pub struct SubagentCompletionsRequest {
    pub parent_session_id: Option<String>,
    pub suppress_ids: Vec<String>,
    pub respond_to: oneshot::Sender<Vec<SubagentCompletionSummary>>,
}

/// Live subagents and whether finished-subagent usage is still missing from the parent bill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubagentOutstandingReply {
    /// Turn-blocking (foreground) children still pending or active.
    pub live_ids: Vec<String>,
    /// A background child is still running: its spend is missing from the
    /// prompt report but reaches the session ledger when it finishes.
    pub background_live: bool,
    pub subagent_usage_not_applied: bool,
}

pub struct SubagentOutstandingRequest {
    pub parent_session_id: String,
    pub prompt_id: String,
    pub respond_to: oneshot::Sender<SubagentOutstandingReply>,
}

/// Clear sticky incomplete after freeze/cancel has snapshotted the bill.
#[derive(Debug)]
pub struct SubagentClearUsageNotAppliedRequest {
    pub parent_session_id: String,
    pub prompt_id: String,
}

/// Mark sticky incomplete for a parent prompt (usage apply failed).
pub struct SubagentMarkUsageNotAppliedRequest {
    pub parent_session_id: String,
    pub prompt_id: String,
    pub respond_to: oneshot::Sender<()>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubagentRegistryCounts {
    pub pending: usize,
    pub active: usize,
    pub completed: usize,
    /// Spawns parked at the session's concurrent limit, not yet started.
    pub queued: usize,
}

pub struct SubagentRegistryCountsRequest {
    pub respond_to: oneshot::Sender<SubagentRegistryCounts>,
}

/// Request for full metadata plus a resolved progress snapshot.
pub struct SubagentInspectRequest {
    pub subagent_id: String,
    pub parent_session_id: Option<String>,
    pub respond_to: oneshot::Sender<Option<SubagentInspection>>,
}

/// Request for all running children owned by one parent session.
pub struct SubagentListRunningRequest {
    pub parent_session_id: String,
    pub respond_to: oneshot::Sender<Vec<SubagentInspection>>,
}

/// Fork/resume provenance retained by the shared coordinator.
#[derive(Debug, Clone, Default)]
pub struct SubagentProvenance {
    pub fork_parent_prompt_id: Option<String>,
    pub resumed_from: Option<String>,
}

/// Reference to a child spawned during one parent prompt.
#[derive(Debug, Clone)]
pub struct SpawnedSubagentRef {
    pub subagent_id: String,
    pub child_session_id: String,
    pub subagent_type: String,
    pub description: String,
    pub persona: Option<String>,
    pub resumed_from: Option<String>,
}

/// Request for prompt-scoped spawned-child references.
pub struct SubagentSpawnedRefsRequest {
    pub parent_session_id: String,
    pub prompt_id: String,
    pub respond_to: oneshot::Sender<Vec<SpawnedSubagentRef>>,
}

/// In-memory source data used by a runtime adapter to resume a child.
#[derive(Debug, Clone)]
pub struct SubagentResumeSource {
    pub subagent_id: String,
    pub child_session_id: String,
    pub child_cwd: String,
    pub worktree_path: Option<String>,
    pub snapshot_ref: Option<String>,
    pub subagent_type: String,
    pub persona: Option<String>,
    pub model_id: Option<String>,
}

/// Result of a resume-source lookup.
#[derive(Debug, Clone)]
pub enum SubagentResumeLookup {
    Active,
    Completed(SubagentResumeSource),
    Missing,
}

// Validate-type protocol

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SubagentValidateTypeOutcome {
    Ok,
    /// `available` is sorted by `str::cmp` and filtered by `[subagents.toggle]`.
    Unknown {
        available: Vec<String>,
    },
    Disabled,
    NotAllowed {
        allowed: Vec<String>,
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

/// Outcome of a `describe_subagent_type` round-trip.
///
/// Mirrors [`SubagentValidateTypeOutcome`] but, on success, additionally
/// carries the resolved toolset summary (tool names + capability flags)
/// so the parent can gate per-role capability and render per-role prompts
/// WITHOUT spawning the toolset. The non-`Ok` variants are 1:1 with the
/// validate outcomes (config-bug cases) plus `Unavailable` (infra
/// flakiness), so a caller maps every variant to a fail-open reason.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SubagentDescribeOutcome {
    Ok(SubagentTypeSummary),
    /// The type does not resolve to an agent definition. `available` is
    /// sorted by `str::cmp` and filtered by `[subagents.toggle]`.
    Unknown {
        available: Vec<String>,
    },
    /// The type resolves but is not on the parent's allow-list.
    NotAllowed {
        allowed: Vec<String>,
    },
    /// The type resolves but is disabled via `[subagents.toggle]`.
    Disabled,
    /// Coordinator unreachable / responder dropped / timed out — treat as
    /// fail-open (the type may be valid; the description just could not be
    /// obtained).
    Unavailable,
}

/// Resolved toolset summary for a subagent type.
///
/// Built by the coordinator from the type's `AgentDefinition` AFTER the
/// same parent-dependent toolset re-selection a real spawn applies, so a
/// parent's described tool names match what the child would
/// actually get. The capability booleans key on the exact `ToolKind`
/// variants used by the per-role gates (`Search` for grep, `Execute` for
/// terminal/bash — there is no `Grep`/`Bash` variant).
#[derive(Debug, Clone, Default)]
pub struct SubagentTypeSummary {
    /// Client-facing tool name per [`ToolKind`](ToolKind),
    /// derived exactly like the finalize-time `kind_to_name` map:
    /// `ToolConfig::resolve_client_name(&entry.id)` (the `name_override`
    /// when set, else the unqualified tool id). First tool per kind wins,
    /// matching `FinalizedToolset`.
    pub tool_names: std::collections::HashMap<ToolKind, String>,
    /// The toolset has a [`ToolKind::Read`](ToolKind::Read) tool.
    pub can_read: bool,
    /// The toolset has a [`ToolKind::Search`](ToolKind::Search)
    /// tool (grep maps to `Search`).
    pub can_search: bool,
    /// The toolset has a [`ToolKind::Execute`](ToolKind::Execute)
    /// tool (terminal/bash maps to `Execute`).
    pub can_execute: bool,
}

pub struct SubagentDescribeRequest {
    pub subagent_type: String,
    /// `/goal`-only harness override mirrored from
    /// [`SubagentRuntimeOverrides::harness_agent_type`]: the coordinator
    /// resolves the toolset for `(subagent_type, harness_agent_type)` so the
    /// per-role capability gate + prompt tool names reflect the harness the
    /// spawn will actually run on. `None` ⇒ the parent agent decides the flavor
    /// (unchanged behavior).
    pub harness_agent_type: Option<String>,
    pub parent_session_id: String,
    pub respond_to: oneshot::Sender<SubagentDescribeOutcome>,
}

/// Coordinator message enum. Kept exhaustive so every actor command is handled.
pub enum SubagentEvent {
    Spawn(SubagentSpawnRequest),
    Query(SubagentQueryRequest),
    Cancel(SubagentCancelRequest),
    ListActive(SubagentListActiveRequest),
    ListRunning(SubagentListRunningRequest),
    Completions(SubagentCompletionsRequest),
    /// Cancel children of `parent_session_id` and drop its buffered completions.
    /// `respond_to`, if set, resolves when no children remain (caller should
    /// time-bound the wait).
    TeardownSession {
        parent_session_id: String,
        respond_to: Option<oneshot::Sender<()>>,
    },
    /// Re-open Task spawns for a parent session after a prior ParentSession stop.
    /// Emitted at the start of each user turn so Stop's late-spawn gate does not
    /// permanently block the next prompt.
    OpenSpawnAdmission {
        parent_session_id: String,
    },
    Outstanding(SubagentOutstandingRequest),
    ClearUsageNotApplied(SubagentClearUsageNotAppliedRequest),
    MarkUsageNotApplied(SubagentMarkUsageNotAppliedRequest),
    RegistryCounts(SubagentRegistryCountsRequest),
    Inspect(SubagentInspectRequest),
    SpawnedRefs(SubagentSpawnedRefsRequest),
    ValidateType(SubagentValidateTypeRequest),
    DescribeType(SubagentDescribeRequest),
    LoopUnitActive(SubagentLoopUnitActiveRequest),
}

// Resource types

/// One shared channel to the subagent coordinator, cloned into each session.
#[derive(Clone)]
pub struct SubagentEventSender(pub mpsc::UnboundedSender<SubagentEvent>);

// Active subagent listing (compaction)

/// Lightweight summary of a running subagent.
///
/// The shared coordinator produces this through the channel protocol, and the
/// compaction pipeline consumes it through `RunningSubagentSummary`.
#[derive(Debug, Clone)]
pub struct ActiveSubagentSummary {
    /// The subagent's unique ID (same ID used by `get_task_output` / `kill_task`).
    pub subagent_id: String,
    /// The agent type name (e.g. "Explore", "general-purpose", "Plan").
    pub subagent_type: String,
    /// Human-readable description of what the subagent is doing.
    pub description: String,
    /// Wall-clock elapsed time since the subagent was spawned, in milliseconds.
    pub elapsed_ms: u64,
}

/// Request to list currently-running subagents for a specific parent session.
///
/// Sent by the compaction pipeline in `SessionActor::run_compact_inner()`.
/// Handled by the shared coordinator actor.
pub struct SubagentListActiveRequest {
    pub parent_session_id: String,
    pub respond_to: oneshot::Sender<Vec<ActiveSubagentSummary>>,
}

/// Current nesting depth (top-level = 0; child = parent + 1).
#[derive(Debug, Clone)]
pub struct SubagentDepthCounter(pub u32);

/// Host-injected max nesting depth; absent → [`super::MAX_SUBAGENT_DEPTH`].
#[derive(Debug, Clone, Copy)]
pub struct MaxSubagentDepth(pub u32);

/// Session-scoped validator for model-facing `Task.model` arguments.
///
/// Returns an error message for an invalid slug and `None` for a valid slug.
/// The closure reads the live model catalog so refreshes apply without rebuilding
/// the tool bridge.
type TaskModelValidationFn = dyn Fn(&str) -> Option<String> + Send + Sync;

#[derive(Clone)]
pub struct TaskModelValidator(Arc<TaskModelValidationFn>);

impl TaskModelValidator {
    pub fn new(validate: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        Self(Arc::new(validate))
    }

    pub fn error_for(&self, model: &str) -> Option<String> {
        (self.0)(model)
    }
}

impl std::fmt::Debug for TaskModelValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskModelValidator").finish()
    }
}

/// Carries the current session ID so TaskTool can set `parent_session_id`
/// on the `SubagentRequest`.
#[derive(Debug, Clone)]
pub struct SessionIdResource(pub String);

/// Host-owned RAII token for an interruptible foreground wait.
pub trait ForegroundWaitGuard: Send {}

impl<T: Send> ForegroundWaitGuard for T {}

type ForegroundWaitFactory = dyn Fn() -> Box<dyn ForegroundWaitGuard> + Send + Sync;

/// Factory injected by hosts that expose a send-now wait window.
#[derive(Clone)]
pub struct SubagentForegroundWait(Arc<ForegroundWaitFactory>);

impl SubagentForegroundWait {
    pub fn new(factory: impl Fn() -> Box<dyn ForegroundWaitGuard> + Send + Sync + 'static) -> Self {
        Self(Arc::new(factory))
    }

    pub fn enter(&self) -> Box<dyn ForegroundWaitGuard> {
        (self.0)()
    }
}

impl std::fmt::Debug for SubagentForegroundWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentForegroundWait").finish()
    }
}

/// Carries the current parent prompt/turn ID for TaskTool subagent scoping.
///
/// Set by xai-grok-shell immediately before a prompt turn begins executing so
/// subagents launched during that turn can be cancelled together if the user
/// aborts the turn.
#[derive(Debug, Clone)]
pub struct CurrentPromptIdResource(pub String);

/// True while a `/goal` loop is active. Set by xai-grok-shell at turn start.
/// When true, `TaskCompletionReminder` suppresses bg-task completion
/// reminders (marking them reported) so async "task completed" nudges don't
/// pull a weak model off the goal continuation (e.g. relaunching a killed
/// dev server).
#[derive(Debug, Clone, Copy, Default)]
pub struct GoalLoopActive(pub bool);
