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

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub content: String,
    /// Multimodal pixels for the next sample (screenshots / MCP images /
    /// `read_file` on PNG). Empty for text-only tools. Not serialized as
    /// base64 into the session transcript — see `session_persist` path refs.
    pub images: Vec<UserImage>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LlmOutput {
    pub text: String,
    pub reasoning: String,
    /// Elapsed thinking time in ms, set when the stream finishes.
    pub reasoning_ms: Option<u64>,
    pub tool_calls: Vec<ToolCall>,
    /// Responses API 的原始 `reasoning` item（含 id / summary / content），按模型
    /// 输出顺序保存。[`Self::reasoning`] 是给人看的拍平文本，回放给模型要用这里
    /// 的原件（Grok `ConversationItem::Reasoning` 同款：推理项是顶层兄弟节点，
    /// 不折进 assistant，这样 input 能复现模型当时的顺序）。其它两条 wire 不用。
    pub reasoning_items: Vec<serde_json::Value>,
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

/// `agent/pre-step` payload. Runs once per user prompt, before the first
/// sample. Handlers may veto the turn by returning `enter: false`.
///
/// Unlike [`StepStart`] / [`TurnEnd`], pre-step handlers append their reminders
/// themselves — and the `Sessions` they can reach is the one on the context
/// they registered on, i.e. the **main** session. A subagent turn fires this
/// waterfall too, so anything that appends to the session, consumes one-shot
/// state, or advances the user's own state must check [`PreStep::identity`]
/// first or it will do it to the parent.
#[derive(Clone, Debug)]
pub struct PreStep {
    pub user: String,
    pub enter: bool,
    /// `Sessions::identity()` of the turn starting — see [`TurnEnd::identity`]
    /// for why it rides the payload.
    pub identity: String,
}

impl PreStep {
    pub fn new(user: impl Into<String>, enter: bool, identity: impl Into<String>) -> Self {
        Self {
            user: user.into(),
            enter,
            identity: identity.into(),
        }
    }

    /// True for the user-facing session (root or any tab). Subagent turns are `child-*`.
    pub fn is_main_session(&self) -> bool {
        crate::session::is_main_identity(&self.identity)
    }
}

/// Order slots on the `agent/step-start` waterfall. Reminders are appended in
/// slot order, so two handlers injecting on the same step land in a fixed
/// sequence instead of one decided by plugin mount order.
pub const ORDER_STEP_START_TODO: i32 = 10;
/// Disk / dynamic Cordis plugins (`host.on("agent/step-start", ...)`). Behind
/// the built-ins: a script may add to the turn's discipline, not outrank it.
pub const ORDER_STEP_START_DYNAMIC: i32 = 50;

/// `agent/step-start` payload. Runs before every sampling step, including the
/// first one of the turn and the ones after an `agent/turn-end` continuation.
/// Handlers call [`StepStart::remind`] to inject a `<system-reminder>` ahead of
/// the sample.
///
/// This is the "every step" grain. `agent/pre-step` fires once per user prompt
/// and `agent/turn-end` once per ending — neither can watch a turn as it runs.
///
/// Handlers **return text only**: the loop owns the append, so a reminder can
/// never land between a `tool_calls` row and its `ToolExecute` rows.
#[derive(Clone, Debug, Default)]
pub struct StepStart {
    /// Sampling steps already taken in this turn — 0 on the first step, and it
    /// keeps counting across turn-end continuations. A handler with per-turn
    /// state rearms on 0; the loop holds no state on anyone's behalf.
    pub step: usize,
    /// `Sessions::identity()` of the running turn — see [`TurnEnd::identity`]
    /// for why it rides the payload.
    pub identity: String,
    reminders: Vec<(i32, String)>,
}

impl StepStart {
    pub fn new(step: usize, identity: impl Into<String>) -> Self {
        Self {
            step,
            identity: identity.into(),
            reminders: Vec::new(),
        }
    }

    /// Queue a `<system-reminder>` body for this step.
    pub fn remind(&mut self, order: i32, reminder: impl Into<String>) {
        self.reminders.push((order, reminder.into()));
    }

    /// Queued reminders in slot order (ties keep registration order).
    pub fn into_reminders(mut self) -> Vec<String> {
        self.reminders.sort_by_key(|(order, _)| *order);
        self.reminders.into_iter().map(|(_, body)| body).collect()
    }

    /// True for the user-facing session (root or any tab). Subagent turns are `child-*`.
    pub fn is_main_session(&self) -> bool {
        crate::session::is_main_identity(&self.identity)
    }
}

/// Order slots on the `agent/turn-end` waterfall. Two handlers can both want
/// another round; the smaller order wins, so the outcome does not depend on
/// plugin mount order (same reason [`crate::PromptAssembly`] uses slots).
///
/// More specific beats more general — "do the next todo" is more actionable
/// than "keep pushing the goal".
pub const ORDER_TURN_END_TODO: i32 = 10;
pub const ORDER_TURN_END_GOAL: i32 = 20;
/// Disk / dynamic Cordis plugins — see [`ORDER_STEP_START_DYNAMIC`].
pub const ORDER_TURN_END_DYNAMIC: i32 = 50;

/// `agent/turn-end` payload. Runs once per turn-ending decision: the model
/// stopped emitting tool calls (or the step budget ran out) and the loop is
/// about to hand control back. Handlers call [`TurnEnd::keep_working`] to say
/// "not yet".
///
/// Handlers **return text only** — the loop owns the `SystemReminder` append,
/// so a reminder can never land between a `tool_calls` row and its
/// `ToolExecute` rows, and "voted but forgot to append" cannot happen.
#[derive(Clone, Debug, Default)]
pub struct TurnEnd {
    /// The model's closing text. Empty when `ended_with_text` is false.
    pub text: String,
    /// How many times this turn has already been continued. Handlers use it to
    /// self-limit; the loop's own cap is a backstop, not a budget to lean on.
    pub rounds: usize,
    /// False = the step budget ran out (the model is stuck in a tool loop),
    /// not a normal text ending.
    pub ended_with_text: bool,
    /// The user already queued the next message. Handlers **must** respect
    /// this: when the user is steering, do not grab the wheel.
    pub queued_followups: bool,
    /// `Sessions::identity()` of the turn being ended. A waterfall handler only
    /// ever sees the context it registered on (`EventArgs` carries no execution
    /// ctx), and `"sessions"` is isolated per subagent while most services are
    /// not — so a handler that must not act on a child's turn reads this
    /// instead of looking up `"sessions"` itself.
    pub identity: String,
    continuations: Vec<Continuation>,
}

/// One handler's vote, plus the bookkeeping it wants to run only if that vote
/// is the one the loop acts on.
#[derive(Clone)]
struct Continuation {
    order: i32,
    reminder: String,
    on_win: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl fmt::Debug for Continuation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Continuation")
            .field("order", &self.order)
            .field("reminder", &self.reminder)
            .field("on_win", &self.on_win.is_some())
            .finish()
    }
}

