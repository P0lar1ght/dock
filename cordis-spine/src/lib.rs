//! Plugin spine: five named services (DSH) + a Grok-shaped one-step turn.
//!
//! Services: `sessions` · `llm` · `tools` · `systemPrompt` · `agents`.
//! The loop injects those five and provides `agentLoop`. Swap the loop by
//! disposing that one plugin and mounting another that injects the same five.

mod agents;
mod bundle;
mod error;
mod llm;
mod loop_plugin;
mod names;
mod prompt;
mod runtime;
mod session;
mod tools;
mod types;

pub use agents::{agents, Agent, Agents};
pub use bundle::{
    install_core, install_fakes, install_foundation, install_spine, install_without_llm,
};
pub use error::{Error, Result};
pub use llm::{llm, Llm, LlmConfig, LlmMode, Sampler};
pub use loop_plugin::agent_loop;
pub use names::{
    AGENT_LOOP, AGENTS, LLM, LLM_STREAM, PRE_STEP, PROMPT_ASSEMBLE, SESSIONS, SESSION_EVENT,
    SYSTEM_PROMPT, TOOLS, TOOLS_EXECUTE, TOOLS_MCP,
};
pub use prompt::{system_prompt, SystemPrompt};
pub use runtime::{BoxFuture, Driver, GrokStep, LoopHandle};
pub use session::{sessions, ArchivedSession, Sessions};
pub use tools::{tools, ExtraTools, ExtraToolsHandle, Tools};
pub use types::{
    LlmOutput, LogEvent, PreStep, PromptRequest, ToolCall, ToolResult, ToolSpec, TurnOutcome,
};
