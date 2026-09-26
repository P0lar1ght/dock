//! Plan collaboration state. Registers `enter_plan_mode` / `exit_plan_mode`.
//!
//! Lifecycle (simplified grok `PlanModeTracker`):
//! `Inactive` → `Pending` (user `/plan` / Shift+Tab) → `Active` (first prompt
//! or `enter_plan_mode`) → approval park on `exit_plan_mode` → back to
//! `Inactive` (approve/quit) or stay `Active` (revise).
//!
//! Plan file is per-session (Grok uses `$GROK_HOME/sessions/<cwd>/<id>/plan.md`):
//! `$DOCK_HOME/sessions/<cwd-key>/<id>/plan.md`, or
//! `sessions/<cwd-key>/tabs/<pid>/<main#N>/plan.md` for a non-disk tab page.
//! The model is told that absolute path. See [`plan_path_for`].

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cordis::{plugin, Context, Inject, Plugin};
use tokio::sync::oneshot;

use crate::names::{PLAN_EVENT, PLAN_MODE, PRE_STEP, SESSIONS, TOOLS};
use crate::session::log::Sessions;
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{LogEvent, PreStep, ToolCall, ToolResult, ToolSpec};

pub const PLAN_REL: &str = ".dock/plan.md";
/// 落盘文件名（在 session 目录里）。路径见 [`plan_path_for`]。
const PLAN_FILENAME: &str = "plan.md";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanPhase {
    Inactive,
    /// User toggled plan on; model has not been told yet.
    Pending,
    /// Plan mode constraints apply; model has instructions.
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanDecision {
    /// Leave plan mode and start implementing.
    Approve,
    /// Stay in plan mode; revise this page's plan file.
    Revise,
    /// Abandon the plan and turn plan mode off.
    Quit,
}

#[derive(Debug, Clone)]
pub struct PlanApprovalPrompt {
    pub path: String,
    /// Full plan markdown (not truncated) for the approval / view chrome.
    pub body: String,
    pub empty: bool,
}

struct PendingApproval {
    /// 序号，见 [`PlanMode::front_seq`]。
    seq: u64,
    prompt: PlanApprovalPrompt,
    tx: oneshot::Sender<PlanDecision>,
}

struct ModeState {
    phase: PlanPhase,
    reminder_count: u32,
    pending_exit_reminder: bool,
    was_previously_active: bool,
}

/// Named `"planMode"` service. Live-lookup; do not capture the Arc.
pub struct PlanMode {
    ctx: Context,
    mode: Mutex<ModeState>,
    approval: Mutex<Option<PendingApproval>>,
    last_decision: Mutex<Option<PlanDecision>>,
    next_seq: std::sync::atomic::AtomicU64,
}

impl PlanMode {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            mode: Mutex::new(ModeState {
                phase: PlanPhase::Inactive,
                reminder_count: 0,
                pending_exit_reminder: false,
                was_previously_active: false,
            }),
            approval: Mutex::new(None),
            last_decision: Mutex::new(None),
            next_seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// 挂着的计划审批的序号（单调递增）；投影方用它判断是不是新的一次审批。
    pub fn front_seq(&self) -> Option<u64> {
        self.approval.lock().unwrap().as_ref().map(|p| p.seq)
    }

    pub fn phase(&self) -> PlanPhase {
        self.mode.lock().unwrap().phase
    }

    /// Sessions of the page this service was mounted on. A child looking up
    /// `planMode` still gets this page, not its own isolated log.
    fn page_sessions(&self) -> Option<std::sync::Arc<Sessions>> {
        self.ctx.get::<Sessions>(SESSIONS)
    }

    /// Chrome chip / Shift+Tab: any non-inactive phase, or parked approval.
    pub fn active(&self) -> bool {
        self.gated() || matches!(self.phase(), PlanPhase::Pending)
    }

    /// Write/shell gate: Active or awaiting approval.
    pub fn gated(&self) -> bool {
        matches!(self.phase(), PlanPhase::Active) || self.approval.lock().unwrap().is_some()
    }

    pub fn awaiting_approval(&self) -> bool {
        self.approval.lock().unwrap().is_some()
    }

    pub fn front(&self) -> Option<PlanApprovalPrompt> {
        self.approval
            .lock()
            .unwrap()
            .as_ref()
            .map(|p| p.prompt.clone())
    }

    /// User toggle / `/plan` without description.
    pub fn enter_pending(&self) {
        let mut mode = self.mode.lock().unwrap();
        if mode.phase == PlanPhase::Inactive {
            mode.phase = PlanPhase::Pending;
            mode.pending_exit_reminder = false;
        }
    }

    /// `enter_plan_mode` tool or `/plan <desc>`.
    pub fn enter_active(&self) {
        let mut mode = self.mode.lock().unwrap();
        if mode.phase != PlanPhase::Active {
            mode.reminder_count = 0;
        }
        mode.phase = PlanPhase::Active;
        mode.was_previously_active = true;
        mode.pending_exit_reminder = false;
    }

    /// Promote Pending → Active on first turn `agent/pre-step`.
    pub fn promote_pending(&self) {
        let mut mode = self.mode.lock().unwrap();
        if mode.phase == PlanPhase::Pending {
            mode.phase = PlanPhase::Active;
            mode.was_previously_active = true;
            mode.reminder_count = 0;
        }
    }

    /// Compatibility: `true` → active, `false` → clear (quit pending approval).
    pub fn set(&self, on: bool) {
        if on {
            self.enter_active();
        } else {
            self.exit_now();
        }
    }

    pub fn toggle(&self) -> bool {
        if self.active() {
            self.exit_now();
            false
        } else {
            self.enter_pending();
            true
        }
    }

    /// Drop approval (Quit) and go Inactive.
    pub fn exit_now(&self) {
        if let Some(pending) = self.approval.lock().unwrap().take() {
            let _ = pending.tx.send(PlanDecision::Quit);
        }
        {
            let mut mode = self.mode.lock().unwrap();
            let leaving = mode.phase != PlanPhase::Inactive;
            mode.phase = PlanPhase::Inactive;
            if leaving {
                mode.pending_exit_reminder = true;
                mode.was_previously_active = true;
            }
        }
        self.ctx.emit(PLAN_EVENT, ());
    }

    /// Last decision that actually popped a parked approval.
    pub fn last_decision(&self) -> Option<PlanDecision> {
        *self.last_decision.lock().unwrap()
    }

    /// Resolve parked approval. Returns `false` when nothing is waiting so a
    /// second resolver (TUI vs web) can fail closed.
    pub fn resolve(&self, decision: PlanDecision) -> bool {
        let Some(pending) = self.approval.lock().unwrap().take() else {
            return false;
        };
        match decision {
            PlanDecision::Approve | PlanDecision::Quit => {
                let mut mode = self.mode.lock().unwrap();
                mode.phase = PlanPhase::Inactive;
                mode.pending_exit_reminder = true;
                mode.was_previously_active = true;
            }
            PlanDecision::Revise => {
                let mut mode = self.mode.lock().unwrap();
                mode.phase = PlanPhase::Active;
                mode.was_previously_active = true;
            }
        }
        *self.last_decision.lock().unwrap() = Some(decision);
        let _ = pending.tx.send(decision);
        self.ctx.emit(PLAN_EVENT, ());
        true
    }

    /// Park until the TUI resolves. Keeps the write gate on.
    pub async fn request_approval(&self, path: String, body: String, empty: bool) -> PlanDecision {
        let (tx, rx) = oneshot::channel();
        {
            let mut slot = self.approval.lock().unwrap();
            if let Some(prev) = slot.take() {
                let _ = prev.tx.send(PlanDecision::Quit);
            }
            *slot = Some(PendingApproval {
                seq: self
                    .next_seq
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1,
                prompt: PlanApprovalPrompt { path, body, empty },
                tx,
            });
        }
        // Stay Active while parked so the gate remains.
        {
            let mut mode = self.mode.lock().unwrap();
            mode.phase = PlanPhase::Active;
            mode.was_previously_active = true;
        }
        self.ctx.emit(PLAN_EVENT, ());
        rx.await.unwrap_or(PlanDecision::Quit)
    }

    /// Grok `inject_plan_mode_reminders`: append at the history tail so the
    /// system-prompt prefix stays byte-stable for provider cache.
    fn inject_turn_reminder(&self) {
        let Some(body) = self.take_reminder_body() else {
            return;
        };
        let Some(sessions) = self.ctx.get::<Sessions>(SESSIONS) else {
            return;
        };
        sessions.append(LogEvent::SystemReminder(wrap_plan_reminder(&body)));
    }

    fn take_reminder_body(&self) -> Option<String> {
        let mut mode = self.mode.lock().unwrap();
        if mode.phase == PlanPhase::Pending {
            let reentry = mode.was_previously_active;
            mode.phase = PlanPhase::Active;
            mode.was_previously_active = true;
            mode.reminder_count = 1;
            return Some(
                if reentry {
                    plan_reentry_addon()
                } else {
                    plan_system_addon()
                }
                .to_string(),
            );
        }
        if mode.phase == PlanPhase::Active {
            let full = mode.reminder_count.is_multiple_of(2);
            mode.reminder_count = mode.reminder_count.saturating_add(1);
            return Some(
                if full {
                    plan_system_addon()
                } else {
                    plan_sparse_addon()
                }
                .to_string(),
            );
        }
        if mode.pending_exit_reminder {
            mode.pending_exit_reminder = false;
            return Some(plan_exit_addon().to_string());
        }
        None
    }

    /// Disk snapshot for `/view-plan` when nothing is parked.
    pub fn disk_preview(&self) -> Option<PlanApprovalPrompt> {
        let path = resolve_plan(&self.ctx).0;
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let empty = content.trim().is_empty();
                Some(PlanApprovalPrompt {
                    path: path.to_string_lossy().to_string(),
                    body: if empty { String::new() } else { content },
                    empty,
                })
            }
            Err(_) => None,
        }
    }
}

