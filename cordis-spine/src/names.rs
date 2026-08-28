//! DSH ctx keys for the five spine services plus the concrete loop.

pub const SESSIONS: &str = "sessions";
pub const LLM: &str = "llm";
pub const TOOLS: &str = "tools";
/// Session todo list. `tool-todo` provides it; TUI live-looks it.
pub const TODOS: &str = "todos";
/// Background jobs (`ctx.jobs`). Bash background + `tool-jobs` consume it.
pub const JOBS: &str = "jobs";
/// `ask_user_question` seam (`ctx.userQuestions`).
pub const ASK: &str = "ask";
/// Plan collaboration state (`ctx.planMode`).
pub const PLAN_MODE: &str = "planMode";
/// MCP connection status for `/mcps`. Tools register into `"tools"`.
pub const MCP: &str = "mcp";
/// Subagent coordinator (`ctx.subagents`). `task` spawn + get/kill/wait.
pub const SUBAGENTS: &str = "subagents";
/// Local memory files (`ctx.memory`).
pub const MEMORY: &str = "memory";
/// Local goal progress (`ctx.goal`). `/goal` + `update_goal`.
pub const GOAL: &str = "goal";
/// Language-server client (`ctx.lsp`). Fail-open.
pub const LSP: &str = "lsp";
pub const SYSTEM_PROMPT: &str = "systemPrompt";
pub const AGENTS: &str = "agents";
pub const AGENT_LOOP: &str = "agentLoop";
pub const SETTINGS: &str = "settings";
pub const CRON: &str = "cron";
pub const PERMISSIONS: &str = "permissions";
pub const TURN: &str = "turn";

pub const PRE_STEP: &str = "agent/pre-step";
pub const PROMPT_ASSEMBLE: &str = "system-prompt/assemble";
pub const LLM_STREAM: &str = "llm/stream";
pub const TOOLS_EXECUTE: &str = "tools/execute";
pub const SESSION_EVENT: &str = "session/event";
pub const PERMISSION_EVENT: &str = "permissions/pending";
pub const ASK_EVENT: &str = "ask/pending";
