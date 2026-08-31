use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use cordis::{plugin, Context, Inject, Plugin};

use crate::names::{SESSIONS, SESSION_EVENT};
use crate::types::LogEvent;
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
}

/// In-memory session log. DSH `ctx.sessions`; Grok conversation persistence.
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
}

/// Official SSE usage held until [`Sessions::finish_llm`] so one sample is
/// one ledger row even if the stream repeats the usage object.
struct PendingCall {
    usage: CallUsage,
    model: String,
    cost_usd_ticks: Option<i64>,
}

impl Sessions {
    pub fn new(ctx: Context) -> Self {
        Self::with_identity(ctx, "main", true)
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
        }
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
        self.bump_events_rev();
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
        self.ledger.lock().unwrap().record_main_loop_call(
            model,
            &pending.usage,
            duration_ms,
            pending.cost_usd_ticks,
        );
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
        self.bump_events_rev();
        if let Some(event) = leftover {
            self.emit_session(event);
        } else {
            self.emit_session(LogEvent::PreStep);
        }
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

    /// Swap the live log for a compacted prefix. Keeps images for User rows
    /// whose text still appears, in order.
    pub fn replace_compacted(&self, events: Vec<LogEvent>) {
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
        for event in &events {
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
        let n = events.len();
        let last = events.last().cloned();
        *self.times.lock().unwrap() = vec![SystemTime::now(); n];
        *self.events.lock().unwrap() = events;
        *self.user_images.lock().unwrap() = mapped;
        self.pending_images.lock().unwrap().clear();
        *self.pending_call.lock().unwrap() = None;
        self.rewound.store(false, Ordering::Relaxed);
        self.bump_events_rev();
        if let Some(event) = last {
            self.emit_session(event);
        }
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
        let mut next = self.next_id.lock().unwrap();
        let id = format!("s{next}");
        *next += 1;
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
        let item = ArchivedSession {
            id,
            title,
            events,
            times,
        };
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
        self.bump_events_rev();
        *self.ledger.lock().unwrap() = UsageLedger::default();
        if let Some(event) = last {
            self.emit_session(event);
        }
        *self.display_title.lock().unwrap() = Some(title);
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
        Some(item.clone())
    }

    pub fn remove_archived(&self, id: &str) -> Option<ArchivedSession> {
        let mut archive = self.archive.lock().unwrap();
        let idx = archive.iter().position(|s| s.id == id)?;
        Some(archive.remove(idx))
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
        assert_eq!(sessions.remove_archived(&item.id).unwrap().title, "new title");
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
}
