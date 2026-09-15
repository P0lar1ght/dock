use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use cordis::{plugin, Context, Inject, Plugin};

use crate::names::{SESSIONS, SESSION_EVENT};
use crate::types::{LogEvent, COMPACT_NOTICE};
use crate::usage::{PromptUsage, TokenUsage as CallUsage, UsageLedger, UsageTotals};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt: u64,
    pub completion: u64,
    pub window: u64,
    pub official: bool,
}

impl TokenUsage {
    pub fn context_used(self) -> u64 {
        self.prompt.saturating_add(self.completion)
    }
}

/// A previous conversation, kept so `/resume` can restore it.
#[derive(Clone, Debug)]
pub struct ArchivedSession {
    pub id: String,
    pub title: String,
    pub events: Vec<LogEvent>,
    pub times: Vec<SystemTime>,
    /// Model-history head after compact. Display `events` stay the full transcript.
    pub compact_prefix: Option<Vec<LogEvent>>,
    /// Display index from which new events are appended onto [`Self::compact_prefix`].
    pub compact_from: usize,
}

/// Session log. DSH `ctx.sessions`; Grok conversation folders on disk after
/// [`Sessions::attach_disk`].
#[derive(Clone)]
pub struct Sessions {
    ctx: Context,
    events: Arc<Mutex<Vec<LogEvent>>>,
    times: Arc<Mutex<Vec<SystemTime>>>,
    archive: Arc<Mutex<Vec<ArchivedSession>>>,
    next_id: Arc<Mutex<u64>>,
    usage: Arc<Mutex<TokenUsage>>,
    ledger: Arc<Mutex<UsageLedger>>,
    pending_call: Arc<Mutex<Option<PendingCall>>>,
    turn_started: Arc<Mutex<Option<Instant>>>,
    pending_images: Arc<Mutex<Vec<crate::types::UserImage>>>,
    user_images: Arc<Mutex<Vec<Vec<crate::types::UserImage>>>>,
    /// Set by [`Self::rewind_inflight_user`] so a dying turn cannot append
    /// onto the previous assistant bubble (Grok cancel-rewind fence).
    rewound: Arc<AtomicBool>,
    /// Bumped on every log mutation so the TUI can invalidate layout caches
    /// without cloning the event vector.
    events_rev: Arc<AtomicU64>,
    /// Child isolates skip `session/event` so the parent TUI is not flooded.
    emit: bool,
    /// Stable id for dynamic Cordis ownership (DSH `plugin.sessionId`).
    identity: Arc<str>,
    /// Set after auto-compact fails or stays over the threshold until the
    /// next real user turn (Grok `auto_compact_suppressed`). Manual compact
    /// ignores this.
    auto_compact_suppressed: Arc<AtomicBool>,
    compacting: Arc<AtomicBool>,
    /// Optional live-thread title (gateway `thread/rename` / `thread/start`).
    /// Empty means derive from the first user message, same as `/resume`.
    display_title: Arc<Mutex<Option<String>>>,
    /// User follow-up prompts waiting in the session actor (not GoalSummary).
    queued_followups: Arc<AtomicUsize>,
    /// One-shot wire addons paired with a specific next user bubble
    /// (display `/loop …` vs `loop_schedule_instruction`, or a scheduled-fire
    /// reminder). Hidden from the pager as [`LogEvent::SystemReminder`].
    pending_user_addons: Arc<Mutex<VecDeque<(String, String)>>>,
    /// When set, archive / live snapshots write Grok-style folders under
    /// `$DOCK_HOME/sessions/<cwd-key>/`. Isolated child logs stay memory-only.
    disk_cwd: Arc<Mutex<Option<PathBuf>>>,
    live_id: Arc<Mutex<String>>,
    /// Compacted prefix sent to the sampler. `None` = display log is the model history.
    compact_prefix: Arc<Mutex<Option<Vec<LogEvent>>>>,
    compact_from: Arc<Mutex<usize>>,
    compact_images: Arc<Mutex<Vec<Vec<crate::types::UserImage>>>>,
}

/// Official SSE usage held until [`Sessions::finish_llm`] so one sample is
/// one ledger row even if the stream repeats the usage object.
struct PendingCall {
    usage: CallUsage,
    model: String,
    cost_usd_ticks: Option<i64>,
}

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
pub fn is_main_identity(identity: &str) -> bool {
    identity == ROOT_IDENTITY || identity.starts_with(TAB_IDENTITY_PREFIX)
}

/// 按 `[model.<id>.pricing]` 估一次调用的费用。`None` = 没配单价。
///
/// 这里认的是**发给上游的 slug**（`pending.model`），不是目录 id：两个目录条目
/// 可以指向同一个上游模型，价是上游模型的属性。找不到 slug 时退回按目录 id 匹配，
/// 那是 `api_model` 省略时的常态。
fn estimated_cost_ticks(wire_model: &str, usage: &crate::usage::TokenUsage) -> Option<i64> {
    let catalog = crate::config::load_catalog();
    let pricing = catalog
        .iter()
        .find(|m| m.wire_model() == wire_model)
        .or_else(|| catalog.iter().find(|m| m.id == wire_model))
        .and_then(|m| m.pricing)?;
    let uncached = usage
        .prompt_tokens
        .saturating_sub(usage.cached_prompt_tokens)
        .saturating_sub(usage.cache_creation_prompt_tokens);
    Some(pricing.cost_ticks(
        uncached,
        usage.cached_prompt_tokens,
        usage.cache_creation_prompt_tokens,
        usage.completion_tokens,
    ))
}

impl Sessions {
    pub fn new(ctx: Context) -> Self {
        Self::with_identity(ctx, ROOT_IDENTITY, true)
    }

    /// 第 `index` 个分页的会话（`index >= 2`；第一页就是 [`Self::new`]）。
    /// 与主会话同级：照发 TUI 事件、系统提示照给目录，只是换一个身份。
    pub fn tab(ctx: Context, index: usize) -> Self {
        Self::with_identity(ctx, format!("{TAB_IDENTITY_PREFIX}{index}"), true)
    }

    /// 用户面的会话（根会话或分页），不是子代理。
    pub fn is_main(&self) -> bool {
        is_main_identity(&self.identity)
    }

