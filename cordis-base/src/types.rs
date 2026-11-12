use std::fmt;

/// Fills a cancelled assistant `tool_calls` row so the next sample is a valid
/// OpenAI conversation. Providers 400 with "tool call result does not follow
/// tool call" if a user message follows unmatched `tool_calls`.
pub const INTERRUPTED_TOOL_RESULT: &str = "已中断。";

/// 用户停止这一轮时还在跑的那次调用的结果：以 [`INTERRUPTED_TOOL_RESULT`] 开头
/// （客户端、旧会话都按这个前缀认「被中断」），写明跑了多久、可能已经部分执行，
/// 带上中止前已经产出的输出。
pub fn interrupted_tool_result(elapsed: std::time::Duration, partial: &str) -> String {
    let head = format!(
        "{INTERRUPTED_TOOL_RESULT}用户停止了这一轮，这次调用运行 {:.1}s 后被中止，可能已经部分执行。",
        elapsed.as_secs_f64()
    );
    let partial = partial.trim();
    if partial.is_empty() || partial == "(no output)" {
        head
    } else {
        format!("{head}\n中止前已产出的输出：\n{partial}")
    }
}

/// 用户在权限门拒绝一次工具调用时，这次调用的结果（`is_error`）。客户端据此把它
/// 和普通失败分开（网关报 `denied`）。
pub const PERMISSION_DENIED_TOOL_RESULT: &str = "权限被拒绝";

/// 插话（steer）落进日志时，紧挨在那条 `User` 之前的提示：这条用户消息是在一轮
/// 进行中送达的，不是新的一轮。模型据此先回应、再接着做没做完的事；网关据此把
/// 那条用户消息标成 `steered`。逐字比较，别改成带变量的文本。
pub const STEER_REMINDER: &str = "<system-reminder>\n用户在你工作期间发来了下面这条消息（插话，不是新的一轮）。先回应它；它没有改变的部分，继续完成此前未完成的工作。\n</system-reminder>";

/// 用户停止了一轮、而这一轮已经有模型输出或工具结果时，记在它末尾的提示：上面的
/// 回复可能不完整、被中止的调用可能已经部分执行。不然模型只看到一段断在半截的
/// 话，下一条消息来了会以为那一轮正常收尾了。
pub const TURN_STOPPED_REMINDER: &str = "<system-reminder>\n用户有意停止了上一轮。上一轮的回复可能不完整；被中止的工具调用（结果以「已中断。」开头）可能已经部分执行，不要假设它们做完了。按用户接下来的消息行动。\n</system-reminder>";

/// 侧边聊天（只读旁问页）写进主线的笔记最多多少个字符（按 `char` 数）。
pub const SIDE_NOTE_MAX_CHARS: usize = 2000;

/// 侧边聊天写进主线的笔记落成一条 [`LogEvent::SystemReminder`]，正文以它开头、
/// 以 [`SIDE_NOTE_TAIL`] 结尾。网关 / TUI 据此认出笔记（逐字比较，别改成带变量的
/// 文本）；中间就是笔记原文。
pub const SIDE_NOTE_HEAD: &str = "<system-reminder>\n用户从侧边聊天带回了下面这段笔记：是用户在一段只读旁问里得出、确认过的结论，供你参考。它本身不是新的任务，也不要求你回复。\n<side-note>\n";
pub const SIDE_NOTE_TAIL: &str = "\n</side-note>\n</system-reminder>";

/// 笔记原文里会提前闭合外层标签的两个结束标签，和落盘时换成的样子。
const SIDE_NOTE_CLOSERS: [(&str, &str); 2] = [
    ("</side-note>", "<\\/side-note>"),
    ("</system-reminder>", "<\\/system-reminder>"),
];

