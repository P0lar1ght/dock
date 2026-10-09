//! Plugin spine: five named services (DSH) + a Grok-shaped one-step turn.
//!
//! Services: `sessions` · `llm` · `tools` · `systemPrompt` · `agents`.
//! The loop injects those five and provides `agentLoop`. Swap the loop by
//! disposing that one plugin and mounting another that injects the same five.

mod agent;
mod bundle;
mod error;
mod host;
mod llm;
mod names;
mod prompt;
mod session;
mod tools;

pub use agent::agents::{agents, Agent, Agents};
pub use agent::loop_plugin::agent_loop;
pub use agent::preset_assist::{
    draft_preset, rewrite_persona, suggest_tools, DraftAgent, PresetDraft, Rewrite, ToolChoice,
    ToolPick,
};
pub use agent::presets::{
    agent_presets, blocked_tool_message, is_shipped, shipped_roles, AgentPreset, AgentPresets,
    PresetEdit, PresetOrigin, SubagentDef, CORDIS_PRESET_ID, DEFAULT_PRESET_ID, MINIMAL_PRESET_ID,
    WARDEN_PRESET_ID,
};
pub use agent::runtime::{BoxFuture, Driver, GrokStep, LoopHandle};
pub use agent::turn::{steer, turn, TurnControl};
pub use bundle::{
    install_app, install_core, install_fakes, install_foundation, install_spine,
    install_without_llm,
};
pub use cordis_base::acp::PermissionOptionKind;
pub use cordis_base::config::{
    effective_browser_headed, load_browser_headed, load_catalog, load_disabled_mcp_tools,
    load_mcp_servers, load_memory_config, model_label, persist_browser_headed,
    persist_disabled_mcp_tools, persist_mcp_server_enabled, register_builtin_browser_mcp,
    ApiBackend, AuthScheme, McpOAuthConfig, McpServer, McpStdioFraming, McpTransport, MemoryConfig,
    MemoryDreamConfig, MemoryFlushConfig, ModelChoice, ModelPricing, BROWSER_MCP_ENV,
    BROWSER_MCP_SERVER, DEFAULT_EFFORT_CHOICES,
};
pub use cordis_base::stream_acc::StreamDelta;
pub use cordis_base::types::{
    LlmOutput, LogEvent, NoticeKind, PreExecute, PreStep, PromptRequest, StepStart, ToolCall,
    ToolResult, ToolSpec, TurnEnd, TurnOutcome, UserImage, AUTO_COMPACT_FAILED_PREFIX,
    COMPACT_NOTICE, INTERRUPTED_TOOL_RESULT, ORDER_STEP_START_DYNAMIC,
    ORDER_STEP_START_INSTRUCTIONS, ORDER_STEP_START_MEMORY, ORDER_STEP_START_TODO,
    ORDER_TURN_END_DYNAMIC, ORDER_TURN_END_GOAL, ORDER_TURN_END_TODO,
    PERMISSION_DENIED_TOOL_RESULT, STEER_REMINDER, TURN_STOPPED_REMINDER,
};
pub use cordis_base::usage::{
    calls_breakdown, format_cost, format_duration, group_thousands, hit_rate_spark, hit_rate_trend,
    miss_breakdown_text, per_model_amounts, session_usage_block_text, share_percent, ticks_to_usd,
    CacheSegment, CacheSegmentKind, CallCost, PromptUsage, PromptUsageModel, UsageLedger,
    RECENT_CALLS_KEPT,
};
pub use error::{Error, Result};
pub use host::gateway_port::{
    CompanionStatus, GatewayPort, GatewayRef, PairingBinding, PairingError, PairingPrompt,
};
pub use host::permissions::{permissions, PermissionPrompt, Permissions};
pub use host::plugin_settings::{plugin_settings, FieldErrors, PluginSettings};
pub use host::session_port::{QueuedItem, SessionPort, SessionRef};
pub use host::settings::{settings, AppSettings, MermaidEngineKind, ModelOverride, PermissionMode};
pub use host::settings_commands::settings_commands;
pub use host::slash::{
    register_commands, slash, slash_handler, tool_slash_arguments, ExtraSlashKind, Slash,
    SlashCommand, SlashEntry, SlashHandler, SlashHint, SlashOutcome, SlashResolved, SlashSurface,
};
pub use host::status_items::{status_items, StatusItem, StatusItems};
pub use host::tabs::{
    tabs, CarryBack, TabInfo, TabKind, TabMount, Tabs, TabsConfig, MAX_TABS, PER_TAB_SERVICES,
};
pub use host::tool_views::{effective_call, tool_views, ToolViewFn, ToolViewInput, ToolViews};
pub use host::tui_slots::{tui_slots, SlotAction, SlotHandler, SlotInfo, SlotKeyResult, TuiSlots};
pub use llm::compact::{
    compact, compact_command, exceeds_threshold, Compact, DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT,
};
pub use llm::http::probe::probe_model;
pub use llm::sampler::{llm, Llm, LlmConfig, LlmMode, Sampler};
pub use names::{
    AGENTS, AGENT_LOOP, AGENT_PRESETS, ASK, ASK_EVENT, BROWSER, COMPACT, COMPUTER, CONTEXT, CRON,
    DYNAMIC_CORDIS_RUNNER, GATEWAY, GATEWAY_PAIRING, GOAL, JOBS, LLM, LLM_STREAM, LSP, MCP,
    MCP_ELICIT_EVENT, MEMORY, PERMISSIONS, PERMISSION_EVENT, PLAN_EVENT, PLAN_MODE,
    PLUGIN_SETTINGS, PLUGIN_SETTINGS_CHANGED, PRE_STEP, PROMPT_ASSEMBLE, RHAI_BAGS, ROSTER,
    SCHEDULE_CHANGED, SESSION, SESSIONS, SESSION_CHILD_EVENT, SESSION_COMPACTION, SESSION_EVENT,
    SESSION_PAGE_EVENT, SESSION_PORT, SESSION_RESET, SESSION_TURN_END, SETTINGS, SKILLS, SLASH,
    STATUS_CHANGED, STATUS_ITEMS, STEP_START, SUBAGENTS, SUBAGENT_CHANGED, SYSTEM_PROMPT, TABS,
    TABS_PAGE_OPENED, TODOS, TOOLS, TOOLS_EXECUTE, TOOLS_PRE_EXECUTE, TOOL_VIEWS,
    TOOL_VIEWS_CHANGED, TUI_SLOTS, TUI_SLOTS_CHANGED, TURN, TURN_END, WORKFLOWS,
};
pub use prompt::assemble::{system_prompt, PromptAssembly, PromptPart, SystemPrompt};
pub use prompt::context_book::{context, own_sections, ContextBook};
pub use prompt::context_usage::{
    occupancy_detail, skill_item_detail, snapshot_context, workflow_item_detail, ContextCategory,
    ContextSnapshot, DetailGroup, DetailRow, OccupancyDetail, OccupancyKind,
};
pub use prompt::project_instructions::{project_instructions, INSTRUCTIONS_FILE};
pub use session::commands::session_commands;
pub use session::compaction::{
    CompactPhase, CompactProgress, CompactStatus, CompactTrigger, PageCompaction,
};
pub use session::cwd::{change_dir, current_cwd, session_cwd, ChangedDir};
pub use session::log::{
    sessions, ArchivedSession, ChildLogEvent, PageLogEvent, PageTurnEnd, Sampling, Sessions, Steer,
    TokenUsage, TurnEndStatus, ROOT_IDENTITY,
};
pub use session::persist::{sessions_cwd_dir, RosterEntry};
pub use session::resume_preset::{apply_restored_preset, ApplyRestoredPreset};
pub use session::roster::{roster, Roster};
pub use session::search::{
    self as session_search, ensure_index as ensure_session_search,
    rebuild_all as rebuild_session_search, SessionHit,
};
pub use tools::ask_user::{tool_ask_user, Ask, AskPrompt, Question, QuestionOption};
pub use tools::browser::{tool_browser, Browser, BrowserState, BROWSER_MCP_PREFIX};
pub use tools::computer::{
    tool_computer, Computer, ComputerState, CuaAction, CuaJobView, CUA_DRIVER_SERVER,
};
pub use tools::cron::{
    cron, Cron, CronError, CronJob, CronOwner, CronTick, MAX_SCHEDULED_TASKS,
    RECURRING_TASK_TTL_DAYS, SCHEDULES_FILE,
};
pub use tools::dynamic_runner::{
    builtins_lines, delete_secret, dynamic_runner, persist_root, plugin_roots, project_root,
    secret_names, set_secret, DiskPluginView, DynEcho, DynNote, DynamicRunner, PersistScope,
    PluginOrigin, PluginReference, PromoteReceipt, RhaiBag, RhaiBags, RunMode, SessionPluginView,
    DYN_ECHO, DYN_ECHO_TOOL, DYN_NOTE, RHAI_FACTORY,
};
pub use tools::goal::{
    goal_command, goal_composer_fill, goal_continuation_directive, goal_instruction,
    goal_offer_addon, goal_service, goal_tool_registration, goal_usage_message, start_goal,
    tool_goal, Goal, GOAL_RESERVED_SUBCOMMANDS,
};
pub use tools::jobs::{jobs, tool_jobs, JobSnapshot, Jobs};
pub use tools::lsp::{
    lsp_auto_setup, lsp_composer_fill, lsp_status_report, tool_lsp, LspBackendAdapter, LspHub,
    LspSetupReport, LspSetupScope,
};
pub use tools::mcp::{
    is_mcp_public_name, mcp_client, public_tool_name, session_key as mcp_session_key,
    split_mcp_public_name, ElicitPrompt, Elicitation, Mcp, McpReloadReport, McpStatus,
    McpToolStatus, SEARCH_TOOL_NAME, USE_TOOL_NAME,
};
pub use tools::memory::{
    maybe_flush_before_compact, run_dream, run_flush, run_remember, run_remember_async,
    tool_memory, Memory,
};
pub use tools::monitor::tool_monitor;
pub use tools::plan_mode::{
    clear_plan_for_session_switch, discard_ephemeral_plan, expected_plan_path, is_plan_file_edit,
    plan_commands, plan_instruction, plan_mode, plan_mode_service, plan_mode_tool_registration,
    plan_system_addon, PlanApprovalPrompt, PlanDecision, PlanMode, PlanPhase, PLAN_REL,
};
pub use tools::registry::{own_registered, tools, workspace_tools, ToolBody, Tools};
pub use tools::sched::{
    expired_task_notice, format_scheduled_task_prompt, format_scheduled_task_reminder,
    interval_to_human, loop_command, loop_composer_fill, loop_schedule_instruction,
    loop_usage_message, parse_interval, tool_scheduler, LoopFireMode, SCHEDULER_CREATE_TOOL_NAME,
};
pub use tools::skills::{
    scan_all_with_shadowed, skills, tool_skills, SkillInfo, SkillScope, Skills,
};
pub use tools::task::admission::SubagentLimits;
pub use tools::task::{tool_task, SubagentChanged, SubagentSnap, Subagents, TaskConfig};
pub use tools::todo_write::{
    todo_service, todo_tool_registration, tool_todo, TodoItem, TodoStats, TodoStatus, Todos,
    TODO_GATE_SENTINEL,
};
pub use tools::tool_cordis::tool_cordis;
pub use tools::tool_images::{
    cap_images, user_image_from_bytes, user_image_from_path, IMAGE_INLINE_PLACEHOLDER,
    MAX_TOOL_IMAGES,
};
pub use tools::web_fetch::{tool_web, web_fetch_params, WebFetchParams};
pub use tools::workflow::{
    extra_tool_slash_arguments, tool_workflow, workflow_command, workflow_command_arguments,
    workflow_slash_arguments, WorkflowInfo, WorkflowRunSnap, Workflows, WORKFLOW_TOOL_NAME,
};
