use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use cordis::Context;

use crate::agent_presets::AgentPresets;
use crate::agents::Agents;
use crate::compact::Compact;
use crate::error::{Error, Result};
use crate::goal::continuation_reminder as goal_continuation_reminder;
use crate::llm::Llm;
use crate::names::{
    AGENTS, AGENT_PRESETS, COMPACT, LLM, PRE_STEP, SESSIONS, SUBAGENTS, SYSTEM_PROMPT, TODOS,
    TOOLS, TURN, TURN_END,
};
use crate::prompt::SystemPrompt;
use crate::session::{Sessions, ROOT_IDENTITY};
use crate::task::Subagents;
use crate::todo_write::{stale_reminder, Todos};
use crate::tools::Tools;
use crate::turn::TurnControl;
use crate::types::{LogEvent, PreStep, PromptRequest, TurnEnd, TurnOutcome};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Grok `max_turns` defaults to `None` (unlimited). Dock keeps a high safety
/// cap so a runaway tool loop can still be Esc-cancelled and leaves a visible
/// note instead of hanging. The old cap of 8 cut ordinary coding turns short.
const MAX_STEPS: usize = 256;
/// Hard stop on `agent/turn-end` continuations. Handlers self-limit, but
/// continuation is plugin-territory now (a dynamic Cordis plugin on disk can
/// register one), so the loop keeps its own backstop. Esc — the per-step
/// `abort_if_cancelled` — is the second line, not the only one.
const MAX_TURN_END_ROUNDS: usize = 64;
/// Sampling steps without a `todo_write` before the mid-turn nudge fires
/// (Grok `TodoNudgeConfig::turns_since_todo_write`, which ships unimplemented).
const TODO_NUDGE_AFTER_STEPS: usize = 6;
/// Nudges per prompt. Two reminders are a hint; five are noise.
const MAX_TODO_NUDGES: usize = 3;

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

/// Grok `PromptOrigin::GoalSummary`: continue an active goal without a user
/// bubble. Not a turn *end*, so it injects the same reminder directly instead
/// of running the `agent/turn-end` chain.
async fn grok_continue_goal(ctx: &Context) -> Result<TurnOutcome> {
    let Some(reminder) = goal_continuation_reminder(ctx) else {
        return Ok(TurnOutcome::Text(String::new()));
    };
    let sessions = ctx.require::<Sessions>(SESSIONS)?;
    sessions.seal_incomplete_tool_calls();
    sessions.append(LogEvent::SystemReminder(reminder));
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
    let mut rounds = 0usize;
    // `"todos"` is not isolated per child (only sessions / turn / agentPresets
    // are), so a subagent sees the parent's list. Nagging it about work it was
    // never given would be wrong — todo discipline is the main session's.
    let mut todo = TodoWatch::new(ctx, sessions.identity() == ROOT_IDENTITY);
    let mut last_text = String::new();
    loop {
        tokio::task::yield_now().await;
        let mut ended_with_text = false;
        for _ in 0..MAX_STEPS {
            abort_if_cancelled(ctx, sessions)?;
            drain_parent_mailbox(ctx, sessions);
            if let Some(text) = todo.stale_nudge(ctx) {
                sessions.append(LogEvent::SystemReminder(text));
            }
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
                        history: sessions.model_history(),
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
                    images: result.images,
                });
            }
        }
        // `agent/turn-end`: goal / todo / anything else gets a say before the
        // turn hands control back. The loop only counts rounds and appends the
        // winning reminder — the policy lives in the handlers.
        if rounds < MAX_TURN_END_ROUNDS {
            if let Some(reminder) = turn_end_decision(
                ctx,
                sessions,
                if ended_with_text { &last_text } else { "" },
                rounds,
                ended_with_text,
            ) {
                rounds += 1;
                sessions.append(LogEvent::SystemReminder(reminder));
                continue;
            }
        }
        if ended_with_text {
            return Ok(TurnOutcome::Text(last_text));
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

/// Run `agent/turn-end` and return the winning reminder, if any. No handlers
/// (or no votes) means the turn may end — fail-open.
fn turn_end_decision(
    ctx: &Context,
    sessions: &Sessions,
    text: &str,
    rounds: usize,
    ended_with_text: bool,
) -> Option<String> {
    let end = TurnEnd::new(
        text,
        rounds,
        ended_with_text,
        sessions.has_queued_followups(),
        sessions.identity(),
    );
    let seed = end.clone();
    ctx.waterfall(TURN_END, end, move || seed)
        .decision()
        .map(str::to_string)
}

/// Mid-turn `todo_write` watchdog. Counts sampling steps since the list last
/// moved; nudges when the model is clearly working but not checking items off.
struct TodoWatch {
    /// `Todos::revision` at the last step, or `None` when no list is mounted.
    seen_revision: Option<u64>,
    steps_since_write: usize,
    nudges: usize,
    /// False in subagent turns: `"todos"` is shared with children, so only the
    /// main session gets nagged about the list.
    armed: bool,
}

impl TodoWatch {
    fn new(ctx: &Context, armed: bool) -> Self {
        Self {
            seen_revision: ctx.get::<Todos>(TODOS).map(|t| t.revision()),
            steps_since_write: 0,
            nudges: 0,
            armed,
        }
    }

    /// Call once per sampling step. `Some(reminder)` when the list has gone
    /// stale for [`TODO_NUDGE_AFTER_STEPS`] steps with work still open.
    fn stale_nudge(&mut self, ctx: &Context) -> Option<String> {
        if !self.armed {
            return None;
        }
        let todos = ctx.get::<Todos>(TODOS)?;
        let revision = todos.revision();
        if self.seen_revision != Some(revision) {
            self.seen_revision = Some(revision);
            self.steps_since_write = 0;
            return None;
        }
        self.steps_since_write += 1;
        if self.nudges >= MAX_TODO_NUDGES || self.steps_since_write < TODO_NUDGE_AFTER_STEPS {
            return None;
        }
        if todos.stats().open() == 0 {
            return None;
        }
        self.nudges += 1;
        let steps = std::mem::take(&mut self.steps_since_write);
        Some(stale_reminder(steps))
    }
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

fn system_cache_key(ctx: &Context) -> (String, PathBuf) {
    let mode = ctx
        .get::<AgentPresets>(AGENT_PRESETS)
        .map(|p| p.current_id())
        .unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    (mode, cwd)
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