/// Whether this tool call is an edit of the session plan file.
///
/// `expected` is the caller's page plan path (see [`resolve_plan`]). Only that
/// path counts: `enter_plan_mode` tells the model the absolute path, and a
/// shared relative name would let two active pages write one file.
pub fn is_plan_file_edit(tool: &str, arguments: &str, expected: &Path) -> bool {
    if !matches!(
        tool,
        "search_replace" | "write_file" | "edit" | "write" | "strreplace"
    ) {
        return false;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return false;
    };
    let path = ["file_path", "target_file", "path", "filePath"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .unwrap_or("");
    path_targets_plan_file(path, expected)
}

fn path_targets_plan_file(path: &str, expected: &Path) -> bool {
    if path.is_empty() {
        return false;
    }
    // 模型可能把路径正规化（macOS 的 /var ↔ /private/var、含 `..` / `./`、
    // 尾斜杠）。两侧都 canonicalize 后再比，仍是「只认本页这个文件」。
    // 文件不存在（计划还没建）时 canonicalize 会失败，回落逐字比较，保持原行为。
    match (std::fs::canonicalize(path), std::fs::canonicalize(expected)) {
        (Ok(a), Ok(b)) => a == b,
        _ => Path::new(path) == expected,
    }
}

/// Backwards-compatible single mount: service + tool registration together, in
/// one synchronous plugin so `wait()` returns with everything provided.
/// New code mounts [`plan_mode_service`] per isolate subtree and
/// [`plan_mode_tool_registration`] once at the root.
pub fn plan_mode() -> Plugin {
    plugin("plan-mode", Inject::from([TOOLS]), |ctx, _: &()| {
        ctx.provide(PLAN_MODE, PlanMode::new(ctx.clone()))?;
        wire_plan_pre_step(ctx);
        let root_ctx = ctx.clone();
        let enter: ToolBody = {
            let root_ctx = root_ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = crate::tools::registry::exec_ctx().unwrap_or_else(|| root_ctx.clone());
                Box::pin(async move { enter_plan(&ctx, call) })
            })
        };
        let exit: ToolBody = {
            std::sync::Arc::new(move |call| {
                let ctx = crate::tools::registry::exec_ctx().unwrap_or_else(|| root_ctx.clone());
                Box::pin(async move { exit_plan(&ctx, call).await })
            })
        };
        let tools = ctx.require::<Tools>(TOOLS)?;
        own_registered(
            ctx,
            vec![
                tools.register(
                    ToolSpec {
                        name: "enter_plan_mode".into(),
                        description: "Use this tool when a task has ambiguity about the right approach or when the user asks you to write a plan. This tool enables a read-only plan mode where you explore the codebase and create an implementation plan for the user.".into(),
                        parameters_json: r#"{"type":"object","properties":{}}"#.into(),
                    },
                    enter,
                )?,
                tools.register(
                    ToolSpec {
                        name: "exit_plan_mode".into(),
                        description: "Exit plan mode and present your plan to the user.\n\nUse this after you have finished writing your plan to the plan file in plan mode.".into(),
                        parameters_json: r#"{"type":"object","properties":{}}"#.into(),
                    },
                    exit,
                )?,
            ],
        )?;
        Ok(None)
    })
}
/// Per-isolate-subtree service: the `"planMode"` state + the `agent/pre-step`
/// reminder. Mounted once per page (`PLAN_MODE` is isolated per tab page), so a
/// second tab has its own mode, approval queue and plan file.
pub fn plan_mode_service() -> Plugin {
    plugin("plan-mode.service", Inject::new(), |ctx, _: &()| {
        ctx.provide(PLAN_MODE, PlanMode::new(ctx.clone()))?;
        wire_plan_pre_step(ctx);
        Ok(None)
    })
}

