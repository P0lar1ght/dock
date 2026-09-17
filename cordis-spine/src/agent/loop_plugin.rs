use std::sync::Arc;

use cordis::{plugin, Inject, Plugin};

use crate::agent::runtime::{GrokStep, LoopHandle};
use crate::names::{AGENTS, AGENT_LOOP, LLM, SESSIONS, SYSTEM_PROMPT, TOOLS};

/// THE concrete loop plugin. Injects the five spine services; swapping the
/// driver means swapping this plugin only.
pub fn agent_loop() -> Plugin {
    plugin(
        "agent-loop",
        Inject::from([SESSIONS, LLM, TOOLS, SYSTEM_PROMPT, AGENTS]),
        |ctx, _: &()| {
            let handle = LoopHandle::new(ctx.clone(), Arc::new(GrokStep));
            Ok(Some(ctx.provide(AGENT_LOOP, handle)?))
        },
    )
}
