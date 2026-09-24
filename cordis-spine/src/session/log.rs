use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use cordis::{plugin, Context, Inject, Plugin};

use crate::names::{SESSIONS, SESSION_EVENT};
pub use cordis_base::types::{is_main_identity, ROOT_IDENTITY, TAB_IDENTITY_PREFIX};
use cordis_base::types::{LogEvent, COMPACT_NOTICE};
use cordis_base::usage::{PromptUsage, TokenUsage as CallUsage, UsageLedger, UsageTotals};

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
    /// Agent preset id active when archived. `None` on sessions saved before this field.
    pub preset_id: Option<String>,
}

/// 这条事件进不进模型上下文。
///
/// [`LogEvent::Notice`] 是**只给用户看**的：滚动区渲染成卡片，但它既不该占上下
/// 文，也不该让模型觉得需要回应。这是唯一被滤掉的一类——[`Sessions::model_history`]
/// 是采样看到的全部，漏在这里就等于漏进模型。
fn model_visible(event: &LogEvent) -> bool {
    !matches!(event, LogEvent::Notice { .. })
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
    pending_images: Arc<Mutex<Vec<cordis_base::types::UserImage>>>,
    user_images: Arc<Mutex<Vec<Vec<cordis_base::types::UserImage>>>>,
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
    /// Cwd captured when a non-disk tab opens. Its plan file stays under this
    /// cwd after a later process-wide `cd`. This does not turn persistence on;
    /// only [`Self::disk_cwd`] does that.
    plan_cwd: Arc<Mutex<Option<PathBuf>>>,
    /// 这一页（或子代理）的工作目录。`None` = 跟随进程 cwd，也就是 TUI 单页的
    /// 现状；钉住之后，工具、系统提示、子进程都按它走，进程 cwd 再变也不影响。
    /// 读取走 [`crate::session::cwd::session_cwd`]，不要直接读这个字段。
    workspace_cwd: Arc<Mutex<Option<PathBuf>>>,
    live_id: Arc<Mutex<String>>,
    /// Preset id stamped into `meta.json` on save / archive. Updated by the TUI.
    live_preset_id: Arc<Mutex<Option<String>>>,
    /// Compacted prefix sent to the sampler. `None` = display log is the model history.
    compact_prefix: Arc<Mutex<Option<Vec<LogEvent>>>>,
    compact_from: Arc<Mutex<usize>>,
    compact_images: Arc<Mutex<Vec<Vec<cordis_base::types::UserImage>>>>,
    /// Successful compact cycles (for flush-once-per-cycle gating).
    compaction_count: Arc<AtomicU64>,
    /// Compaction cycle when memory was last flushed.
    last_flush_compaction: Arc<AtomicU64>,
    /// Last accepted flush markdown (for delta flushes).
    last_flush_content: Arc<Mutex<Option<String>>>,
    /// User-facing page that owns a child log (`main` / `main#N`).
    /// Main and tab sessions leave this empty and report their own identity.
    page_home: Arc<Mutex<Option<String>>>,
}

/// Official SSE usage held until [`Sessions::finish_llm`] so one sample is
/// one ledger row even if the stream repeats the usage object.
struct PendingCall {
    usage: CallUsage,
    model: String,
    cost_usd_ticks: Option<i64>,
}

