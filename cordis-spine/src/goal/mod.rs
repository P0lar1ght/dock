//! Grok `update_goal` + SessionActor drain, mounted as Cordis `"goal"` + `"tools"`.

mod drain;
mod grok_tool;
mod serde_lenient;

use std::sync::Arc;

use cordis::{plugin, Inject, Plugin};

use crate::names::{GOAL, PRE_STEP, SESSIONS, TOOLS, TOOLS_EXECUTE, TURN_END};
use crate::session::Sessions;
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{
    LogEvent, PreStep, ToolCall, ToolResult, ToolSpec, TurnEnd, ORDER_TURN_END_GOAL,
};

/// Grok keeps `/goal` on an outer turn loop until `update_goal(completed)` (or
/// cancel). The agent loop's `MAX_STEPS` is per round, not per goal.
const MAX_GOAL_ROUNDS: usize = 64;

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

    /// One-shot standing contract for the current objective (slash or
    /// `update_goal`). Injected as a history-tail reminder, not system prompt.
    pub fn take_instruction(&self) -> Option<String> {
        self.state.take_instruction()
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
        let ctx_pre = ctx.clone();
        let _ = ctx.on_waterfall(PRE_STEP, move |step: PreStep, args| {
            let next = args.next::<PreStep>().unwrap_or(step);
            // Main session only: `take_instruction` is one-shot, so a child
            // turn would eat the objective the user's own next turn should see
            // — and drop it into the parent's history at that.
            if next.enter && next.is_main_session() {
                inject_goal_instruction(&ctx_pre);
            }
            next
        });
        let ctx_end = ctx.clone();
        let _ = ctx.on_waterfall(TURN_END, move |end: TurnEnd, args| {
            let mut next = args.next::<TurnEnd>().unwrap_or(end);
            // Main session only. `"goal"` is not isolated per subagent (the
            // child runner isolates `sessions` / `turn` / `agentPresets`), so
            // without this a child turn is continued by its parent's goal all
            // the way to the hard stop. The identity rides on the payload
            // because a waterfall handler cannot see the executing context.
            if !next.is_main_session() {
                return next;
            }
            // The user queued the next message: they steer, not the goal loop.
            if next.queued_followups || next.rounds >= MAX_GOAL_ROUNDS {
                return next;
            }
            // Both endings continue a goal: text without `update_goal(completed)`
            // is stopping short, and a blown step budget still leaves the goal open.
            if let Some(body) = continuation_reminder(&ctx_end) {
                next.keep_working(ORDER_TURN_END_GOAL, body);
            }
            next
        });
        let ctx_exec = ctx.clone();
        let _ = ctx.on_waterfall(TOOLS_EXECUTE, move |result: ToolResult, args| {
            let result = args.next::<ToolResult>().unwrap_or(result);
            if result.name == UPDATE_GOAL_TOOL_NAME {
                inject_goal_instruction(&ctx_exec);
            }
            result
        });
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
            vec![tools.register_deferred(
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

/// `<system-reminder>` body that keeps an active goal going, or `None` when no
/// goal is running. Shared by the `agent/turn-end` handler and the GoalSummary
/// entry (`LoopHandle::continue_goal`), which injects it without a turn end.
pub fn continuation_reminder(ctx: &cordis::Context) -> Option<String> {
    let goal = ctx.get::<Goal>(GOAL)?;
    if !goal.active() {
        return None;
    }
    let title = goal.title();
    let objective = title.trim();
    Some(goal_continuation_directive(if objective.is_empty() {
        "目标"
    } else {
        objective
    }))
}

fn inject_goal_instruction(ctx: &cordis::Context) {
    let Some(goal) = ctx.get::<Goal>(GOAL) else {
        return;
    };
    let Some(body) = goal.take_instruction() else {
        return;
    };
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return;
    };
    sessions.append(LogEvent::SystemReminder(format!(
        "<system-reminder>\n{body}\n</system-reminder>"
    )));
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
        Ok(out) => tool_result(call, serde_json::to_string(&out).unwrap_or(out.summary)),
        Err(e) => tool_result(call, format!("{}: {}", e.kind, e.detail)),
    }
}