/// Register `enter_plan_mode` / `exit_plan_mode` against the global `"tools"`
/// table. Registered once; the bodies dispatch on the **executing** context so a
/// call from any tab page reaches that page's own `"planMode"` service and that
/// page's plan file.
pub fn plan_mode_tool_registration() -> Plugin {
    plugin("plan-mode.tools", Inject::from([TOOLS]), |ctx, _: &()| {
        let tools = ctx.require::<Tools>(TOOLS)?;
        let root_ctx = ctx.clone();
        let enter: ToolBody = {
            let root_ctx = root_ctx.clone();
            std::sync::Arc::new(move |call| {
                // `PLAN_MODE` is isolated per tab page, so resolve the caller's
                // subtree, not the (root) one that registered the tool.
                let ctx = crate::tools::registry::exec_ctx().unwrap_or_else(|| root_ctx.clone());
                Box::pin(async move { enter_plan(&ctx, call) })
            })
        };
        let exit: ToolBody = {
            std::sync::Arc::new(move |call| {
                let ctx = crate::tools::registry::exec_ctx().unwrap_or_else(|| root_ctx.clone());
                Box::pin(async move { exit_plan(&ctx, call).await })
            })
        };
        own_registered(
                ctx,
                vec![
                    tools.register(
                        ToolSpec {
                            name: "enter_plan_mode".into(),
                            description: "Use this tool when a task has ambiguity about the right approach or when the user asks you to write a plan. This tool enables a read-only plan mode where you explore the codebase and create an implementation plan for the user.".into(),
                            parameters_json: r#"{"type":"object","properties":{}}"#.into(),
                        },
                        enter,
                    )?,
                    tools.register(
                        ToolSpec {
                            name: "exit_plan_mode".into(),
                            description: "Exit plan mode and present your plan to the user.\n\nUse this after you have finished writing your plan to the plan file in plan mode.".into(),
                            parameters_json: r#"{"type":"object","properties":{}}"#.into(),
                        },
                        exit,
                    )?,
                ],
            )?;
        Ok(None)
    })
}

