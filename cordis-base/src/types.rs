use std::fmt;

/// Fills a cancelled assistant `tool_calls` row so the next sample is a valid
/// OpenAI conversation. Providers 400 with "tool call result does not follow
/// tool call" if a user message follows unmatched `tool_calls`.
pub const INTERRUPTED_TOOL_RESULT: &str = "已中断。";

/// 用户在权限门拒绝一次工具调用时，这次调用的结果（`is_error`）。客户端据此把它
/// 和普通失败分开（网关报 `denied`）。
pub const PERMISSION_DENIED_TOOL_RESULT: &str = "权限被拒绝";

/// Visible compact marker. The pager keeps older bubbles; this assistant
/// line is appended so the user sees that a compact ran. The model history
/// carries the same bubble plus a hidden continuation summary.
pub const COMPACT_NOTICE: &str = "已压缩上下文。";

/// Root session identity. The subagent coordinator binds its spawns to this
/// value, so a Stop or session switch can cancel exactly this session's
/// children.
pub const ROOT_IDENTITY: &str = "main";

/// 分页会话身份前缀：`main#2`、`main#3`……第一页就是 [`ROOT_IDENTITY`]。
pub const TAB_IDENTITY_PREFIX: &str = "main#";

/// 用户面的会话（根会话或任一分页）。子代理是 `child-…`，不算。
///
/// 三处 `is_main_session()` 与系统提示的目录开关都问这个：分页是**并列的主线**，
/// 不是子代理，判错会让第 2 页拿不到工具目录、也收不到主线才有的提醒。
///
/// 住在 `types` 而不是 `session`：`LogEvent` 一族要判身份，而 `types` 是整棵树的
/// 叶子；反过来依赖 `session` 会让基础层整个拆不出去（唯一的那条反向边）。
pub fn is_main_identity(identity: &str) -> bool {
    identity == ROOT_IDENTITY || identity.starts_with(TAB_IDENTITY_PREFIX)
}

/// Durable session log events. Grok persists conversation items; DSH persists
/// a typed event log. This is the thin shared shape.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserImage {
    pub mime: String,
    pub data: std::sync::Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}

/// [`LogEvent::Notice`] 的类别。落盘用 kebab-case 字符串，认不得的旧值读成
/// [`Self::Other`]（丢一个配色，不丢一整条会话）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    /// workflow 的某个子代理给父级发了消息（`send_message`）。
    WorkflowReport,
    /// 一次 workflow run 收尾。
    WorkflowDone,
    Other,
}

impl NoticeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkflowReport => "workflow-report",
            Self::WorkflowDone => "workflow-done",
            Self::Other => "other",
        }
    }

    pub fn from_str_lenient(raw: &str) -> Self {
        match raw {
            "workflow-report" => Self::WorkflowReport,
            "workflow-done" => Self::WorkflowDone,
            _ => Self::Other,
        }
    }
}

#[cfg(test)]
mod notice_kind_tests {
    use super::NoticeKind;

    #[test]
    fn unknown_kind_is_other_so_old_jsonl_still_loads() {
        assert_eq!(NoticeKind::from_str_lenient("nope"), NoticeKind::Other);
        assert_eq!(
            NoticeKind::from_str_lenient("workflow-report"),
            NoticeKind::WorkflowReport
        );
    }
}

/// 一轮是怎么结束的。落盘成 `turn-end` 行，网关投影成 `turn/completed` 的
/// `status`（失败时带 `error`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnEndStatus {
    Completed,
    /// 用户停止。
    Cancelled,
    /// 模型请求失败、超步数等。带给用户看的错误文本。
    Failed(String),
}

impl TurnEndStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed(_) => "failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogEvent {
    User(String),
    PreStep,
    Prompt(String),
    /// Grok `PromptOrigin::GoalSummary`: hidden from the pager, sent as a
    /// user-role reminder so the model keeps an active `/goal` turn going.
    SystemReminder(String),
    /// **只给用户看**的通知：滚动区渲染成卡片，`Sessions::model_history` 把它
    /// 滤掉，所以它既不进模型上下文，也不会触发一轮采样。
    ///
    /// 和 [`Self::SystemReminder`] 正好相反（那个是模型看得到、pager 藏起来）。
    /// workflow 的子代理上报走这条：run 还在跑时推给模型会让它在半份结果上开一
    /// 轮，但用户该看得见发生了什么。
    Notice {
        /// 通知类别，渲染器据此选卡片配色。
        kind: NoticeKind,
        /// 卡片头那一行。
        title: String,
        /// 卡片体，默认折叠。
        body: String,
    },
    LlmStream(LlmOutput),
    ToolExecute {
        id: String,
        name: String,
        arguments: String,
        content: String,
        /// In-memory only for the live turn / HTTP builders. Persist stores
        /// filesystem path refs, never raw bytes in JSONL.
        images: Vec<UserImage>,
        /// 见 [`ToolResult::is_error`]。
        is_error: bool,
    },
    /// 一轮结束（完成、停止、出错都记）。和 [`Self::Notice`] 一样只给用户看，
    /// 不进模型上下文。只有会落盘的页会写；更早的会话没有这一行。
    TurnEnd(TurnEndStatus),
}