impl TurnEnd {
    pub fn new(
        text: impl Into<String>,
        rounds: usize,
        ended_with_text: bool,
        queued_followups: bool,
        identity: impl Into<String>,
    ) -> Self {
        Self {
            text: text.into(),
            rounds,
            ended_with_text,
            queued_followups,
            identity: identity.into(),
            continuations: Vec::new(),
        }
    }

    /// Vote for another round. `reminder` is the `<system-reminder>` body the
    /// loop appends if this vote wins.
    pub fn keep_working(&mut self, order: i32, reminder: impl Into<String>) {
        self.continuations.push(Continuation {
            order,
            reminder: reminder.into(),
            on_win: None,
        });
    }

    /// Vote, and run `on_win` only if this vote is the one the loop acts on.
    ///
    /// A handler cannot tell whether it won: handlers registered outside it
    /// vote after it returns, and the winner is picked once the whole chain is
    /// back. So anything that must be charged per *actual* continuation — a
    /// self-limiting quota, a counter the user sees — belongs here rather than
    /// next to the `keep_working` call, where it would be spent on rounds this
    /// handler never got.
    pub fn keep_working_with(
        &mut self,
        order: i32,
        reminder: impl Into<String>,
        on_win: impl Fn() + Send + Sync + 'static,
    ) {
        self.continuations.push(Continuation {
            order,
            reminder: reminder.into(),
            on_win: Some(std::sync::Arc::new(on_win)),
        });
    }

    /// The winning reminder (smallest order), or `None` to end the turn.
    /// Read-only — the loop calls [`TurnEnd::settle`] instead.
    pub fn decision(&self) -> Option<&str> {
        self.winner().map(|c| c.reminder.as_str())
    }

    /// [`TurnEnd::decision`], plus the winner's `on_win`. Consuming on purpose:
    /// `on_win` is a side effect, and the loop is the last reader anyway, so
    /// "settled twice, charged twice" is not a mistake anyone can make.
    pub fn settle(self) -> Option<String> {
        let win = self.winner()?;
        if let Some(on_win) = &win.on_win {
            on_win();
        }
        Some(win.reminder.clone())
    }

    /// Smallest order wins; ties go to whoever voted first.
    fn winner(&self) -> Option<&Continuation> {
        self.continuations.iter().min_by_key(|c| c.order)
    }

    /// True for the user-facing session (root or any tab). Subagent turns are `child-*`.
    pub fn is_main_session(&self) -> bool {
        crate::session::is_main_identity(&self.identity)
    }
}
