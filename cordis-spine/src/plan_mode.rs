//! Plan collaboration state. Registers `enter_plan_mode` / `exit_plan_mode`.
//!
//! Lifecycle (simplified grok `PlanModeTracker`):
//! `Inactive` → `Pending` (user `/plan` / Shift+Tab) → `Active` (first prompt
//! or `enter_plan_mode`) → approval park on `exit_plan_mode` → back to
//! `Inactive` (approve/quit) or stay `Active` (revise).
//!
//! Plan file is `.dock/plan.md` (Grok uses `.grok/plan.md`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cordis::{plugin, Context, Inject, Plugin};
use tokio::sync::oneshot;

use crate::names::{PLAN_EVENT, PLAN_MODE, PRE_STEP, SESSIONS, TOOLS};
use crate::session::Sessions;
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{LogEvent, PreStep, ToolCall, ToolResult, ToolSpec};

pub const PLAN_REL: &str = ".dock/plan.md";

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
    /// Stay in plan mode; revise `.dock/plan.md`.
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
        }
    }

    pub fn phase(&self) -> PlanPhase {
        self.mode.lock().unwrap().phase
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
    pub async fn request_approval(&self, body: String, empty: bool) -> PlanDecision {
        let (tx, rx) = oneshot::channel();
        {
            let mut slot = self.approval.lock().unwrap();
            if let Some(prev) = slot.take() {
                let _ = prev.tx.send(PlanDecision::Quit);
            }
            *slot = Some(PendingApproval {
                prompt: PlanApprovalPrompt {
                    path: PLAN_REL.to_string(),
                    body,
                    empty,
                },
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
        let path = plan_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let empty = content.trim().is_empty();
                Some(PlanApprovalPrompt {
                    path: PLAN_REL.to_string(),
                    body: if empty { String::new() } else { content },
                    empty,
                })
            }
            Err(_) => None,
        }
    }
}

/// Whether this tool call is an edit of the session plan file.
pub fn is_plan_file_edit(tool: &str, arguments: &str) -> bool {
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
    path_targets_plan_file(path)
}

fn path_targets_plan_file(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    if path == PLAN_REL || path.ends_with("/.dock/plan.md") || path.ends_with("\\.dock\\plan.md") {
        return true;
    }
    let p = Path::new(path);
    p.file_name().and_then(|n| n.to_str()) == Some("plan.md")
        && p.parent()
            .and_then(|par| par.file_name())
            .and_then(|n| n.to_str())
            == Some(".dock")
}

pub fn plan_mode() -> Plugin {
    plugin("plan-mode", Inject::from([TOOLS]), |ctx, _: &()| {
        ctx.provide(PLAN_MODE, PlanMode::new(ctx.clone()))?;
        let ctx_pre = ctx.clone();
        let _ = ctx.on_waterfall(PRE_STEP, move |step: PreStep, args| {
            let next = args.next::<PreStep>().unwrap_or(step);
            if next.enter {
                if let Some(plan) = ctx_pre.get::<PlanMode>(PLAN_MODE) {
                    plan.inject_turn_reminder();
                }
            }
            next
        });
        let tools = ctx.require::<Tools>(TOOLS)?;
        let enter: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
                Box::pin(async move { enter_plan(&ctx, call) })
            })
        };
        let exit: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
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

fn plan_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(PLAN_REL)
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
pub fn plan_system_addon() -> &'static str {
    "计划模式已开启。除计划文件外，不要改文件或跑会改环境的命令。\n\n\
     计划写到 `.dock/plan.md`。这是唯一允许编辑的文件。\n\
     若文件还不存在，先调用 enter_plan_mode 创建。\n\n\
     只读探索代码并写出实现计划。需要澄清时用 ask_user_question。\
     准备好后用 exit_plan_mode 把计划交给用户。"
}

fn plan_sparse_addon() -> &'static str {
    "计划模式仍开启。除计划文件外，不要改文件或跑会改环境的命令。"
}

fn plan_exit_addon() -> &'static str {
    "已退出计划模式。现在可以改文件、跑工具。"
}

fn plan_reentry_addon() -> &'static str {
    "再次进入计划模式。先前的计划在 `.dock/plan.md`。除该文件外不要改文件。\
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
    let path = plan_path();
    let display = PLAN_REL;
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

    let path = plan_path();
    let display = PLAN_REL;
    let (body, empty) = match std::fs::read_to_string(&path) {
        Ok(content) if content.trim().is_empty() => (String::new(), true),
        Ok(content) => (content, false),
        Err(_) => (String::new(), true),
    };

    let decision = plan.request_approval(body.clone(), empty).await;
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
    fn plan_file_edit_detects_relative_and_nested() {
        assert!(is_plan_file_edit(
            "write_file",
            r##"{"target_file":".dock/plan.md","contents":"# x"}"##
        ));
        assert!(is_plan_file_edit(
            "search_replace",
            r#"{"file_path":"/repo/.dock/plan.md","old_string":"a","new_string":"b"}"#
        ));
        assert!(!is_plan_file_edit(
            "write_file",
            r#"{"target_file":"src/main.rs","contents":"fn main(){}"}"#
        ));
        assert!(!is_plan_file_edit("bash", r#"{"command":"echo hi"}"#));
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
