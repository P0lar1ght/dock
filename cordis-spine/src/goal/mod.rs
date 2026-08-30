//! Grok `update_goal` + SessionActor drain, mounted as Cordis `"goal"` + `"tools"`.

mod drain;
mod grok_tool;
mod serde_lenient;

use std::sync::Arc;

use cordis::{plugin, Inject, Plugin};

use crate::names::{GOAL, TOOLS};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

pub use drain::GoalState;
pub use grok_tool::{
    goal_composer_fill, goal_continuation_directive, goal_instruction, goal_offer_addon,
    goal_usage_message, render_ack_into_output, GoalUpdateHandle, UpdateGoalInput,
    GOAL_RESERVED_SUBCOMMANDS, UPDATE_GOAL_TOOL_NAME,
};

/// Named `"goal"` service. TUI live-looks `active()`.
pub struct Goal {
    state: Arc<GoalState>,
    pub handle: GoalUpdateHandle,
}

impl Goal {
    pub fn active(&self) -> bool {
        self.state.active()
    }

    pub fn paused(&self) -> bool {
        self.state.paused()
    }

    /// Running or paused — TUI chrome / overlay still have a goal.
    pub fn present(&self) -> bool {
        self.state.present()
    }

    pub fn title(&self) -> String {
        self.state.title.lock().unwrap().clone()
    }

    /// Last progress note from `update_goal` (empty until the first message).
    pub fn status(&self) -> String {
        self.state.status()
    }

    pub fn start(&self, title: impl Into<String>) {
        self.state.start(title);
    }

    pub fn pause(&self) -> bool {
        self.state.pause()
    }

    pub fn resume(&self) -> bool {
        self.state.resume()
    }

    pub fn clear(&self) {
        self.state.clear();
    }

    pub fn set_title(&self, title: impl Into<String>) {
        self.state.set_title(title);
    }

    pub fn arm_composer(&self) {
        self.state.arm_composer();
    }

    pub fn disarm_composer(&self) {
        self.state.disarm_composer();
    }

    pub fn awaiting_composer(&self) -> bool {
        self.state.awaiting_composer()
    }
}

pub fn tool_goal() -> Plugin {
    plugin("tool-goal", Inject::from([TOOLS]), |ctx, _: &()| {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let state = Arc::new(GoalState::new());
        {
            let state = state.clone();
            tokio::spawn(drain::drain_loop(state, rx));
        }
        ctx.provide(
            GOAL,
            Goal {
                state,
                handle: GoalUpdateHandle(tx),
            },
        )?;
        let tools = ctx.require::<Tools>(TOOLS)?;
        let body: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
                Box::pin(async move { run_update_goal(&ctx, call).await })
            })
        };
        own_registered(
            ctx,
            vec![tools.register(
                ToolSpec {
                    name: UPDATE_GOAL_TOOL_NAME.into(),
                    description: "Set a goal for a multi-step task, or report progress on the active goal. Call with objective to start or retitle. Then message for progress, completed:true when done, blocked_reason when stuck after 3+ failures.".into(),
                    parameters_json: r#"{"type":"object","properties":{"objective":{"type":"string","description":"Set or replace the goal. Call this when the task is multi-step and should run until complete. Omit for a one-shot question."},"completed":{"type":"boolean","description":"Set to true ONLY when the goal is fully achieved. This ends goal mode. Use together with message to include a completion summary."},"message":{"type":"string"},"blocked_reason":{"type":"string"}}}"#.into(),
                },
                body,
            )?],
        )?;
        Ok(None)
    })
}

/// Copied from Grok `UpdateGoalTool::run` (handle → oneshot ack → render).
async fn run_update_goal(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let input: UpdateGoalInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: {e}")),
    };
    let Some(goal) = ctx.get::<Goal>(GOAL) else {
        return tool_result(
            call,
            "No active goal to update (GoalUpdateHandle not registered)",
        );
    };
    let fallback_summary = grok_tool::build_summary(&input);
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    if goal.handle.0.send((input, ack_tx)).is_err() {
        return tool_result(
            call,
            "Goal update channel closed — the session may be shutting down",
        );
    }
    let ack = match ack_rx.await {
        Ok(ack) => ack,
        Err(_) => {
            return tool_result(
                call,
                format!(
                    "Goal-update harness dropped the response channel before producing an \
                     ack. This is a harness-side bug; the goal may not have been updated. \
                     Original intent: {fallback_summary}"
                ),
            );
        }
    };
    match render_ack_into_output(ack) {
        Ok(out) => tool_result(
            call,
            serde_json::to_string(&out).unwrap_or_else(|_| out.summary),
        ),
        Err(e) => tool_result(call, format!("{}: {}", e.kind, e.detail)),
    }
}