/// 把笔记包成写进主线的那条提醒（见 [`SIDE_NOTE_HEAD`]）。笔记来自读过仓库文件的
/// 旁问页，原文里的 `</side-note>` / `</system-reminder>` 先转义，免得提前闭合标签。
pub fn side_note_reminder(note: &str) -> String {
    let mut body = note.trim().to_string();
    for (tag, escaped) in SIDE_NOTE_CLOSERS {
        body = body.replace(tag, escaped);
    }
    format!("{SIDE_NOTE_HEAD}{body}{SIDE_NOTE_TAIL}")
}

/// [`side_note_reminder`] 的反向：这条提醒是侧边聊天笔记就取出原文（转义还原）。
pub fn side_note_text(reminder: &str) -> Option<std::borrow::Cow<'_, str>> {
    let body = reminder
        .strip_prefix(SIDE_NOTE_HEAD)?
        .strip_suffix(SIDE_NOTE_TAIL)?;
    if !SIDE_NOTE_CLOSERS.iter().any(|(_, e)| body.contains(e)) {
        return Some(std::borrow::Cow::Borrowed(body));
    }
    let mut out = body.to_string();
    for (tag, escaped) in SIDE_NOTE_CLOSERS {
        out = out.replace(escaped, tag);
    }
    Some(std::borrow::Cow::Owned(out))
}

/// Visible compact marker. The pager keeps older bubbles; this assistant
/// line is appended so the user sees that a compact ran. The model history
/// carries the same bubble plus a hidden continuation summary.
pub const COMPACT_NOTICE: &str = "已压缩上下文。";

/// 自动压缩失败时循环追加的那条说明的开头（后面接原因）。和 [`COMPACT_NOTICE`]
/// 一样不是模型说的话：网关不把它当助手消息，失败由压缩进展推送说。
pub const AUTO_COMPACT_FAILED_PREFIX: &str = "自动压缩失败：";

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
    /// 内容的 sha256（十六进制）。有它就不用每次重算摘要 —— 投影、取图、
    /// 落盘都靠它定位同一张图。`None` = 还没算（落盘 / 投影时现算）。
    pub digest: Option<String>,
}

/// 一条用户消息：正文 + 随这条消息带上来的图。
///
/// 图的像素跟着消息走（`UserImage::data` 是 `Arc`，和采样用的那份共享同一块字节）。
/// 落盘时像素不进 JSONL，只写内容寻址的引用（见 `session/persist`），读回来再填上，
/// 这样历史会话照样能看到用户发的图。
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct UserMessage {
    pub text: String,
    pub images: Vec<UserImage>,
}

