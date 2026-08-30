//! Plugin spine: five named services (DSH) + a Grok-shaped one-step turn.
//!
//! Services: `sessions` · `llm` · `tools` · `systemPrompt` · `agents`.
//! The loop injects those five and provides `agentLoop`. Swap the loop by
//! disposing that one plugin and mounting another that injects the same five.

mod acp;
mod agent_presets;
mod agents;
mod ask_user;
mod bundle;
mod chat_chunk;
mod compact;
mod config;
mod context_usage;
mod cron;
mod dynamic_runner;
mod error;
mod goal;
mod http;
mod jobs;
mod llm;
mod loop_plugin;
mod lsp;
mod mcp;
mod memory;
mod monitor;
mod names;
mod permissions;
mod plan_mode;
mod prompt;
mod runtime;
mod sched;
mod session;
mod settings;
mod slash;
mod stream_acc;
mod task;
mod todo_write;
mod tool_cordis;
mod tools;
mod tui_slots;
mod turn;
mod types;
mod usage;
mod web_fetch;
mod workflow;
mod workspace;

pub use acp::PermissionOptionKind;
pub use agent_presets::{
    AgentPreset, AgentPresets, CORDIS_PRESET_ID, DEFAULT_PRESET_ID, MINIMAL_PRESET_ID,
    PresetOrigin, SubagentDef, WARDEN_PRESET_ID, agent_presets, blocked_tool_message, is_shipped,
};
pub use agents::{Agent, Agents, agents};
pub use ask_user::{Ask, AskPrompt, Question, QuestionOption, tool_ask_user};
pub use bundle::{
    install_app, install_core, install_fakes, install_foundation, install_spine,
    install_without_llm,
};
pub use compact::{Compact, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT, compact, exceeds_threshold};
pub use config::{
    McpOAuthConfig, McpServer, McpTransport, ModelChoice, load_catalog, load_disabled_mcp_tools,
    load_mcp_servers, persist_disabled_mcp_tools, persist_mcp_server_enabled,
};
pub use context_usage::{
    ContextCategory, ContextSnapshot, OccupancyDetail, OccupancyKind, occupancy_detail,
    snapshot_context,
};
pub use cron::{
    Cron, CronError, CronJob, CronTick, MAX_SCHEDULED_TASKS, RECURRING_TASK_TTL_DAYS, cron,
};
pub use dynamic_runner::{
    DYN_ECHO, DYN_ECHO_TOOL, DYN_NOTE, DynEcho, DynNote, DynamicRunner, PluginReference,
    RHAI_FACTORY, RhaiBag, RhaiBags, RunMode, builtins_lines, dynamic_runner,
};
pub use error::{Error, Result};
pub use goal::{
    GOAL_RESERVED_SUBCOMMANDS, Goal, goal_composer_fill, goal_continuation_directive,
    goal_instruction, goal_offer_addon, goal_usage_message, tool_goal,
};
pub use jobs::{JobSnapshot, Jobs, jobs, tool_jobs};
pub use llm::{Llm, LlmConfig, LlmMode, Sampler, llm};
pub use loop_plugin::agent_loop;
pub use lsp::tool_lsp;
pub use mcp::{
    ElicitPrompt, Elicitation, Mcp, McpStatus, McpToolStatus, is_mcp_public_name, mcp_client,
    public_tool_name,
};
pub use memory::tool_memory;
pub use monitor::tool_monitor;
pub use names::{
    AGENT_LOOP, AGENT_PRESETS, AGENTS, ASK, ASK_EVENT, COMPACT, CRON, DYNAMIC_CORDIS_RUNNER, GOAL,
    JOBS, LLM, LLM_STREAM, LSP, MCP, MCP_ELICIT_EVENT, MEMORY, PERMISSION_EVENT, PERMISSIONS,
    PLAN_EVENT, PLAN_MODE, PRE_STEP, PROMPT_ASSEMBLE, RHAI_BAGS, SESSION_EVENT, SESSIONS, SETTINGS,
    SLASH, SUBAGENTS, SYSTEM_PROMPT, TODOS, TOOLS, TOOLS_EXECUTE, TUI_SLOTS, TURN, WORKFLOWS,
};
pub use permissions::{PermissionPrompt, Permissions, permissions};
pub use plan_mode::{
    PLAN_REL, PlanApprovalPrompt, PlanDecision, PlanMode, PlanPhase, is_plan_file_edit,
    plan_instruction, plan_mode, plan_system_addon,
};
pub use prompt::{SystemPrompt, system_prompt};
pub use runtime::{BoxFuture, Driver, GrokStep, LoopHandle};
pub use sched::{
    LoopFireMode, SCHEDULER_CREATE_TOOL_NAME, expired_task_notice, format_scheduled_task_prompt,
    format_scheduled_task_reminder, interval_to_human, loop_composer_fill,
    loop_schedule_instruction, loop_usage_message, parse_interval, tool_scheduler,
};
pub use session::{ArchivedSession, Sessions, TokenUsage, sessions};
pub use settings::{AppSettings, MermaidEngineKind, PermissionMode, settings};
pub use slash::{
    ExtraSlashKind, RESERVED_SLASH, Slash, SlashEntry, slash, slash_name_reserved,
    tool_slash_arguments,
};
pub use stream_acc::StreamDelta;
pub use task::{SubagentSnap, Subagents, tool_subagent, tool_task};
pub use todo_write::{TodoItem, TodoStatus, Todos, tool_todo};
pub use tool_cordis::tool_cordis;
pub use tools::{ToolBody, Tools, own_registered, tools, workspace_tools};
pub use tui_slots::{SlotHandler, SlotInfo, SlotKeyResult, TuiSlots, tui_slots};
pub use turn::{TurnControl, turn};
pub use types::{
    INTERRUPTED_TOOL_RESULT, LlmOutput, LogEvent, PreStep, PromptRequest, ToolCall, ToolResult,
    ToolSpec, TurnOutcome, UserImage,
};
pub use usage::{PromptUsage, UsageLedger, session_usage_block_text};
pub use web_fetch::tool_web;
pub use workflow::{WorkflowRunSnap, Workflows, tool_workflow};