/// Pending → Active belongs to the page whose turn is starting. A subagent
/// identity matches no page, so it cannot promote or remind the user's plan.
fn wire_plan_pre_step(ctx: &Context) {
    let ctx_pre = ctx.clone();
    let _ = ctx.on_waterfall(PRE_STEP, move |step: PreStep, args| {
        let next = args.next::<PreStep>().unwrap_or(step);
        if next.enter && Sessions::turn_is_this_page(&ctx_pre, &next.identity) {
            if let Some(plan) = ctx_pre.get::<PlanMode>(PLAN_MODE) {
                plan.inject_turn_reminder();
            }
        }
        next
    });
}

/// Drop plan mode because the live thread on this page just changed.
///
/// Phase is in memory and is not stored with the session. New session and
/// resume both have to come through here, or the write gate follows the new
/// `live_id` while the chip still says the previous thread is planning.
pub fn clear_plan_for_session_switch(ctx: &Context) {
    if let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) {
        plan.set(false);
    }
}

/// 计划文件按**会话**划分，而不是按工作区 cwd：同 cwd 下多个分页各有自己的计划，
/// 互不覆盖（对齐 grok-build 的 `$GROK_HOME/sessions/<cwd>/<id>/plan.md`）。
///
/// - 已落盘的会话（主会话、常驻分页）→ 自己的 session 目录
/// - 未落盘的页（`/btw` 旁问页，`main#N`，不 attach_disk）→ `sessions/<cwd-key>/tabs/<pid>/<identity>/`
/// - 拿不到会话（测试 / 未挂载）→ 回落到 cwd 下的 `.dock/plan.md`
fn plan_path_for(sessions: Option<&Sessions>) -> PathBuf {
    if let Some(sessions) = sessions {
        if let Some(dir) = sessions.disk_session_dir() {
            return dir.join(PLAN_FILENAME);
        }
        if let Some(dir) = ephemeral_plan_dir(sessions) {
            return dir.join(PLAN_FILENAME);
        }
    }
    crate::session::cwd::current_cwd().join(PLAN_REL)
}