impl UserMessage {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }

    /// 正文（`Deref` 到 `str` 的等价写法）。
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl From<String> for UserMessage {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<&str> for UserMessage {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

/// 正文当字符串用（`msg == "喂"`、`msg.trim()`）。
impl std::ops::Deref for UserMessage {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

impl PartialEq<str> for UserMessage {
    fn eq(&self, other: &str) -> bool {
        self.text == other
    }
}

impl PartialEq<&str> for UserMessage {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

impl PartialEq<UserMessage> for str {
    fn eq(&self, other: &UserMessage) -> bool {
        self == other.text
    }
}

impl PartialEq<UserMessage> for &str {
    fn eq(&self, other: &UserMessage) -> bool {
        *self == other.text
    }
}

impl fmt::Display for UserMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
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
mod side_note_tests {
    use super::*;

    /// 包进去的笔记原样认得回来；别的提醒（插话、停止）不算笔记。
    #[test]
    fn a_side_note_round_trips_and_other_reminders_are_not_notes() {
        let wrapped = side_note_reminder("  结论：retry 要退避  ");
        assert_eq!(
            side_note_text(&wrapped).as_deref(),
            Some("结论：retry 要退避")
        );
        assert_eq!(side_note_text(STEER_REMINDER), None);
        assert_eq!(side_note_text(TURN_STOPPED_REMINDER), None);
    }

    /// 笔记原文里的结束标签不能提前闭合外层：落盘时转义，认回来时还原。
    #[test]
    fn a_side_note_cannot_close_its_tags_early() {
        let note = "看到 </side-note> 和 </system-reminder> 两行\n照抄进来";
        let wrapped = side_note_reminder(note);
        let body = wrapped
            .strip_prefix(SIDE_NOTE_HEAD)
            .and_then(|b| b.strip_suffix(SIDE_NOTE_TAIL))
            .unwrap();
        assert!(!body.contains("</side-note>"), "{wrapped}");
        assert!(!body.contains("</system-reminder>"), "{wrapped}");
        assert_eq!(side_note_text(&wrapped).as_deref(), Some(note));
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
    /// 一条用户消息：正文 + 随消息带上来的图（`UserMessage`）。
    User(UserMessage),
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
/// `error` 开头、job 非零退出的首行 `exit …`、以 [`INTERRUPTED_TOOL_RESULT`] 开头的中断结果。
/// 读旧会话（落盘时还没有 `is_error`）也用它，新旧表现一致。
pub fn tool_output_looks_failed(content: &str) -> bool {
    let t = content.trim_start();
    t.starts_with("Error")
        || t.starts_with("error")
        || t.starts_with("exit ")
        || t.starts_with(INTERRUPTED_TOOL_RESULT)
}

/// Anthropic Messages 可回放的 thinking / redacted_thinking 块。
///
/// 与 [`LlmOutput::reasoning_items`]（Responses 原件）分开存：两条 wire 各用各的，
/// 互不串台。`signature` 缺或空时**整块不回放**——只发 thinking 正文会被上游
/// 400（Invalid signature），中转常再剥 thinking 重试。
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingBlock {
    Thinking {
        thinking: String,
        /// 加密签名；回放时必须与收到时逐字节一致。
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
}

impl ThinkingBlock {
    /// 有有效 signature 的 thinking，或任意 redacted 块，才值得回放。
    pub fn is_replayable(&self) -> bool {
        match self {
            Self::Thinking { signature, .. } => !signature.is_empty(),
            Self::RedactedThinking { data } => !data.is_empty(),
        }
    }
}

/// 兼容中转把 signature 包进 `reasoning` / `reasoning_content` 的标签。
const SIGNATURE_OPEN: &str = "<signature>";
const SIGNATURE_CLOSE: &str = "</signature>";

/// 从推理文本里拆出 `<signature>…</signature>`：返回（可读正文, 最后一个签名）。
/// 多个标签时正文全部剥掉；签名取最后一个非空的（中转通常只塞一个）。
///
/// 也处理流式跨 chunk 常见的残片：开标签已在上一片被消费，本片只剩
/// `AQEA…</signature>摘要`——仍然抽出签名、只留摘要。纯密文（无摘要）不进正文。
pub fn split_reasoning_signature(text: &str) -> (String, Option<String>) {
    let mut display = String::with_capacity(text.len());
    let mut signature = None::<String>;
    let mut rest = text;
    while let Some(start) = rest.find(SIGNATURE_OPEN) {
        display.push_str(&rest[..start]);
        let after_open = &rest[start + SIGNATURE_OPEN.len()..];
        match after_open.find(SIGNATURE_CLOSE) {
            Some(end) => {
                let sig = after_open[..end].trim();
                if !sig.is_empty() {
                    signature = Some(sig.to_string());
                }
                rest = &after_open[end + SIGNATURE_CLOSE.len()..];
            }
            None => {
                // 未闭合标签：从开标签起整段丢掉，避免把密文画到思维卡片上。
                rest = "";
                break;
            }
        }
    }
    // 残片：没有开标签、却有闭标签（上一 chunk 吃掉了 `<signature>`）。
    // `AQEA…</signature>\n摘要` → 签名=AQEA…，正文=摘要。
    if let Some(end) = rest.find(SIGNATURE_CLOSE) {
        let sig = rest[..end].trim();
        if !sig.is_empty() {
            signature = Some(sig.to_string());
        }
        rest = &rest[end + SIGNATURE_CLOSE.len()..];
    }
    display.push_str(rest);

    // 整段都是密文、没有任何标签时：当签名留下，正文清空（不把 AQEA… 画上卡片）。
    let trimmed = display.trim();
    if signature.is_none() && looks_like_encrypted_signature(trimmed) {
        signature = Some(trimmed.to_string());
        display.clear();
    }

    (display, signature)
}

/// Anthropic / 兼容中转的加密 thinking signature：常见以 `AQEA` 起头的 base64url。
fn looks_like_encrypted_signature(text: &str) -> bool {
    if text.len() < 16 {
        return false;
    }
    if text.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    if text.starts_with("AQEA") {
        return true;
    }
    // 长串且几乎全是 base64url 字符 → 当密文，不进思维卡片。
    text.len() >= 32
        && text
            .bytes()
            .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' | b'-' | b'_' | b'='))
}

/// 思维卡片 / 流式展示用：剥掉 signature 标签与 `[REDACTED]` 占位。
pub fn visible_reasoning_text(text: &str) -> String {
    let (display, _) = split_reasoning_signature(text);
    let trimmed = display.trim();
    if trimmed.is_empty() || trimmed == "[REDACTED]" {
        String::new()
    } else {
        display
    }
}

/// 把兼容层塞进 reasoning 文本的 `<signature>` 拆成独立 [`ThinkingBlock`]。
/// 返回（可读正文, 可回放块）。没有签名时块列表为空——缺签名不造 thinking 块。
pub fn reasoning_with_thinking_blocks(text: &str) -> (String, Vec<ThinkingBlock>) {
    let (display, signature) = split_reasoning_signature(text);
    let blocks = match signature {
        Some(signature) if !signature.is_empty() => vec![ThinkingBlock::Thinking {
            thinking: display.clone(),
            signature,
        }],
        _ => Vec::new(),
    };
    (display, blocks)
}

/// chat/completions 回放用：有带签名的 thinking 块时，把 signature 重新包回
/// `reasoning_content`（sub2api 等兼容层认这个形状）；否则原样回传 `reasoning`。
pub fn chat_reasoning_wire(out: &LlmOutput) -> Option<String> {
    let mut parts = Vec::new();
    for block in &out.thinking_blocks {
        match block {
            ThinkingBlock::Thinking {
                thinking,
                signature,
            } if !signature.is_empty() => {
                parts.push(format!(
                    "{thinking}{SIGNATURE_OPEN}{signature}{SIGNATURE_CLOSE}"
                ));
            }
            ThinkingBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                parts.push(thinking.clone());
            }
            ThinkingBlock::RedactedThinking { .. } | ThinkingBlock::Thinking { .. } => {}
        }
    }
    if !parts.is_empty() {
        return Some(parts.join(""));
    }
    (!out.reasoning.is_empty()).then(|| out.reasoning.clone())
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
    /// Anthropic Messages 的 thinking / redacted_thinking 原件（含 signature）。
    /// [`Self::reasoning`] 只给人看；回放给 Messages（及把 signature 塞进
    /// `reasoning_content` 的兼容 chat/completions）用这里。Responses 路径忽略。
    pub thinking_blocks: Vec<ThinkingBlock>,
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

    /// 思维卡片展示用正文：优先 `thinking_blocks[].thinking`（落盘已拆好的摘要），
    /// 否则再对 `reasoning` 跑 [`visible_reasoning_text`]（兼容旧会话里
    /// `AQEA…</signature>\n摘要` 这种缺开标签的残片）。
    pub fn display_reasoning(&self) -> String {
        let from_blocks: String = self
            .thinking_blocks
            .iter()
            .filter_map(|b| match b {
                ThinkingBlock::Thinking { thinking, .. } if !thinking.trim().is_empty() => {
                    Some(thinking.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        if !from_blocks.trim().is_empty() {
            return from_blocks;
        }
        visible_reasoning_text(&self.reasoning)
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
/// sample. Handlers veto with [`PreStep::deny`]; the loop appends the reason
/// on the session that is actually starting, not the one the plugin registered on.
///
/// A subagent or tab turn fires this waterfall too. Handlers that consume
/// one-shot state or touch the user's own session must check [`PreStep::identity`]
/// first, or they will do it to the parent.
#[derive(Clone, Debug)]
pub struct PreStep {
    pub user: String,
    pub enter: bool,
    /// `Sessions::identity()` of the turn starting — see [`TurnEnd::identity`]
    /// for why it rides the payload.
    pub identity: String,
    /// First refusal. The loop writes it; handlers must not append it themselves.
    denial: Option<String>,
}

impl PreStep {
    pub fn new(user: impl Into<String>, enter: bool, identity: impl Into<String>) -> Self {
        Self {
            user: user.into(),
            enter,
            identity: identity.into(),
            denial: None,
        }
    }

    /// Refuse the turn. The first reason wins; a later handler cannot clear it.
    pub fn deny(&mut self, reason: impl Into<String>) {
        self.enter = false;
        if self.denial.is_some() {
            return;
        }
        let reason = reason.into();
        let reason = reason.trim();
        if !reason.is_empty() {
            self.denial = Some(reason.to_string());
        }
    }

    /// The reason from the first [`Self::deny`], if any.
    pub fn denial(&self) -> Option<&str> {
        self.denial.as_deref()
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
/// 用户在文件面板里直接改了文件（GUI）：排在待办之前，先知道文件变了再催办。
pub const ORDER_STEP_START_USER_EDITS: i32 = 8;
/// MCP App 界面交来的最新状态（`ui/update-model-context`）：同属「外面变了」，排在用户
/// 改文件之后、待办之前。
pub const ORDER_STEP_START_MCP_APPS: i32 = 9;
pub const ORDER_STEP_START_TODO: i32 = 10;
/// 目标模式的定期自查（每用掉一批请求提醒模型对照完成标准收尾）。
pub const ORDER_STEP_START_GOAL: i32 = 20;
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

#[cfg(test)]
mod thinking_signature_tests {
    use super::*;

    #[test]
    fn split_reasoning_signature_strips_tags() {
        let (vis, sig) = split_reasoning_signature("plan <signature>abc123</signature> more");
        assert_eq!(vis, "plan  more");
        assert_eq!(sig.as_deref(), Some("abc123"));
    }

    #[test]
    fn visible_reasoning_hides_redacted_and_empty() {
        assert!(visible_reasoning_text("[REDACTED]").is_empty());
        assert!(visible_reasoning_text("  <signature>x</signature>  ").is_empty());
        assert_eq!(visible_reasoning_text("hello"), "hello");
    }

    /// 截图泄露形态：开标签已在上流被吃掉，卡片正文以 `AQEA…</signature>` 开头。
    #[test]
    fn orphan_close_tag_keeps_only_summary() {
        let raw = "AQEAxY98byFUH9pW8-HCEzli3wAAAAAa3tDxKPiNBnLBNry-_0wFAUfhbZ8akGc_JDMPVT78QAPrltzmOdjrXDkEdQgh5GrtXfMTI1Ub-f8tjh9j19Ixgumhhy-RGzb7x8-GNlii_jxbue8sLVH_7cyo0QJo-</signature>\n用户问有哪些未提交的改动。运行 git status / git diff --stat。";
        let (vis, sig) = split_reasoning_signature(raw);
        assert!(
            vis.contains("用户问有哪些未提交的改动"),
            "summary must remain: {vis:?}"
        );
        assert!(
            !vis.contains("AQEA"),
            "ciphertext must not stay in display: {vis:?}"
        );
        assert!(!vis.contains("signature"), "{vis:?}");
        assert!(sig.as_deref().unwrap_or("").starts_with("AQEA"), "{sig:?}");
        assert_eq!(visible_reasoning_text(raw).trim(), vis.trim());
    }

    #[test]
    fn complete_signature_block_before_summary() {
        let raw = "<signature>AQEA2beLGUvG15nxB7aU1yzeSAAAAAD_8I2L0_eT47qUHUfgrdh75HVzX6Plt1zzvm-p8t0w55muUPAs4o_03bOuwP_gkvkxlWbV9fzekYsaGtwWTMhex73N13a2Ju5Ft8hnxSz9Yo0EFWTFBQh07hqdSJ-lsn5kleBp3CZwGyCqci5tQ8WkEMAdrfgH41ds6ugH1KgOCcWWGCbwVZ8RD2YpHSU-</signature>\n输出是空的？git status --porcelain 无输出说明没有未提交改动。";
        assert_eq!(
            visible_reasoning_text(raw).trim(),
            "输出是空的？git status --porcelain 无输出说明没有未提交改动。"
        );
        let (_, sig) = split_reasoning_signature(raw);
        assert!(sig.as_deref().unwrap_or("").starts_with("AQEA"), "{sig:?}");
    }

    #[test]
    fn bare_signature_blob_hides_ciphertext() {
        let raw = "AQEAxY98byFUH9pW8-HCEzli3wAAAAAa3tDxKPiNBnLBNry-_0wFAUfhbZ8akGc_JDMPVT78QAPrltzmOdjrXDkEdQgh5GrtXfMTI1Ub-f8tjh9j19Ixgumhhy-RGzb7x8-GNlii_jxbue8sLVH_7cyo0QJo-";
        assert!(
            visible_reasoning_text(raw).is_empty(),
            "bare ciphertext must not paint"
        );
        let (vis, sig) = split_reasoning_signature(raw);
        assert!(vis.is_empty(), "{vis:?}");
        assert_eq!(sig.as_deref(), Some(raw));
    }

    #[test]
    fn display_reasoning_prefers_thinking_blocks_over_leaky_reasoning() {
        let out = LlmOutput {
            reasoning:
                "AQEAxY98byFUH9pW8-HCEzli3wAAAAAa3tDx-</signature>\n\n用户问有哪些未提交的改动。"
                    .into(),
            thinking_blocks: vec![ThinkingBlock::Thinking {
                thinking: "用户问有哪些未提交的改动。".into(),
                signature: "AQEAxY98byFUH9pW8-HCEzli3wAAAAAa3tDx-".into(),
            }],
            ..LlmOutput::default()
        };
        assert_eq!(out.display_reasoning(), "用户问有哪些未提交的改动。");
        assert!(!out.display_reasoning().contains("AQEA"));
        assert!(!out.display_reasoning().contains("signature"));
    }

    #[test]
    fn display_reasoning_falls_back_to_stripping_leaky_reasoning() {
        let out = LlmOutput {
            reasoning: "AQEAxY98byFUH9pW8-HCEzli3wAAAAAa3tDx-</signature>\n\n确认一下是不是仓库、分支状态。".into(),
            ..LlmOutput::default()
        };
        assert_eq!(
            out.display_reasoning().trim(),
            "确认一下是不是仓库、分支状态。"
        );
    }

    #[test]
    fn thinking_block_needs_signature_to_replay() {
        assert!(!ThinkingBlock::Thinking {
            thinking: "x".into(),
            signature: String::new(),
        }
        .is_replayable());
        assert!(ThinkingBlock::Thinking {
            thinking: "x".into(),
            signature: "sig".into(),
        }
        .is_replayable());
        assert!(ThinkingBlock::RedactedThinking {
            data: "blob".into(),
        }
        .is_replayable());
    }
}
