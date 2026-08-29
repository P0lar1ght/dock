use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use cordis::Context;

use crate::agent_presets::AgentPresets;
use crate::agents::Agents;
use crate::compact::Compact;
use crate::error::{Error, Result};
use crate::goal::{goal_continuation_directive, Goal};
use crate::llm::Llm;
use crate::names::{
    AGENTS, AGENT_PRESETS, COMPACT, GOAL, LLM, PLAN_MODE, PRE_STEP, SESSIONS, SUBAGENTS,
    SYSTEM_PROMPT, TOOLS, TURN,
};
use crate::plan_mode::PlanMode;
use crate::prompt::SystemPrompt;
use crate::session::Sessions;
use crate::task::Subagents;
use crate::tools::Tools;
use crate::turn::TurnControl;
use crate::types::{LogEvent, PreStep, PromptRequest, TurnOutcome};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Grok `max_turns` defaults to `None` (unlimited). Dock keeps a high safety
/// cap so a runaway tool loop can still be Esc-cancelled and leaves a visible
/// note instead of hanging. The old cap of 8 cut ordinary coding turns short.
const MAX_STEPS: usize = 256;
/// Grok keeps `/goal` on an outer turn loop until `update_goal(completed)`
/// (or cancel). Inner `MAX_STEPS` is per round, not the whole goal.
const MAX_GOAL_ROUNDS: usize = 64;

/// One Grok-shaped turn: pre-step once, then sample → tools → sample until text.
///
/// The driver is the only thing the loop plugin owns. Services are live-looked
/// up from `ctx` at the call site.
pub trait Driver: Send + Sync {
    fn handle_prompt<'a>(
        &'a self,
        ctx: &'a Context,
        prompt: String,
    ) -> BoxFuture<'a, Result<TurnOutcome>>;
}

/// Default driver: Grok `SessionActor` inner loop, one user prompt.
pub struct GrokStep;

impl Driver for GrokStep {
    fn handle_prompt<'a>(
        &'a self,
        ctx: &'a Context,
        prompt: String,
    ) -> BoxFuture<'a, Result<TurnOutcome>> {
        Box::pin(async move { grok_turn(ctx, prompt).await })
    }
}

async fn grok_turn(ctx: &Context, prompt: String) -> Result<TurnOutcome> {
    let sessions = ctx.require::<Sessions>(SESSIONS)?;
    let llm = ctx.require::<Llm>(LLM)?;
    let tools = ctx.require::<Tools>(TOOLS)?;
    let system_prompt = ctx.require::<SystemPrompt>(SYSTEM_PROMPT)?;
    let agents = ctx.require::<Agents>(AGENTS)?;

    agents.ensure("main");
    sessions.seal_incomplete_tool_calls();
    sessions.set_auto_compact_suppressed(false);
    sessions.append(LogEvent::User(prompt.clone()));

    let decision = {
        let user = prompt.clone();
        ctx.waterfall(
            PRE_STEP,
            PreStep {
                user: user.clone(),
                enter: true,
            },
            move || PreStep { user, enter: true },
        )
    };
    if !decision.enter {
        return Err(Error::PreStepRejected);
    }
    sessions.append(LogEvent::PreStep);

    let system = system_prompt.assemble_on(ctx);
    sessions.append(LogEvent::Prompt(system.clone()));
    grok_sample_loop(ctx, &sessions, &llm, &tools, system).await
}

/// Parent mailbox: sample without a new user bubble so `report` reaches the model.
async fn grok_continue_mailbox(ctx: &Context) -> Result<TurnOutcome> {
    let sessions = ctx.require::<Sessions>(SESSIONS)?;
    if sessions.identity() != "main" {
        return Ok(TurnOutcome::Text(String::new()));
    }
    let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
        return Ok(TurnOutcome::Text(String::new()));
    };
    if !sub.has_parent_notices() {
        return Ok(TurnOutcome::Text(String::new()));
    }
    sessions.seal_incomplete_tool_calls();
    let llm = ctx.require::<Llm>(LLM)?;
    let tools = ctx.require::<Tools>(TOOLS)?;
    let system_prompt = ctx.require::<SystemPrompt>(SYSTEM_PROMPT)?;
    let system = system_prompt.assemble_on(ctx);
    grok_sample_loop(ctx, &sessions, &llm, &tools, system).await
}

/// Grok `PromptOrigin::GoalSummary`: continue an active goal without a user bubble.
async fn grok_continue_goal(ctx: &Context) -> Result<TurnOutcome> {
    if !goal_keeps_working(ctx) {
        return Ok(TurnOutcome::Text(String::new()));
    }
    let sessions = ctx.require::<Sessions>(SESSIONS)?;
    sessions.seal_incomplete_tool_calls();
    inject_goal_continuation(ctx, &sessions);
    let llm = ctx.require::<Llm>(LLM)?;
    let tools = ctx.require::<Tools>(TOOLS)?;
    let system_prompt = ctx.require::<SystemPrompt>(SYSTEM_PROMPT)?;
    let system = system_prompt.assemble_on(ctx);
    grok_sample_loop(ctx, &sessions, &llm, &tools, system).await
}

