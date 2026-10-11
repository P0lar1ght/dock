//! Grok `update_goal` + SessionActor drain, mounted as Cordis `"goal"` + `"tools"`.

mod command;
mod drain;
mod grok_tool;
mod serde_lenient;

use std::sync::Arc;

use cordis::{plugin, Inject, Plugin};

use crate::names::{GOAL, PRE_STEP, SESSIONS, STEP_START, TOOLS, TOOLS_EXECUTE, TURN_END};
use crate::session::log::Sessions;
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::{
    LogEvent, NoticeKind, PreStep, StepStart, ToolCall, ToolResult, ToolSpec, TurnEnd,
    ORDER_TURN_END_GOAL,
};

/// Grok keeps `/goal` on an outer turn loop until `update_goal(completed)` (or
/// cancel). The agent loop's `MAX_STEPS` is per round, not per goal.
const MAX_GOAL_ROUNDS: usize = 64;

pub use command::{goal_command, start_goal};
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

    /// 上一个目标由模型报完成（`update_goal(completed)`）收尾；`start` / `clear` 后复位。
    pub fn completed(&self) -> bool {
        self.state.completed()
    }

    /// 目标实际在跑的时长，暂停的时段不算。
    pub fn elapsed(&self) -> std::time::Duration {
        self.state.elapsed()
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

/// Named `"goal"` service + the waterfall wiring. Mounted once per isolate
/// subtree that wants its own goal: the root session and every tab page. The
/// `update_goal` tool itself is registered separately by
/// [`goal_tool_registration`], once, against the global `"tools"` table.
pub fn goal_service() -> Plugin {
    plugin("tool-goal", Inject::new(), |ctx, _: &()| {
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
        wire_goal_waterfalls(ctx);
        Ok(None)
    })
}

/// Register the single `update_goal` tool against the global `"tools"` table.
/// Registered once; the body dispatches on the **executing** context so a call
/// from any tab page reaches that page's own `"goal"` service.
pub fn goal_tool_registration() -> Plugin {
    plugin("tool-goal.tools", Inject::from([TOOLS]), |ctx, _: &()| {
        let tools = ctx.require::<Tools>(TOOLS)?;
        let root_ctx = ctx.clone();
        let body: ToolBody = {
            std::sync::Arc::new(move |call| {
                // `GOAL` is isolated per tab page, so resolve the caller's
                // subtree, not the (root) one that registered the tool.
                let ctx = crate::tools::registry::exec_ctx().unwrap_or_else(|| root_ctx.clone());
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

/// Backwards-compatible single mount: service + tool registration together.
/// New code mounts [`goal_service`] per isolate subtree and
/// [`goal_tool_registration`] once at the root.
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
        wire_goal_waterfalls(ctx);
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

/// Per-page listeners. Waterfalls reach every isolate, so each handler acts
/// only for the page it was mounted on. A subagent identity matches neither
/// page, which keeps a child turn from consuming or continuing the user's goal.
fn wire_goal_waterfalls(ctx: &cordis::Context) {
    let ctx_pre = ctx.clone();
    let _ = ctx.on_waterfall(PRE_STEP, move |step: PreStep, args| {
        let next = args.next::<PreStep>().unwrap_or(step);
        // One-shot: the wrong page (or a child) would eat the instruction the
        // user's own next turn on this page should see.
        if next.enter && Sessions::turn_is_this_page(&ctx_pre, &next.identity) {
            inject_goal_instruction(&ctx_pre);
            // 用户发了消息：新的一次连续推进。
            if let Some(goal) = ctx_pre.get::<Goal>(GOAL) {
                goal.state.reset_run();
            }
        }
        next
    });
    let ctx_step = ctx.clone();
    let _ = ctx.on_waterfall(STEP_START, move |step: StepStart, args| {
        let next = args.next::<StepStart>().unwrap_or(step);
        if Sessions::turn_is_this_page(&ctx_step, &next.identity) {
            if let Some(goal) = ctx_step.get::<Goal>(GOAL) {
                goal.state.note_sample();
            }
        }
        next
    });
    let ctx_end = ctx.clone();
    let _ = ctx.on_waterfall(TURN_END, move |end: TurnEnd, args| {
        let mut next = args.next::<TurnEnd>().unwrap_or(end);
        if !Sessions::turn_is_this_page(&ctx_end, &next.identity) {
            return next;
        }
        // The user queued the next message: they steer, not the goal loop.
        if next.queued_followups || next.rounds >= MAX_GOAL_ROUNDS {
            return next;
        }
        // 一次连续推进的总预算：轮内 64 次上限每轮归零，会话层又会在每轮结束后再排一次
        // 续跑，主观目标（模型永远不报完成）就会一直跑下去。超了就暂停、告诉用户。
        if pause_if_over_budget(&ctx_end) {
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
        // `tools/execute` carries no identity. The executing ctx is still set
        // around `finish`, so match the `Goal` arc the call would resolve.
        if result.name == UPDATE_GOAL_TOOL_NAME && goal_call_is_this_page(&ctx_exec) {
            inject_goal_instruction(&ctx_exec);
        }
        result
    });
}

/// 这一次连续推进超了预算：暂停目标（不清掉，标题和进展留着），给用户留一张提示卡。
fn pause_if_over_budget(ctx: &cordis::Context) -> bool {
    let Some(goal) = ctx.get::<Goal>(GOAL) else {
        return false;
    };
    if !goal.active() {
        return false;
    }
    let Some((samples, elapsed)) = goal.state.run_exceeded() else {
        return false;
    };
    goal.pause();
    // GUI 的 GoalBar 显示进展说明（Notice 卡只在 TUI 出现）：原因写在前面，模型上次的进展留着。
    {
        let mut status = goal.state.status.lock().unwrap();
        let before = status.trim().to_string();
        *status = format!(
            "自动暂停：这次连续推进已 {samples} 次请求、{} 分钟。{before}",
            elapsed.as_secs() / 60
        );
    }
    if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
        let title = goal.title();
        sessions.append(LogEvent::Notice {
            kind: NoticeKind::Other,
            title: "目标已暂停".into(),
            body: format!(
                "这次连续推进已经 {samples} 次请求、{} 分钟，目标「{}」还没完成，先停下来等你决定。\n\
                 想接着做：恢复目标（GoalBar「继续」或 /goal resume）；想换个做法：直接发消息说明；不做了：/goal clear。",
                elapsed.as_secs() / 60,
                title.trim()
            ),
        });
    }
    true
}

/// True when the in-flight tool call resolves to this page's `"goal"`.
///
/// A child isolate does not get its own goal, so its lookup walks up to the
/// page that spawned it. Another tab's service is a different arc.
fn goal_call_is_this_page(page: &cordis::Context) -> bool {
    let Some(exec) = crate::tools::registry::exec_ctx() else {
        return false;
    };
    match (page.get::<Goal>(GOAL), exec.get::<Goal>(GOAL)) {
        (Some(here), Some(there)) => Arc::ptr_eq(&here, &there),
        _ => false,
    }
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

#[cfg(test)]
mod run_budget_tests {
    use super::*;
    use crate::names::{STEP_START, TURN_END};
    use cordis_base::types::{StepStart, TurnEnd};

    async fn boot() -> (cordis::Context, Sessions) {
        let ctx = cordis::Context::new();
        crate::install_without_llm(&ctx).await.unwrap();
        ctx.plugin(goal_service(), ())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let sessions = (*ctx.require::<Sessions>(SESSIONS).unwrap()).clone();
        (ctx, sessions)
    }

    fn sample(ctx: &cordis::Context, identity: &str, n: usize) {
        for i in 0..n {
            let step = StepStart::new(i, identity);
            let seed = step.clone();
            let _ = ctx.waterfall(STEP_START, step, move || seed);
        }
    }

    fn turn_end(ctx: &cordis::Context, identity: &str) -> TurnEnd {
        let end = TurnEnd::new("这一版做完了", 0, true, false, identity);
        let seed = end.clone();
        ctx.waterfall(TURN_END, end, move || seed)
    }

    /// 回归：目标模式以前没有总上限——每轮结束后只要目标没完成就再续，轮内 64 次的上限每次
    /// 归零。主观目标（「达到 Awwwards 获奖品质」）模型永远不报完成，一条消息跑了 1234 次
    /// 请求。现在一次连续推进超了预算就暂停目标、留一张提示卡给用户。
    #[tokio::test]
    async fn a_goal_pauses_when_one_run_uses_up_its_budget() {
        let (ctx, sessions) = boot().await;
        let identity = sessions.identity().to_string();
        let goal = ctx.require::<Goal>(GOAL).unwrap();
        goal.start("达到获奖品质");
        goal.state
            .set_run_budget(3, std::time::Duration::from_secs(3600));

        sample(&ctx, &identity, 2);
        assert!(
            turn_end(&ctx, &identity).decision().is_some(),
            "预算内照常续跑"
        );
        assert!(goal.active());

        sample(&ctx, &identity, 1);
        assert!(
            turn_end(&ctx, &identity).decision().is_none(),
            "超了预算不再续跑"
        );
        assert!(goal.paused(), "目标要暂停，不是清掉");
        assert!(
            goal.status().starts_with("自动暂停"),
            "GoalBar 要看得到原因：{}",
            goal.status()
        );
        let notice = sessions.events().into_iter().find_map(|e| match e {
            LogEvent::Notice { title, body, .. } => Some(format!("{title} {body}")),
            _ => None,
        });
        let notice = notice.expect("要给用户留一张提示卡");
        assert!(notice.contains("3 次请求"), "{notice}");
        assert!(notice.contains("/goal resume"), "{notice}");

        // 恢复：重新算一次连续推进。
        assert!(goal.resume());
        sample(&ctx, &identity, 1);
        assert!(
            turn_end(&ctx, &identity).decision().is_some(),
            "恢复后重新计"
        );
    }

    /// 用户发了新消息：那是新的一次连续推进，计数清零。
    #[tokio::test]
    async fn a_user_message_starts_a_new_run() {
        let (ctx, sessions) = boot().await;
        let identity = sessions.identity().to_string();
        let goal = ctx.require::<Goal>(GOAL).unwrap();
        goal.start("目标");
        goal.state
            .set_run_budget(3, std::time::Duration::from_secs(3600));
        sample(&ctx, &identity, 3);
        let step = PreStep::new("接着做", true, identity.clone());
        let seed = step.clone();
        let _ = ctx.waterfall(PRE_STEP, step, move || seed);
        sample(&ctx, &identity, 1);
        assert!(turn_end(&ctx, &identity).decision().is_some());
        assert!(goal.active());
    }
}
