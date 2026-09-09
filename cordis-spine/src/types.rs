use std::fmt;

/// Fills a cancelled assistant `tool_calls` row so the next sample is a valid
/// OpenAI conversation. Providers 400 with "tool call result does not follow
/// tool call" if a user message follows unmatched `tool_calls`.
pub const INTERRUPTED_TOOL_RESULT: &str = "已中断。";

/// Visible compact marker. The pager keeps older bubbles; this assistant
/// line is appended so the user sees that a compact ran. The model history
/// carries the same bubble plus a hidden continuation summary.
pub const COMPACT_NOTICE: &str = "已压缩上下文。";

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
    /// Grok `PromptOrigin::GoalSummary`: hidden from the pager, sent as a
    /// user-role reminder so the model keeps an active `/goal` turn going.
    SystemReminder(String),
    LlmStream(LlmOutput),
    ToolExecute {
        id: String,
        name: String,
        arguments: String,
        content: String,
        /// In-memory only for the live turn / HTTP builders. Persist stores
        /// filesystem path refs, never raw bytes in JSONL.
        images: Vec<UserImage>,
    },
}

impl LogEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::PreStep => "pre-step",
            Self::Prompt(_) => "prompt",
            Self::SystemReminder(_) => "system-reminder",
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
            Self::SystemReminder(text) => write!(f, "reminder: {text}"),
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
    /// Multimodal pixels for the next sample (screenshots / MCP images /
    /// `read_file` on PNG). Empty for text-only tools. Not serialized as
    /// base64 into the session transcript — see `session_persist` path refs.
    pub images: Vec<UserImage>,
}

impl Default for ToolResult {
    fn default() -> Self {
        Self {
            call_id: String::new(),
            name: String::new(),
            content: String::new(),
            images: Vec::new(),
        }
    }
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
