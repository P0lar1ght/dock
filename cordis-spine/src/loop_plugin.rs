use std::sync::Arc;

use cordis::{Inject, Plugin, plugin};

use crate::names::{AGENT_LOOP, AGENTS, LLM, SESSIONS, SYSTEM_PROMPT, TOOLS};
use crate::runtime::{GrokStep, LoopHandle};

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
