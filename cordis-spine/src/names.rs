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
/// 本会话的能力档位（`crate::agent::capability::CapabilityMode`）。**只挂在受限
/// 子代理的隔离 ctx 上**：主会话没有这一项，等于不设限。工具允许名单查它，且
/// 它压在 MCP / 动态包的允许名单豁免之上。
pub const CAPABILITY: &str = "capability";
/// 这次委派的采样覆写（`crate::host::settings::ModelOverride`）。**只挂在子代理
/// 的隔离 ctx 上**：主会话没有这一项，采样照 `"settings"` 走。
pub const MODEL_OVERRIDE: &str = "model-override";
/// Local memory files (`ctx.memory`).
pub const MEMORY: &str = "memory";
/// Browser cockpit (`ctx.browser`): status of the built-in `browser` MCP + headed pref for `/browser`.
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
/// 同一条会话事件，再带上发出它的**页**（`main` / `main#N`，载荷
/// [`crate::PageLogEvent`]）。`session/event` 不分 realm、载荷也不带身份，任何一页
/// 说话根上都收得到；要按页区分的监听者（网关的线程投影）订这一条。
pub const SESSION_PAGE_EVENT: &str = "session/page-event";
/// 一页的一轮跑完了（成功、出错、被取消都算），载荷 [`crate::PageTurnEnd`]。
/// 流式事件本身分不出「这一段是不是最后一段」，要知道一轮何时结束的监听者
/// （网关的 `turn/completed`）订这一条。
pub const SESSION_TURN_END: &str = "session/turn-end";
/// 一页的压缩进展变了（开始、换阶段、摘要又吐了一段、结束），载荷
/// [`crate::PageCompaction`]。摘要计数最多 250ms 一条。
pub const SESSION_COMPACTION: &str = "session/compaction";
/// 子代理会话发了一条事件，载荷 [`crate::ChildLogEvent`]。子代理的会话不发
/// `session/event` / `session/page-event`（TUI 只画主会话），要看子代理过程的监听者
/// （网关的子代理投影）订这一条。
pub const SESSION_CHILD_EVENT: &str = "session/child-event";
/// 一页的实时日志被整份换掉了（`Sessions::clear` / `restore`），载荷是那一页的身份
/// （`String`）。谁换的都发——终端 `/new`、网关 `thread/start`、恢复会话——投影这一页
/// 的监听者（网关的线程）据此按当前会话重建。子代理不发。
pub const SESSION_RESET: &str = "session/reset";
/// 一个子代理出现了或状态变了（运行 / 空闲 / 结束），载荷 [`crate::SubagentChanged`]。
/// 只带 id；当前状态在监听里 `Subagents::snapshot` 现取。
pub const SUBAGENT_CHANGED: &str = "subagent/changed";
/// 定时任务（`"cron"`）变了：增删改、触发、或读到别的 Dock 进程的改动。载荷 `()`；
/// 现状在监听里 `Cron::list` 现取。`cordis-app` 的定时任务驱动每秒对一次账再发。
pub const SCHEDULE_CHANGED: &str = "schedule/changed";
pub const PERMISSION_EVENT: &str = "permissions/pending";
pub const ASK_EVENT: &str = "ask/pending";
pub const PLAN_EVENT: &str = "plan/pending";
/// MCP `elicitation/create` waiting on the TUI.
pub const MCP_ELICIT_EVENT: &str = "mcp/elicit";
/// 一页的会话 actor 句柄（`cordis-app` 的 `session_actor` provide）。
pub const SESSION: &str = "session";
/// 一页的提交口（[`crate::SessionRef`]）：排队一条输入、问在不在跑、取消。
/// TUI、网关、定时任务驱动都在调用点 live-lookup 它。
pub const SESSION_PORT: &str = "session.port";
/// 分页服务（[`crate::Tabs`]）。每页一棵 isolate 子树；TUI 按它取当前页，
/// 网关按它开 / 关线程，定时任务驱动按它把会话开成一页。
pub const TABS: &str = "tabs";
/// [`crate::Tabs::open_session`] 在后台开出一页（不是用户开的）。载荷是那一页的
/// 身份（`String`）。网关据此把这一页纳入投影（回放历史、装项目插件）。
pub const TABS_PAGE_OPENED: &str = "tabs/page-opened";
/// 回环网关（[`crate::GatewayRef`]），由 `cordis-gateway` 的插件 provide。
pub const GATEWAY: &str = "gateway";
/// 配对队列 / 绑定变了。载荷 `()`。
pub const GATEWAY_PAIRING: &str = "gateway/pairing";
/// 插槽（`"tui.slots"`）增删了、被操作了或正文要重画，载荷是插槽 id（`String`）。
pub const TUI_SLOTS_CHANGED: &str = "tui.slots/changed";