    /// Nested subagent log: same API, no TUI events.
    pub fn isolated(ctx: Context) -> Self {
        Self::isolated_as(ctx, format!("child-{}", uuid::Uuid::now_v7().as_simple()))
    }

    pub fn isolated_as(ctx: Context, identity: impl Into<String>) -> Self {
        Self::with_identity(ctx, identity, false)
    }

    fn with_identity(ctx: Context, identity: impl Into<String>, emit: bool) -> Self {
        Self {
            ctx,
            events: Arc::new(Mutex::new(Vec::new())),
            times: Arc::new(Mutex::new(Vec::new())),
            archive: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(Mutex::new(1)),
            usage: Arc::new(Mutex::new(TokenUsage {
                window: 128_000,
                ..TokenUsage::default()
            })),
            ledger: Arc::new(Mutex::new(UsageLedger::default())),
            pending_call: Arc::new(Mutex::new(None)),
            turn_started: Arc::new(Mutex::new(None)),
            pending_images: Arc::new(Mutex::new(Vec::new())),
            user_images: Arc::new(Mutex::new(Vec::new())),
            rewound: Arc::new(AtomicBool::new(false)),
            events_rev: Arc::new(AtomicU64::new(0)),
            emit,
            identity: Arc::from(identity.into()),
            auto_compact_suppressed: Arc::new(AtomicBool::new(false)),
            compacting: Arc::new(AtomicBool::new(false)),
            display_title: Arc::new(Mutex::new(None)),
            queued_followups: Arc::new(AtomicUsize::new(0)),
            pending_user_addons: Arc::new(Mutex::new(VecDeque::new())),
            disk_cwd: Arc::new(Mutex::new(None)),
            live_id: Arc::new(Mutex::new(String::new())),
            compact_prefix: Arc::new(Mutex::new(None)),
            compact_from: Arc::new(Mutex::new(0)),
            compact_images: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Load `$DOCK_HOME/sessions/<cwd>/` into the resume list. No-op for
    /// isolated child logs. Safe to call more than once (replaces the list).
    pub fn attach_disk(&self) {
        if !self.emit {
            return;
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let loaded = crate::session_persist::load_cwd(&cwd);
        *self.archive.lock().unwrap() = loaded;
        *self.live_id.lock().unwrap() = crate::session_persist::new_id();
        *self.disk_cwd.lock().unwrap() = Some(cwd);
    }

    /// Restore the newest on-disk session for this cwd (Grok `--resume`).
    /// Archives a non-empty live log first. Usage ledger still resets.
    pub fn resume_latest(&self) -> bool {
        let id = self.archived().into_iter().next().map(|s| s.id);
        self.archive_current();
        id.is_some_and(|id| self.restore(&id))
    }

    /// Restore a specific on-disk / archived id. Same ledger rule as [`Self::restore`].
    pub fn resume_id(&self, id: &str) -> bool {
        self.archive_current();
        self.restore(id)
    }

    /// Queue a hidden model-only reminder for the next user bubble whose
    /// visible text equals `expected_user` (so a queued `/loop` cannot steal
    /// an unrelated follow-up).
    pub fn arm_user_addon(&self, expected_user: impl Into<String>, text: impl Into<String>) {
        self.pending_user_addons
            .lock()
            .unwrap()
            .push_back((expected_user.into(), text.into()));
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn set_queued_followups(&self, n: usize) {
        self.queued_followups.store(n, Ordering::Relaxed);
    }

    pub fn has_queued_followups(&self) -> bool {
        self.queued_followups.load(Ordering::Relaxed) > 0
    }

    /// Monotonic generation for scrollback / layout cache invalidation.
    pub fn events_rev(&self) -> u64 {
        self.events_rev.load(Ordering::Relaxed)
    }

    /// Cheap occupancy cache key: log generation + last token usage.
    pub fn occupancy_stamp(&self) -> (u64, TokenUsage) {
        (self.events_rev(), self.usage())
    }

    fn bump_events_rev(&self) {
        self.events_rev.fetch_add(1, Ordering::Relaxed);
    }

    /// Borrow the live log without cloning. Do not re-enter `Sessions` from `f`.
    pub fn with_log<R>(&self, f: impl FnOnce(&[LogEvent], &[SystemTime]) -> R) -> R {
        let events = self.events.lock().unwrap();
        let times = self.times.lock().unwrap();
        f(&events, &times)
    }

    pub fn seed(&self, events: Vec<LogEvent>) {
        let n = events.len();
        *self.events.lock().unwrap() = events;
        *self.times.lock().unwrap() = vec![SystemTime::now(); n];
        self.reset_compact();
        self.bump_events_rev();
        self.persist_live();
    }

    pub fn usage(&self) -> TokenUsage {
        *self.usage.lock().unwrap()
    }

    pub fn ledger(&self) -> UsageLedger {
        self.ledger.lock().unwrap().clone()
    }

    pub fn prompt_usage(&self) -> PromptUsage {
        PromptUsage::from(&*self.ledger.lock().unwrap())
    }

    /// Fold a child isolate's ledger into this session. Does not bump
    /// `main_loop_model_calls` (Grok `record_subagent`).
    pub fn fold_subagent_ledger(&self, child: &UsageLedger) {
        if child.totals.model_calls == 0 && !child.incomplete {
            return;
        }
        let rows: Vec<(String, UsageTotals)> = child
            .by_model
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.ledger
            .lock()
            .unwrap()
            .record_subagent(&rows, child.incomplete);
    }

    /// 记一次**旁路**调用的用量：压缩这类隔离掉 `"sessions"`、因而不经
    /// `begin_llm` / `finish_llm` 的采样。
    ///
    /// 不记的话，一次压缩（整段历史、几乎零命中的满价请求）在 `/usage` 里一个
    /// token 都看不见，而它之后那次全量重算的成本又只能摊在主循环头上。
    pub fn record_side_call(
        &self,
        model: &str,
        usage: &crate::usage::TokenUsage,
        api_duration_ms: Option<u64>,
        reported_cost_ticks: Option<i64>,
    ) {
        if usage.prompt_tokens == 0 && usage.completion_tokens == 0 {
            return;
        }
        let model = if model.trim().is_empty() {
            "unknown"
        } else {
            model
        };
        let cost =
            crate::usage::CallCost::pick(reported_cost_ticks, estimated_cost_ticks(model, usage));
        self.ledger
            .lock()
            .unwrap()
            .record_side_call(model, usage, api_duration_ms, cost);
    }

    pub fn turn_elapsed(&self) -> Option<std::time::Duration> {
        self.turn_started.lock().unwrap().map(|t| t.elapsed())
    }

    pub fn set_window(&self, window: u64) {
        if window > 0 {
            self.usage.lock().unwrap().window = window;
        }
    }

    pub fn append(&self, event: LogEvent) {
        let is_user = matches!(event, LogEvent::User(_));
        if is_user {
            self.rewound.store(false, Ordering::Relaxed);
            let imgs = std::mem::take(&mut *self.pending_images.lock().unwrap());
            self.user_images.lock().unwrap().push(imgs);
        } else if self.rewound.load(Ordering::Relaxed) {
            return;
        }
        self.events.lock().unwrap().push(event.clone());
        self.times.lock().unwrap().push(SystemTime::now());
        self.bump_events_rev();
        self.emit_session(event.clone());
        if let LogEvent::User(user_text) = &event {
            let addon = {
                let mut q = self.pending_user_addons.lock().unwrap();
                q.iter()
                    .position(|(expected, _)| expected == user_text)
                    .map(|i| q.remove(i).unwrap().1)
            };
            if let Some(addon) = addon {
                let reminder = LogEvent::SystemReminder(addon);
                self.events.lock().unwrap().push(reminder.clone());
                self.times.lock().unwrap().push(SystemTime::now());
                self.bump_events_rev();
                self.emit_session(reminder);
            }
        }
        self.persist_live();
    }

    fn emit_session(&self, event: LogEvent) {
        if self.emit {
            self.ctx.emit(SESSION_EVENT, event);
        }
    }

    pub fn queue_user_images(&self, images: Vec<crate::types::UserImage>) {
        *self.pending_images.lock().unwrap() = images;
    }

    pub fn user_images(&self) -> Vec<Vec<crate::types::UserImage>> {
        self.user_images.lock().unwrap().clone()
    }

    pub fn user_image_count(&self) -> u64 {
        self.user_images
            .lock()
            .unwrap()
            .iter()
            .map(|row| row.len() as u64)
            .sum()
    }

    pub fn events(&self) -> Vec<LogEvent> {
        self.events.lock().unwrap().clone()
    }

    /// History sent to the sampler. After compact this is the summary prefix
    /// plus turns that landed after the visible notice — not the pager log.
    pub fn model_history(&self) -> Vec<LogEvent> {
        let prefix = self.compact_prefix.lock().unwrap();
        let Some(prefix) = prefix.as_ref() else {
            return self.events();
        };
        let from = *self.compact_from.lock().unwrap();
        let events = self.events.lock().unwrap();
        let start = from.min(events.len());
        let mut out = prefix.clone();
        out.extend(events[start..].iter().cloned());
        out
    }

    /// Images aligned with [`Self::model_history`] user rows (not the pager log).
    pub fn model_user_images(&self) -> Vec<Vec<crate::types::UserImage>> {
        let prefix = self.compact_prefix.lock().unwrap();
        let Some(prefix) = prefix.as_ref() else {
            return self.user_images();
        };
        let prefix_users = prefix
            .iter()
            .filter(|e| matches!(e, LogEvent::User(_)))
            .count();
        let mut out = self.compact_images.lock().unwrap().clone();
        out.resize(prefix_users, Vec::new());
        let from = *self.compact_from.lock().unwrap();
        let events = self.events.lock().unwrap();
        let images = self.user_images.lock().unwrap();
        let mut user_i = 0usize;
        for (i, event) in events.iter().enumerate() {
            if matches!(event, LogEvent::User(_)) {
                if i >= from {
                    out.push(images.get(user_i).cloned().unwrap_or_default());
                }
                user_i += 1;
            }
        }
        out
    }

    pub fn model_user_image_count(&self) -> u64 {
        self.model_user_images()
            .iter()
            .map(|row| row.len() as u64)
            .sum()
    }

    fn reset_compact(&self) {
        *self.compact_prefix.lock().unwrap() = None;
        *self.compact_from.lock().unwrap() = 0;
        self.compact_images.lock().unwrap().clear();
    }

    fn compact_snapshot(&self) -> (Option<Vec<LogEvent>>, usize) {
        (
            self.compact_prefix.lock().unwrap().clone(),
            *self.compact_from.lock().unwrap(),
        )
    }

    fn apply_compact_snapshot(&self, prefix: Option<Vec<LogEvent>>, from: usize) {
        let n = self.events.lock().unwrap().len();
        let from = from.min(n);
        let image_n = prefix
            .as_ref()
            .map(|p| p.iter().filter(|e| matches!(e, LogEvent::User(_))).count())
            .unwrap_or(0);
        *self.compact_from.lock().unwrap() = from;
        *self.compact_images.lock().unwrap() = vec![Vec::new(); image_n];
        *self.compact_prefix.lock().unwrap() = prefix;
    }

    pub fn times(&self) -> Vec<SystemTime> {
        self.times.lock().unwrap().clone()
    }

    /// Empty `llm/stream` slot the HTTP sampler fills via [`Self::apply_llm_delta`].
    pub fn begin_llm(&self) {
        if self.rewound.load(Ordering::Relaxed) {
            return;
        }
        {
            let mut usage = self.usage.lock().unwrap();
            usage.completion = 0;
            usage.official = false;
        }
        *self.pending_call.lock().unwrap() = None;
        *self.turn_started.lock().unwrap() = Some(Instant::now());
        self.append(LogEvent::LlmStream(crate::types::LlmOutput::default()));
    }

    pub fn apply_llm_delta(&self, delta: &crate::stream_acc::StreamDelta) {
        if self.rewound.load(Ordering::Relaxed) {
            return;
        }
        match delta {
            crate::stream_acc::StreamDelta::Text(text) => {
                if text.is_empty() {
                    return;
                }
                let mut events = self.events.lock().unwrap();
                let Some(LogEvent::LlmStream(out)) = events.last_mut() else {
                    return;
                };
                out.text.push_str(text);
                {
                    let mut usage = self.usage.lock().unwrap();
                    if !usage.official {
                        usage.completion = Self::estimate_tokens(&out.text);
                    }
                }
                let event = events.last().cloned().unwrap();
                drop(events);
                self.bump_events_rev();
                self.emit_session(event);
            }
            crate::stream_acc::StreamDelta::Reasoning(text) => {
                if text.is_empty() {
                    return;
                }
                let mut events = self.events.lock().unwrap();
                let Some(LogEvent::LlmStream(out)) = events.last_mut() else {
                    return;
                };
                out.reasoning.push_str(text);
                let event = events.last().cloned().unwrap();
                drop(events);
                self.bump_events_rev();
                self.emit_session(event);
            }
            crate::stream_acc::StreamDelta::Usage {
                tokens,
                official,
                model,
                cost_usd_ticks,
            } => {
                {
                    let mut usage = self.usage.lock().unwrap();
                    if tokens.prompt_tokens > 0 {
                        usage.prompt = tokens.prompt_tokens;
                    }
                    if tokens.completion_tokens > 0 {
                        usage.completion = tokens.completion_tokens;
                        if *official {
                            usage.official = true;
                        }
                    }
                }
                if *official && (tokens.prompt_tokens > 0 || tokens.completion_tokens > 0) {
                    *self.pending_call.lock().unwrap() = Some(PendingCall {
                        usage: tokens.clone(),
                        model: model.clone(),
                        cost_usd_ticks: *cost_usd_ticks,
                    });
                }
                self.emit_session(LogEvent::LlmStream(crate::types::LlmOutput::default()));
            }
        }
    }

    pub fn finish_llm(&self, output: &crate::types::LlmOutput) {
        if self.rewound.load(Ordering::Relaxed) {
            return;
        }
        let mut events = self.events.lock().unwrap();
        let Some(LogEvent::LlmStream(out)) = events.last_mut() else {
            drop(events);
            self.commit_pending_call();
            return;
        };
        if out.text.is_empty() {
            out.text = output.text.clone();
        }
        if out.reasoning.is_empty() {
            out.reasoning = output.reasoning.clone();
        }
        // Responses 的 reasoning item 原件只在 `response.completed` 里拿得到，
        // 没有对应的增量，所以只能在收尾时落进这一行。漏掉这句的后果是整条
        // 推理链存不进会话：下一轮回放不出来，而要求「带 tools 就必须回传
        // reasoning」的上游（DeepSeek thinking 模式）会直接 400。
        if out.reasoning_items.is_empty() {
            out.reasoning_items = output.reasoning_items.clone();
        }
        if out.reasoning_ms.is_none() && !out.reasoning.is_empty() {
            out.reasoning_ms = self
                .turn_started
                .lock()
                .unwrap()
                .map(|t| t.elapsed().as_millis() as u64);
        }
        out.tool_calls = output.tool_calls.clone();
        let event = events.last().cloned().unwrap();
        drop(events);
        self.bump_events_rev();
        self.commit_pending_call();
        self.emit_session(event);
        self.persist_live();
    }

    fn commit_pending_call(&self) {
        let Some(pending) = self.pending_call.lock().unwrap().take() else {
            return;
        };
        let model = if pending.model.trim().is_empty() {
            "unknown"
        } else {
            pending.model.as_str()
        };
        let duration_ms = self
            .turn_started
            .lock()
            .unwrap()
            .map(|t| t.elapsed().as_millis() as u64);
        let cost = crate::usage::CallCost::pick(
            pending.cost_usd_ticks,
            estimated_cost_ticks(model, &pending.usage),
        );
        self.ledger
            .lock()
            .unwrap()
            .record_main_loop_call(model, &pending.usage, duration_ms, cost);
    }

    pub fn kinds(&self) -> Vec<&'static str> {
        self.events().iter().map(LogEvent::kind).collect()
    }

    /// Grok `RewindIfNoOutput`: true once the last user turn has assistant
    /// text, reasoning, tools, or a goal continuation.
    pub fn last_turn_has_output(&self) -> bool {
        let events = self.events.lock().unwrap();
        match events.iter().rposition(|e| matches!(e, LogEvent::User(_))) {
            None => false,
            Some(i) => inflight_has_output(&events[i + 1..]),
        }
    }

    /// Drop the in-flight user turn when it has no model/tool output yet.
    /// Returns the restored prompt (Grok cancel-rewind).
    pub fn rewind_inflight_user(&self) -> Option<(String, Vec<crate::types::UserImage>)> {
        if self.last_turn_has_output() {
            return None;
        }
        let mut events = self.events.lock().unwrap();
        let Some(start) = events.iter().rposition(|e| matches!(e, LogEvent::User(_))) else {
            self.rewound.store(true, Ordering::Relaxed);
            *self.pending_call.lock().unwrap() = None;
            *self.turn_started.lock().unwrap() = None;
            return None;
        };
        if inflight_has_output(&events[start + 1..]) {
            return None;
        }
        let text = match &events[start] {
            LogEvent::User(t) => t.clone(),
            _ => return None,
        };
        let mut times = self.times.lock().unwrap();
        events.truncate(start);
        times.truncate(start);
        drop(times);
        let leftover = events.last().cloned();
        drop(events);
        let images = self.user_images.lock().unwrap().pop().unwrap_or_default();
        self.pending_images.lock().unwrap().clear();
        *self.pending_call.lock().unwrap() = None;
        *self.turn_started.lock().unwrap() = None;
        self.rewound.store(true, Ordering::Relaxed);
        {
            let n = self.events.lock().unwrap().len();
            let mut from = self.compact_from.lock().unwrap();
            if *from > n {
                *from = n;
            }
        }
        self.bump_events_rev();
        if let Some(event) = leftover {
            self.emit_session(event);
        } else {
            self.emit_session(LogEvent::PreStep);
        }
        self.persist_live();
        Some((text, images))
    }

    pub fn take_pending_images(&self) -> Vec<crate::types::UserImage> {
        std::mem::take(&mut *self.pending_images.lock().unwrap())
    }

    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
        self.times.lock().unwrap().clear();
        *self.turn_started.lock().unwrap() = None;
        self.pending_images.lock().unwrap().clear();
        self.user_images.lock().unwrap().clear();
        *self.pending_call.lock().unwrap() = None;
        self.rewound.store(false, Ordering::Relaxed);
        self.auto_compact_suppressed.store(false, Ordering::Relaxed);
        self.compacting.store(false, Ordering::Relaxed);
        *self.display_title.lock().unwrap() = None;
        self.reset_compact();
        self.bump_events_rev();
        *self.ledger.lock().unwrap() = UsageLedger::default();
        {
            let mut usage = self.usage.lock().unwrap();
            let window = usage.window;
            *usage = TokenUsage {
                window,
                ..TokenUsage::default()
            };
        }
        self.persist_live();
    }

    pub fn auto_compact_suppressed(&self) -> bool {
        self.auto_compact_suppressed.load(Ordering::Relaxed)
    }

    pub fn set_auto_compact_suppressed(&self, on: bool) {
        self.auto_compact_suppressed.store(on, Ordering::Relaxed);
    }

    /// Drop a trailing empty `begin_llm` slot and insert interrupted
    /// [`crate::types::INTERRUPTED_TOOL_RESULT`] rows for any assistant
    /// `tool_calls` that never got a matching [`LogEvent::ToolExecute`].
    /// Stubs sit immediately after that assistant (and any results that did
    /// land), before a following user message.
    pub fn seal_incomplete_tool_calls(&self) {
        if self.rewound.load(Ordering::Relaxed) {
            return;
        }
        let mut events = self.events.lock().unwrap();
        let mut times = self.times.lock().unwrap();
        let mut changed = false;
        loop {
            match events.last() {
                Some(LogEvent::LlmStream(out))
                    if out.text.is_empty()
                        && out.reasoning.is_empty()
                        && out.tool_calls.is_empty() =>
                {
                    events.pop();
                    times.pop();
                    changed = true;
                }
                _ => break,
            }
        }
        let Some(asst_idx) = events.iter().rposition(|e| match e {
            LogEvent::LlmStream(out) => !out.tool_calls.is_empty(),
            _ => false,
        }) else {
            drop(times);
            if changed {
                let leftover = events.last().cloned();
                drop(events);
                self.bump_events_rev();
                if let Some(event) = leftover {
                    self.emit_session(event);
                }
                self.persist_live();
            }
            return;
        };
        let calls = match &events[asst_idx] {
            LogEvent::LlmStream(out) => out.tool_calls.clone(),
            _ => return,
        };
        let mut have = std::collections::HashSet::new();
        let mut insert_at = asst_idx + 1;
        for (i, event) in events.iter().enumerate().skip(asst_idx + 1) {
            match event {
                LogEvent::ToolExecute { id, .. } => {
                    have.insert(id.clone());
                    insert_at = i + 1;
                }
                _ => break,
            }
        }
        let missing: Vec<_> = calls
            .into_iter()
            .filter(|c| !have.contains(&c.id))
            .collect();
        if missing.is_empty() {
            drop(times);
            if changed {
                let leftover = events.last().cloned();
                drop(events);
                self.bump_events_rev();
                if let Some(event) = leftover {
                    self.emit_session(event);
                }
                self.persist_live();
            }
            return;
        }
        let now = SystemTime::now();
        for call in missing.into_iter().rev() {
            events.insert(
                insert_at,
                LogEvent::ToolExecute {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                    content: crate::types::INTERRUPTED_TOOL_RESULT.into(),

                    images: Vec::new(),
                },
            );
            times.insert(insert_at, now);
        }
        let event = events.get(insert_at).cloned();
        drop(times);
        drop(events);
        self.bump_events_rev();
        if let Some(event) = event {
            self.emit_session(event);
        }
        self.persist_live();
    }

    /// Exclusive compact lock so auto and `/compact` cannot overlap.
    pub fn try_begin_compact(&self) -> bool {
        self.compacting
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
    }

    pub fn end_compact(&self) {
        self.compacting.store(false, Ordering::Relaxed);
    }

    /// Keep the pager transcript. Store `prefix` as the sampler history head
    /// and append [`COMPACT_NOTICE`] so the TUI shows that a compact ran.
    pub fn replace_compacted(&self, prefix: Vec<LogEvent>) {
        let (old_users, old_images) = {
            let log = self.events.lock().unwrap();
            let images = self.user_images.lock().unwrap();
            let users: Vec<String> = log
                .iter()
                .filter_map(|e| match e {
                    LogEvent::User(t) => Some(t.clone()),
                    _ => None,
                })
                .collect();
            (users, images.clone())
        };
        let mut used = vec![false; old_users.len()];
        let mut mapped = Vec::new();
        for event in &prefix {
            if let LogEvent::User(text) = event {
                let img = old_users
                    .iter()
                    .enumerate()
                    .find(|(i, u)| {
                        !used[*i]
                            && (*u == text
                                || text.as_str() == format!("<user_query>\n{u}\n</user_query>"))
                    })
                    .map(|(i, _)| {
                        used[i] = true;
                        old_images.get(i).cloned().unwrap_or_default()
                    })
                    .unwrap_or_default();
                mapped.push(img);
            }
        }
        *self.compact_images.lock().unwrap() = mapped;
        let already = matches!(
            self.events.lock().unwrap().last(),
            Some(LogEvent::LlmStream(out)) if out.text == COMPACT_NOTICE
        );
        if !already {
            self.append(LogEvent::LlmStream(crate::types::LlmOutput {
                text: COMPACT_NOTICE.into(),
                ..crate::types::LlmOutput::default()
            }));
        }
        let from = self.events.lock().unwrap().len();
        *self.compact_from.lock().unwrap() = from;
        *self.compact_prefix.lock().unwrap() = Some(prefix);
        *self.pending_call.lock().unwrap() = None;
        self.rewound.store(false, Ordering::Relaxed);
        self.bump_events_rev();
        self.persist_live();
    }

    fn estimate_tokens(text: &str) -> u64 {
        let mut ascii = 0u64;
        let mut other = 0u64;
        for c in text.chars() {
            if c.is_ascii() {
                ascii += 1;
            } else {
                other += 1;
            }
        }
        other + ascii.saturating_add(3) / 4
    }

    /// Snapshot the live log into the resume list. No-op when empty.
    pub fn archive_current(&self) -> Option<ArchivedSession> {
        let events = self.events();
        if events.is_empty() {
            return None;
        }
        self.persist_live();
        let id = self.take_archive_id();
        let title = self
            .live_title()
            .or_else(|| {
                events.iter().find_map(|e| match e {
                    LogEvent::User(text) => {
                        let t = text.trim();
                        (!t.is_empty()).then(|| t.chars().take(40).collect())
                    }
                    _ => None,
                })
            })
            .unwrap_or_else(|| id.clone());
        let times = self.times();
        let (compact_prefix, compact_from) = self.compact_snapshot();
        let item = ArchivedSession {
            id,
            title,
            events,
            times,
            compact_prefix,
            compact_from,
        };
        self.write_archived(&item);
        self.archive.lock().unwrap().insert(0, item.clone());
        Some(item)
    }

    pub fn archived(&self) -> Vec<ArchivedSession> {
        self.archive.lock().unwrap().clone()
    }

    /// Replace the live log (Grok `/resume`). Emits the last event so the TUI redraws.
    pub fn restore(&self, id: &str) -> bool {
        let Some(item) = self
            .archive
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .cloned()
        else {
            return false;
        };
        let last = item.events.last().cloned();
        let user_n = item
            .events
            .iter()
            .filter(|e| matches!(e, LogEvent::User(_)))
            .count();
        let title = item.title.clone();
        *self.times.lock().unwrap() = item.times;
        *self.events.lock().unwrap() = item.events;
        *self.user_images.lock().unwrap() = vec![Vec::new(); user_n];
        self.pending_images.lock().unwrap().clear();
        *self.pending_call.lock().unwrap() = None;
        self.rewound.store(false, Ordering::Relaxed);
        self.apply_compact_snapshot(item.compact_prefix, item.compact_from);
        self.bump_events_rev();
        *self.ledger.lock().unwrap() = UsageLedger::default();
        if let Some(event) = last {
            self.emit_session(event);
        }
        *self.display_title.lock().unwrap() = Some(title);
        if self.disk_cwd.lock().unwrap().is_some() {
            *self.live_id.lock().unwrap() = item.id;
        }
        true
    }

    /// Title shown for the live thread. Gateway and `/resume` share this.
    pub fn live_title(&self) -> Option<String> {
        self.display_title
            .lock()
            .unwrap()
            .as_ref()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
    }

    pub fn set_live_title(&self, title: impl Into<String>) {
        let title = title.into();
        let trimmed = title.trim();
        *self.display_title.lock().unwrap() = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.chars().take(160).collect())
        };
        self.persist_live();
    }