/// Directory of a non-disk tab's plan. `None` for the root session and for
/// subagents — those either have a disk session dir or no plan of their own.
///
/// `<pid>` 段把不同进程的同一页号隔开：`main#N` 每个进程都从 2 开始编号，
/// 两个 dock 进程共用同一 `DOCK_HOME` 与工作目录时，没有 pid 段就会开页
/// 删掉对方正在写的计划。同一进程内页号唯一，不会撞。
fn ephemeral_plan_dir(sessions: &Sessions) -> Option<PathBuf> {
    if sessions.on_disk() {
        return None;
    }
    let cwd = sessions.plan_cwd()?;
    let id = sessions.identity();
    if !id.starts_with(cordis_base::types::TAB_IDENTITY_PREFIX) {
        return None;
    }
    Some(
        crate::session::persist::sessions_cwd_dir(&cwd)
            .join("tabs")
            .join(std::process::id().to_string())
            .join(id),
    )
}

/// Remove a non-disk tab's plan directory.
///
/// Aside pages (`/btw`) are not persisted; resident tabs are, and this is a
/// no-op for them. The directory name is `main#N`, and that number
/// restarts at 2 next process, so a leftover file would be treated as this
/// page's plan. Called when the page opens (drop the previous run of this
/// process) and when it closes (drop this one). The `<pid>` parent makes sure
/// that only ever touches this process's own pages.
pub fn discard_ephemeral_plan(sessions: &Sessions) {
    let Some(dir) = ephemeral_plan_dir(sessions) else {
        return;
    };
    let _ = std::fs::remove_dir_all(dir);
}

/// Resolve `(path, display)` for the page that owns plan mode.
///
/// A child isolate has its own `Sessions` but inherits the parent's
/// `planMode`. The file has to be the parent's, or the write lands under the
/// child's id while the gate watches the page.
fn resolve_plan(ctx: &Context) -> (PathBuf, String) {
    let sessions = ctx
        .get::<PlanMode>(PLAN_MODE)
        .and_then(|plan| plan.page_sessions())
        .or_else(|| ctx.get::<Sessions>(SESSIONS));
    let path = plan_path_for(sessions.as_deref());
    let display = path.to_string_lossy().to_string();
    (path, display)
}

/// The plan file path for the page owning `sessions`. Used by the write gate
/// ([`is_plan_file_edit`]) so a second tab's `write_file` isn't mistaken for an
/// edit of the first tab's plan.
pub fn expected_plan_path(sessions: Option<&Sessions>) -> PathBuf {
    plan_path_for(sessions)
}

enum Seed {
    Empty,
    NonEmpty,
    Missing(&'static str),
}

fn probe_or_create(path: &Path) -> Seed {
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => Seed::Missing("该路径已是目录。"),
        Ok(m) if m.len() == 0 => Seed::Empty,
        Ok(_) => Seed::NonEmpty,
        Err(_) => {
            if let Some(parent) = path.parent() {
                if std::fs::create_dir_all(parent).is_err() {
                    return Seed::Missing("计划文件位置不可用。");
                }
            }
            match std::fs::write(path, "") {
                Ok(()) => Seed::Empty,
                Err(_) => Seed::Missing("文件尚未创建。"),
            }
        }
    }
}

