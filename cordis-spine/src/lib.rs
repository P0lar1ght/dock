//! Plugin spine: five named services (DSH) + a Grok-shaped one-step turn.
//!
//! Services: `sessions` · `llm` · `tools` · `systemPrompt` · `agents`.
//! The loop injects those five and provides `agentLoop`. Swap the loop by
//! disposing that one plugin and mounting another that injects the same five.

mod acp;
mod agents;
mod ask_user;
mod bundle;
mod chat_chunk;
mod config;
mod cron;
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
mod stream_acc;
mod task;
mod todo_write;
mod tools;
mod turn;
mod types;
mod web_fetch;
mod workspace;

pub use acp::PermissionOptionKind;
pub use agents::{agents, Agent, Agents};
pub use ask_user::{tool_ask_user, Ask, AskPrompt, Question, QuestionOption};
pub use bundle::{
    install_app, install_core, install_fakes, install_foundation, install_spine, install_without_llm,
};
pub use config::{load_catalog, load_mcp_servers, McpServer, ModelChoice};
pub use cron::{cron, Cron, CronJob};
pub use error::{Error, Result};
pub use goal::{
    goal_instruction, goal_usage_message, tool_goal, Goal, GOAL_RESERVED_SUBCOMMANDS,
};
pub use jobs::{jobs, tool_jobs, JobSnapshot, Jobs};
pub use llm::{llm, Llm, LlmConfig, LlmMode, Sampler};
pub use loop_plugin::agent_loop;
pub use lsp::tool_lsp;
pub use mcp::{mcp_client, Mcp, McpStatus};
pub use memory::tool_memory;
pub use monitor::tool_monitor;
pub use names::{
    AGENT_LOOP, AGENTS, ASK, ASK_EVENT, CRON, GOAL, JOBS, LLM, LLM_STREAM, LSP, MCP, MEMORY,
    PERMISSIONS, PERMISSION_EVENT, PLAN_MODE, PRE_STEP, PROMPT_ASSEMBLE, SESSIONS, SESSION_EVENT,
    SETTINGS, SUBAGENTS, SYSTEM_PROMPT, TODOS, TOOLS, TOOLS_EXECUTE, TURN,
};
pub use stream_acc::StreamDelta;
pub use turn::{turn, TurnControl};
pub use permissions::{permissions, PermissionPrompt, Permissions};
pub use plan_mode::{plan_mode, PlanMode};
pub use prompt::{system_prompt, SystemPrompt};
pub use runtime::{BoxFuture, Driver, GrokStep, LoopHandle};
pub use sched::tool_scheduler;
pub use session::{sessions, ArchivedSession, Sessions, TokenUsage};
pub use settings::{settings, AppSettings, MermaidEngineKind, PermissionMode};
pub use task::{tool_task, SubagentSnap, Subagents};
pub use todo_write::{tool_todo, TodoItem, TodoStatus, Todos};
pub use tools::{own_registered, tools, workspace_tools, ToolBody, Tools};
pub use types::{
    LlmOutput, LogEvent, PreStep, PromptRequest, ToolCall, ToolResult, ToolSpec, TurnOutcome,
    UserImage,
};
pub use web_fetch::tool_web;
