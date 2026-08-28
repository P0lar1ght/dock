use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use cordis::Context;

use crate::agents::Agents;
use crate::error::{Error, Result};
use crate::llm::Llm;
use crate::names::{AGENTS, LLM, PRE_STEP, SESSIONS, SYSTEM_PROMPT, TOOLS, TURN};
use crate::prompt::SystemPrompt;
use crate::session::Sessions;
use crate::tools::Tools;
use crate::turn::TurnControl;
use crate::types::{LogEvent, PreStep, PromptRequest, TurnOutcome};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

const MAX_STEPS: usize = 8;

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
    sessions.append(LogEvent::User(prompt.clone()));

    let decision = {
        let user = prompt.clone();
        ctx.waterfall(
            PRE_STEP,
            PreStep {
                user: user.clone(),
                enter: true,
            },
            move || PreStep {
                user,
                enter: true,
            },
        )
    };
    if !decision.enter {
        return Err(Error::PreStepRejected);
    }
    sessions.append(LogEvent::PreStep);

    let system = system_prompt.assemble();
    sessions.append(LogEvent::Prompt(system.clone()));

    for _ in 0..MAX_STEPS {
        if cancelled(ctx) {
            return Err(Error::Cancelled);
        }
        let output = llm
            .stream_on(
                ctx,
                PromptRequest {
                    system: system.clone(),
                    history: sessions.events(),
                    tools: tools.specs(),
                },
            )
            .await;
        if cancelled(ctx) {
            return Err(Error::Cancelled);
        }
        if output.tool_calls.is_empty() {
            return Ok(TurnOutcome::Text(output.text));
        }
        for call in output.tool_calls {
            if cancelled(ctx) {
                return Err(Error::Cancelled);
            }
            let arguments = call.arguments.clone();
            let result = tools.execute(call).await;
            sessions.append(LogEvent::ToolExecute {
                id: result.call_id,
                name: result.name,
                arguments,
                content: result.content,
            });
        }
    }
    Err(Error::MaxSteps { max: MAX_STEPS })
}

fn cancelled(ctx: &Context) -> bool {
    ctx.get::<TurnControl>(TURN)
        .is_some_and(|t| t.is_cancelled())
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
}
