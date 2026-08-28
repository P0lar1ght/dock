use cordis::{plugin, Context, Inject, Plugin};

use crate::names::{PROMPT_ASSEMBLE, SYSTEM_PROMPT};

/// Abstract `systemPrompt`. Fake assemble returns a fixed string.
/// Grok caches this on `Agent`; DSH assembles per step via the registry.
#[derive(Clone)]
pub struct SystemPrompt {
    ctx: Context,
    text: String,
}

impl SystemPrompt {
    pub fn fake(ctx: Context) -> Self {
        Self {
            ctx,
            text: "You are a test agent.".into(),
        }
    }

    pub fn assemble(&self) -> String {
        let text = self.text.clone();
        self.ctx
            .waterfall(PROMPT_ASSEMBLE, text.clone(), move || text.clone())
    }
}

pub fn system_prompt() -> Plugin {
    plugin("system-prompt", Inject::new(), |ctx, _: &()| {
        Ok(Some(
            ctx.provide(SYSTEM_PROMPT, SystemPrompt::fake(ctx.clone()))?,
        ))
    })
}
