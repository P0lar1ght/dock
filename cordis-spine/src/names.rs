//! DSH ctx keys for the five spine services plus the concrete loop.

pub const SESSIONS: &str = "sessions";
pub const LLM: &str = "llm";
pub const TOOLS: &str = "tools";
/// Optional extra dispatcher (MCP). `"tools"` live-looks this up per call.
pub const TOOLS_MCP: &str = "tools.mcp";
pub const SYSTEM_PROMPT: &str = "systemPrompt";
pub const AGENTS: &str = "agents";
pub const AGENT_LOOP: &str = "agentLoop";

pub const PRE_STEP: &str = "agent/pre-step";
pub const PROMPT_ASSEMBLE: &str = "system-prompt/assemble";
pub const LLM_STREAM: &str = "llm/stream";
pub const TOOLS_EXECUTE: &str = "tools/execute";
pub const SESSION_EVENT: &str = "session/event";
