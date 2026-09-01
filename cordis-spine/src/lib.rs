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
mod skills;
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
    agent_presets, blocked_tool_message, is_shipped, AgentPreset, AgentPresets, PresetOrigin,
    SubagentDef, CORDIS_PRESET_ID, DEFAULT_PRESET_ID, MINIMAL_PRESET_ID, WARDEN_PRESET_ID,
};
pub use agents::{agents, Agent, Agents};
pub use ask_user::{tool_ask_user, Ask, AskPrompt, Question, QuestionOption};
pub use bundle::{
    install_app, install_core, install_fakes, install_foundation, install_spine,
    install_without_llm,
};
pub use compact::{compact, exceeds_threshold, Compact, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT};
pub use config::{
    load_catalog, load_disabled_mcp_tools, load_mcp_servers, persist_disabled_mcp_tools,
    persist_mcp_server_enabled, McpOAuthConfig, McpServer, McpTransport, ModelChoice,
};
pub use context_usage::{
    occupancy_detail, snapshot_context, ContextCategory, ContextSnapshot, OccupancyDetail,
    OccupancyKind,
};
pub use cron::{
    cron, Cron, CronError, CronJob, CronTick, MAX_SCHEDULED_TASKS, RECURRING_TASK_TTL_DAYS,
};
pub use dynamic_runner::{
    builtins_lines, dynamic_runner, DynEcho, DynNote, DynamicRunner, PersistScope, PluginOrigin,
    PluginReference, PromoteReceipt, RhaiBag, RhaiBags, RunMode, DYN_ECHO, DYN_ECHO_TOOL, DYN_NOTE,
    RHAI_FACTORY,
};
pub use error::{Error, Result};
pub use goal::{
    goal_composer_fill, goal_continuation_directive, goal_instruction, goal_offer_addon,
    goal_usage_message, tool_goal, Goal, GOAL_RESERVED_SUBCOMMANDS,
};
pub use jobs::{jobs, tool_jobs, JobSnapshot, Jobs};
pub use llm::{llm, Llm, LlmConfig, LlmMode, Sampler};
pub use loop_plugin::agent_loop;
pub use lsp::{
    lsp_auto_setup, lsp_composer_fill, lsp_status_report, tool_lsp, LspBackendAdapter,
    LspSetupReport, LspSetupScope,
};
pub use mcp::{
    is_mcp_public_name, mcp_client, public_tool_name, split_mcp_public_name, ElicitPrompt,
    Elicitation, Mcp, McpStatus, McpToolStatus, SEARCH_TOOL_NAME, USE_TOOL_NAME,
};
pub use memory::{tool_memory, Memory};
pub use monitor::tool_monitor;
pub use names::{
    AGENTS, AGENT_LOOP, AGENT_PRESETS, ASK, ASK_EVENT, COMPACT, CRON, DYNAMIC_CORDIS_RUNNER, GOAL,
    JOBS, LLM, LLM_STREAM, LSP, MCP, MCP_ELICIT_EVENT, MEMORY, PERMISSIONS, PERMISSION_EVENT,
    PLAN_EVENT, PLAN_MODE, PRE_STEP, PROMPT_ASSEMBLE, RHAI_BAGS, SESSIONS, SESSION_EVENT, SETTINGS,
    SKILLS, SLASH, SUBAGENTS, SYSTEM_PROMPT, TODOS, TOOLS, TOOLS_EXECUTE, TUI_SLOTS, TURN,
    WORKFLOWS,
};
pub use permissions::{permissions, PermissionPrompt, Permissions};
pub use plan_mode::{
    is_plan_file_edit, plan_instruction, plan_mode, plan_system_addon, PlanApprovalPrompt,
    PlanDecision, PlanMode, PlanPhase, PLAN_REL,
};
pub use prompt::{system_prompt, PromptAssembly, SystemPrompt};
pub use runtime::{BoxFuture, Driver, GrokStep, LoopHandle};
pub use sched::{
    expired_task_notice, format_scheduled_task_prompt, format_scheduled_task_reminder,
    interval_to_human, loop_composer_fill, loop_schedule_instruction, loop_usage_message,
    parse_interval, tool_scheduler, LoopFireMode, SCHEDULER_CREATE_TOOL_NAME,
};
pub use session::{sessions, ArchivedSession, Sessions, TokenUsage};
pub use settings::{settings, AppSettings, MermaidEngineKind, PermissionMode};
pub use skills::{skills, tool_skills, SkillInfo, SkillScope, Skills};
pub use slash::{
    slash, slash_name_reserved, tool_slash_arguments, ExtraSlashKind, Slash, SlashEntry,
    RESERVED_SLASH,
};
pub use stream_acc::StreamDelta;
pub use task::{tool_subagent, tool_task, SubagentSnap, Subagents};
pub use todo_write::{tool_todo, TodoItem, TodoStatus, Todos};
pub use tool_cordis::tool_cordis;
pub use tools::{own_registered, tools, workspace_tools, ToolBody, Tools};
pub use tui_slots::{tui_slots, SlotHandler, SlotInfo, SlotKeyResult, TuiSlots};
pub use turn::{turn, TurnControl};
pub use types::{
    LlmOutput, LogEvent, PreStep, PromptRequest, ToolCall, ToolResult, ToolSpec, TurnOutcome,
    UserImage, INTERRUPTED_TOOL_RESULT,
};
pub use usage::{session_usage_block_text, PromptUsage, UsageLedger};
pub use web_fetch::tool_web;
pub use workflow::{
    extra_tool_slash_arguments, tool_workflow, workflow_command_arguments,
    workflow_slash_arguments, WorkflowInfo, WorkflowRunSnap, Workflows, WORKFLOW_TOOL_NAME,
};