/// 计划模式打开时追加到 history 尾部（Grok `plan_mode_reminder_full_template` 中文）。
///
/// 路径**故意不写死**：计划文件按会话划分（见 [`plan_path_for`]），具体路径由
/// `enter_plan_mode` 的输出给出；这里写死任何路径都会在分页场景下误导模型。
pub fn plan_system_addon() -> &'static str {
    "计划模式已开启。除计划文件外，不要改文件或跑会改环境的命令。\n\n\
     计划写到 enter_plan_mode 返回的计划文件里。这是唯一允许编辑的文件。\n\
     若文件还不存在，先调用 enter_plan_mode 创建。\n\n\
     只读探索代码并写出实现计划。bash 可以直接跑只读命令（ls、cat、grep/rg、find、\
     git status/log/diff 等，不带重定向）；会改东西的命令要用户批准。\
     需要澄清时用 ask_user_question。准备好后用 exit_plan_mode 把计划交给用户。"
}

fn plan_sparse_addon() -> &'static str {
    "计划模式仍开启。除计划文件外，不要改文件或跑会改环境的命令。"
}

fn plan_exit_addon() -> &'static str {
    "已退出计划模式。现在可以改文件、跑工具。"
}

fn plan_reentry_addon() -> &'static str {
    "再次进入计划模式。先前的计划在 enter_plan_mode 返回的计划文件里。\
     除该文件外不要改文件。\
     准备好后用 exit_plan_mode 把计划交给用户。"
}

fn wrap_plan_reminder(body: &str) -> String {
    format!("<system-reminder>\n{body}\n</system-reminder>")
}

/// 斜杠 `/plan` 注入给模型的说明（工具名保持英文）。
pub fn plan_instruction(description: Option<&str>) -> String {
    match description.map(str::trim).filter(|s| !s.is_empty()) {
        Some(desc) => format!(
            "# /plan — 进入计划模式\n\n\
             用户要求进入计划模式，说明：{desc}\n\n\
             请调用 enter_plan_mode，只读探索代码并写出实现计划。未经用户批准不要改文件，也不要跑会改环境的命令。准备好后用 exit_plan_mode 把计划交给用户。"
        ),
        None => "# /plan — 进入计划模式\n\n\
             用户要求进入计划模式。\n\n\
             请调用 enter_plan_mode，只读探索代码并写出实现计划。未经用户批准不要改文件，也不要跑会改环境的命令。准备好后用 exit_plan_mode 把计划交给用户。"
            .into(),
    }
}

fn enter_plan(ctx: &Context, call: ToolCall) -> ToolResult {
    if let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) {
        plan.enter_active();
    }
    let (path, display) = resolve_plan(ctx);
    let seed = probe_or_create(&path);
    let plan_status = match seed {
        Seed::Empty => format!("把计划写到 {display}。文件已存在且为空。"),
        Seed::NonEmpty => format!("把计划写到 {display}。文件已有内容。"),
        Seed::Missing(detail) => format!("把计划写到 {display}。{detail}"),
    };
    let ask = "ask_user_question";
    let exit = "exit_plan_mode";
    tool_result(
        call,
        format!(
            "已进入计划模式\n\n\
             {plan_status}\n\n\
             在计划模式中你应当：\n\
             1. 充分探索代码库，理解现有模式\n\
             2. 找出类似功能、架构与取舍\n\
             3. 需要澄清做法时用 {ask}\n\
             4. 设计具体实现策略\n\
             5. 把计划写入上面的计划文件\n\
             6. 准备好后用 {exit} 把计划交给用户。"
        ),
    )
}