impl LogEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::PreStep => "pre-step",
            Self::Prompt(_) => "prompt",
            Self::SystemReminder(_) => "system-reminder",
            Self::LlmStream(_) => "llm/stream",
            Self::Notice { .. } => "notice",
            Self::ToolExecute { .. } => "tools/execute",
            Self::TurnEnd(_) => "turn-end",
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
            Self::Notice { kind, title, .. } => write!(f, "notice[{}]: {title}", kind.as_str()),
            Self::ToolExecute { name, content, .. } => write!(f, "tool {name}: {content}"),
            Self::TurnEnd(TurnEndStatus::Failed(error)) => write!(f, "turn-end: failed: {error}"),
            Self::TurnEnd(status) => write!(f, "turn-end: {}", status.as_str()),
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

/// `tools/pre-execute` payload. Runs **before** the plan gate, the permission
/// gate and the tool body — the seam for rewriting, redirecting or denying a
/// call while its arguments are still on the table.
///
/// Split of labour with `tools/execute`: that one is post-hoc and carries a
/// [`ToolResult`], so it can only touch what already happened. This one carries
/// the arguments, so it can change what happens.
///
/// **Synchronous.** `Context::waterfall` takes sync handlers, so a listener here
/// cannot await — no asking the user, no network. Rewriting, routing and
/// denying are all pure decisions, which is the whole intended surface; a
/// custom permission prompt still belongs behind `acp::needs_permission` +
/// `Permissions::request`, which the registry awaits after this waterfall.
///
/// Redirects are re-gated: the registry reads the **post-waterfall** name for
/// the plan and permission checks, so rewriting `bash` into `read_file` really
/// does drop the permission prompt instead of having already paid for it.
/// A redirect cannot escape the preset allowlist: the registry checks it twice,
/// once on the inbound name and again on the post-waterfall one, so a read-only
/// preset stays read-only no matter what a handler rewrites the call into.
///
/// **No order slots.** The kernel's `EventOptions` carries `prepend` / `global`
/// / `once` and no ordering key. The `ORDER_STEP_START_*` slots work because
/// reminders *accumulate* and get sorted at the end; a rewrite mutates in place,
/// so there is nothing to sort. With the house idiom (take `args.next()`, then
/// mutate what it returned) edits land innermost-first — reverse mount order.
/// The case where order would actually be dangerous — one handler allowing what
/// another denied — is closed by making [`PreExecute::deny`] monotonic instead.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreExecute {
    /// Tool name as the model called it. Rewrites do not change this — read
    /// [`PreExecute::name`] for what will actually run.
    pub called: String,
    /// `Sessions::identity()` of the turn making the call — see
    /// [`TurnEnd::identity`] for why identity rides the payload.
    pub identity: String,
    name: String,
    arguments: String,
    denied: Option<String>,
}

impl PreExecute {
    pub fn new(
        name: impl Into<String>,
        arguments: impl Into<String>,
        identity: impl Into<String>,
    ) -> Self {
        let name = name.into();
        Self {
            called: name.clone(),
            identity: identity.into(),
            name,
            arguments: arguments.into(),
            denied: None,
        }
    }

    /// What will run after this waterfall settles.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn arguments(&self) -> &str {
        &self.arguments
    }

    /// True once some handler redirected the call to a different tool.
    pub fn redirected(&self) -> bool {
        self.name != self.called
    }

    /// Redirect to another tool. A denied call is left alone — the first denial
    /// stands, so a later handler cannot quietly turn a refusal into a call.
    pub fn rewrite(&mut self, name: impl Into<String>, arguments: impl Into<String>) {
        if self.denied.is_some() {
            return;
        }
        self.name = name.into();
        self.arguments = arguments.into();
    }

    /// Replace the arguments, same tool.
    pub fn rewrite_args(&mut self, arguments: impl Into<String>) {
        if self.denied.is_some() {
            return;
        }
        self.arguments = arguments.into();
    }

    /// Refuse the call. `reason` becomes the model-facing `ToolResult` body.
    ///
    /// Monotonic, like DSH's tool guards: the first denial wins and later
    /// handlers cannot lift it. Otherwise mount order would decide whether a
    /// policy holds.
    pub fn deny(&mut self, reason: impl Into<String>) {
        if self.denied.is_none() {
            self.denied = Some(reason.into());
        }
    }

    /// The refusal reason, if any handler denied the call.
    pub fn denial(&self) -> Option<&str> {
        self.denied.as_deref()
    }

    /// True for the user-facing session (root or any tab). Subagents are `child-*`.
    pub fn is_main_session(&self) -> bool {
        is_main_identity(&self.identity)
    }
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
    /// 这次调用没做成（工具报错、被拒、被拦、命令非零退出、被中断）。随结果落盘，
    /// 客户端直接读它，不再各自按输出猜。工具不表态时由注册表按
    /// [`tool_output_looks_failed`] 补一次。
    pub is_error: bool,
}