/// 按 `[model.<id>.pricing]` 估一次调用的费用。`None` = 没配单价。
///
/// 这里认的是**发给上游的 slug**（`pending.model`），不是目录 id：两个目录条目
/// 可以指向同一个上游模型，价是上游模型的属性。找不到 slug 时退回按目录 id 匹配，
/// 那是 `api_model` 省略时的常态。
fn estimated_cost_ticks(wire_model: &str, usage: &cordis_base::usage::TokenUsage) -> Option<i64> {
    let catalog = cordis_base::config::load_catalog();
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
    ///
    /// 开页时把 cwd 钉在 [`Self::plan_cwd`] 上。分页不 `attach_disk`，但计划
    /// 文件要跟着开页时的工作区走，不能每次再读 `current_dir()`。
    pub fn tab(ctx: Context, index: usize) -> Self {
        let session = Self::with_identity(ctx, format!("{TAB_IDENTITY_PREFIX}{index}"), true);
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        session.pin_plan_cwd(cwd);
        session
    }

    /// Pin the cwd used for this page's ephemeral plan directory.
    pub fn pin_plan_cwd(&self, cwd: impl Into<PathBuf>) {
        *self.plan_cwd.lock().unwrap() = Some(cwd.into());
    }

    /// Cwd pinned at tab open. `None` for the disk-backed root session and
    /// for subagents.
    pub fn plan_cwd(&self) -> Option<PathBuf> {
        self.plan_cwd.lock().unwrap().clone()
    }

    /// The user-facing page (`main` / `main#N`) this context belongs to.
    pub fn page_of(ctx: &Context) -> Option<String> {
        ctx.get::<Sessions>(SESSIONS).and_then(|s| s.ui_page())
    }

    /// True after [`Self::attach_disk`]. Those sessions persist; tab plans must
    /// not be deleted out from under them.
    pub fn on_disk(&self) -> bool {
        self.disk_cwd.lock().unwrap().is_some()
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
            // 窗口从 **0（未知）** 起步，不预先塞一个 128_000。
            //
            // `context_usage::window_size` 把任何非零的会话窗口当权威值，直接
            // 返回、不再查目录；而真正的窗口要到第一次组请求时才由
            // `http::…` 的 `set_window` 写进来。于是冷启动那一段顶栏显示的是
            // 种子值 128K，聊一句之后才跳成 config 里配的 1M——看起来像是窗口
            // 自己变了。`project_instructions` / `workflow` / `skills` 早就写成
            // `filter(|w| *w > 0).or_else(查目录)`，本来就把 0 当"还不知道"；
            // 是这颗种子把那套兜底全废掉了。
            //
            // 0 对下游是安全的：`set_window` 只接受 `> 0`，
            // `compact::exceeds_threshold` 对 window == 0 直接返回 false。
            usage: Arc::new(Mutex::new(TokenUsage::default())),
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
            plan_cwd: Arc::new(Mutex::new(None)),
            workspace_cwd: Arc::new(Mutex::new(None)),
            live_id: Arc::new(Mutex::new(String::new())),
            live_preset_id: Arc::new(Mutex::new(None)),
            compact_prefix: Arc::new(Mutex::new(None)),
            compact_from: Arc::new(Mutex::new(0)),
            compact_images: Arc::new(Mutex::new(Vec::new())),
            compaction_count: Arc::new(AtomicU64::new(0)),
            last_flush_compaction: Arc::new(AtomicU64::new(0)),
            last_flush_content: Arc::new(Mutex::new(None)),
            page_home: Arc::new(Mutex::new(None)),
        }
    }

    /// Page this log should surface prompts on. Tabs are their own page.
    /// A subagent copies the spawning page so its permission prompt opens there.
    pub fn ui_page(&self) -> Option<String> {
        if self.is_main() {
            Some(self.identity().to_string())
        } else {
            self.page_home.lock().unwrap().clone()
        }
    }

    pub fn pin_page_home(&self, page: impl Into<String>) {
        *self.page_home.lock().unwrap() = Some(page.into());
    }

    /// 把这一页的工作目录钉在 `cwd` 上。子代理在起步时继承父会话钉住的值。
    pub fn pin_workspace_cwd(&self, cwd: impl Into<PathBuf>) {
        *self.workspace_cwd.lock().unwrap() = Some(cwd.into());
    }

    /// 钉住的工作目录；`None` 表示跟随进程 cwd。
    pub fn workspace_cwd(&self) -> Option<PathBuf> {
        self.workspace_cwd.lock().unwrap().clone()
    }

    /// Stamp the active agent preset into subsequent `meta.json` writes.
    pub fn set_preset_id(&self, id: Option<String>) {
        *self.live_preset_id.lock().unwrap() = id.filter(|s| !s.trim().is_empty());
    }

    /// Preset stamped on the live session (also set by `/resume` when meta has one).
    pub fn preset_id(&self) -> Option<String> {
        self.live_preset_id.lock().unwrap().clone()
    }

    /// `preset_id` field on an archived session, if present.
    ///
    /// Use this (not [`Self::preset_id`] after [`Self::restore`]) when calling
    /// [`crate::session::resume_preset::apply_restored_preset`]: restore leaves
    /// a startup seed alone when meta omitted the field.
    pub fn archived_preset_id(&self, id: &str) -> Option<String> {
        self.archive
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| s.preset_id.clone())
    }

    /// Seed `live_preset_id` from [`crate::agent::presets::AgentPresets::current_id`]
    /// when still unset so the first persist stamps a real id (old metas stay
    /// loadable with the field absent).
    pub fn seed_preset_if_unset(&self, current_id: &str) {
        let trimmed = current_id.trim();
        if trimmed.is_empty() {
            return;
        }
        let mut live = self.live_preset_id.lock().unwrap();
        if live.is_none() {
            *live = Some(trimmed.to_string());
        }
    }

    /// Load `$DOCK_HOME/sessions/<cwd>/` into the resume list and start writing
    /// this log there. `<cwd>` 是这个会话自己的工作目录（钉住的，没钉就是进程
    /// cwd）——分页各落到各自项目下。No-op for isolated child logs. Safe to call
    /// more than once (replaces the list).
    pub fn attach_disk(&self) {
        if !self.emit {
            return;
        }
        let cwd = self.own_cwd();
        let loaded = crate::session::persist::load_cwd(&cwd);
        *self.archive.lock().unwrap() = loaded;
        *self.live_id.lock().unwrap() = crate::session::persist::new_id();
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

    /// Whether `cwd` has this session on disk. Does not touch the live log.
    /// `cwd` 是要开页的那一页的工作目录（[`crate::session_cwd`]），不是进程 cwd。
    pub fn can_adopt_archived(id: &str, cwd: &Path) -> bool {
        if id.is_empty() {
            return false;
        }
        crate::session::persist::load_session(id, cwd).is_some()
    }

    /// 这个会话的工作目录：钉住的值，没钉就是进程 cwd。
    fn own_cwd(&self) -> PathBuf {
        self.workspace_cwd()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Load one on-disk session of this page's cwd into an **empty** log and
    /// keep writing back to that same id.
    ///
    /// A new tab starts empty. [`Self::resume_id`] would archive that empty log
    /// first; once `live_id` is set, persisting an empty log deletes the folder.
    /// This skips the archive step and refuses a log that already has events,
    /// so opening history on a page cannot wipe the thread it is adopting.
    pub fn adopt_archived(&self, id: &str) -> bool {
        let cwd = self.own_cwd();
        if !self.emit || !Self::can_adopt_archived(id, &cwd) {
            return false;
        }
        if !self.events.lock().unwrap().is_empty() {
            return false;
        }
        let loaded = crate::session::persist::load_cwd(&cwd);
        if !loaded.iter().any(|s| s.id == id) {
            return false;
        }
        *self.archive.lock().unwrap() = loaded;
        *self.disk_cwd.lock().unwrap() = Some(cwd);
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

    /// A per-tab waterfall handler may act only when `identity` is this page.
    ///
    /// Listeners are process-global (`Context::waterfall` passes no isolate
    /// filter). [`cordis_base::types::is_main_identity`] is true for every tab,
    /// so it does not keep page 2's turn out of page 1's handler.
    pub fn turn_is_this_page(ctx: &Context, identity: &str) -> bool {
        ctx.get::<Sessions>(SESSIONS)
            .is_some_and(|sessions| sessions.identity() == identity)
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
        usage: &cordis_base::usage::TokenUsage,
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
        let cost = cordis_base::usage::CallCost::pick(
            reported_cost_ticks,
            estimated_cost_ticks(model, usage),
        );
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

    /// 放一条整场对话的背景（工作区规约、长期记忆）：本会话还没向模型发过任何
    /// 请求时，插到第一条用户消息**之前**；否则和 [`Self::append`] 一样追加。
    ///
    /// 只在「什么都还没发」时往前插：那时插在哪都改写不到已经发出去、被上游缓存
    /// 过的前缀，而排在用户消息之前能让每个新会话的请求头逐字节相同，跨会话命中
    /// 前缀缓存。有过任何一条模型输出（失败的也算——请求已经出去了），或模型历史
    /// 从压缩头开始，就退回追加。
    pub fn insert_preamble(&self, event: LogEvent) {
        if matches!(event, LogEvent::User(_)) || self.rewound.load(Ordering::Relaxed) {
            return self.append(event);
        }
        // 锁序与 `model_history` 一致：先压缩头，再事件表。
        let compacted = self.compact_prefix.lock().unwrap().is_some();
        let at = {
            let mut events = self.events.lock().unwrap();
            let sent = compacted || events.iter().any(|e| matches!(e, LogEvent::LlmStream(_)));
            let at = if sent {
                None
            } else {
                events.iter().position(|e| matches!(e, LogEvent::User(_)))
            };
            if let Some(at) = at {
                events.insert(at, event.clone());
            }
            at
        };
        let Some(at) = at else {
            return self.append(event);
        };
        {
            let mut times = self.times.lock().unwrap();
            let at = at.min(times.len());
            times.insert(at, SystemTime::now());
        }
        self.bump_events_rev();
        self.emit_session(event);
        self.persist_live();
    }

    fn emit_session(&self, event: LogEvent) {
        if self.emit {
            self.ctx.emit(SESSION_EVENT, event);
        }
    }

    pub fn queue_user_images(&self, images: Vec<cordis_base::types::UserImage>) {
        *self.pending_images.lock().unwrap() = images;
    }

    pub fn user_images(&self) -> Vec<Vec<cordis_base::types::UserImage>> {
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
            let mut out = self.events();
            out.retain(model_visible);
            return out;
        };
        let from = *self.compact_from.lock().unwrap();
        let events = self.events.lock().unwrap();
        let start = from.min(events.len());
        let mut out = prefix.clone();
        out.extend(events[start..].iter().filter(|e| model_visible(e)).cloned());
        out
    }

    /// Images aligned with [`Self::model_history`] user rows (not the pager log).
    pub fn model_user_images(&self) -> Vec<Vec<cordis_base::types::UserImage>> {
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
        self.append(LogEvent::LlmStream(cordis_base::types::LlmOutput::default()));
    }

    pub fn apply_llm_delta(&self, delta: &cordis_base::stream_acc::StreamDelta) {
        if self.rewound.load(Ordering::Relaxed) {
            return;
        }
        match delta {
            cordis_base::stream_acc::StreamDelta::Text(text) => {
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
            cordis_base::stream_acc::StreamDelta::Reasoning(text) => {
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
            cordis_base::stream_acc::StreamDelta::Usage {
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
                self.emit_session(LogEvent::LlmStream(cordis_base::types::LlmOutput::default()));
            }
        }
    }

    pub fn finish_llm(&self, output: &cordis_base::types::LlmOutput) {
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
        // 失败详情只落事件、不进 `text`：三条 wire builder 都以 `text` 非空或
        // 有 tool_call 为门槛，所以它天然不会被回放给模型。
        if out.error.is_none() {
            out.error = output.error.clone();
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
        let cost = cordis_base::usage::CallCost::pick(
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

    /// [`Self::rewind_inflight_user`] 会不会成功——但**不克隆**日志。
    ///
    /// 快捷键条每帧都要问一次「现在能不能撤」，所以这里只上一次锁、反向扫到最近
    /// 一条 `User` 为止。用 `events()` 判会把整条会话（连同助手正文、工具结果、
    /// 图片）深拷贝一遍，每帧一次。
    pub fn has_undoable_send(&self) -> bool {
        let events = self.events.lock().unwrap();
        match events.iter().rposition(|e| matches!(e, LogEvent::User(_))) {
            None => false,
            Some(i) => !inflight_has_output(&events[i + 1..]),
        }
    }

    /// Drop the in-flight user turn when it has no model/tool output yet.
    /// Returns the restored prompt (Grok cancel-rewind).
    pub fn rewind_inflight_user(&self) -> Option<(String, Vec<cordis_base::types::UserImage>)> {
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

    pub fn take_pending_images(&self) -> Vec<cordis_base::types::UserImage> {
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

    pub fn compaction_count(&self) -> u64 {
        self.compaction_count.load(Ordering::Relaxed)
    }

    pub fn bump_compaction_count(&self) {
        self.compaction_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn last_flush_compaction(&self) -> u64 {
        self.last_flush_compaction.load(Ordering::Relaxed)
    }

    pub fn mark_flushed_for_compaction(&self, cycle: u64) {
        self.last_flush_compaction.store(cycle, Ordering::Relaxed);
    }

    pub fn last_flush_content(&self) -> Option<String> {
        self.last_flush_content.lock().unwrap().clone()
    }

    pub fn set_last_flush_content(&self, content: Option<String>) {
        *self.last_flush_content.lock().unwrap() = content;
    }

    /// Drop a trailing empty `begin_llm` slot and insert interrupted
    /// [`cordis_base::types::INTERRUPTED_TOOL_RESULT`] rows for any assistant
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
                    content: cordis_base::types::INTERRUPTED_TOOL_RESULT.into(),

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
            self.append(LogEvent::LlmStream(cordis_base::types::LlmOutput {
                text: COMPACT_NOTICE.into(),
                ..cordis_base::types::LlmOutput::default()
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
            preset_id: self.live_preset_id.lock().unwrap().clone(),
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
        // Old sessions omit preset_id — keep the live preset as-is.
        if let Some(pid) = item.preset_id {
            *self.live_preset_id.lock().unwrap() = Some(pid);
        }
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
            let _ = crate::session::persist::save_title(&saved.id, &saved.title, &cwd);
        }
        Some(saved)
    }

    pub fn remove_archived(&self, id: &str) -> Option<ArchivedSession> {
        let mut archive = self.archive.lock().unwrap();
        let idx = archive.iter().position(|s| s.id == id)?;
        let item = archive.remove(idx);
        drop(archive);
        if let Some(cwd) = self.disk_cwd.lock().unwrap().clone() {
            let _ = crate::session::persist::remove(&item.id, &cwd);
        }
        Some(item)
    }

    /// 当前这一份会话在磁盘上的 id（`persist_live` 写的那个），还没落过盘就是
    /// 空串。dashboard 用它把名册里的同一条去重——已经开着的会话不该在「历史」
    /// 里再出现一次。
    pub fn live_session_id(&self) -> String {
        self.live_id.lock().unwrap().clone()
    }

    /// `$DOCK_HOME/sessions/<cwd-key>/<id>/` for the live thread, when disk-backed.
    pub fn disk_session_dir(&self) -> Option<std::path::PathBuf> {
        let cwd = self.disk_cwd.lock().unwrap().clone()?;
        let mut live = self.live_id.lock().unwrap();
        if live.is_empty() {
            *live = crate::session::persist::new_id();
        }
        let dir = crate::session::persist::sessions_cwd_dir(&cwd).join(live.as_str());
        let _ = std::fs::create_dir_all(&dir);
        Some(dir)
    }

    fn take_archive_id(&self) -> String {
        if self.disk_cwd.lock().unwrap().is_some() {
            let mut live = self.live_id.lock().unwrap();
            if live.is_empty() {
                *live = crate::session::persist::new_id();
            }
            let id = live.clone();
            *live = crate::session::persist::new_id();
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
        let _ = crate::session::persist::save(item, &cwd);
    }

    fn persist_live(&self) {
        let Some(cwd) = self.disk_cwd.lock().unwrap().clone() else {
            return;
        };
        let events = self.events();
        let mut live = self.live_id.lock().unwrap();
        if live.is_empty() {
            *live = crate::session::persist::new_id();
        }
        let id = live.clone();
        drop(live);
        if events.is_empty() {
            let _ = crate::session::persist::remove(&id, &cwd);
            return;
        }
        if crate::session::persist::empty_llm_placeholder(&events) {
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
            preset_id: self.live_preset_id.lock().unwrap().clone(),
        };
        let _ = crate::session::persist::save(&item, &cwd);
    }
}

fn inflight_has_output(tail: &[LogEvent]) -> bool {
    tail.iter().any(|e| match e {
        LogEvent::ToolExecute { .. } | LogEvent::SystemReminder(_) => true,
        LogEvent::LlmStream(out) => {
            !out.text.is_empty() || !out.reasoning.is_empty() || !out.tool_calls.is_empty()
        }
        // Notice 只是给用户看的卡片，不算"这一轮产出过东西"。
        LogEvent::User(_) | LogEvent::PreStep | LogEvent::Prompt(_) | LogEvent::Notice { .. } => {
            false
        }
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
    use cordis_base::types::LogEvent;

    /// Responses 的 reasoning item 只在流收尾时才有，必须由 `finish_llm` 落进
    /// 会话行；漏了就等于整条推理链没存过，下一轮回放不出来。
    #[tokio::test]
    async fn finish_llm_stores_reasoning_items_for_replay() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("go".into()));
        sessions.begin_llm();
        sessions.apply_llm_delta(&cordis_base::stream_acc::StreamDelta::Reasoning(
            "plan".into(),
        ));
        sessions.finish_llm(&cordis_base::types::LlmOutput {
            text: "answer".into(),
            reasoning: "plan".into(),
            tool_calls: vec![cordis_base::types::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: "{}".into(),
            }],
            reasoning_items: vec![serde_json::json!({"type":"reasoning","id":"rs_1"})],
            ..cordis_base::types::LlmOutput::default()
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
        sessions.apply_llm_delta(&cordis_base::stream_acc::StreamDelta::Text("Hel".into()));
        sessions.apply_llm_delta(&cordis_base::stream_acc::StreamDelta::Text("lo".into()));
        sessions.finish_llm(&cordis_base::types::LlmOutput {
            text: "Hello".into(),
            ..cordis_base::types::LlmOutput::default()
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
        let mut tokens = cordis_base::usage::TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 10,
            cached_prompt_tokens: 40,
            reasoning_tokens: 3,
            ..cordis_base::usage::TokenUsage::default()
        };
        sessions.begin_llm();
        sessions.apply_llm_delta(&cordis_base::stream_acc::StreamDelta::Usage {
            tokens: cordis_base::usage::TokenUsage {
                prompt_tokens: 80,
                ..cordis_base::usage::TokenUsage::default()
            },
            official: false,
            model: String::new(),
            cost_usd_ticks: None,
        });
        sessions.apply_llm_delta(&cordis_base::stream_acc::StreamDelta::Usage {
            tokens: tokens.clone(),
            official: true,
            model: "grok-4".into(),
            cost_usd_ticks: Some(70),
        });
        sessions.finish_llm(&cordis_base::types::LlmOutput::default());
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
        sessions.apply_llm_delta(&cordis_base::stream_acc::StreamDelta::Usage {
            tokens,
            official: true,
            model: "grok-4".into(),
            cost_usd_ticks: None,
        });
        sessions.finish_llm(&cordis_base::types::LlmOutput::default());
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
        sessions.apply_llm_delta(&cordis_base::stream_acc::StreamDelta::Usage {
            tokens: cordis_base::usage::TokenUsage {
                prompt_tokens: 12,
                ..cordis_base::usage::TokenUsage::default()
            },
            official: false,
            model: String::new(),
            cost_usd_ticks: None,
        });
        sessions.finish_llm(&cordis_base::types::LlmOutput::default());
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
        child.apply_llm_delta(&cordis_base::stream_acc::StreamDelta::Usage {
            tokens: cordis_base::usage::TokenUsage {
                prompt_tokens: 5,
                completion_tokens: 1,
                ..cordis_base::usage::TokenUsage::default()
            },
            official: true,
            model: "child-model".into(),
            cost_usd_ticks: None,
        });
        child.finish_llm(&cordis_base::types::LlmOutput::default());
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
        sessions.append(LogEvent::LlmStream(cordis_base::types::LlmOutput {
            text: "reply".into(),
            ..cordis_base::types::LlmOutput::default()
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
        sessions.apply_llm_delta(&cordis_base::stream_acc::StreamDelta::Text("Hi".into()));
        assert!(sessions.last_turn_has_output());
        assert!(sessions.rewind_inflight_user().is_none());
        assert_eq!(sessions.events().len(), 2);
    }

    #[tokio::test]
    async fn rewind_allows_error_only_llm_stream() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("retry me".into()));
        sessions.append(LogEvent::LlmStream(cordis_base::types::LlmOutput {
            error: Some("llm request failed: boom".into()),
            ..cordis_base::types::LlmOutput::default()
        }));
        assert!(!sessions.last_turn_has_output());
        let restored = sessions
            .rewind_inflight_user()
            .expect("error-only stream does not block undo");
        assert_eq!(restored.0, "retry me");
        assert!(sessions.events().is_empty());
    }

    /// `has_undoable_send` 是底栏每帧问的那个谓词，必须和真正执行的
    /// `rewind_inflight_user` 同进退——否则会出现「写着 Esc:undo，按下去没反应」。
    #[tokio::test]
    async fn has_undoable_send_matches_rewind_outcome() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);

        // 空会话：没得撤。
        assert!(!sessions.has_undoable_send());

        // 只有用户消息：可撤。
        sessions.append(LogEvent::User("retry me".into()));
        assert!(sessions.has_undoable_send());

        // 只有 error 的那一轮不算输出：仍可撤。
        sessions.append(LogEvent::LlmStream(cordis_base::types::LlmOutput {
            error: Some("llm stream failed: boom".into()),
            ..cordis_base::types::LlmOutput::default()
        }));
        assert!(sessions.has_undoable_send());

        // 有正文之后：撤不动，两个判定要一致。
        sessions.append(LogEvent::LlmStream(cordis_base::types::LlmOutput {
            text: "ok".into(),
            ..cordis_base::types::LlmOutput::default()
        }));
        assert!(!sessions.has_undoable_send());
        assert!(sessions.rewind_inflight_user().is_none());

        // 谓词说能撤，执行就得真能撤。
        let sessions2 = Sessions::new(Context::new());
        sessions2.append(LogEvent::User("undo me".into()));
        assert!(sessions2.has_undoable_send());
        assert_eq!(sessions2.rewind_inflight_user().unwrap().0, "undo me");
        assert!(!sessions2.has_undoable_send(), "撤完就不该再说能撤");
    }

    #[tokio::test]
    async fn rewind_blocks_when_assistant_text_is_present() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("keep".into()));
        sessions.append(LogEvent::LlmStream(cordis_base::types::LlmOutput {
            text: "ok".into(),
            ..cordis_base::types::LlmOutput::default()
        }));
        assert!(sessions.last_turn_has_output());
        assert!(sessions.rewind_inflight_user().is_none());
        assert_eq!(sessions.events().len(), 2);
    }

    #[tokio::test]
    async fn seal_inserts_interrupted_results_before_following_user() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("hi".into()));
        sessions.append(LogEvent::LlmStream(cordis_base::types::LlmOutput {
            tool_calls: vec![
                cordis_base::types::ToolCall {
                    id: "c1".into(),
                    name: "bash".into(),
                    arguments: "{}".into(),
                },
                cordis_base::types::ToolCall {
                    id: "c2".into(),
                    name: "read".into(),
                    arguments: "{}".into(),
                },
            ],
            ..cordis_base::types::LlmOutput::default()
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
                assert_eq!(content, cordis_base::types::INTERRUPTED_TOOL_RESULT);
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
            LogEvent::LlmStream(cordis_base::types::LlmOutput {
                text: cordis_base::types::COMPACT_NOTICE.into(),
                ..cordis_base::types::LlmOutput::default()
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
            LogEvent::LlmStream(o) if o.text == cordis_base::types::COMPACT_NOTICE
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

    #[tokio::test]
    async fn notice_stays_on_the_pager_and_out_of_model_history() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("hi".into()));
        sessions.append(LogEvent::Notice {
            kind: cordis_base::types::NoticeKind::WorkflowReport,
            title: "deep-research · planner 上报".into(),
            body: "阶段性结论".into(),
        });
        let pager = sessions.events();
        assert!(
            pager.iter().any(|e| matches!(
                e,
                LogEvent::Notice { title, .. } if title.contains("planner")
            )),
            "{pager:?}"
        );
        let model = sessions.model_history();
        assert!(
            !model.iter().any(|e| matches!(e, LogEvent::Notice { .. })),
            "Notice 进了模型历史就会把主线程叫醒：{model:?}"
        );
        assert!(model
            .iter()
            .any(|e| matches!(e, LogEvent::User(t) if t == "hi")));
    }

    /// 前导只在「什么都还没发」时往前插；发过之后退回追加，已发出的前缀一字节
    /// 不动。时间戳跟着事件一起插，两张表始终对齐。
    #[tokio::test]
    async fn preamble_goes_first_only_until_something_is_sent() {
        let ctx = Context::new();
        let sessions = Sessions::new(ctx);
        sessions.append(LogEvent::User("hi".into()));
        sessions.append(LogEvent::PreStep);
        sessions.insert_preamble(LogEvent::SystemReminder("rules".into()));
        sessions.insert_preamble(LogEvent::SystemReminder("memory".into()));
        assert_eq!(
            sessions.kinds(),
            ["system-reminder", "system-reminder", "user", "pre-step"],
            "还没发过：排在首条用户消息之前，且保持插入顺序"
        );
        assert!(matches!(
            &sessions.events()[..2],
            [LogEvent::SystemReminder(a), LogEvent::SystemReminder(b)]
                if a == "rules" && b == "memory"
        ));
        assert_eq!(sessions.times().len(), sessions.events().len());

        sessions.append(LogEvent::LlmStream(cordis_base::types::LlmOutput {
            text: "ok".into(),
            ..Default::default()
        }));
        sessions.insert_preamble(LogEvent::SystemReminder("rules v2".into()));
        assert!(
            matches!(sessions.events().last(), Some(LogEvent::SystemReminder(t)) if t == "rules v2"),
            "发过之后要追加在尾部：{:?}",
            sessions.kinds()
        );
        assert_eq!(sessions.times().len(), sessions.events().len());
    }

    #[test]
    fn seed_preset_if_unset_only_fills_none() {
        let sessions = Sessions::new(Context::new());
        assert!(sessions.preset_id().is_none());
        sessions.seed_preset_if_unset("code");
        assert_eq!(sessions.preset_id().as_deref(), Some("code"));
        sessions.seed_preset_if_unset("warden");
        assert_eq!(
            sessions.preset_id().as_deref(),
            Some("code"),
            "must not overwrite an existing stamp"
        );
        sessions.set_preset_id(Some("warden".into()));
        assert_eq!(sessions.preset_id().as_deref(), Some("warden"));
    }

    #[tokio::test]
    async fn restore_copies_preset_id_but_absent_keeps_seed() {
        let sessions = Sessions::new(Context::new());
        sessions.set_preset_id(Some("warden".into()));
        sessions.append(LogEvent::User("with preset".into()));
        let with = sessions.archive_current().unwrap();
        assert_eq!(with.preset_id.as_deref(), Some("warden"));

        sessions.clear();
        sessions.set_preset_id(None);
        sessions.append(LogEvent::User("old style".into()));
        // Simulate pre-field archive: clear stamp before snapshotting.
        sessions.set_preset_id(None);
        let mut old = sessions.archive_current().unwrap();
        old.preset_id = None;
        // Replace archived entry with field-absent copy.
        {
            let mut archive = sessions.archive.lock().unwrap();
            if let Some(slot) = archive.iter_mut().find(|s| s.id == old.id) {
                *slot = old.clone();
            }
        }
        assert!(sessions.archived_preset_id(&old.id).is_none());

        sessions.seed_preset_if_unset("code");
        assert!(sessions.restore(&old.id));
        assert_eq!(
            sessions.preset_id().as_deref(),
            Some("code"),
            "absent meta must leave the seed alone"
        );
        assert!(sessions.archived_preset_id(&with.id).as_deref() == Some("warden"));
        assert!(sessions.restore(&with.id));
        assert_eq!(sessions.preset_id().as_deref(), Some("warden"));
    }
}