    pub fn rename_archived(&self, id: &str, title: &str) -> Option<ArchivedSession> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return None;
        }
        let title: String = trimmed.chars().take(160).collect();
        let mut archive = self.archive.lock().unwrap();
        let item = archive.iter_mut().find(|s| s.id == id)?;
        item.title = title;
        let saved = item.clone();
        drop(archive);
        if let Some(cwd) = self.disk_cwd.lock().unwrap().clone() {
            let _ = crate::session_persist::save_title(&saved.id, &saved.title, &cwd);
        }
        Some(saved)
    }

    pub fn remove_archived(&self, id: &str) -> Option<ArchivedSession> {
        let mut archive = self.archive.lock().unwrap();
        let idx = archive.iter().position(|s| s.id == id)?;
        let item = archive.remove(idx);
        drop(archive);
        if let Some(cwd) = self.disk_cwd.lock().unwrap().clone() {
            let _ = crate::session_persist::remove(&item.id, &cwd);
        }
        Some(item)
    }

    fn take_archive_id(&self) -> String {
        if self.disk_cwd.lock().unwrap().is_some() {
            let mut live = self.live_id.lock().unwrap();
            if live.is_empty() {
                *live = crate::session_persist::new_id();
            }
            let id = live.clone();
            *live = crate::session_persist::new_id();
            id
        } else {
            let mut next = self.next_id.lock().unwrap();
            let id = format!("s{next}");
            *next += 1;
            id
        }
    }

    fn write_archived(&self, item: &ArchivedSession) {
        let Some(cwd) = self.disk_cwd.lock().unwrap().clone() else {
            return;
        };
        let _ = crate::session_persist::save(item, &cwd);
    }

    fn persist_live(&self) {
        let Some(cwd) = self.disk_cwd.lock().unwrap().clone() else {
            return;
        };
        let events = self.events();
        let mut live = self.live_id.lock().unwrap();
        if live.is_empty() {
            *live = crate::session_persist::new_id();
        }
        let id = live.clone();
        drop(live);
        if events.is_empty() {
            let _ = crate::session_persist::remove(&id, &cwd);
            return;
        }
        if crate::session_persist::empty_llm_placeholder(&events) {
            return;
        }
        let title = self
            .live_title()
            .or_else(|| {
                events.iter().find_map(|e| match e {
                    LogEvent::User(text) => {
                        let t = text.trim();
                        (!t.is_empty()).then(|| t.chars().take(40).collect())
                    }
                    _ => None,
                })
            })
            .unwrap_or_else(|| id.clone());
        let (compact_prefix, compact_from) = self.compact_snapshot();
        let item = ArchivedSession {
            id,
            title,
            events,
            times: self.times(),
            compact_prefix,
            compact_from,
        };
        let _ = crate::session_persist::save(&item, &cwd);
    }
}