async fn grok_sample_loop(
    ctx: &Context,
    sessions: &Sessions,
    llm: &Llm,
    tools: &Tools,
    mut system: String,
) -> Result<TurnOutcome> {
    let system_prompt = ctx.require::<SystemPrompt>(SYSTEM_PROMPT)?;
    let mut cache_key = system_cache_key(ctx);
    let mut goal_rounds = 0usize;
    let mut last_text = String::new();
    loop {
        tokio::task::yield_now().await;
        let mut ended_with_text = false;
        for _ in 0..MAX_STEPS {
            abort_if_cancelled(ctx, sessions)?;
            drain_parent_mailbox(ctx, sessions);
            if let Some(compact) = ctx.get::<Compact>(COMPACT) {
                match compact.maybe_auto(ctx).await {
                    Ok(_) => {}
                    Err(Error::Cancelled) => {
                        sessions.seal_incomplete_tool_calls();
                        return Err(Error::Cancelled);
                    }
                    Err(err) => {
                        sessions.append(LogEvent::LlmStream(crate::types::LlmOutput {
                            text: format!("自动压缩失败：{err}"),
                            ..crate::types::LlmOutput::default()
                        }));
                    }
                }
            }
            let key = system_cache_key(ctx);
            if key != cache_key {
                cache_key = key;
                system = system_prompt.assemble_on(ctx);
            }
            let output = llm
                .stream_on(
                    ctx,
                    PromptRequest {
                        system: system.clone(),
                        history: sessions.events(),
                        tools: tools.specs_for_model_on(ctx),
                    },
                )
                .await;
            abort_if_cancelled(ctx, sessions)?;
            if output.tool_calls.is_empty() {
                last_text = output.text;
                ended_with_text = true;
                break;
            }
            for call in output.tool_calls {
                abort_if_cancelled(ctx, sessions)?;
                let arguments = call.arguments.clone();
                let result = tools.execute_on(ctx, call).await;
                sessions.append(LogEvent::ToolExecute {
                    id: result.call_id,
                    name: result.name,
                    arguments,
                    content: result.content,
                });
            }
        }
        if ended_with_text {
            if goal_keeps_working(ctx) && sessions.has_queued_followups() {
                return Ok(TurnOutcome::Text(last_text));
            }
            if goal_keeps_working(ctx) && goal_rounds < MAX_GOAL_ROUNDS {
                goal_rounds += 1;
                inject_goal_continuation(ctx, sessions);
                continue;
            }
            return Ok(TurnOutcome::Text(last_text));
        }
        if goal_keeps_working(ctx) && goal_rounds < MAX_GOAL_ROUNDS {
            goal_rounds += 1;
            inject_goal_continuation(ctx, sessions);
            continue;
        }
        sessions.append(LogEvent::LlmStream(crate::types::LlmOutput {
            text: format!(
                "本轮已连续采样 {MAX_STEPS} 步仍无文本回复，已停下。发一条新消息可继续。"
            ),
            ..crate::types::LlmOutput::default()
        }));
        return Err(Error::MaxSteps { max: MAX_STEPS });
    }
}

fn goal_keeps_working(ctx: &Context) -> bool {
    ctx.get::<Goal>(GOAL).is_some_and(|g| g.active())
}

/// Continuable `subagent` mailbox: inject after a complete tool round (or at
/// turn start), never between `tool_calls` and their `ToolExecute` rows.
fn drain_parent_mailbox(ctx: &Context, sessions: &Sessions) {
    if sessions.identity() != "main" {
        return;
    }
    let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
        return;
    };
    for text in sub.drain_parent_notices() {
        sessions.append(LogEvent::SystemReminder(text));
    }
}

fn inject_goal_continuation(ctx: &Context, sessions: &Sessions) {
    let objective = ctx
        .get::<Goal>(GOAL)
        .map(|g| g.title())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "目标".into());
    sessions.append(LogEvent::SystemReminder(goal_continuation_directive(
        &objective,
    )));
}

fn system_cache_key(ctx: &Context) -> (String, PathBuf, bool, Option<String>) {
    let mode = ctx
        .get::<AgentPresets>(AGENT_PRESETS)
        .map(|p| p.current_id())
        .unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let plan = ctx.get::<PlanMode>(PLAN_MODE).is_some_and(|p| p.gated());
    let goal = ctx
        .get::<Goal>(GOAL)
        .and_then(|g| if g.active() { Some(g.title()) } else { None });
    (mode, cwd, plan, goal)
}

fn cancelled(ctx: &Context) -> bool {
    ctx.get::<TurnControl>(TURN)
        .is_some_and(|t| t.is_cancelled())
}

fn abort_if_cancelled(ctx: &Context, sessions: &Sessions) -> Result<()> {
    if cancelled(ctx) {
        sessions.seal_incomplete_tool_calls();
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

pub struct LoopHandle {
    ctx: Context,
    driver: Arc<dyn Driver>,
}

impl LoopHandle {
    pub fn new(ctx: Context, driver: Arc<dyn Driver>) -> Self {
        Self { ctx, driver }
    }

    pub async fn run(&self, prompt: impl Into<String>) -> Result<TurnOutcome> {
        self.driver.handle_prompt(&self.ctx, prompt.into()).await
    }

    /// Drain `report` / first-idle notices into the parent session and sample.
    pub async fn continue_mailbox(&self) -> Result<TurnOutcome> {
        grok_continue_mailbox(&self.ctx).await
    }

    /// Hidden GoalSummary turn: reminder then sample, no user bubble.
    pub async fn continue_goal(&self) -> Result<TurnOutcome> {
        grok_continue_goal(&self.ctx).await
    }
}
