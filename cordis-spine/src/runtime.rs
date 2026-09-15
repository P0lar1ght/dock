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
    AGENTS, AGENT_PRESETS, COMPACT, LLM, PRE_STEP, SESSIONS, STEP_START, SUBAGENTS, SYSTEM_PROMPT,
    TOOLS, TURN, TURN_END,
};
use crate::prompt::SystemPrompt;
use crate::session::Sessions;
use crate::task::Subagents;
use crate::tools::Tools;
use crate::turn::TurnControl;
use crate::types::{LogEvent, PreStep, PromptRequest, StepStart, TurnEnd, TurnOutcome};

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
        let step = PreStep::new(prompt.clone(), true, sessions.identity());
        let seed = step.clone();
        ctx.waterfall(PRE_STEP, step, move || seed)
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
    let mut steps = 0usize;
    let mut last_text = String::new();
    loop {
        tokio::task::yield_now().await;
        let mut ended_with_text = false;
        for _ in 0..MAX_STEPS {
            abort_if_cancelled(ctx, sessions)?;
            drain_parent_mailbox(ctx, sessions);
            // `agent/step-start`: whoever wants to watch the turn as it runs
            // (todo staleness today) gets a look before each sample. The loop
            // counts steps and appends — the policy lives in the handlers.
            // handler 要能看见**当前这个** agent 的会话（子代理是它自己的隔离
            // ctx），而 waterfall 不传 ctx —— 挂成 task-local。
            for reminder in
                crate::tools::with_exec_ctx(ctx, || step_start_reminders(ctx, sessions, steps))
            {
                sessions.append(LogEvent::SystemReminder(reminder));
            }
            steps += 1;
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
                let Some(result) = execute_cancellable(ctx, tools, call).await else {
                    // 工具跑到一半被 Stop：给未完成的 tool call 补上中断结果，
                    // 否则下一次采样会因为缺 tool result 400。
                    sessions.seal_incomplete_tool_calls();
                    return Err(Error::Cancelled);
                };
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
    ctx.waterfall(TURN_END, end, move || seed).settle()
}

/// Run `agent/step-start` and collect what to inject before this sample, in
/// slot order. No handlers means no reminders — fail-open.
fn step_start_reminders(ctx: &Context, sessions: &Sessions, step: usize) -> Vec<String> {
    let start = StepStart::new(step, sessions.identity());
    let seed = start.clone();
    ctx.waterfall(STEP_START, start, move || seed)
        .into_reminders()
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

/// 工具执行与取消赛跑。`None` = 工具还在跑的时候用户按了 Stop。
///
/// 裸 `await` 的话 `[stop]` 只是把 `TurnControl` 的 token 置位，没人监听，工具
/// 一旦开始就只能等它自己返回。`bash` 显得能取消纯粹是因为它**自己**在循环里
/// 轮询 `is_cancelled()`；MCP 的 `tools/call` 是 `rx.await`，没有这层，于是
/// 服务器不回 = 整轮对话永远结束不了，Stop 按了也没反应。
///
/// 放在这里而不是让每个工具各自轮询：取消是循环的事，不该要求每个工具体都记得
/// 自己实现一遍。
async fn execute_cancellable(
    ctx: &Context,
    tools: &Tools,
    call: crate::types::ToolCall,
) -> Option<crate::types::ToolResult> {
    let Some(token) = ctx.get::<TurnControl>(TURN).map(|t| t.token()) else {
        return Some(tools.execute_on(ctx, call).await);
    };
    tokio::select! {
        // 两边同时就绪时优先收工具的结果，别把已经跑完的活丢掉。
        biased;
        result = tools.execute_on(ctx, call) => Some(result),
        _ = token.cancelled() => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tools;
    use crate::types::ToolSpec;
    use std::time::Duration;

    /// Stop 必须能打断已经在跑的工具。
    ///
    /// 改之前这里是裸 `await`：`[stop]` 只把 token 置位，没人监听，工具一旦开始
    /// 就只能等它自己返回。`bash` 显得能取消是因为它自己在循环里轮询
    /// `is_cancelled()`；MCP 的 `tools/call` 是 `rx.await`，没有这层 —— 服务器
    /// 不回就是整轮对话永远结束不了，Stop 按了也没反应。
    #[tokio::test]
    async fn stop_interrupts_a_tool_that_never_returns() {
        let root = Context::new();
        let tools = Tools::echo(root.clone());
        let _hold_tools = root.provide(TOOLS, tools).unwrap();
        let _hold_turn = root.provide(TURN, TurnControl::new()).unwrap();
        let tools = root.require::<Tools>(TOOLS).unwrap();

        let _reg = tools
            .register(
                ToolSpec {
                    name: "never_returns".into(),
                    description: "挂住不返回".into(),
                    parameters_json: r#"{"type":"object"}"#.into(),
                },
                std::sync::Arc::new(|_call| {
                    Box::pin(async move {
                        std::future::pending::<()>().await;
                        unreachable!()
                    })
                }),
            )
            .unwrap();

        let turn = root.require::<TurnControl>(TURN).unwrap();
        let stopper = turn.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stopper.cancel();
        });

        let call = crate::types::ToolCall {
            id: "1".into(),
            name: "never_returns".into(),
            arguments: "{}".into(),
        };
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            execute_cancellable(&root, &tools, call),
        )
        .await
        .expect("Stop 之后必须让出控制权，不能一直挂着");
        assert!(outcome.is_none(), "被 Stop 打断时不该返回工具结果");
    }

    /// 没挂 `TurnControl` 时退回原来的行为：直接等工具跑完。
    #[tokio::test]
    async fn without_turn_control_the_tool_still_runs() {
        let root = Context::new();
        let tools = Tools::echo(root.clone());
        let _hold = root.provide(TOOLS, tools).unwrap();
        let tools = root.require::<Tools>(TOOLS).unwrap();
        let call = crate::types::ToolCall {
            id: "1".into(),
            name: "echo".into(),
            arguments: "hi".into(),
        };
        assert!(execute_cancellable(&root, &tools, call).await.is_some());
    }
}