fn inflight_has_output(tail: &[LogEvent]) -> bool {
    tail.iter().any(|e| match e {
        LogEvent::ToolExecute { .. } | LogEvent::SystemReminder(_) => true,
        LogEvent::LlmStream(out) => {
            !out.text.is_empty() || !out.reasoning.is_empty() || !out.tool_calls.is_empty()
        }
        LogEvent::User(_) | LogEvent::PreStep | LogEvent::Prompt(_) => false,
    })
}

pub fn sessions() -> Plugin {
    plugin("sessions", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(SESSIONS, Sessions::new(ctx.clone()))?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LogEvent;

    /// Responses 的 reasoning item 只在流收尾时才有，必须由 `finish_llm` 落进
    /// 会话行；漏了就等于整条推理链没存过，下一轮回放不出来。
    #[tokio::test]
    async fn finish_llm_stores_reasoning_items_for_replay() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("go".into()));
        sessions.begin_llm();
        sessions.apply_llm_delta(&crate::stream_acc::StreamDelta::Reasoning("plan".into()));
        sessions.finish_llm(&crate::types::LlmOutput {
            text: "answer".into(),
            reasoning: "plan".into(),
            tool_calls: vec![crate::types::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: "{}".into(),
            }],
            reasoning_items: vec![serde_json::json!({"type":"reasoning","id":"rs_1"})],
            ..crate::types::LlmOutput::default()
        });
        let out = sessions
            .model_history()
            .into_iter()
            .find_map(|e| match e {
                LogEvent::LlmStream(o) if !o.tool_calls.is_empty() => Some(o),
                _ => None,
            })
            .expect("assistant turn");
        assert_eq!(out.reasoning, "plan");
        assert_eq!(out.reasoning_items.len(), 1, "原件要落进会话：{out:?}");
        assert_eq!(out.reasoning_items[0]["id"], "rs_1");
    }

    #[tokio::test]
    async fn archive_then_restore() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("hello world".into()));
        let item = sessions.archive_current().expect("non-empty log archives");
        assert_eq!(item.title, "hello world");
        sessions.clear();
        assert!(sessions.events().is_empty());
        assert!(sessions.restore(&item.id));
        assert_eq!(sessions.events().len(), 1);
        assert_eq!(sessions.times().len(), 1);
    }

    #[tokio::test]
    async fn rename_and_remove_archived() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("keep me".into()));
        let item = sessions.archive_current().unwrap();
        let renamed = sessions
            .rename_archived(&item.id, "  new title  ")
            .expect("archived rename");
        assert_eq!(renamed.title, "new title");
        assert_eq!(
            sessions.remove_archived(&item.id).unwrap().title,
            "new title"
        );
        assert!(sessions.archived().is_empty());
        assert!(sessions.rename_archived(&item.id, "gone").is_none());
    }

    #[tokio::test]
    async fn empty_log_does_not_archive() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        assert!(sessions.archive_current().is_none());
        assert!(sessions.archived().is_empty());
    }

    #[test]
    fn user_addon_appends_hidden_reminder() {
        let root = Context::new();
        let sessions = Sessions::new(root);
        sessions.arm_user_addon("/loop 5m check", "wire instruction");
        sessions.append(LogEvent::User("/loop 5m check".into()));
        let events = sessions.events();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], LogEvent::User(t) if t == "/loop 5m check"));
        assert!(matches!(&events[1], LogEvent::SystemReminder(t) if t == "wire instruction"));
        sessions.arm_user_addon("/loop 1h x", "loop wire");
        sessions.append(LogEvent::User("hello".into()));
        assert_eq!(sessions.events().len(), 3);
        sessions.append(LogEvent::User("/loop 1h x".into()));
        assert!(matches!(
            sessions.events().last(),
            Some(LogEvent::SystemReminder(t)) if t == "loop wire"
        ));
    }

    #[test]
    fn events_rev_bumps_on_append_and_clear() {
        let root = Context::new();
        let sessions = Sessions::new(root);
        assert_eq!(sessions.events_rev(), 0);
        sessions.append(LogEvent::User("a".into()));
        let r1 = sessions.events_rev();
        assert!(r1 > 0);
        assert_eq!(sessions.occupancy_stamp().0, r1);
        sessions.append(LogEvent::User("b".into()));
        assert!(sessions.events_rev() > r1);
        sessions.clear();
        assert!(sessions.events_rev() > r1);
        sessions.with_log(|events, _| assert!(events.is_empty()));
    }

    #[tokio::test]
    async fn llm_deltas_patch_the_same_event() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.begin_llm();
        sessions.apply_llm_delta(&crate::stream_acc::StreamDelta::Text("Hel".into()));
        sessions.apply_llm_delta(&crate::stream_acc::StreamDelta::Text("lo".into()));
        sessions.finish_llm(&crate::types::LlmOutput {
            text: "Hello".into(),
            ..crate::types::LlmOutput::default()
        });
        assert_eq!(sessions.events().len(), 1);
        match &sessions.events()[0] {
            LogEvent::LlmStream(out) => assert_eq!(out.text, "Hello"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn official_usage_accumulates_on_the_ledger() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        let mut tokens = crate::usage::TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 10,
            cached_prompt_tokens: 40,
            reasoning_tokens: 3,
            ..crate::usage::TokenUsage::default()
        };
        sessions.begin_llm();
        sessions.apply_llm_delta(&crate::stream_acc::StreamDelta::Usage {
            tokens: crate::usage::TokenUsage {
                prompt_tokens: 80,
                ..crate::usage::TokenUsage::default()
            },
            official: false,
            model: String::new(),
            cost_usd_ticks: None,
        });
        sessions.apply_llm_delta(&crate::stream_acc::StreamDelta::Usage {
            tokens: tokens.clone(),
            official: true,
            model: "grok-4".into(),
            cost_usd_ticks: Some(70),
        });
        sessions.finish_llm(&crate::types::LlmOutput::default());
        assert_eq!(sessions.ledger().totals.model_calls, 1);
        assert_eq!(sessions.ledger().totals.input_tokens, 100);
        assert_eq!(sessions.ledger().totals.cached_read_tokens, 40);
        assert_eq!(sessions.ledger().totals.reasoning_tokens, 3);
        assert_eq!(sessions.usage().prompt, 100);
        assert!(sessions.usage().official);

        tokens.prompt_tokens = 50;
        tokens.completion_tokens = 5;
        tokens.cached_prompt_tokens = 10;
        tokens.reasoning_tokens = 1;
        sessions.begin_llm();
        sessions.apply_llm_delta(&crate::stream_acc::StreamDelta::Usage {
            tokens,
            official: true,
            model: "grok-4".into(),
            cost_usd_ticks: None,
        });
        sessions.finish_llm(&crate::types::LlmOutput::default());
        let u = sessions.prompt_usage();
        assert_eq!(u.totals.input_tokens, 150);
        assert_eq!(u.totals.output_tokens, 15);
        assert_eq!(u.totals.cached_read_tokens, 50);
        assert_eq!(u.totals.reasoning_tokens, 4);
        assert_eq!(u.num_turns, 2);
        assert!(u.totals.cost_is_partial);
        assert!(u.totals.cost_usd_ticks.is_none());

        sessions.clear();
        assert_eq!(sessions.ledger().totals.model_calls, 0);
    }

    #[tokio::test]
    async fn estimate_usage_does_not_hit_the_ledger() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.begin_llm();
        sessions.apply_llm_delta(&crate::stream_acc::StreamDelta::Usage {
            tokens: crate::usage::TokenUsage {
                prompt_tokens: 12,
                ..crate::usage::TokenUsage::default()
            },
            official: false,
            model: String::new(),
            cost_usd_ticks: None,
        });
        sessions.finish_llm(&crate::types::LlmOutput::default());
        assert_eq!(sessions.ledger().totals.model_calls, 0);
        assert_eq!(sessions.usage().prompt, 12);
        assert!(!sessions.usage().official);
    }

    #[tokio::test]
    async fn subagent_ledger_folds_without_bumping_turns() {
        let ctx = Context::new();
        let parent = Sessions::new(ctx.clone());
        let child = Sessions::isolated(ctx);
        child.begin_llm();
        child.apply_llm_delta(&crate::stream_acc::StreamDelta::Usage {
            tokens: crate::usage::TokenUsage {
                prompt_tokens: 5,
                completion_tokens: 1,
                ..crate::usage::TokenUsage::default()
            },
            official: true,
            model: "child-model".into(),
            cost_usd_ticks: None,
        });
        child.finish_llm(&crate::types::LlmOutput::default());
        parent.fold_subagent_ledger(&child.ledger());
        assert_eq!(parent.ledger().main_loop_model_calls, 0);
        assert_eq!(parent.ledger().totals.model_calls, 1);
        assert_eq!(parent.ledger().totals.input_tokens, 5);
        assert_eq!(parent.ledger().by_model["child-model"].input_tokens, 5);
    }

    #[tokio::test]
    async fn rewind_inflight_user_drops_user_without_output() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("keep".into()));
        sessions.append(LogEvent::LlmStream(crate::types::LlmOutput {
            text: "reply".into(),
            ..crate::types::LlmOutput::default()
        }));
        sessions.append(LogEvent::User("undo me".into()));
        sessions.append(LogEvent::PreStep);
        sessions.begin_llm();
        let restored = sessions
            .rewind_inflight_user()
            .expect("no-output user rewinds");
        assert_eq!(restored.0, "undo me");
        assert_eq!(sessions.events().len(), 2);
        match &sessions.events()[0] {
            LogEvent::User(t) => assert_eq!(t, "keep"),
            other => panic!("{other:?}"),
        }
        assert!(sessions.rewind_inflight_user().is_none());
    }

    #[tokio::test]
    async fn rewind_skips_once_assistant_text_arrives() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("keep me".into()));
        sessions.begin_llm();
        sessions.apply_llm_delta(&crate::stream_acc::StreamDelta::Text("Hi".into()));
        assert!(sessions.last_turn_has_output());
        assert!(sessions.rewind_inflight_user().is_none());
        assert_eq!(sessions.events().len(), 2);
    }

    #[tokio::test]
    async fn seal_inserts_interrupted_results_before_following_user() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("hi".into()));
        sessions.append(LogEvent::LlmStream(crate::types::LlmOutput {
            tool_calls: vec![
                crate::types::ToolCall {
                    id: "c1".into(),
                    name: "bash".into(),
                    arguments: "{}".into(),
                },
                crate::types::ToolCall {
                    id: "c2".into(),
                    name: "read".into(),
                    arguments: "{}".into(),
                },
            ],
            ..crate::types::LlmOutput::default()
        }));
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
            content: "ok".into(),

            images: Vec::new(),
        });
        sessions.append(LogEvent::User("follow-up".into()));
        sessions.seal_incomplete_tool_calls();
        let events = sessions.events();
        match &events[2] {
            LogEvent::ToolExecute { id, content, .. } => {
                assert_eq!(id, "c1");
                assert_eq!(content, "ok");
            }
            other => panic!("{other:?}"),
        }
        match &events[3] {
            LogEvent::ToolExecute { id, content, .. } => {
                assert_eq!(id, "c2");
                assert_eq!(content, crate::types::INTERRUPTED_TOOL_RESULT);
            }
            other => panic!("{other:?}"),
        }
        match &events[4] {
            LogEvent::User(t) => assert_eq!(t, "follow-up"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn seal_drops_trailing_empty_llm_stream() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("hi".into()));
        sessions.begin_llm();
        sessions.seal_incomplete_tool_calls();
        assert_eq!(sessions.events().len(), 1);
    }

    #[tokio::test]
    async fn compact_keeps_pager_log_and_shortens_model_history() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("first".into()));
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
            content: "secret body".into(),

            images: Vec::new(),
        });
        sessions.append(LogEvent::User("second".into()));
        let prefix = vec![
            LogEvent::User("first".into()),
            LogEvent::SystemReminder("summary of earlier turns".into()),
            LogEvent::LlmStream(crate::types::LlmOutput {
                text: crate::types::COMPACT_NOTICE.into(),
                ..crate::types::LlmOutput::default()
            }),
        ];
        sessions.replace_compacted(prefix);
        let display = sessions.events();
        assert!(
            display.iter().any(|e| matches!(
                e,
                LogEvent::ToolExecute { content, .. } if content.contains("secret body")
            )),
            "{display:?}"
        );
        assert!(display.iter().any(|e| matches!(
            e,
            LogEvent::LlmStream(o) if o.text == crate::types::COMPACT_NOTICE
        )));
        let model = sessions.model_history();
        assert!(!model.iter().any(|e| matches!(
            e,
            LogEvent::ToolExecute { content, .. } if content.contains("secret body")
        )));
        assert!(model.iter().any(|e| matches!(
            e,
            LogEvent::SystemReminder(t) if t.contains("summary of earlier")
        )));
        sessions.append(LogEvent::User("after compact".into()));
        assert!(sessions.events().iter().any(|e| matches!(
            e,
            LogEvent::User(t) if t == "after compact"
        )));
        assert!(sessions.model_history().iter().any(|e| matches!(
            e,
            LogEvent::User(t) if t == "after compact"
        )));
    }
}
