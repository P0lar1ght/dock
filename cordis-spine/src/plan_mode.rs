//! Plan collaboration state. Registers `enter_plan_mode` / `exit_plan_mode`.
//!
//! Prompt strings copied from Grok `EnterPlanModeOutput` / `ExitPlanModeOutput`
//! `to_prompt_format`. Plan file is `.dock/plan.md` (Grok uses `.grok/plan.md`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cordis::{plugin, Inject, Plugin};

use crate::names::{PLAN_MODE, TOOLS};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

const PLAN_REL: &str = ".dock/plan.md";

/// Named `"planMode"` service. Live-lookup; do not capture the Arc.
pub struct PlanMode {
    active: Mutex<bool>,
}

impl PlanMode {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(false),
        }
    }

    pub fn active(&self) -> bool {
        *self.active.lock().unwrap()
    }

    pub fn set(&self, active: bool) {
        *self.active.lock().unwrap() = active;
    }

    pub fn toggle(&self) -> bool {
        let mut on = self.active.lock().unwrap();
        *on = !*on;
        *on
    }
}

impl Default for PlanMode {
    fn default() -> Self {
        Self::new()
    }
}

pub fn plan_mode() -> Plugin {
    plugin(
        "plan-mode",
        Inject::from([TOOLS]),
        |ctx, _: &()| {
            ctx.provide(PLAN_MODE, PlanMode::new())?;
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
                    Box::pin(async move { exit_plan(&ctx, call) })
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
        },
    )
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
        Ok(m) if m.is_dir() => Seed::Missing("A directory already exists at that path."),
        Ok(m) if m.len() == 0 => Seed::Empty,
        Ok(_) => Seed::NonEmpty,
        Err(_) => {
            if let Some(parent) = path.parent() {
                if std::fs::create_dir_all(parent).is_err() {
                    return Seed::Missing("The plan file location is unavailable.");
                }
            }
            match std::fs::write(path, "") {
                Ok(()) => Seed::Empty,
                Err(_) => Seed::Missing("The file has not yet been created."),
            }
        }
    }
}

fn enter_plan(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    if let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) {
        plan.set(true);
    }
    let path = plan_path();
    let display = path.display().to_string();
    let seed = probe_or_create(&path);
    let plan_status = match seed {
        Seed::Empty => format!("Write your plan to {display}. The file exists and is empty."),
        Seed::NonEmpty => {
            format!("Write your plan to {display}. The file exists but is not empty.")
        }
        Seed::Missing(detail) => format!("Write your plan to {display}. {detail}"),
    };
    let ask = "ask_user_question";
    let exit = "exit_plan_mode";
    tool_result(
        call,
        format!(
            "entered plan mode\n\n\
             {plan_status}\n\n\
             In plan mode, you should:\n\
             1. Thoroughly explore the codebase to understand existing patterns\n\
             2. Identify similar features, codebase architecture, and understand trade-offs\n\
             3. Use {ask} if you need to clarify the approach\n\
             4. Design a concrete implementation strategy\n\
             5. Write your plan to the plan file above\n\
             6. When ready, use {exit} to present your plan to the user."
        ),
    )
}

fn exit_plan(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let Some(plan) = ctx.get::<PlanMode>(PLAN_MODE) else {
        return tool_result(call, "plan mode is not mounted");
    };
    if !plan.active() {
        return tool_result(call, "Not in plan mode.");
    }
    plan.set(false);
    let path = plan_path();
    let display = path.display().to_string();
    match std::fs::read_to_string(&path) {
        Ok(content) if content.trim().is_empty() => tool_result(
            call,
            "Plan mode exited, but the plan file is empty. You may now make file and shell changes.",
        ),
        Ok(content) => tool_result(
            call,
            format!(
                "Plan mode exited. You may now make file and shell changes.\n\n\
                 Your plan has been saved at: {display}\n\n\
                 ## Plan:\n{content}"
            ),
        ),
        Err(_) => tool_result(
            call,
            "Plan mode exited. You may now make file and shell changes.",
        ),
    }
}