/// 工具没显式表态时的**唯一**兜底规则：Dock 自己产出的失败格式——`Error` /
/// `error` 开头、job 非零退出的首行 `exit …`、中断回填 [`INTERRUPTED_TOOL_RESULT`]。
/// 读旧会话（落盘时还没有 `is_error`）也用它，新旧表现一致。
pub fn tool_output_looks_failed(content: &str) -> bool {
    let t = content.trim_start();
    t.starts_with("Error")
        || t.starts_with("error")
        || t.starts_with("exit ")
        || t.trim_end() == INTERRUPTED_TOOL_RESULT
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
    /// 本轮采样的失败详情（传输错误 / HTTP 状态码 + `request-id`）。
    ///
    /// **刻意不放进 [`Self::text`]**：`text` 会被三条 wire builder 当成助手消息
    /// 回放给模型，于是「llm request failed: …」会变成模型自己说过的一句话，
    /// 下一轮还要为它付 token。三条 builder 都以 `text` 非空或有 tool_call 为
    /// 门槛，所以错误只写这里就天然不进模型历史，同时仍留在会话事件里供 TUI
    /// 渲染与 `/resume` 回放。
    pub error: Option<String>,
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
        is_main_identity(&self.identity)
    }
}

/// Order slots on the `agent/step-start` waterfall. Reminders are appended in
/// slot order, so two handlers injecting on the same step land in a fixed
/// sequence instead of one decided by plugin mount order.
/// 工作区规约（`AGENTS.md`）。排在所有提醒最前：它是这一步的**规则**，其余提醒
/// 都是在这些规则之下的具体催办。
pub const ORDER_STEP_START_INSTRUCTIONS: i32 = 5;
/// 跨会话长期记忆（`MEMORY.md` 索引）。排在工程规约之后、待办之前。
pub const ORDER_STEP_START_MEMORY: i32 = 7;
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
    /// [`Self::remind_preamble`] 排进来的：整场对话的背景，不是对这一步的催办。
    preamble: Vec<(i32, String)>,
}

impl StepStart {
    pub fn new(step: usize, identity: impl Into<String>) -> Self {
        Self {
            step,
            identity: identity.into(),
            reminders: Vec::new(),
            preamble: Vec::new(),
        }
    }

    /// Queue a `<system-reminder>` body for this step.
    pub fn remind(&mut self, order: i32, reminder: impl Into<String>) {
        self.reminders.push((order, reminder.into()));
    }

    /// Queue a `<system-reminder>` that is standing context for the whole
    /// conversation (workspace conventions, long-term memory) rather than a
    /// nudge about this step.
    ///
    /// 会话还没向模型发过任何请求时，循环把它放在第一条用户消息**之前**：这样
    /// 每个新会话的请求在用户消息之前逐字节相同，上游前缀缓存能跨会话命中。已经
    /// 发过之后就和 [`Self::remind`] 一样追加到尾部——那时再往前插会改写已经
    /// 发出去的前缀。
    pub fn remind_preamble(&mut self, order: i32, reminder: impl Into<String>) {
        self.preamble.push((order, reminder.into()));
    }

    /// Every queued reminder in slot order (ties keep registration order),
    /// preamble ones included — for a caller that only knows how to append.
    pub fn into_reminders(self) -> Vec<String> {
        let mut all = self.preamble;
        all.extend(self.reminders);
        ordered(all)
    }

    /// `(preamble, tail)`, each in slot order. The loop places the first list
    /// with [`Self::remind_preamble`]'s rule and appends the second.
    pub fn into_parts(self) -> (Vec<String>, Vec<String>) {
        (ordered(self.preamble), ordered(self.reminders))
    }

    /// True for the user-facing session (root or any tab). Subagent turns are `child-*`.
    pub fn is_main_session(&self) -> bool {
        is_main_identity(&self.identity)
    }
}

/// Stable sort by slot, bodies only.
fn ordered(mut slots: Vec<(i32, String)>) -> Vec<String> {
    slots.sort_by_key(|(order, _)| *order);
    slots.into_iter().map(|(_, body)| body).collect()
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
        is_main_identity(&self.identity)
    }
}

#[cfg(test)]
mod step_start_tests {
    use super::*;

    /// 前导与尾部各自按槽位排；只会追加的旧调用方拿到的是全部，一条都不丢。
    #[test]
    fn preamble_and_tail_split_but_nothing_is_dropped() {
        let mut step = StepStart::new(0, "main");
        step.remind(ORDER_STEP_START_TODO, "todo");
        step.remind_preamble(ORDER_STEP_START_MEMORY, "memory");
        step.remind_preamble(ORDER_STEP_START_INSTRUCTIONS, "rules");

        let (preamble, tail) = step.clone().into_parts();
        assert_eq!(preamble, ["rules", "memory"]);
        assert_eq!(tail, ["todo"]);
        assert_eq!(step.into_reminders(), ["rules", "memory", "todo"]);
    }
}
