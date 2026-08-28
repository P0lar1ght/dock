use std::fmt;

/// Durable session log events. Grok persists conversation items; DSH persists
/// a typed event log. This is the thin shared shape.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserImage {
    pub mime: String,
    pub data: std::sync::Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogEvent {
    User(String),
    PreStep,
    Prompt(String),
    LlmStream(LlmOutput),
    ToolExecute {
        id: String,
        name: String,
        arguments: String,
        content: String,
    },
}

impl LogEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::PreStep => "pre-step",
            Self::Prompt(_) => "prompt",
            Self::LlmStream(_) => "llm/stream",
            Self::ToolExecute { .. } => "tools/execute",
        }
    }
}

impl fmt::Display for LogEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User(text) => write!(f, "user: {text}"),
            Self::PreStep => write!(f, "pre-step"),
            Self::Prompt(text) => write!(f, "prompt: {text}"),
            Self::LlmStream(out) => write!(f, "llm: {}", out.summary()),
            Self::ToolExecute { name, content, .. } => write!(f, "tool {name}: {content}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TurnOutcome {
    Text(String),
}

impl fmt::Display for TurnOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => write!(f, "{text}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LlmOutput {
    pub text: String,
    pub reasoning: String,
    /// Elapsed thinking time in ms, set when the stream finishes.
    pub reasoning_ms: Option<u64>,
    pub tool_calls: Vec<ToolCall>,
}

impl LlmOutput {
    pub fn summary(&self) -> String {
        if self.tool_calls.is_empty() {
            return self.text.clone();
        }
        self.tool_calls
            .iter()
            .map(|c| format!("{}({})", c.name, c.arguments))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Advertised to the sampler. Parameters are a JSON Schema object as text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters_json: String,
}

#[derive(Clone, Debug)]
pub struct PromptRequest {
    pub system: String,
    pub history: Vec<LogEvent>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Clone, Debug)]
pub struct PreStep {
    pub user: String,
    pub enter: bool,
}