async fn exit_plan(ctx: &Context, call: ToolCall) -> ToolResult {
    let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) else {
        return tool_result(call, "计划模式未挂载");
    };
    if !plan.active() {
        return tool_result(call, "当前不在计划模式。");
    }
    // Ensure Active so the write gate stays while we wait for the user.
    plan.enter_active();

    let (path, display) = resolve_plan(ctx);
    let (body, empty) = match std::fs::read_to_string(&path) {
        Ok(content) if content.trim().is_empty() => (String::new(), true),
        Ok(content) => (content, false),
        Err(_) => (String::new(), true),
    };

    let decision = plan
        .request_approval(display.clone(), body.clone(), empty)
        .await;
    match decision {
        PlanDecision::Approve => {
            if empty {
                tool_result(
                    call,
                    format!(
                        "用户批准退出计划模式（计划文件为空）。现在可以改文件、跑 shell。\n\n\
                         计划路径：{display}"
                    ),
                )
            } else {
                tool_result(
                    call,
                    format!(
                        "用户已批准计划。现在可以改文件、跑 shell。\n\n\
                         计划已保存在：{display}\n\n\
                         ## 计划：\n{body}"
                    ),
                )
            }
        }
        PlanDecision::Revise => tool_result(
            call,
            format!(
                "用户要求修改计划。请继续在计划模式下修订 {display}，完成后再次调用 exit_plan_mode。"
            ),
        ),
        PlanDecision::Quit => tool_result(call, "用户放弃了该计划，已退出计划模式。"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_file_edit_matches_only_the_expected_path() {
        let expected = std::path::Path::new("/dock/sessions/aaa/plan.md");
        assert!(is_plan_file_edit(
            "write_file",
            r##"{"target_file":"/dock/sessions/aaa/plan.md","contents":"# x"}"##,
            expected
        ));
        assert!(!is_plan_file_edit(
            "write_file",
            r##"{"target_file":".dock/plan.md","contents":"# x"}"##,
            expected
        ));
        assert!(!is_plan_file_edit(
            "search_replace",
            r#"{"file_path":"/repo/.dock/plan.md","old_string":"a","new_string":"b"}"#,
            expected
        ));
        assert!(!is_plan_file_edit(
            "write_file",
            r#"{"target_file":"src/main.rs","contents":"fn main(){}"}"#,
            expected
        ));
        assert!(!is_plan_file_edit(
            "bash",
            r#"{"command":"echo hi"}"#,
            expected
        ));
    }

    /// 回归：两个分页（`main#1` / `main#2`）必须解析到**不同的**计划文件，
    /// 否则第 2 页的计划会覆盖第 1 页的（这是改 cwd 相对路径之前的老缺陷）。
    #[test]
    fn tab_pages_get_distinct_plan_paths() {
        let ctx = Context::new();
        let p1 = Sessions::tab(ctx.clone(), 2);
        p1.pin_plan_cwd("/work/alpha");
        let p2 = Sessions::tab(ctx.clone(), 2);
        p2.pin_plan_cwd("/work/beta");
        let other = Sessions::tab(ctx, 3);
        other.pin_plan_cwd("/work/alpha");
        let a = plan_path_for(Some(&p1));
        let b = plan_path_for(Some(&p2));
        let c = plan_path_for(Some(&other));
        assert_ne!(a, b, "不同工作区的 main#2 不能写同一个计划");
        assert_ne!(a, c, "同一工作区的两个分页不能共用一个计划文件");
        for path in [&a, &b, &c] {
            let lossy = path.to_string_lossy();
            assert!(
                !lossy.contains("/sessions/tabs/") && !lossy.contains("\\sessions\\tabs\\"),
                "cwd-key 被丢掉了: {path:?}"
            );
            let comps: Vec<String> = path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            let tabs = comps.iter().position(|c| c == "tabs").expect("{path:?}");
            assert_ne!(comps[tabs - 1], "sessions", "{path:?}");
        }
        assert!(a.to_string_lossy().contains("main#2"), "{a:?}");
        assert!(c.to_string_lossy().contains("main#3"), "{c:?}");
        let key = crate::session::persist::encode_cwd_dirname(std::path::Path::new("/work/alpha"));
        assert!(a.to_string_lossy().contains(&key), "{a:?}");
    }

    /// 回归：计划文件按页划分后，只有写**本页**的 session 目录才算计划编辑——
    /// 第 2 页的 `write_file` 不能被当成第 1 页的计划编辑而绕过权限门。
    #[test]
    fn plan_file_edit_only_matches_own_session_path() {
        let mine = std::path::Path::new("/dock/sessions/aaa/plan.md");
        let theirs = std::path::Path::new("/dock/sessions/bbb/plan.md");
        assert!(is_plan_file_edit(
            "write_file",
            r##"{"target_file":"/dock/sessions/aaa/plan.md","contents":"# x"}"##,
            mine
        ));
        assert!(!is_plan_file_edit(
            "write_file",
            r##"{"target_file":"/dock/sessions/bbb/plan.md","contents":"# x"}"##,
            mine
        ));
        assert!(
            !is_plan_file_edit(
                "write_file",
                r##"{"target_file":".dock/plan.md","contents":"# x"}"##,
                theirs
            ),
            "相对路径不再是本页计划，不能给每一页开一张共用写门"
        );
    }

    /// 回归：分页计划目录带 `<pid>` 段。`main#N` 每个进程都从 2 开始编号，
    /// 不隔开的话，另一个 dock 进程开它的第 2 页会把这一进程第 2 页正在写的
    /// 计划删掉（`discard_ephemeral_plan` 按页号删目录）。
    #[test]
    fn tab_plan_dir_is_scoped_to_this_process() {
        let ctx = Context::new();
        let page = Sessions::tab(ctx, 2);
        page.pin_plan_cwd("/work/alpha");
        let path = plan_path_for(Some(&page));
        let comps: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let pid = std::process::id().to_string();
        assert!(
            comps.iter().any(|c| c == &pid),
            "分页计划目录该带 pid 段: {path:?}"
        );
        assert!(comps.iter().any(|c| c == "main#2"), "{path:?}");
    }

    /// 回归：模型回传的计划路径可能正规化过（`..` / `.` / 符号链接前缀），
    /// 逐字比较会连「写自己的计划」一起挡下，计划模式下没有别的出路。
    /// 两侧 canonicalize 后该放行同名同位置的文件，别的文件仍挡。
    #[test]
    fn plan_gate_accepts_normalized_variants_of_the_same_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plan = dir.path().join("plan.md");
        std::fs::write(&plan, "").unwrap();
        std::fs::write(dir.path().join("other.md"), "").unwrap();

        // 模型回传的路径可能带 `.` / `..` 这类冗余段，或走过符号链接前缀
        // （macOS 的 /var ↔ /private/var）。两侧 canonicalize 后该认成同一个文件。
        let dotted = dir.path().join(".").join("plan.md");
        assert!(path_targets_plan_file(&dotted.to_string_lossy(), &plan));
        let name = dir.path().file_name().expect("tempdir has a name");
        let up_down = dir.path().parent().unwrap().join(name).join("plan.md");
        assert!(
            path_targets_plan_file(&up_down.to_string_lossy(), &plan),
            "{} 该是 {} 的正规化写法",
            up_down.display(),
            plan.display()
        );
        assert!(
            path_targets_plan_file(
                &std::fs::canonicalize(&plan)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| plan.to_string_lossy().into_owned()),
                &plan
            ),
            "符号链接前缀的正规化写法也该认"
        );
        assert!(!path_targets_plan_file(
            &dir.path().join("other.md").to_string_lossy(),
            &plan
        ));
        // canonicalize 不了（文件不存在）时回落逐字比较，仍只认这一个文件。
        assert!(!path_targets_plan_file(
            &dir.path().join("missing.md").to_string_lossy(),
            &plan
        ));
        assert!(path_targets_plan_file(&plan.to_string_lossy(), &plan));
    }

    #[test]
    fn pending_does_not_gate_until_active() {
        let ctx = Context::new();
        let plan = PlanMode::new(ctx);
        plan.enter_pending();
        assert!(plan.active());
        assert!(!plan.gated());
        plan.promote_pending();
        assert!(plan.gated());
    }

    #[test]
    fn reminder_alternates_full_then_sparse() {
        let ctx = Context::new();
        let plan = PlanMode::new(ctx);
        plan.enter_pending();
        let first = plan.take_reminder_body().expect("activation");
        assert!(first.contains("计划模式已开启"), "{first}");
        assert!(first.contains("exit_plan_mode"), "{first}");
        let second = plan.take_reminder_body().expect("per-turn");
        assert!(second.contains("计划模式仍开启"), "{second}");
        plan.exit_now();
        let exit = plan.take_reminder_body().expect("exit");
        assert!(exit.contains("已退出计划模式"), "{exit}");
    }

    #[test]
    fn resolve_returns_false_when_empty() {
        let ctx = Context::new();
        let plan = PlanMode::new(ctx);
        assert!(!plan.resolve(PlanDecision::Approve));
        assert!(plan.last_decision().is_none());
    }
}
