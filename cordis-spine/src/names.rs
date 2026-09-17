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
/// Subagent coordinator (`ctx.subagents`). `tool-task` provides it; the
/// spawn + mailbox tools live-look it.
pub const SUBAGENTS: &str = "subagents";
/// 本会话的能力档位（`crate::tools::capability::CapabilityMode`）。**只挂在受限
/// 子代理的隔离 ctx 上**：主会话没有这一项，等于不设限。工具允许名单查它，且
/// 它压在 MCP / 动态包的允许名单豁免之上。
pub const CAPABILITY: &str = "capability";
/// Local memory files (`ctx.memory`).
pub const MEMORY: &str = "memory";
/// External Chromium cockpit (`ctx.browser`). chromiumoxide CDP + live `/browser` TUI cockpit (lean a11y refs).
pub const BROWSER: &str = "browser";
/// Host desktop CUA cockpit (`ctx.computer`). Thin status over cua-driver MCP; no key/mouse kernel.
pub const COMPUTER: &str = "computer";
/// Local goal progress (`ctx.goal`). `/goal` + `update_goal`.
pub const GOAL: &str = "goal";
/// Language-server client (`ctx.lsp`). Fail-open.
pub const LSP: &str = "lsp";
/// Agent skills catalog (`ctx.skills`). Fail-open.
pub const SKILLS: &str = "skills";
/// Rhai workflow coordinator (`ctx.workflows`). `workflow` + `/workflow`.
pub const WORKFLOWS: &str = "workflows";
/// Extra prompt-bar slash commands. TUI live-looks it; builtins stay in CATALOG.
pub const SLASH: &str = "slash";
/// Per-preset persona + tool allowlist. TUI `/preset` live-looks it.
pub const AGENT_PRESETS: &str = "agentPresets";
pub const DYNAMIC_CORDIS_RUNNER: &str = "dynamicCordisRunner";
/// TUI slots owned by dynamic packages. TUI live-looks this; Rhai `register_slot`.
pub const TUI_SLOTS: &str = "tui.slots";
/// In-memory maps `provide`d by Rhai host halves.
pub const RHAI_BAGS: &str = "rhaiBags";
/// Prompt-section inventory + occupancy snapshot (`ContextBook`).
pub const CONTEXT: &str = "context";
pub const SYSTEM_PROMPT: &str = "systemPrompt";
pub const AGENTS: &str = "agents";
pub const AGENT_LOOP: &str = "agentLoop";
pub const SETTINGS: &str = "settings";
/// Conversation history compaction (`/compact` + auto at 85% of the window).
pub const COMPACT: &str = "compact";
pub const CRON: &str = "cron";
/// 跨 cwd 的会话名册（`Roster`）。`sessions` 是**本页**的会话日志，两者不同。
pub const ROSTER: &str = "roster";
pub const PERMISSIONS: &str = "permissions";
pub const TURN: &str = "turn";

pub const PRE_STEP: &str = "agent/pre-step";
/// Before every sampling step of a turn — the "watch the turn as it runs"
/// grain. Payload [`crate::StepStart`].
pub const STEP_START: &str = "agent/step-start";
/// Model stopped calling tools (or ran out of steps) — last chance to say the
/// turn is not done. Payload [`crate::TurnEnd`].
pub const TURN_END: &str = "agent/turn-end";
pub const PROMPT_ASSEMBLE: &str = "system-prompt/assemble";
pub const LLM_STREAM: &str = "llm/stream";
/// Before the plan gate, the permission gate and the tool body. Payload
/// [`cordis_base::types::PreExecute`] — rewrite / redirect / deny.
pub const TOOLS_PRE_EXECUTE: &str = "tools/pre-execute";
/// After the tool body. Payload [`cordis_base::types::ToolResult`] — inspect or
/// replace what already happened.
pub const TOOLS_EXECUTE: &str = "tools/execute";
pub const SESSION_EVENT: &str = "session/event";
pub const PERMISSION_EVENT: &str = "permissions/pending";
pub const ASK_EVENT: &str = "ask/pending";
pub const PLAN_EVENT: &str = "plan/pending";
/// MCP `elicitation/create` waiting on the TUI.
pub const MCP_ELICIT_EVENT: &str = "mcp/elicit";
