//! Plugin spine: five named services (DSH) + a Grok-shaped one-step turn.
//!
//! Services: `sessions` · `llm` · `tools` · `systemPrompt` · `agents`.
//! The loop injects those five and provides `agentLoop`. Swap the loop by
//! disposing that one plugin and mounting another that injects the same five.

mod agent;
mod auth;
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
    ask_preset, draft_preset, rewrite_persona, suggest_tools, AskTurn, DraftAgent, PresetDraft,
    Rewrite, ToolChoice, ToolPick,
};
pub use agent::preset_propose::{tool_propose_preset, PROPOSE_PRESET_TOOL};
pub use agent::presets::{
    agent_presets, blocked_tool_message, is_shipped, shipped_roles, AgentPreset, AgentPresets,
    PresetEdit, PresetOrigin, SubagentDef, CORDIS_PRESET_ID, DEFAULT_PRESET_ID, MINIMAL_PRESET_ID,
    WARDEN_PRESET_ID,
};
pub use agent::runtime::{BoxFuture, Driver, GrokStep, LoopHandle};
pub use agent::turn::{steer, turn, TurnControl};
pub use auth::{
    auth_commands, auth_manager, chatgpt_auth, chatgpt_policy, grok_auth, grok_policy,
    register_provider, AuthManager, BrowserLoginUi, ChatGptProvider, Credential, CredentialStore,
    GrokProvider, LoginState,
};
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
    approval_note, approval_notice, approval_timed_out_tool_result, ApprovalBy, ApprovalNote,
};
pub use cordis_base::types::{
    side_note_reminder, side_note_text, LlmOutput, LogEvent, NoticeKind, PreExecute, PreStep,
    PromptRequest, StepStart, ThinkingBlock, ToolCall, ToolResult, ToolSpec, TurnEnd, TurnEndRound,
    TurnOutcome, UserImage, AUTO_COMPACT_FAILED_PREFIX, COMPACT_NOTICE, INTERRUPTED_TOOL_RESULT,
    ORDER_STEP_START_DYNAMIC, ORDER_STEP_START_INSTRUCTIONS, ORDER_STEP_START_MCP_APPS,
    ORDER_STEP_START_MEMORY, ORDER_STEP_START_ROSTER, ORDER_STEP_START_SKILLS,
    ORDER_STEP_START_TODO, ORDER_STEP_START_USER_EDITS, ORDER_STEP_START_WORKFLOWS,
    ORDER_TURN_END_DYNAMIC, ORDER_TURN_END_GOAL, ORDER_TURN_END_TODO,
    PERMISSION_DENIED_TOOL_RESULT, SIDE_NOTE_MAX_CHARS, STEER_REMINDER, TURN_STOPPED_REMINDER,
};
pub use cordis_base::usage::{
    calls_breakdown, format_cost, format_duration, group_thousands, hit_rate_spark, hit_rate_trend,
    miss_breakdown_text, per_model_amounts, session_usage_block_text, share_percent, ticks_to_usd,
    CacheSegment, CacheSegmentKind, CallCost, PromptUsage, PromptUsageModel, UsageLedger,
    RECENT_CALLS_KEPT,
};
pub use error::{Error, Result};
pub use host::approval::{
    approval_judge, classify as classify_approval, smart_verdict, ApprovalJudge, Judge,
    JudgeRequest, Judged, Rule as ApprovalRule, Smart,
};
pub use host::audit::{sandbox_flow, sandbox_http, sandbox_status, sandbox_trace};
pub use host::gateway_port::{
    CompanionStatus, GatewayPort, GatewayRef, PairingBinding, PairingError, PairingPrompt,
};
pub use host::hooks_bridge::{describe as describe_hooks, hooks_bridge, hooks_command};
pub use host::mcp_app_context::{
    mcp_app_context, page_key, record_app_context, AppContextUpdate, McpAppContext,
};
pub use host::mcp_app_views::{
    mcp_app_views, McpAppViews, ViewCall, ViewExpose, ViewTool, APP_VIEW_TOOL_PREFIX,
    VIEW_CALL_TIMEOUT,
};
pub use host::permissions::{permissions, Decision, PermissionPrompt, Permissions, RunApproval};
pub use host::plugin_settings::{plugin_settings, FieldErrors, PluginSettings};
pub use host::sandbox::sandbox as sandbox_plugin;
pub use host::session_port::{QueuedItem, SessionPort, SessionRef};
pub use host::settings::{settings, AppSettings, MermaidEngineKind, ModelOverride, PermissionMode};
pub use host::settings_commands::settings_commands;
pub use host::side_note::draft_side_note;
pub use host::slash::{
    register_commands, slash, slash_handler, tool_slash_arguments, ExtraSlashKind, Slash,
    SlashCommand, SlashEntry, SlashHandler, SlashHint, SlashOutcome, SlashResolved, SlashSurface,
};
pub use host::status_items::{status_items, StatusItem, StatusItems};
pub use host::tabs::{
    tabs, CarryBack, PageClosed, TabInfo, TabKind, TabMount, Tabs, TabsConfig, MAX_TABS,
    PER_TAB_SERVICES,
};
pub use host::tool_views::{effective_call, tool_views, ToolViewFn, ToolViewInput, ToolViews};
pub use host::tui_slots::{tui_slots, SlotAction, SlotHandler, SlotInfo, SlotKeyResult, TuiSlots};
pub use host::turn_cards::{
    turn_cards, CardHandler, CardSpec, CardView, TurnCards, TurnCardsChanged,
};
pub use host::turn_changes::{
    turn_changes, unified_diff, FileChange, RevertOutcome, Snapshot, TurnChanges,
    TurnChangesChanged, TurnSummary,
};
pub use host::user_edits::{record_user_edit, user_edits, UserEdits};
pub use llm::compact::{
    compact, compact_command, estimate_text_tokens, exceeds_threshold, Compact,
    DEFAULT_AUTO_COMPACT_THRESHOLD_PERCENT,
};
pub use llm::http::catalog::{fetch_models_dev, MODELS_DEV_URL};
pub use llm::http::probe::{list_upstream_models, probe_model, UpstreamModel};
pub use llm::sampler::{llm, Llm, LlmConfig, LlmMode, Sampler};
pub use names::{
    AGENTS, AGENT_LOOP, AGENT_PRESETS, ASK, ASK_EVENT, AUTH, AUTH_CHANGED, BROWSER, COMPACT,
    COMPUTER, CONTEXT, CRON, DYNAMIC_CORDIS_RUNNER, GATEWAY, GATEWAY_PAIRING, GOAL, JOBS,
    JOB_FINISHED, JOB_REPORTED, LLM, LLM_FINISHED, LLM_STREAM, LSP, MCP, MCP_APP_CONTEXT_STORE,
    MCP_APP_VIEWS, MCP_APP_VIEW_CALL, MCP_ELICIT_EVENT, MEMORY, PERMISSIONS, PERMISSION_EVENT,
    PLAN_EVENT, PLAN_MODE, PLUGIN_SETTINGS, PLUGIN_SETTINGS_CHANGED, PRE_STEP, PROMPT_ASSEMBLE,
    RHAI_BAGS, ROSTER, SCHEDULE_CHANGED, SESSION, SESSIONS, SESSION_CHILD_EVENT,
    SESSION_COMPACTION, SESSION_EVENT, SESSION_PAGE_EVENT, SESSION_PORT, SESSION_RESET,
    SESSION_TOOL_INPUT, SESSION_TURN_END, SETTINGS, SKILLS, SLASH, STATUS_CHANGED, STATUS_ITEMS,
    STEP_START, SUBAGENTS, SUBAGENT_CHANGED, SYSTEM_PROMPT, TABS, TABS_PAGE_CLOSED,
    TABS_PAGE_OPENED, TODOS, TOOLS, TOOLS_EXECUTE, TOOLS_PRE_EXECUTE, TOOL_VIEWS,
    TOOL_VIEWS_CHANGED, TUI_SLOTS, TUI_SLOTS_CHANGED, TURN, TURN_CARDS, TURN_CARDS_CHANGED,
    TURN_CHANGES, TURN_CHANGES_CHANGED, TURN_END, USER_EDITS, WORKFLOWS, WORKFLOW_CHANGED,
};
pub use prompt::assemble::{system_prompt, PromptAssembly, PromptPart, SystemPrompt};
pub use prompt::context_book::{context, own_sections, ContextBook};
pub use prompt::context_usage::{
    anchor_tokens, base_of, context_usage, history_tokens, occupancy_detail, skill_item_detail,
    snapshot_context, usage_from, workflow_item_detail, ContextBase, ContextCategory,
    ContextSnapshot, ContextUsage, DetailGroup, DetailRow, OccupancyDetail, OccupancyKind,
};
pub use prompt::project_instructions::{project_instructions, INSTRUCTIONS_FILE, USER_FILE};
pub use session::attachments::{
    blob_path, digest_of, hydrate as hydrate_user_image, image_len as user_image_len,
    store as store_user_image, UserImageRef, MAX_USER_IMAGE_BYTES,
};
pub use session::commands::session_commands;
pub use session::compaction::{
    CompactPhase, CompactProgress, CompactStatus, CompactTrigger, PageCompaction,
};
pub use session::cwd::{
    change_dir, current_cwd, enter_workspace, session_cwd, session_home, session_space,
    switch_space, ChangedDir,
};
pub use session::log::{
    sessions, ArchivedSession, ChildLogEvent, LlmFinished, PageLogEvent, PageToolInput,
    PageTurnEnd, Place, Sampling, Sessions, Steer, TokenUsage, TurnEndStatus, ROOT_IDENTITY,
};
pub use session::persist::{
    count_subagents, first_user_title, load_subagent, load_subagents, sessions_cwd_dir,
    subagent_dir, RosterEntry, SubagentMeta,
};
pub use session::resume_preset::{apply_restored_preset, ApplyRestoredPreset};
pub use session::roster::{roster, Roster};
pub use session::search::{
    self as session_search, ensure_index as ensure_session_search,
    rebuild_all as rebuild_session_search, SessionHit,
};
pub use tools::ask_user::handoff::HANDOFF_QUESTION_ID;
pub use tools::ask_user::{tool_ask_user, Ask, AskPrompt, Question, QuestionOption};
pub use tools::browser::{tool_browser, Browser, BrowserState, BROWSER_MCP_PREFIX};
pub use tools::computer::{
    tool_computer, Computer, ComputerState, CuaAction, CuaJobView, CUA_DRIVER_SERVER,
};
pub use tools::cron::{
    cron, parse_at, run_check, Cron, CronApproval, CronCheck, CronError, CronExpiry, CronFire,
    CronJob, CronOwner, CronPatch, CronRun, CronSchedule, CronTarget, CronTick, CronWatchState,
    FileWatches, FilesReady, FilesSpec, FireReason, FireReport, NewCronTask, RunOutcome,
    WatchObservation, WatchSpec, WatchWhen, CHECK_TIMEOUT, DEFAULT_APPROVAL_TIMEOUT,
    DEFAULT_CHECK_EVERY, DEFAULT_FILES_DEBOUNCE, HISTORY_LEN, MAX_SCHEDULED_TASKS, MIN_CHECK_EVERY,
    MISSED_GRACE, RECURRING_TASK_TTL_DAYS, SCHEDULES_FILE,
};
pub use tools::dynamic_runner::{
    builtins_lines, delete_secret, dynamic_runner, find_disk, persist_root, plugin_roots,
    project_root, secret_names, set_secret, DisableReceipt, DiskPluginView, DiskSpec, DynEcho,
    DynNote, DynamicRunner, PersistScope, PluginOrigin, PluginReference, PluginSel, ReloadReceipt,
    ReloadStatus, RhaiBag, RhaiBags, RunMode, SourceInput, Surface, DYN_ECHO, DYN_ECHO_TOOL,
    DYN_NOTE, RHAI_FACTORY,
};
pub use tools::goal::{
    goal_command, goal_composer_fill, goal_continuation_directive, goal_instruction,
    goal_offer_addon, goal_service, goal_tool_registration, goal_usage_message, start_goal,
    tool_goal, Goal, GoalBudget, CHECKIN_EVERY, GOAL_RESERVED_SUBCOMMANDS, STALL_LIMIT,
};
pub use tools::jobs::{
    job_notify, jobs, tool_jobs, JobEnd, JobFinished, JobReported, JobSnapshot, Jobs,
};
pub use tools::lsp::{
    lsp_auto_setup, lsp_composer_fill, lsp_status_report, tool_lsp, LspBackendAdapter, LspHub,
    LspSetupReport, LspSetupScope,
};
pub use tools::mcp::{
    is_mcp_public_name, mcp_client, public_tool_name, read_app_record,
    session_key as mcp_session_key, split_mcp_public_name, ElicitPrompt, Elicitation, Mcp,
    McpAppTool, McpReloadReport, McpStatus, McpToolStatus, APP_RECORD_DIR, SEARCH_TOOL_NAME,
    USE_TOOL_NAME,
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
    expired_task_notice, format_fire_note, format_scheduled_fire_reminder,
    format_scheduled_task_prompt, format_scheduled_task_reminder, interval_to_human, loop_command,
    loop_composer_fill, loop_schedule_instruction, loop_usage_message, missed_task_notice,
    parse_interval, tool_scheduler, FireNote, LoopFireMode, SCHEDULER_CREATE_TOOL_NAME,
};
pub use tools::skills::{
    scan_all_with_shadowed, skills, tool_skills, SkillInfo, SkillScope, Skills,
};
pub use tools::task::admission::SubagentLimits;
pub use tools::task::{tool_task, SubagentChanged, SubagentSnap, Subagents, TaskConfig};
pub use tools::todo_write::{
    todo_service, todo_tool_registration, tool_todo, TodoItem, TodoStats, TodoStatus, Todos,
    TEST_BUILD_COMMANDS, TODO_ALL_CLOSED, TODO_ALL_DONE, TODO_BOOKKEEPING_SENTINEL,
};
pub use tools::tool_cordis::tool_cordis;
pub use tools::tool_images::{
    cap_images, user_image_from_bytes, user_image_from_path, IMAGE_INLINE_PLACEHOLDER,
    MAX_TOOL_IMAGES,
};
pub use tools::web_fetch::{tool_web, web_fetch_params, WebFetchParams};
pub use tools::workflow::{
    extra_tool_slash_arguments, tool_workflow, workflow_command, workflow_command_arguments,
    workflow_slash_arguments, WorkflowAgentRow, WorkflowChanged, WorkflowInfo, WorkflowRunSnap,
    Workflows, WORKFLOW_TOOL_NAME,
};
