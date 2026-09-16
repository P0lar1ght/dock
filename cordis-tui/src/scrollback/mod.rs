//! Scrollback renders `session/event`. Live-lookup `sessions` each frame.
//!
//! Blocks are Grok pager chrome: user `❯` band, markdown assistant,
//! collapsed `◆` tool card (click to expand). Lines are wrapped to the
//! pane width with Grok `word_wrap_lines`.
//!
//! Layout is cached by `(events_rev, width, folds, …)`. Scrolling only
//! changes the viewport — it does not rebuild the transcript.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::{DateTime, Local};
use cordis::Context;
use cordis_spine::{
    AgentPresets, AppSettings, JobSnapshot, Jobs, LogEvent, Sessions, Subagents, Todos,
    AGENT_PRESETS, JOBS, SESSIONS, SETTINGS, SUBAGENTS, TODOS,
};

use crate::names::SESSION_PORT;
use crate::session::SessionRef;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::grok::glyphs;
use crate::grok::mermaid::{self, AffordanceKind};
use crate::theme::Theme;

mod ask;
mod assistant;
mod bg_task;
pub(crate) mod card;
mod edit;
mod execute;
mod goal;
mod list_dir;
pub(crate) mod live;
mod mcp;
mod plan;
mod read;
mod sched;
mod search;
mod search_tool;
mod skill;
mod subagent;
mod task_ops;
mod text_selection;
mod thinking;
pub(crate) mod todo;
pub(crate) mod tool;
mod user;
mod web;

use text_selection::{
    drag_threshold_exceeded, endpoint_at, endpoint_clamped, paint_text_drag, text_for_drag,
    PendingPress, TextDrag,
};

struct Frame {
    lines: Vec<Line<'static>>,
    /// `(line_index, tool_id)` for the collapsed/expanded header row.
    tool_headers: Vec<(usize, String)>,
    /// `(line_index, mermaid source)` for `[Copy Source]` clicks.
    mermaid: Vec<(usize, String)>,
    /// First content line of user/assistant blocks, for `/timestamps`.
    stamps: Vec<(usize, SystemTime)>,
    /// Header rows of cards that are still running.
    live: Vec<LiveRow>,
}

/// A card header that is still running. The frame it belongs to is cached and
/// may be painted for minutes, so nothing about it is baked in: the painter
/// sweeps [`live::shine_at`] across the row and right-aligns a fresh clock on
/// every frame. See [`live`] for why this is not a rebuild.
struct LiveRow {
    line: usize,
    /// Display width of the built row, so the clock lands past the card's own
    /// text instead of eating its tail.
    used: u16,
    /// Wall-clock start, when the card has one to show.
    started: Option<SystemTime>,
    /// Colour the sweep tints toward — the card's own accent, read off its `◆`.
    accent: Option<ratatui::style::Color>,
}

pub enum ClickHit {
    None,
    ToolToggle,
    OpenSubagent {
        id: String,
    },
    OpenJob {
        id: String,
    },
    Mermaid {
        kind: AffordanceKind,
        source: String,
    },
}

/// Result of releasing the left mouse button over scrollback.
pub enum MouseUpResult {
    /// No gesture / outside pane.
    None,
    /// Click without drag (tool toggle, mermaid affordance, or empty).
    Click(ClickHit),
    /// Finished a non-empty drag; caller should copy `text`.
    Copied(String),
    /// State changed (cleared selection, empty drag); redraw only.
    Redraw,
}

struct SelectionState {
    pending: Option<PendingPress>,
    /// Active drag while the button is held, or persistent highlight after copy.
    drag: Option<TextDrag>,
    /// True only while the left button is held after promoting a drag.
    dragging: bool,
}

pub struct Scrollback {
    ctx: Context,
    /// 0 = follow the latest line (Grok default). Positive = lines up from bottom.
    offset: Mutex<u16>,
    /// Thinking blocks: presence = fully expanded.
    expanded: Mutex<HashSet<String>>,
    /// Tool cards: Collapsed (absent) → Truncated → Expanded (grok fold cycle).
    tool_fold: Mutex<HashMap<String, tool::ToolMode>>,
    /// Todo card fold. Its own cell (not `tool_fold`) because the default is
    /// Active, not Collapsed, and the cycle is card-specific.
    todo_fold: Mutex<todo::TodoFold>,
    last_frame: Mutex<Frame>,
    /// When [`Self::layout_key`] matches, [`Self::last_frame`] is reused.
    layout_cache_key: Mutex<Option<u64>>,
    last_area: Mutex<Rect>,
    mouse: Mutex<Option<(u16, u16)>>,
    selection: Mutex<SelectionState>,
    /// `/find` 结果按 query 记一层 memo：paint_overlay 每帧都查一遍，
    /// 不 memo 的话每帧都要扫整份 transcript 的行文本。带上 layout key，
    /// transcript 一变 memo 即失效。
    find_memo: Mutex<FindMemo>,
}

impl Scrollback {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            offset: Mutex::new(0),
            expanded: Mutex::new(HashSet::new()),
            tool_fold: Mutex::new(HashMap::new()),
            todo_fold: Mutex::new(todo::TodoFold::default()),
            last_frame: Mutex::new(Frame {
                lines: Vec::new(),
                tool_headers: Vec::new(),
                mermaid: Vec::new(),
                stamps: Vec::new(),
                live: Vec::new(),
            }),
            layout_cache_key: Mutex::new(None),
            last_area: Mutex::new(Rect::default()),
            mouse: Mutex::new(None),
            find_memo: Mutex::new(None),
            selection: Mutex::new(SelectionState {
                pending: None,
                drag: None,
                dragging: false,
            }),
        }
    }

    pub fn scroll(&self, delta: i16) {
        let mut sel = self.selection.lock().unwrap();
        if sel.dragging {
            // Edge-autoscroll while dragging: keep the gesture, move content.
            let mouse = *self.mouse.lock().unwrap();
            let mut offset = self.offset.lock().unwrap();
            *offset = (*offset as i32 + delta as i32).clamp(0, u16::MAX as i32) as u16;
            drop(offset);
            if let Some(drag) = sel.drag.as_mut() {
                let (plain, area, scroll) = self.plain_view();
                let (col, row) = mouse.unwrap_or((area.x, area.y));
                if let Some(ep) = endpoint_clamped(&plain, area, scroll, col, row) {
                    drag.head = ep;
                }
            }
            return;
        }
        // Content moved under a still pointer — drop unfinished / persistent selection.
        sel.pending = None;
        sel.drag = None;
        drop(sel);
        let mut offset = self.offset.lock().unwrap();
        *offset = (*offset as i32 + delta as i32).clamp(0, u16::MAX as i32) as u16;
    }

    pub fn scroll_page(&self, pages: i16) {
        let height = self.last_area.lock().unwrap().height.max(1);
        let half = height as i16 / 2;
        // 窗口太矮时退化为整页 → 一行；正常时按半屏翻。保号，不许把
        // PageDown 的 -half 夹成 -1（旧实现的 `.max(pages)` 正是这么干的）。
        let delta = if half == 0 {
            pages
        } else {
            pages.saturating_mul(half)
        };
        self.scroll(delta);
    }

    pub fn set_mouse(&self, column: u16, row: u16) {
        *self.mouse.lock().unwrap() = Some((column, row));
    }

    /// Clear pending/persistent selection (Esc, overlay open, etc.).
    pub fn clear_selection(&self) {
        let mut sel = self.selection.lock().unwrap();
        sel.pending = None;
        sel.drag = None;
        sel.dragging = false;
    }

    pub fn has_selection(&self) -> bool {
        let sel = self.selection.lock().unwrap();
        sel.pending.is_some() || sel.drag.is_some()
    }

    /// Left button down: arm a pending press; promote to drag after 1-cell move.
    pub fn mouse_down(&self, column: u16, row: u16) -> bool {
        self.set_mouse(column, row);
        let (plain, area, scroll) = self.plain_view();
        let mut sel = self.selection.lock().unwrap();
        sel.drag = None;
        sel.dragging = false;
        let Some(ep) = endpoint_at(&plain, area, scroll, column, row) else {
            sel.pending = None;
            return false;
        };
        sel.pending = Some(PendingPress {
            start_col: column,
            start_row: row,
            endpoint: ep,
        });
        true
    }

    /// Left drag: promote pending → active drag past the 1-cell threshold.
    pub fn mouse_drag(&self, column: u16, row: u16) -> bool {
        self.set_mouse(column, row);
        let area = *self.last_area.lock().unwrap();
        let mut sel = self.selection.lock().unwrap();

        if let Some(pending) = sel.pending {
            if !drag_threshold_exceeded(&pending, column, row) {
                return false;
            }
            sel.pending = None;
            sel.drag = Some(TextDrag {
                anchor: pending.endpoint,
                head: pending.endpoint,
            });
            sel.dragging = true;
        }
        if !sel.dragging || sel.drag.is_none() {
            return false;
        }

        // Near-edge autoscroll (same idea as usage_modal).
        if area.height > 0 {
            if row < area.y {
                drop(sel);
                self.scroll(1);
                sel = self.selection.lock().unwrap();
            } else if row >= area.y.saturating_add(area.height) {
                drop(sel);
                self.scroll(-1);
                sel = self.selection.lock().unwrap();
            }
        }

        let (plain, area, scroll) = self.plain_view();
        if let Some(ep) = endpoint_clamped(&plain, area, scroll, column, row) {
            if let Some(d) = sel.drag.as_mut() {
                d.head = ep;
            }
        }
        true
    }

    /// Left button up: copy a finished drag, or fire a click if never promoted.
    pub fn mouse_up(&self, column: u16, row: u16) -> MouseUpResult {
        self.set_mouse(column, row);
        let mut sel = self.selection.lock().unwrap();

        if let Some(_pending) = sel.pending.take() {
            sel.dragging = false;
            drop(sel);
            return MouseUpResult::Click(self.click_at(column, row));
        }

        let Some(mut final_drag) = sel.drag.take() else {
            sel.dragging = false;
            return MouseUpResult::None;
        };
        let was_dragging = sel.dragging;
        sel.dragging = false;

        if !was_dragging {
            // Persistent highlight click-elsewhere already cleared on mouse_down.
            // Restore so a stray Up doesn't wipe the band.
            sel.drag = Some(final_drag);
            return MouseUpResult::None;
        }

        let (plain, area, scroll) = self.plain_view();
        if let Some(ep) = endpoint_clamped(&plain, area, scroll, column, row) {
            final_drag.head = ep;
        }

        if final_drag.is_non_empty() {
            if let Some(text) = text_for_drag(final_drag, &plain, area.width) {
                // Keep highlight until the next press (grok hold-style feedback).
                sel.drag = Some(final_drag);
                return MouseUpResult::Copied(text);
            }
        }
        MouseUpResult::Redraw
    }

    /// Bare `Moved` with an active drag: finish as a lost Up (copy if non-empty).
    pub fn finish_lost_drag(&self) -> Option<String> {
        let mut sel = self.selection.lock().unwrap();
        if !sel.dragging {
            return None;
        }
        sel.dragging = false;
        let drag = sel.drag.take()?;
        if !drag.is_non_empty() {
            return None;
        }
        let (plain, area, _scroll) = self.plain_view();
        let text = text_for_drag(drag, &plain, area.width);
        if text.is_some() {
            sel.drag = Some(drag);
        }
        text
    }

    fn plain_view(&self) -> (Vec<String>, Rect, u16) {
        let area = *self.last_area.lock().unwrap();
        let frame = self.last_frame.lock().unwrap();
        let offset = *self.offset.lock().unwrap();
        let extra = frame.lines.len().saturating_sub(area.height as usize) as u16;
        let scroll = extra.saturating_sub(offset);
        let plain = frame
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        (plain, area, scroll)
    }

    fn click_at(&self, column: u16, row: u16) -> ClickHit {
        let area = *self.last_area.lock().unwrap();
        if !area.contains(ratatui::layout::Position { x: column, y: row }) {
            return ClickHit::None;
        }
        let frame = self.last_frame.lock().unwrap();
        let offset = *self.offset.lock().unwrap();
        let extra = frame.lines.len().saturating_sub(area.height as usize) as u16;
        let scroll = extra.saturating_sub(offset);
        let line_idx = (row.saturating_sub(area.y) as usize).saturating_add(scroll as usize);
        if let Some((_, src)) = frame.mermaid.iter().find(|(i, _)| *i == line_idx) {
            return match mermaid::hit_kind(column.saturating_sub(area.x), area.width) {
                Some(kind) => ClickHit::Mermaid {
                    kind,
                    source: src.clone(),
                },
                None => ClickHit::None,
            };
        }
        let Some((_, id)) = frame
            .tool_headers
            .iter()
            .find(|(i, _)| *i == line_idx)
            .cloned()
        else {
            return ClickHit::None;
        };
        drop(frame);
        if let Some(id) = subagent::open_id(&id) {
            return ClickHit::OpenSubagent { id: id.to_string() };
        }
        if let Some(id) = bg_task::open_id(&id) {
            return ClickHit::OpenJob { id: id.to_string() };
        }
        if id == todo::HEADER_ID {
            self.cycle_todo_fold();
        } else if id.starts_with("think:") {
            let mut expanded = self.expanded.lock().unwrap();
            if !expanded.remove(&id) {
                expanded.insert(id);
            }
        } else {
            let mut folds = self.tool_fold.lock().unwrap();
            let current = folds.get(&id).copied().unwrap_or(tool::ToolMode::Collapsed);
            match current.next() {
                Some(next) => {
                    folds.insert(id, next);
                }
                None => {
                    folds.remove(&id);
                }
            }
        }
        ClickHit::ToolToggle
    }

    /// Advance the todo card fold (header click or Ctrl+t). No-op visually
    /// when there is no list — [`todo::lines`] renders nothing.
    pub fn cycle_todo_fold(&self) {
        let mut fold = self.todo_fold.lock().unwrap();
        *fold = fold.next();
    }

    pub fn todo_fold(&self) -> todo::TodoFold {
        *self.todo_fold.lock().unwrap()
    }

    /// Whether a todo list is mounted and non-empty (drives the Ctrl+t hint).
    pub fn has_todos(&self) -> bool {
        self.ctx
            .get::<Todos>(TODOS)
            .is_some_and(|t| t.stats().total() > 0)
    }

    pub fn last_assistant(&self, n: usize) -> Option<String> {
        let n = n.max(1);
        let Ok(sessions) = self.ctx.require::<Sessions>(SESSIONS) else {
            return None;
        };
        sessions
            .events()
            .iter()
            .rev()
            .filter_map(|e| match e {
                LogEvent::LlmStream(llm) if !llm.text.trim().is_empty() => Some(llm.text.clone()),
                _ => None,
            })
            .nth(n - 1)
    }

    pub fn find(&self, query: &str) -> Vec<(usize, String)> {
        let q = query.trim();
        if q.is_empty() {
            return Vec::new();
        }
        // 走 ensure_frame + last_frame：paint_overlay 每帧都调 find，绕过
        // 缓存等于在后台任务跑着时 12.5Hz 重跑整份 markdown + syntect。
        // 宽度用真实宽度 —— 旧实现 .max(40) 建出的帧行数和 last_frame
        // 对不上，jump_to_line 是按 last_frame 算 offset 的，窄窗下跳过去
        // 是错的行。
        let width = self.last_area.lock().unwrap().width as usize;
        if width == 0 {
            return Vec::new();
        }
        self.ensure_frame(width);
        let layout_key = *self.layout_cache_key.lock().unwrap();
        if let Some((key, cached_q, hits)) = self.find_memo.lock().unwrap().as_ref() {
            if Some(*key) == layout_key && cached_q == q {
                return hits.clone();
            }
        }
        let needle = q.to_ascii_lowercase();
        let frame = self.last_frame.lock().unwrap();
        let hits: Vec<(usize, String)> = frame
            .lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                text.to_ascii_lowercase()
                    .contains(&needle)
                    .then_some((i, text.trim().to_string()))
            })
            .collect();
        drop(frame);
        if let Some(key) = layout_key {
            *self.find_memo.lock().unwrap() = Some((key, q.to_string(), hits.clone()));
        }
        hits
    }
    pub fn jump_to_line(&self, line_idx: usize) {
        let area = *self.last_area.lock().unwrap();
        let frame = self.last_frame.lock().unwrap();
        let extra = frame.lines.len().saturating_sub(area.height as usize) as u16;
        let scroll = (line_idx as u16).min(extra);
        *self.offset.lock().unwrap() = extra.saturating_sub(scroll);
    }

    fn build(&self, width: usize) -> Frame {
        let Ok(sessions) = self.ctx.require::<Sessions>(SESSIONS) else {
            return Frame {
                lines: Vec::new(),
                tool_headers: Vec::new(),
                mermaid: Vec::new(),
                stamps: Vec::new(),
                live: Vec::new(),
            };
        };
        let expanded = self.expanded.lock().unwrap().clone();
        let tool_fold = self.tool_fold.lock().unwrap().clone();
        let show_ts = self
            .ctx
            .get::<AppSettings>(SETTINGS)
            .is_some_and(|s| s.timestamps());
        let working = self
            .ctx
            .get::<SessionRef>(SESSION_PORT)
            .is_some_and(|s| s.working());
        let todos = self
            .ctx
            .get::<Todos>(TODOS)
            .map(|t| t.snapshot())
            .unwrap_or_default();
        let todo_fold = self.todo_fold();
        let subagents = self.ctx.get::<Subagents>(SUBAGENTS);
        let jobs = self.ctx.get::<Jobs>(JOBS);
        let presets = self.ctx.get::<AgentPresets>(AGENT_PRESETS);
        sessions.with_log(|events, times| {
            build_frame(
                events,
                times,
                width,
                &expanded,
                &tool_fold,
                working,
                show_ts,
                &todos,
                todo_fold,
                subagents.as_deref(),
                jobs.as_deref(),
                presets.as_deref(),
            )
        })
    }

    /// Fingerprint of everything that affects transcript layout (not scroll offset).
    fn layout_key(&self, width: usize) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        width.hash(&mut h);
        // Every span carries baked colours — accents, the user band, syntect
        // fallbacks. Without this a palette switch keeps the old frame and
        // paints GrokNight's #e1e1e1 text onto GrokDay's #eeeeee background.
        Theme::current_kind().hash(&mut h);
        if let Ok(sessions) = self.ctx.require::<Sessions>(SESSIONS) {
            sessions.events_rev().hash(&mut h);
        } else {
            0u64.hash(&mut h);
        }
        {
            let mut ids: Vec<_> = self.expanded.lock().unwrap().iter().cloned().collect();
            ids.sort();
            ids.hash(&mut h);
        }
        {
            let folds = self.tool_fold.lock().unwrap();
            let mut pairs: Vec<_> = folds
                .iter()
                .map(|(k, v)| (k.clone(), std::mem::discriminant(v)))
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            pairs.hash(&mut h);
        }
        self.ctx
            .get::<SessionRef>(SESSION_PORT)
            .is_some_and(|s| s.working())
            .hash(&mut h);
        self.ctx
            .get::<AppSettings>(SETTINGS)
            .is_some_and(|s| s.timestamps())
            .hash(&mut h);
        if let Some(todos) = self.ctx.get::<Todos>(TODOS) {
            for (id, item) in todos.snapshot() {
                id.hash(&mut h);
                // Content too: `todo_write` can re-word an item without
                // touching its id or status, and the card shows the wording.
                item.content.hash(&mut h);
                std::mem::discriminant(&item.status).hash(&mut h);
            }
        }
        std::mem::discriminant(&self.todo_fold()).hash(&mut h);
        if let Some(subagents) = self.ctx.get::<Subagents>(SUBAGENTS) {
            for a in subagents.list() {
                a.id.hash(&mut h);
                a.running().hash(&mut h);
                a.idle.hash(&mut h);
                a.done.hash(&mut h);
            }
        }
        if let Some(jobs) = self.ctx.get::<Jobs>(JOBS) {
            for j in jobs.list() {
                j.id.hash(&mut h);
                j.done.hash(&mut h);
                // 输出是流式增长的：不把长度算进指纹，布局缓存就永远命中，
                // 跑着的命令在 transcript 上看起来是一动不动的。
                j.output.len().hash(&mut h);
            }
        }
        h.finish()
    }

    /// Ensure `last_frame` matches `layout_key(width)`; rebuild only on miss.
    fn ensure_frame(&self, width: usize) {
        let key = self.layout_key(width);
        if *self.layout_cache_key.lock().unwrap() == Some(key) {
            return;
        }
        let frame = self.build(width);
        *self.last_frame.lock().unwrap() = frame;
        *self.layout_cache_key.lock().unwrap() = Some(key);
    }
}

#[cfg(test)]
pub fn inspect_lines(events: &[LogEvent], width: usize, running: bool) -> Vec<Line<'static>> {
    child_transcript(
        events,
        width,
        &HashSet::new(),
        &HashMap::new(),
        running,
        false,
        None,
        None,
    )
    .lines
}

/// Child inspect: skip the spawn User prompt so the overlay starts at work + replies.
#[cfg(test)]
pub fn inspect_child_lines(events: &[LogEvent], width: usize, running: bool) -> Vec<Line<'static>> {
    child_transcript(
        events,
        width,
        &HashSet::new(),
        &HashMap::new(),
        running,
        true,
        None,
        None,
    )
    .lines
}

pub(crate) struct ChildTranscript {
    pub lines: Vec<Line<'static>>,
    pub tool_headers: Vec<(usize, String)>,
}

/// Same cards as the main scrollback (`build_frame`), with caller-owned folds.
/// `skip_first_user` drops only the spawn prompt so later steering User rows stay.
#[allow(clippy::too_many_arguments)]
// TUI 绘制/布局函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到所有调用点，故意保留。
pub(crate) fn child_transcript(
    events: &[LogEvent],
    width: usize,
    expanded: &HashSet<String>,
    tool_fold: &HashMap<String, tool::ToolMode>,
    running: bool,
    skip_first_user: bool,
    presets: Option<&AgentPresets>,
    subagents: Option<&Subagents>,
) -> ChildTranscript {
    let filtered = if skip_first_user {
        skip_first_user_event(events)
    } else {
        events.to_vec()
    };
    let frame = build_frame(
        &filtered,
        &[],
        width,
        expanded,
        tool_fold,
        running,
        false,
        &[],
        todo::TodoFold::default(),
        subagents,
        None,
        presets,
    );
    ChildTranscript {
        lines: frame.lines,
        tool_headers: frame.tool_headers,
    }
}

fn skip_first_user_event(events: &[LogEvent]) -> Vec<LogEvent> {
    let mut seen_user = false;
    events
        .iter()
        .filter(|e| {
            if matches!(e, LogEvent::User(_)) && !seen_user {
                seen_user = true;
                return false;
            }
            true
        })
        .cloned()
        .collect()
}

#[cfg(test)]
pub fn markdown_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    assistant::render(text, &Theme::current(), width).lines
}

pub(crate) fn subagent_live_activity(events: &[LogEvent]) -> Option<String> {
    subagent::live_activity(events)
}

#[cfg(test)]
pub(crate) fn lines_from_events(events: &[LogEvent]) -> Vec<Line<'static>> {
    build_frame(
        events,
        &[],
        80,
        &HashSet::new(),
        &HashMap::new(),
        false,
        false,
        &[],
        todo::TodoFold::default(),
        None,
        None,
        None,
    )
    .lines
}

/// Grok `timestamp_reserved`: `"  12:30 PM"` is 10 columns.
const TIMESTAMP_RESERVED: usize = 10;

/// `(layout_key, query, hits)` — `/find` 的 memo 载荷。
type FindMemo = Option<(u64, String, Vec<(usize, String)>)>;

fn message_wrap(width: usize, show_ts: bool) -> usize {
    if show_ts {
        width.saturating_sub(TIMESTAMP_RESERVED).max(20)
    } else {
        width
    }
}

#[allow(clippy::too_many_arguments)]
// TUI 绘制/布局函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到所有调用点，故意保留。
fn build_frame(
    events: &[LogEvent],
    times: &[SystemTime],
    width: usize,
    expanded: &HashSet<String>,
    tool_fold: &HashMap<String, tool::ToolMode>,
    working: bool,
    show_ts: bool,
    todos: &[(String, cordis_spine::TodoItem)],
    todo_fold: todo::TodoFold,
    subagents: Option<&Subagents>,
    jobs: Option<&Jobs>,
    presets: Option<&AgentPresets>,
) -> Frame {
    let theme = Theme::current();
    let agents = subagents.map(|s| s.list()).unwrap_or_default();
    let job_snaps = jobs.map(|j| j.list()).unwrap_or_default();
    let mut lines = Vec::new();
    let mut tool_headers = Vec::new();
    let mut mermaid = Vec::new();
    let mut stamps = Vec::new();
    let mut live = Vec::new();
    // 已经被前面某个合并卡吃掉的事件下标（连续 bash 合成一张卡）。
    let mut merged_into_group: HashSet<usize> = HashSet::new();
    for (i, event) in events.iter().enumerate() {
        let stamp = times.get(i).copied();
        if merged_into_group.contains(&i) {
            continue;
        }
        match event {
            LogEvent::User(text) => {
                if let Some(at) = stamp {
                    stamps.push((lines.len(), at));
                }
                lines.extend(user::lines(
                    text,
                    &theme,
                    message_wrap(width, show_ts),
                    width,
                ));
                lines.push(Line::from(""));
            }
            LogEvent::LlmStream(llm) => {
                let think_id = format!("think:{i}");
                let think_open = expanded.contains(&think_id);
                let streaming_think = working && i + 1 == events.len() && llm.tool_calls.is_empty();
                let think = thinking::lines(
                    &llm.reasoning,
                    llm.reasoning_ms,
                    streaming_think,
                    &theme,
                    width,
                    think_open,
                );
                if !think.is_empty() {
                    if streaming_think {
                        // Header only. The clock stays off: the block already
                        // prints `思考了 1.2s` once the model stops.
                        live.push(LiveRow {
                            line: lines.len(),
                            used: line_width(&think[0]),
                            started: None,
                            accent: think[0].spans.first().and_then(|s| s.style.fg),
                        });
                    }
                    tool_headers.push((lines.len(), think_id));
                    lines.extend(think);
                    lines.push(Line::from(""));
                }
                let rendered = assistant::render(&llm.text, &theme, message_wrap(width, show_ts));
                if !rendered.lines.is_empty() {
                    if let Some(at) = stamp {
                        stamps.push((lines.len(), at));
                    }
                    let base = lines.len();
                    mermaid.extend(rendered.mermaid.into_iter().map(|(i, src)| (base + i, src)));
                    lines.extend(rendered.lines);
                    lines.push(Line::from(""));
                }
                // 采样失败详情。只画给人看——它不在 `llm.text` 里，所以不会被
                // 当成助手消息回放给模型。
                if let Some(err) = llm.error.as_deref() {
                    if let Some(at) = stamp {
                        stamps.push((lines.len(), at));
                    }
                    lines.extend(llm_error_lines(err, &theme, message_wrap(width, show_ts)));
                    lines.push(Line::from(""));
                }
                for call in &llm.tool_calls {
                    let done = events[i + 1..].iter().any(|e| match e {
                        LogEvent::ToolExecute { id, .. } => id == &call.id,
                        _ => false,
                    });
                    if done {
                        continue;
                    }
                    push_tool_card(
                        &mut lines,
                        &mut tool_headers,
                        &mut live,
                        stamp,
                        &call.id,
                        &call.name,
                        &call.arguments,
                        "",
                        true,
                        &theme,
                        width,
                        tool_fold,
                        &agents,
                        subagents,
                        jobs,
                        &job_snaps,
                        presets,
                    );
                }
            }
            LogEvent::ToolExecute {
                id,
                name,
                arguments,
                content,
                images: _,
            } => {
                // 相邻的、都已完成的前台 bash 合成一张卡。后台 bash 归 bg_task
                // 卡（自带任务 id 与实时输出），不参与合并。
                let group = mergeable_shell_run(events, i, &job_snaps);
                if group.len() > 1 {
                    // 组键与组内第一条调用的 id **必须分开**：共用一个键时点
                    // 第一条命令就会把整组折掉，组内单独折叠永远轮不到它。
                    let group_key = format!("{BASH_GROUP_PREFIX}{id}");
                    let group_mode = tool_fold
                        .get(&group_key)
                        .copied()
                        .unwrap_or(tool::ToolMode::Collapsed);
                    let calls: Vec<execute::Call<'_>> = group
                        .iter()
                        .filter_map(|&idx| match &events[idx] {
                            LogEvent::ToolExecute {
                                id,
                                arguments,
                                content,
                                ..
                            } => Some(execute::Call {
                                id,
                                arguments,
                                content,
                                // 缺省继承组的折叠态，用户单独点过那条才有自己的值。
                                mode: tool_fold.get(id).copied().unwrap_or(group_mode),
                            }),
                            _ => None,
                        })
                        .collect();
                    if let Some(at) = stamp {
                        stamps.push((lines.len(), at));
                    }
                    let header_at = lines.len();
                    let card = execute::group_lines(&calls, &group_key, &theme, width, group_mode);
                    for (row, key) in card.row_ids {
                        tool_headers.push((header_at + row, key));
                    }
                    lines.extend(card.lines);
                    if lines.last().is_some_and(|l| !l.spans.is_empty()) {
                        lines.push(Line::from(""));
                    }
                    merged_into_group.extend(group.iter().skip(1).copied());
                    continue;
                }
                push_tool_card(
                    &mut lines,
                    &mut tool_headers,
                    &mut live,
                    stamp,
                    id,
                    name,
                    arguments,
                    content,
                    false,
                    &theme,
                    width,
                    tool_fold,
                    &agents,
                    subagents,
                    jobs,
                    &job_snaps,
                    presets,
                );
            }
            LogEvent::PreStep | LogEvent::Prompt(_) | LogEvent::SystemReminder(_) => {}
        }
    }
    let todo_card = todo::lines(todos, todo_fold, &theme, width);
    if !todo_card.is_empty() {
        tool_headers.push((lines.len(), todo::HEADER_ID.to_string()));
        lines.extend(todo_card);
        lines.push(Line::from(""));
    }
    Frame {
        lines,
        tool_headers,
        mermaid,
        stamps,
        live,
    }
}

/// 采样失败的行：错误色 `⚠` 头行 + 逐行详情。
///
/// 这段**只进滚动区**。错误存在 `LlmOutput::error` 而不是 `text` 里，三条 wire
/// builder 都以 `text` 非空或有 tool_call 为门槛，所以它不会被回放给模型——
/// 否则「LLM 请求失败…」会变成模型自己说过的一句话，还要为它反复付 token。
fn llm_error_lines(err: &str, theme: &Theme, wrap: usize) -> Vec<Line<'static>> {
    let style = Style::default().fg(theme.accent_error);
    let mut out = vec![Line::from(Span::styled(
        format!("{} 请求失败", glyphs::diamond_filled()),
        style.add_modifier(Modifier::BOLD),
    ))];
    for raw in err.lines() {
        for chunk in wrap_plain(raw, wrap.max(20)) {
            out.push(card::indent(Line::from(Span::styled(chunk, style))));
        }
    }
    out
}

/// 按显示宽度硬折行——错误详情里是 URL 和 TLS 串，没有可断的空格。
fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            used = 0;
        }
        cur.push(ch);
        used += w;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 合并卡的组折叠键前缀。带前缀是为了和组内任何一条调用的 id 都不撞——
/// 撞了就等于组和第一条命令共用一个折叠态。
const BASH_GROUP_PREFIX: &str = "bash-group:";

/// 从 `start` 起，连续的、可合并的前台 bash 事件下标。
///
/// 返回至少包含 `start` 自己；长度为 1 时调用方按老路走单卡，渲染逐字不变。
///
/// 「可合并」要同时满足：是 execute 族工具、事件是已完成的 `ToolExecute`、且
/// **不是** `bg_task` 卡（后台 bash 有自己的任务 id、实时输出与时钟，合进来会
/// 丢掉这些）。中间夹任何别的事件（包括模型的思考与文本）都断开——合并的前提
/// 是它们在视觉上本来就连成一片。
fn mergeable_shell_run(events: &[LogEvent], start: usize, job_snaps: &[JobSnapshot]) -> Vec<usize> {
    let mergeable = |idx: usize| -> bool {
        match &events[idx] {
            LogEvent::ToolExecute {
                name,
                arguments,
                content,
                ..
            } => {
                execute::is_execute_tool(name)
                    && bg_task::header_id(name, content, arguments, job_snaps).is_none()
            }
            _ => false,
        }
    };
    if !mergeable(start) {
        return vec![start];
    }
    let mut run = vec![start];
    let mut next = start + 1;
    while next < events.len() && mergeable(next) {
        run.push(next);
        next += 1;
    }
    run
}

/// `use_tool` 包装（deferred 工具都走它）：返回内层 `(tool_name, tool_input)`。
/// 纯解包，不认识任何工具名——调用方（task 族 / `skill`）各自判定是不是自己
/// 的卡片。
pub(crate) fn unwrap_use_tool(name: &str, arguments: &str) -> Option<(String, String)> {
    if name != "use_tool" {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(arguments).ok()?;
    let inner = v.get("tool_name")?.as_str()?;
    let input = v
        .get("tool_input")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Some((
        inner.to_string(),
        serde_json::to_string(&input).unwrap_or_else(|_| "{}".into()),
    ))
}

#[allow(clippy::too_many_arguments)]
// TUI 绘制/布局函数：参数都是 buf/坐标/主题等绘制碎片，抽结构体只会把噪音搬到所有调用点，故意保留。
fn push_tool_card(
    lines: &mut Vec<Line<'static>>,
    tool_headers: &mut Vec<(usize, String)>,
    live_rows: &mut Vec<LiveRow>,
    // `started`: when this card is a still-running tool call, when the model
    // asked for it — the painter turns it into a clock.
    started: Option<SystemTime>,
    id: &str,
    name: &str,
    arguments: &str,
    content: &str,
    running: bool,
    theme: &Theme,
    width: usize,
    tool_fold: &HashMap<String, tool::ToolMode>,
    agents: &[cordis_spine::SubagentSnap],
    subagents: Option<&Subagents>,
    jobs: Option<&Jobs>,
    job_snaps: &[JobSnapshot],
    presets: Option<&AgentPresets>,
) {
    // `use_tool` 是 deferred 工具的包装：内层是 task 族/skill 操作时按内层
    // 操作渲染卡片（参数取 `tool_input`）。
    let unwrapped = unwrap_use_tool(name, arguments)
        .filter(|(inner, _)| task_ops::is_task_op(inner) || skill::is_skill(inner));
    let (name, arguments) = match &unwrapped {
        Some((inner_name, inner_args)) => (inner_name.as_str(), inner_args.as_str()),
        None => (name, arguments),
    };
    let header_at = lines.len();
    let hid = bg_task::header_id(name, content, arguments, job_snaps)
        .unwrap_or_else(|| subagent::header_id(id, name, content, arguments, agents));
    if subagent::opens_child_overlay(name) {
        let sid = subagent::open_id(&hid);
        let snap = sid.and_then(|id| subagents.and_then(|s| s.snapshot(id)));
        let events = sid
            .map(|id| subagents.map(|s| s.events(id)).unwrap_or_default())
            .unwrap_or_default();
        let live = Some(subagent::CardLive {
            snap: snap.as_ref(),
            events: &events,
            role: {
                let typ = snap
                    .as_ref()
                    .map(|s| s.subagent_type.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        serde_json::from_str::<serde_json::Value>(arguments)
                            .ok()
                            .and_then(|v| {
                                v.get("subagent_type")
                                    .and_then(|x| x.as_str())
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty())
                            })
                    });
                typ.and_then(|t| presets.and_then(|p| p.role_label(&t)))
            },
        });
        let still_running = subagent::is_running(content, snap.as_ref());
        lines.extend(subagent::lines(arguments, content, live, theme, width));
        if still_running {
            mark_live(
                live_rows,
                lines,
                header_at,
                snap.as_ref().and_then(|s| live::wall_start(s.started_at)),
            );
        }
        for i in header_at..lines.len() {
            tool_headers.push((i, hid.clone()));
        }
    } else if let Some(jid) = bg_task::open_id(&hid) {
        let snap = jobs.and_then(|j| j.snapshot(jid));
        let still_running = bg_task::is_running(content, snap.as_ref());
        lines.extend(bg_task::lines(
            name,
            arguments,
            content,
            snap.as_ref(),
            theme,
            width,
        ));
        if still_running {
            mark_live(
                live_rows,
                lines,
                header_at,
                snap.as_ref().map(|s| s.start_time),
            );
        }
        for i in header_at..lines.len() {
            tool_headers.push((i, hid.clone()));
        }
    } else if skill::is_skill(name) {
        lines.extend(skill::lines(arguments, content, theme, width));
    } else if task_ops::is_task_op(name) {
        let op_hid = task_ops::header_id(name, arguments, agents, job_snaps);
        lines.extend(task_ops::lines(
            name, arguments, content, agents, job_snaps, theme, width,
        ));
        // `wait_tasks` / `get_task_output` can hang for minutes. The clock is
        // painted (`started` = when the model asked), never baked.
        if running {
            mark_live(live_rows, lines, header_at, started);
        }
        if let Some(h) = op_hid {
            for i in header_at..lines.len() {
                tool_headers.push((i, h.clone()));
            }
        }
    } else {
        let mode = tool_fold
            .get(id)
            .copied()
            .unwrap_or(tool::ToolMode::Collapsed);
        lines.extend(tool_card_lines(
            name, arguments, content, theme, width, mode, running,
        ));
        if running {
            mark_live(live_rows, lines, header_at, started);
        }
        // 整张卡都可点，不只是折叠头那一行：展开后点正文没反应是原先最让人
        // 困惑的一处 —— 子代理 / 后台任务卡本来就整卡可点，两套手感。
        for i in header_at..lines.len() {
            tool_headers.push((i, hid.clone()));
        }
    }
    // 卡片之间的空行由外壳统一负责，各卡片不自己推 —— 原先两边都推就成了
    // 两行空白。
    if lines.last().is_some_and(|l| !l.spans.is_empty()) {
        lines.push(Line::from(""));
    }
}

/// Register the card header at `header_at` as still running. No-op when the
/// card rendered nothing (a card that produced no header has nothing to sweep).
fn mark_live(
    live_rows: &mut Vec<LiveRow>,
    lines: &[Line<'static>],
    header_at: usize,
    started: Option<SystemTime>,
) {
    let Some(header) = lines.get(header_at) else {
        return;
    };
    live_rows.push(LiveRow {
        line: header_at,
        used: line_width(header),
        started,
        // The bullet already carries the card's state colour; no second source
        // of truth to drift.
        accent: header.spans.first().and_then(|s| s.style.fg),
    });
}

fn line_width(line: &Line<'_>) -> u16 {
    line.spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()) as u16)
        .sum()
}

impl Widget for &Scrollback {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let theme = Theme::current();
        buf.set_style(area, Style::default().bg(theme.bg_base));
        self.ensure_frame(area.width as usize);
        let frame = self.last_frame.lock().unwrap();
        let offset = *self.offset.lock().unwrap();
        let extra = frame.lines.len().saturating_sub(area.height as usize) as u16;
        let scroll = extra.saturating_sub(offset);
        paint_visible_lines(buf, area, &frame.lines, scroll);
        let show_ts = self
            .ctx
            .get::<AppSettings>(SETTINGS)
            .is_some_and(|s| s.timestamps());
        paint_live_chrome(
            buf,
            area,
            &frame,
            scroll,
            show_ts,
            &theme,
            crate::grok::color::shimmer_tick(),
        );
        if show_ts {
            let mouse = *self.mouse.lock().unwrap();
            paint_timestamps(buf, area, &frame, scroll, mouse, &theme);
        }
        let sel = self.selection.lock().unwrap();
        if let Some(drag) = sel.drag.filter(|d| d.is_non_empty()) {
            let plain: Vec<String> = frame
                .lines
                .iter()
                .map(|line| {
                    line.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                })
                .collect();
            paint_text_drag(drag, &plain, area, scroll, buf, &theme);
        }
        drop(sel);
        drop(frame);
        *self.last_area.lock().unwrap() = area;
    }
}

/// Paint only the viewport rows — avoids cloning the full transcript into Paragraph.
fn paint_visible_lines(buf: &mut Buffer, area: Rect, lines: &[Line<'static>], scroll: u16) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let start = scroll as usize;
    for row in 0..area.height {
        let y = area.y + row;
        let Some(line) = lines.get(start + row as usize) else {
            break;
        };
        buf.set_line(area.x, y, line, area.width);
    }
}

fn tool_card_lines(
    name: &str,
    arguments: &str,
    content: &str,
    theme: &Theme,
    width: usize,
    mode: tool::ToolMode,
    running: bool,
) -> Vec<Line<'static>> {
    if read::is_read_tool(name) {
        read::lines(arguments, content, theme, width, mode, running)
    } else if edit::is_edit_tool(name) {
        edit::lines(name, arguments, content, theme, width, mode, running)
    } else if search_tool::is_search_tool(name) {
        search_tool::lines(arguments, content, theme, width, mode, running)
    } else if search::is_search_tool(name) {
        search::lines(name, arguments, content, theme, width, mode, running)
    } else if list_dir::is_list_dir_tool(name) {
        list_dir::lines(arguments, content, theme, width, mode, running)
    } else if execute::is_execute_tool(name) {
        execute::lines(arguments, content, theme, width, mode, running)
    } else if web::is_web_tool(name) {
        web::lines(name, arguments, content, theme, width, mode, running)
    } else if todo::is_todo_tool(name) {
        todo::card_lines(arguments, theme, width, mode, running)
    } else if plan::is_plan_tool(name) {
        plan::lines(name, arguments, content, theme, width, mode, running)
    } else if goal::is_goal_tool(name) {
        goal::lines(name, arguments, content, theme, width, mode, running)
    } else if sched::is_scheduler_tool(name) {
        sched::lines(name, arguments, content, theme, width, mode, running)
    } else if mcp::is_mcp_tool(name) {
        mcp::lines(name, arguments, content, theme, width, mode, running)
    } else if ask::is_ask_tool(name) {
        ask::lines(name, arguments, content, theme, width, mode, running)
    } else {
        tool::lines(name, arguments, content, theme, width, mode, running)
    }
}

/// The one moving part of the transcript. Runs after [`paint_visible_lines`]
/// over the rows [`Frame::live`] recorded, and only those: a shine head with a
/// trailing glow travels across each running card, and a fresh clock is
/// right-aligned past the card's own text.
///
/// This is a paint-time pass on purpose. The frame it decorates is cached by
/// [`Scrollback::layout_key`] and can be minutes old — see [`live`] for why
/// rebuilding on the shimmer tick is not an option.
fn paint_live_chrome(
    buf: &mut Buffer,
    area: Rect,
    frame: &Frame,
    scroll: u16,
    show_ts: bool,
    theme: &Theme,
    tick: u64,
) {
    if area.width == 0 || area.height == 0 || frame.live.is_empty() {
        return;
    }
    // `/timestamps` owns the right edge; the clock tucks in beside it.
    let right_edge = area.width.saturating_sub(if show_ts {
        TIMESTAMP_RESERVED as u16
    } else {
        0
    });
    for row in &frame.live {
        if row.line < scroll as usize {
            continue;
        }
        let y = area.y + (row.line as u16).saturating_sub(scroll);
        if y >= area.y.saturating_add(area.height) {
            continue;
        }
        // The sweep belongs to the card, not the pane: running it over the full
        // width would spend most of the cycle crossing blank columns to the
        // right of a short header, which reads as a stutter.
        let sweep = row.used.min(area.width);
        for col in 0..sweep {
            let heat = live::shine_at(col as usize, sweep as usize, tick);
            if heat <= 0.0 {
                continue;
            }
            let Some(cell) = buf.cell_mut((area.x + col, y)) else {
                continue;
            };
            if cell.symbol() == " " {
                continue;
            }
            let Some(accent) = row.accent else { continue };
            if let Some(hot) = crate::grok::color::blend_color(cell.fg, accent, heat) {
                cell.set_fg(hot);
            }
        }
        let Some(label) = row.started.and_then(live::elapsed_label) else {
            continue;
        };
        let w = unicode_width::UnicodeWidthStr::width(label.as_str()) as u16;
        // Only when it fits past the header, with a gap. Never eat the card.
        if right_edge < w + 2 || row.used + 2 > right_edge - w {
            continue;
        }
        buf.set_string(area.x + right_edge - w, y, &label, theme.dim());
    }
}

fn format_timestamp(at: SystemTime, expanded: bool) -> String {
    let dt: DateTime<Local> = at.into();
    // hover 态也必须 ≤ TIMESTAMP_RESERVED（10 列）：正文是按 10 列让位的，
    // 超宽会把行尾 9 列字盖掉。展开只换内容，不换宽度。
    if expanded {
        dt.format("  %H:%M:%S").to_string()
    } else if cfg!(unix) {
        dt.format("  %-I:%M %p").to_string()
    } else {
        dt.format("  %I:%M %p").to_string()
    }
}

fn paint_timestamps(
    buf: &mut Buffer,
    area: Rect,
    frame: &Frame,
    scroll: u16,
    mouse: Option<(u16, u16)>,
    theme: &Theme,
) {
    let style = theme.muted();
    for (line_idx, at) in &frame.stamps {
        if *line_idx < scroll as usize {
            continue;
        }
        let y = area.y + (*line_idx as u16).saturating_sub(scroll);
        if y >= area.y + area.height {
            continue;
        }
        let hover =
            mouse.is_some_and(|(mx, my)| my == y && mx >= area.x + area.width.saturating_sub(10));
        let ts = format_timestamp(*at, hover);
        let w = unicode_width::UnicodeWidthStr::width(ts.as_str()) as u16;
        if area.width <= w + 1 {
            continue;
        }
        let x = area.x + area.width - w;
        buf.set_string(x, y, &ts, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_spine::{LlmOutput, ToolCall};
    use ratatui::widgets::Paragraph;

    fn plain(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn user_gets_grok_prompt_arrow() {
        let lines = lines_from_events(&[LogEvent::User("hello".into())]);
        let text = plain(&lines);
        assert!(text.contains('\u{276F}'), "{text}");
        assert!(text.contains("hello"), "{text}");
    }

    #[test]
    fn user_band_fills_the_pane_width() {
        let theme = Theme::current();
        let lines = super::user::lines("hi", &theme, 40, 80);
        let used: usize = lines[0]
            .spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(used, 80, "Grok prompt band pads to the pane width");
        let area = Rect::new(0, 0, 80, 1);
        let mut buf = Buffer::empty(area);
        Paragraph::new(lines).render(area, &mut buf);
        assert_eq!(
            buf[(79, 0)].bg,
            theme.bg_light,
            "right edge of the user row must carry the band background"
        );
    }

    #[test]
    fn system_reminder_is_hidden_from_scrollback() {
        let lines = lines_from_events(&[
            LogEvent::User("shown".into()),
            LogEvent::SystemReminder("Goal NOT complete — continue working".into()),
        ]);
        let text = plain(&lines);
        assert!(text.contains("shown"), "{text}");
        assert!(
            !text.contains("Goal NOT complete"),
            "continuation reminder must not render as a user bubble: {text}"
        );
    }

    #[test]
    fn compact_timestamp_has_clock() {
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let compact = format_timestamp(at, false);
        assert!(compact.contains(':'), "{compact}");
        // hover 展开到秒，但宽度必须仍 ≤ TIMESTAMP_RESERVED（正文按 10 列
        // 让位）；旧格式 `"  %H:%M:%S | %b %d"` 是 19 列，会盖掉行尾 9 列字。
        let hover = format_timestamp(at, true);
        assert!(hover.contains(':'), "{hover}");
        assert!(
            unicode_width::UnicodeWidthStr::width(hover.as_str()) <= TIMESTAMP_RESERVED,
            "{hover}"
        );
    }

    #[test]
    fn user_block_paints_clock_on_the_right() {
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let frame = build_frame(
            &[LogEvent::User("你好".into())],
            &[at],
            80,
            &HashSet::new(),
            &HashMap::new(),
            false,
            true,
            &[],
            todo::TodoFold::default(),
            None,
            None,
            None,
        );
        assert_eq!(frame.stamps, vec![(0, at)]);
        let area = Rect::new(0, 0, 80, 3);
        let mut buf = Buffer::empty(area);
        Paragraph::new(frame.lines.clone()).render(area, &mut buf);
        paint_timestamps(&mut buf, area, &frame, 0, None, &Theme::current());
        let left: String = (0..16).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        let right: String = (60..80).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(left.contains('你') && left.contains('好'), "{left:?}");
        assert!(
            right.contains("AM") || right.contains("PM"),
            "expected Grok-style clock on the user line: {right:?}"
        );
    }

    #[test]
    fn user_bang_uses_bash_prefix() {
        let lines = lines_from_events(&[LogEvent::User("!ls".into())]);
        let text = plain(&lines);
        assert!(text.contains("$ "), "{text}");
        assert!(text.contains("ls"), "{text}");
    }

    #[test]
    fn user_cron_uses_recycle_prefix() {
        let lines = lines_from_events(&[LogEvent::User("\u{21BB} check".into())]);
        let text = plain(&lines);
        assert!(text.contains('\u{21BB}'), "{text}");
        assert!(text.contains("check"), "{text}");
    }

    #[test]
    fn assistant_renders_markdown_heading() {
        let lines = lines_from_events(&[LogEvent::LlmStream(LlmOutput {
            text: "# Title\n\nbody".into(),
            ..LlmOutput::default()
        })]);
        let text = plain(&lines);
        assert!(text.contains("Title"), "{text}");
        assert!(text.contains("body"), "{text}");
    }

    #[test]
    fn read_file_collapsed_uses_read_header() {
        let lines = lines_from_events(&[LogEvent::ToolExecute {
            id: "r1".into(),
            name: "read_file".into(),
            arguments: r#"{"target_file":"src/lib.rs","offset":1,"limit":20}"#.into(),
            content: "1→pub fn x() {}\n".into(),
            images: vec![],
        }]);
        let text = plain(&lines);
        assert!(text.contains("Read "), "{text}");
        assert!(text.contains("src/lib.rs"), "{text}");
        assert!(text.contains("(1-20)"), "{text}");
        assert!(!text.contains("pub fn"), "collapsed hides body: {text}");
    }

    /// deferred 包装的 `skill` 走 `skill::lines`，不落回通用工具卡。
    #[test]
    fn use_tool_wrapped_skill_uses_skill_card() {
        let lines = lines_from_events(&[LogEvent::ToolExecute {
            id: "sk1".into(),
            name: "use_tool".into(),
            arguments: r#"{"tool_name":"skill","tool_input":{"name":"dock-config"}}"#.into(),
            content: r#"<skill name="dock-config" path="bundled/skills/dock-config/SKILL.md">
# Dock 配置手册
</skill>"#
                .into(),
            images: vec![],
        }]);
        let text = plain(&lines);
        assert!(text.contains("已加载技能"), "{text}");
        assert!(text.contains("dock-config"), "{text}");
        assert!(text.contains("内置"), "{text}");
    }

    #[test]
    fn update_goal_card_uses_goal_header() {
        let lines = lines_from_events(&[LogEvent::ToolExecute {
            id: "g1".into(),
            name: "update_goal".into(),
            arguments: r#"{"objective":"理解并分析 TUI"}"#.into(),
            content: r#"{"success":true,"summary":"Goal set: 理解并分析 TUI."}"#.into(),
            images: vec![],
        }]);
        let text = plain(&lines);
        assert!(text.contains("Goal: 设定"), "{text}");
        assert!(text.contains("理解并分析 TUI"), "{text}");
        assert!(
            !text.contains("\"success\""),
            "collapsed hides json body: {text}"
        );
    }

    #[test]
    fn scheduler_create_card_uses_loop_header() {
        let lines = lines_from_events(&[LogEvent::ToolExecute {
            id: "s1".into(),
            name: "scheduler_create".into(),
            arguments: r#"{"interval":"5m","prompt":"检查部署","fire_immediately":true}"#.into(),
            content: "已设定 cron-1（every 5 minutes）".into(),
            images: vec![],
        }]);
        let text = plain(&lines);
        assert!(text.contains("Loop: 设定"), "{text}");
        assert!(text.contains("检查部署"), "{text}");
        assert!(
            !text.contains("fire_immediately"),
            "collapsed hides json args: {text}"
        );
    }

    #[test]
    fn task_card_is_clickable_subagent_row() {
        let lines = lines_from_events(&[LogEvent::ToolExecute {
            id: "t1".into(),
            name: "task".into(),
            arguments: r#"{"prompt":"x","description":"观 观察","subagent_type":"观"}"#.into(),
            content: "Subagent started in background.\n         subagent_id: kid-1\n         type: 观\n         description: 观 观察\n".into(),
            images: vec![],
        }]);
        let text = plain(&lines);
        assert!(text.contains("子代理"), "{text}");
        assert!(text.contains("观"), "{text}");
        assert!(text.contains("点击查看"), "{text}");
        let hid = super::subagent::header_id("t1", "task", "subagent_id: kid-1\n", "{}", &[]);
        assert_eq!(super::subagent::open_id(&hid), Some("kid-1"));
    }

    #[test]
    fn subagent_tool_card_is_clickable() {
        let lines = lines_from_events(&[LogEvent::ToolExecute {
            id: "s1".into(),
            name: "task".into(),
            arguments: r#"{"prompt":"x","description":"观 观察","subagent_type":"观"}"#.into(),
            content: "Subagent started in background.\n         subagent_id: kid-2\n         type: 观\n         description: 观 观察\n".into(),
            images: vec![],
        }]);
        let text = plain(&lines);
        assert!(text.contains("子代理"), "{text}");
        assert!(text.contains("观"), "{text}");
        assert!(text.contains("点击查看"), "{text}");
        let hid = super::subagent::header_id("s1", "task", "subagent_id: kid-2\n", "{}", &[]);
        assert_eq!(hid, "sub:kid-2");
        assert_eq!(super::subagent::open_id(&hid), Some("kid-2"));
    }

    #[test]
    fn inspect_child_skips_spawn_prompt_and_renders_markdown() {
        let lines = inspect_child_lines(
            &[
                LogEvent::User("[观] 观 观察\n\n请观察仓库".into()),
                LogEvent::LlmStream(LlmOutput {
                    text: "## 结论\n\n仓库干净".into(),
                    ..LlmOutput::default()
                }),
            ],
            80,
            false,
        );
        let text = plain(&lines);
        assert!(
            !text.contains("请观察仓库"),
            "spawn User prompt must be skipped: {text}"
        );
        let with_user = inspect_lines(
            &[
                LogEvent::User("[观] 观 观察\n\n请观察仓库".into()),
                LogEvent::LlmStream(LlmOutput {
                    text: "## 结论\n\n仓库干净".into(),
                    ..LlmOutput::default()
                }),
            ],
            80,
            false,
        );
        assert!(
            plain(&with_user).contains("请观察仓库"),
            "{}",
            plain(&with_user)
        );
        assert!(text.contains("仓库干净"), "{text}");
        assert!(text.contains("结论"), "{text}");
    }

    #[test]
    fn inspect_child_keeps_later_user_steering() {
        let lines = inspect_child_lines(
            &[
                LogEvent::User("spawn prompt".into()),
                LogEvent::LlmStream(LlmOutput {
                    text: "first".into(),
                    ..LlmOutput::default()
                }),
                LogEvent::User("请改成只看 auth".into()),
            ],
            80,
            false,
        );
        let text = plain(&lines);
        assert!(!text.contains("spawn prompt"), "{text}");
        assert!(text.contains("请改成只看 auth"), "{text}");
    }

    #[test]
    fn markdown_lines_renders_emphasis() {
        let text = plain(&markdown_lines("**加粗** 正文", 40));
        assert!(text.contains("加粗"), "{text}");
        assert!(text.contains("正文"), "{text}");
    }

    #[test]
    fn background_bash_is_clickable_job_row() {
        let notice = "[Command moved to background]\n\ntask_id: job-3\n\nThe command is still running in the background.";
        let lines = lines_from_events(&[LogEvent::ToolExecute {
            id: "b1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"sleep 9"}"#.into(),
            content: notice.into(),
            images: vec![],
        }]);
        let text = plain(&lines);
        assert!(text.contains("任务"), "{text}");
        assert!(text.contains("点击查看"), "{text}");
        assert!(
            !text.contains("$ sleep"),
            "job card is not the execute fold: {text}"
        );
        let hid =
            super::bg_task::header_id("bash", notice, r#"{"command":"sleep 9"}"#, &[]).unwrap();
        assert_eq!(super::bg_task::open_id(&hid), Some("job-3"));
    }

    /// 失败详情要画给人看（`llm.text` 里没有它，所以模型看不到）。
    #[test]
    fn llm_error_is_rendered_in_scrollback() {
        let detail = "[连接失败] error sending request ← connection reset by peer";
        let lines = lines_from_events(&[LogEvent::LlmStream(cordis_spine::LlmOutput {
            error: Some(detail.into()),
            ..Default::default()
        })]);
        let text = plain(&lines);
        assert!(text.contains("请求失败"), "{text}");
        assert!(text.contains("connection reset by peer"), "{text}");
    }

    fn shell_exec(id: &str, command: &str, content: &str) -> LogEvent {
        LogEvent::ToolExecute {
            id: id.into(),
            name: "bash".into(),
            arguments: serde_json::json!({ "command": command }).to_string(),
            content: content.into(),
            images: vec![],
        }
    }

    /// 相邻的多次前台 bash 折叠成一张卡，折叠态只占一行。
    #[test]
    fn consecutive_shell_calls_collapse_into_one_card() {
        let lines = lines_from_events(&[
            shell_exec("1", "cargo fmt --check", ""),
            shell_exec("2", "cargo clippy", "Finished"),
            shell_exec("3", "cargo test", "test result: ok"),
        ]);
        let text = plain(&lines);
        assert!(text.contains("Bash 3 条命令"), "{text}");
        assert_eq!(
            text.matches('\u{25C6}').count(),
            1,
            "三次调用应只剩一个卡片菱形：{text}"
        );
        assert!(!text.contains("cargo clippy"), "折叠态不该露出正文：{text}");
    }

    /// 展开后逐条命令与各自输出都在。
    #[test]
    fn merged_shell_card_expands_to_each_command() {
        let theme = Theme::current();
        let calls = [
            execute::Call {
                id: "1",
                arguments: r#"{"command":"cargo fmt --check"}"#,
                content: "",
                mode: tool::ToolMode::Expanded,
            },
            execute::Call {
                id: "2",
                arguments: r#"{"command":"cargo test"}"#,
                content: "test result: ok",
                mode: tool::ToolMode::Expanded,
            },
        ];
        let card = execute::group_lines(&calls, "g", &theme, 80, tool::ToolMode::Expanded);
        let text = plain(&card.lines);
        assert!(text.contains("$ cargo fmt --check"), "{text}");
        assert!(text.contains("（无输出）"), "空输出也要留痕：{text}");
        assert!(text.contains("$ cargo test"), "{text}");
        assert!(text.contains("test result: ok"), "{text}");
    }

    /// S1：组内第 2、3 条要能**单独**折叠。grok 的做法是每条 execute 调用各是
    /// 一个带 `DisplayMode` 的块；这里在「一张卡」的外观下保住同一件事。
    /// 原实现把整卡所有行都绑组内第一条的 id，于是点哪都是切整组。
    #[test]
    fn each_command_in_a_merged_card_folds_on_its_own_key() {
        let theme = Theme::current();
        let calls = [
            execute::Call {
                id: "c1",
                arguments: r#"{"command":"one"}"#,
                content: "OUT-ONE",
                mode: tool::ToolMode::Expanded,
            },
            execute::Call {
                id: "c2",
                arguments: r#"{"command":"two"}"#,
                content: "OUT-TWO",
                mode: tool::ToolMode::Collapsed,
            },
        ];
        let card = execute::group_lines(
            &calls,
            "bash-group:c1",
            &theme,
            80,
            tool::ToolMode::Expanded,
        );
        let text = plain(&card.lines);
        // 第二条被单独折起来了：命令还在，正文没了。
        assert!(text.contains("$ two"), "{text}");
        assert!(!text.contains("OUT-TWO"), "第二条应已单独折叠：{text}");
        assert!(text.contains("OUT-ONE"), "第一条不该受影响：{text}");

        // 组头归组键，各命令的行归各自的 id——否则点谁都是切整组。
        let key_at = |row: usize| {
            card.row_ids
                .iter()
                .find(|(r, _)| *r == row)
                .map(|(_, k)| k.as_str())
        };
        assert_eq!(key_at(0), Some("bash-group:c1"), "组头归组键");
        let one_row = card
            .lines
            .iter()
            .position(|l| plain(std::slice::from_ref(l)).contains("$ one"))
            .expect("找不到第一条命令行");
        assert_eq!(key_at(one_row), Some("c1"));
        let two_row = card
            .lines
            .iter()
            .position(|l| plain(std::slice::from_ref(l)).contains("$ two"))
            .expect("找不到第二条命令行");
        assert_eq!(key_at(two_row), Some("c2"));
    }

    /// 组键不能等于组内任何一条调用的 id，否则点第一条命令就折掉整组。
    #[test]
    fn group_key_never_collides_with_a_member_id() {
        let lines = lines_from_events(&[shell_exec("1", "a", "A"), shell_exec("2", "b", "B")]);
        let _ = plain(&lines);
        assert_ne!(format!("{BASH_GROUP_PREFIX}1"), "1");
    }

    /// 单独一次 bash 仍走原来的单卡，渲染逐字不变。
    #[test]
    fn lone_shell_call_keeps_the_single_card() {
        let lines = lines_from_events(&[shell_exec("1", "cargo test", "ok")]);
        let text = plain(&lines);
        assert!(text.contains("Bash $ cargo test"), "{text}");
        assert!(!text.contains("条命令"), "{text}");
    }

    /// 中间夹了别的工具就断开——合并的前提是它们视觉上本来连成一片。
    #[test]
    fn shell_calls_split_by_another_tool_do_not_merge() {
        let lines = lines_from_events(&[
            shell_exec("1", "pwd", "/tmp"),
            LogEvent::ToolExecute {
                id: "2".into(),
                name: "list_dir".into(),
                arguments: r#"{"target_directory":"."}"#.into(),
                content: "a.txt".into(),
                images: vec![],
            },
            shell_exec("3", "whoami", "polar"),
        ]);
        let text = plain(&lines);
        assert!(!text.contains("条命令"), "不相邻不该合并：{text}");
        assert!(text.contains("Bash $ pwd"), "{text}");
        assert!(text.contains("Bash $ whoami"), "{text}");
    }

    /// 后台 bash 归 bg_task 卡（自带任务 id / 实时输出 / 时钟），不能被合并吃掉。
    #[test]
    fn background_shell_calls_are_not_merged() {
        let notice = "[Command moved to background]\n\ntask_id: job-3\n";
        let lines = lines_from_events(&[
            shell_exec("1", "sleep 9", notice),
            shell_exec("2", "sleep 10", notice),
        ]);
        let text = plain(&lines);
        assert!(!text.contains("条命令"), "后台卡不该被合并：{text}");
    }

    #[test]
    fn tool_collapsed_by_default_hides_body() {
        let lines = lines_from_events(&[LogEvent::ToolExecute {
            id: "1".into(),
            name: "list_dir".into(),
            arguments: r#"{"target_directory":"."}"#.into(),
            content: "MARKER.txt\nother".into(),
            images: vec![],
        }]);
        let text = plain(&lines);
        assert!(text.contains('\u{25C6}'), "{text}");
        assert!(text.contains("List"), "{text}");
        assert!(text.contains('.'), "{text}");
        assert!(
            !text.contains("MARKER.txt"),
            "collapsed card must hide body: {text}"
        );
    }

    #[test]
    fn tool_expanded_shows_wrapped_body() {
        let theme = Theme::current();
        let lines = tool::lines(
            "list_dir",
            r#"{"target_directory":"."}"#,
            "MARKER.txt",
            &theme,
            80,
            tool::ToolMode::Expanded,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("MARKER.txt"), "{text}");
        assert!(text.contains("输入"), "{text}");
        assert!(text.contains("输出"), "{text}");
        assert!(text.contains("target_directory"), "{text}");
    }

    #[test]
    fn tool_truncated_shows_head_tail_not_middle() {
        let theme = Theme::current();
        let body: String = (1..=12).map(|i| format!("L{i:02}\n")).collect();
        let lines = tool::lines(
            "bash",
            r#"{"command":"seq"}"#,
            &body,
            &theme,
            80,
            tool::ToolMode::Truncated,
            false,
        );
        let text = plain(&lines);
        assert!(text.contains("\u{2026} 还有 7 行"), "{text}");
        assert!(!text.contains("L05"), "{text}");
    }

    #[test]
    fn thinking_collapsed_hides_reasoning() {
        let lines = lines_from_events(&[LogEvent::LlmStream(LlmOutput {
            text: "hello".into(),
            reasoning: "secret plan".into(),
            reasoning_ms: Some(1500),
            ..LlmOutput::default()
        })]);
        let text = plain(&lines);
        assert!(text.contains("思考"), "{text}");
        assert!(!text.contains("secret plan"), "{text}");
        assert!(text.contains("hello"), "{text}");
    }

    #[test]
    fn mermaid_copy_source_is_hit_mapped() {
        let lines = lines_from_events(&[LogEvent::LlmStream(LlmOutput {
            text: "```mermaid\nflowchart TD\nA-->B\n```\n".into(),
            ..LlmOutput::default()
        })]);
        let text = plain(&lines);
        assert!(text.contains("[Copy Source]"), "{text}");
        assert!(text.contains("[Open Image]"), "{text}");
    }

    #[test]
    fn empty_llm_text_with_tool_calls_shows_pending_card() {
        let lines = lines_from_events(&[LogEvent::LlmStream(LlmOutput {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "1".into(),
                name: "echo".into(),
                arguments: "{}".into(),
            }],
            ..LlmOutput::default()
        })]);
        let text = plain(&lines);
        assert!(text.contains("echo"), "{text}");
        assert!(text.contains("运行中"), "{text}");
    }

    #[test]
    fn layout_cache_reuses_frame_across_scroll_paints() {
        use cordis::Context;
        use cordis_spine::{Sessions, SESSIONS};
        use ratatui::widgets::Widget;

        // The palette is in the layout key now, so a test that flips it must not
        // land between these two renders.
        let _guard = crate::theme::test_guard();
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        root.provide(SESSIONS, sessions.clone()).unwrap();
        for i in 0..40 {
            sessions.append(LogEvent::User(format!("msg-{i}")));
            sessions.append(LogEvent::LlmStream(LlmOutput {
                text: format!("reply-{i} {}", "more text for wrapping ".repeat(8)),
                ..LlmOutput::default()
            }));
        }
        let rev_after_seed = sessions.events_rev();
        assert!(rev_after_seed > 0);

        let scrollback = Scrollback::new(root.clone());
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        Widget::render(&scrollback, area, &mut buf);
        let key1 = *scrollback.layout_cache_key.lock().unwrap();
        let lines1 = scrollback.last_frame.lock().unwrap().lines.len();
        assert!(key1.is_some());
        assert!(lines1 > 24, "seeded history should exceed viewport");

        scrollback.scroll(5);
        Widget::render(&scrollback, area, &mut buf);
        let key2 = *scrollback.layout_cache_key.lock().unwrap();
        let lines2 = scrollback.last_frame.lock().unwrap().lines.len();
        assert_eq!(key1, key2, "scroll must not invalidate layout cache");
        assert_eq!(lines1, lines2);
        assert_eq!(sessions.events_rev(), rev_after_seed);

        sessions.append(LogEvent::User("new".into()));
        Widget::render(&scrollback, area, &mut buf);
        let key3 = *scrollback.layout_cache_key.lock().unwrap();
        assert_ne!(key1, key3, "new event must rebuild layout");
    }

    fn todo(content: &str, status: cordis_spine::TodoStatus) -> (String, cordis_spine::TodoItem) {
        (
            content.to_string(),
            cordis_spine::TodoItem {
                content: content.into(),
                priority: Default::default(),
                status,
                meta: None,
            },
        )
    }

    fn frame_with_todos(todos: &[(String, cordis_spine::TodoItem)], fold: todo::TodoFold) -> Frame {
        build_frame(
            &[LogEvent::User("干活".into())],
            &[],
            80,
            &HashSet::new(),
            &HashMap::new(),
            false,
            false,
            todos,
            fold,
            None,
            None,
            None,
        )
    }

    #[test]
    fn todo_card_header_is_click_mapped() {
        use cordis_spine::TodoStatus;
        let todos = vec![
            todo("读代码", TodoStatus::Completed),
            todo("改 UI", TodoStatus::InProgress),
            todo("补测试", TodoStatus::Pending),
        ];
        let frame = frame_with_todos(&todos, todo::TodoFold::Active);
        let (idx, id) = frame
            .tool_headers
            .iter()
            .find(|(_, id)| id == todo::HEADER_ID)
            .cloned()
            .expect("todo header must be hit-mapped");
        assert_eq!(id, todo::HEADER_ID);
        let header = plain(std::slice::from_ref(&frame.lines[idx]));
        assert!(header.contains("待办"), "{header}");
        assert!(header.contains("1/3"), "{header}");
    }

    #[test]
    fn folding_the_todo_card_shrinks_the_transcript() {
        use cordis_spine::TodoStatus;
        let todos: Vec<_> = (0..5)
            .map(|i| todo(&format!("步骤 {i}"), TodoStatus::Pending))
            .collect();
        let active = frame_with_todos(&todos, todo::TodoFold::Active).lines.len();
        let summary = frame_with_todos(&todos, todo::TodoFold::Summary)
            .lines
            .len();
        assert_eq!(active - summary, 5, "summary drops every item row");
    }

    #[test]
    fn scrollback_cycles_the_todo_fold() {
        use cordis::Context;
        use cordis_spine::{Sessions, SESSIONS};

        let root = Context::new();
        root.provide(SESSIONS, Sessions::new(root.clone())).unwrap();
        let scrollback = Scrollback::new(root.clone());
        assert_eq!(scrollback.todo_fold(), todo::TodoFold::Active);
        scrollback.cycle_todo_fold();
        assert_eq!(scrollback.todo_fold(), todo::TodoFold::Full);
        scrollback.cycle_todo_fold();
        assert_eq!(scrollback.todo_fold(), todo::TodoFold::Summary);
        scrollback.cycle_todo_fold();
        assert_eq!(scrollback.todo_fold(), todo::TodoFold::Active);
        assert!(!scrollback.has_todos(), "no todos service mounted");
    }
}

#[cfg(test)]
mod live_chrome_tests {
    use super::*;
    use cordis::Context;
    use cordis_spine::{LlmOutput, Sessions, ToolCall, SESSIONS};
    use ratatui::widgets::Widget;

    fn plain(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn row_text(buf: &Buffer, y: u16, width: u16) -> String {
        (0..width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    fn row_colors(buf: &Buffer, y: u16, width: u16) -> Vec<ratatui::style::Color> {
        (0..width).map(|x| buf[(x, y)].fg).collect()
    }

    /// A session parked on a tool call the model made but that has not come back.
    fn running_session() -> (Context, Sessions) {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        root.provide(SESSIONS, sessions.clone()).unwrap();
        sessions.append(LogEvent::User("跑一下构建".into()));
        sessions.append(LogEvent::LlmStream(LlmOutput {
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: r#"{"command":"cargo build --release"}"#.into(),
            }],
            ..LlmOutput::default()
        }));
        (root, sessions)
    }

    /// The bug: `running_dots()` / the old `pulse_fg` were evaluated during
    /// `build`, so a cached frame froze them. Nothing time-varying may be baked.
    #[test]
    fn a_running_card_bakes_nothing_that_moves() {
        let (root, _sessions) = running_session();
        let sb = Scrollback::new(root);
        let before = sb.build(80);
        let t0 = crate::grok::color::shimmer_tick();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while crate::grok::color::shimmer_tick() == t0 {
            assert!(
                std::time::Instant::now() < deadline,
                "shimmer clock stopped"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let after = sb.build(80);
        assert_eq!(
            plain(&before.lines),
            plain(&after.lines),
            "a card built one tick later must be identical — anything that \
             differs here is a moving part baked into a cached frame"
        );
    }

    #[test]
    fn a_pending_tool_call_registers_its_header_as_live() {
        let (root, _sessions) = running_session();
        let sb = Scrollback::new(root);
        let frame = sb.build(80);
        assert_eq!(frame.live.len(), 1, "{}", plain(&frame.lines));
        let header = plain(std::slice::from_ref(&frame.lines[frame.live[0].line]));
        assert!(header.contains("cargo build"), "{header}");
        assert!(header.contains("运行中"), "{header}");
        assert!(frame.live[0].started.is_some(), "clock needs a start");
    }

    /// `12.3s` — the clock the painter owns. A built line must never carry one:
    /// the frame around it is cached and can be painted for minutes.
    fn has_baked_clock(text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        chars
            .windows(4)
            .any(|w| w[0].is_ascii_digit() && w[1] == '.' && w[2].is_ascii_digit() && w[3] == 's')
    }

    /// `wait_tasks` parks on this card for minutes, and the clock it used to
    /// bake was the *target's* birth time, not this call's start. It has to ride
    /// the same paint-time pass as every other running card.
    #[test]
    fn a_running_task_op_registers_its_header_as_live() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        root.provide(SESSIONS, sessions.clone()).unwrap();
        sessions.append(LogEvent::User("等一下子代理".into()));
        sessions.append(LogEvent::LlmStream(LlmOutput {
            tool_calls: vec![ToolCall {
                id: "call-1".into(),
                name: "wait_tasks".into(),
                arguments: r#"{"ids":["kid-1"]}"#.into(),
            }],
            ..LlmOutput::default()
        }));
        let sb = Scrollback::new(root);
        let frame = sb.build(80);
        assert_eq!(frame.live.len(), 1, "{}", plain(&frame.lines));
        let row = &frame.live[0];
        let header = plain(std::slice::from_ref(&frame.lines[row.line]));
        assert!(header.contains("查询任务"), "{header}");
        assert!(
            !has_baked_clock(&header),
            "the elapsed clock must be painted, not baked: {header}"
        );
        assert!(row.started.is_some(), "clock needs a start");

        // …and the painter has to actually put it on the row.
        let area = Rect::new(0, 0, 80, 12);
        let theme = Theme::current();
        let mut buf = Buffer::empty(area);
        paint_visible_lines(&mut buf, area, &frame.lines, 0);
        paint_live_chrome(&mut buf, area, &frame, 0, false, &theme, 0);
        let painted: String = (0..area.width)
            .map(|x| buf[(area.x + x, row.line as u16)].symbol())
            .collect();
        assert!(
            has_baked_clock(&painted),
            "no painted clock on the running task-op row: {painted:?}"
        );
    }

    #[test]
    fn a_settled_tool_call_is_not_live() {
        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        root.provide(SESSIONS, sessions.clone()).unwrap();
        sessions.append(LogEvent::ToolExecute {
            id: "call-1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"echo hi"}"#.into(),
            content: "hi\n".into(),
            images: vec![],
        });
        let sb = Scrollback::new(root);
        assert!(sb.build(80).live.is_empty());
    }

    /// The headline regression: the shine has to advance between paints of the
    /// *same* cached frame. Before this change the card was repainted
    /// byte-identically for as long as the layout key held.
    #[test]
    fn the_shine_advances_across_paints_of_one_cached_frame() {
        let (root, _sessions) = running_session();
        let sb = Scrollback::new(root);
        let area = Rect::new(0, 0, 80, 10);
        let frame = sb.build(80);
        let theme = Theme::current();
        let y = frame.live[0].line as u16;

        let ticks = 40u64;
        let mut lit = Vec::new();
        for tick in 0..ticks {
            let mut buf = Buffer::empty(area);
            paint_visible_lines(&mut buf, area, &frame.lines, 0);
            paint_live_chrome(&mut buf, area, &frame, 0, false, &theme, tick);
            lit.push(row_colors(&buf, y, area.width));
        }
        let moved = lit.windows(2).filter(|w| w[0] != w[1]).count();
        // Before this change the card was repainted byte-identically forever,
        // so `moved` was 0. It is not 39 either: the head spends a few ticks
        // off the left edge between passes, which is the pause by design.
        assert!(
            moved * 5 >= (ticks as usize - 1) * 3,
            "the sweep is stalling: changed on only {moved}/{}",
            ticks - 1
        );
        let accent = frame.live[0].accent.expect("card accent");
        let bare = {
            let mut b = Buffer::empty(area);
            paint_visible_lines(&mut b, area, &frame.lines, 0);
            row_colors(&b, y, area.width)
        };
        let peak = lit
            .iter()
            .map(|r| r.iter().zip(bare.iter()).filter(|(a, b)| a != b).count())
            .max()
            .unwrap_or(0);
        assert!(peak >= 8, "the lit window is only {peak} cells wide");
        assert!(
            lit.iter().any(|r| r.contains(&accent)),
            "the head must reach the card's accent at full strength"
        );
    }

    /// The sweep is decoration: it may recolour the row but never rewrite it.
    #[test]
    fn the_shine_never_changes_a_glyph() {
        let (root, _sessions) = running_session();
        let sb = Scrollback::new(root);
        let area = Rect::new(0, 0, 80, 10);
        let mut frame = sb.build(80);
        // Clock off: this test is about the sweep, which may recolour but must
        // never rewrite.
        frame.live[0].started = None;
        let theme = Theme::current();
        let y = frame.live[0].line as u16;

        let mut bare = Buffer::empty(area);
        paint_visible_lines(&mut bare, area, &frame.lines, 0);
        let expected = row_text(&bare, y, area.width);
        for tick in 0..40u64 {
            let mut buf = Buffer::empty(area);
            paint_visible_lines(&mut buf, area, &frame.lines, 0);
            paint_live_chrome(&mut buf, area, &frame, 0, false, &theme, tick);
            assert_eq!(row_text(&buf, y, area.width), expected, "tick {tick}");
        }
    }

    #[test]
    fn the_clock_lands_on_the_right_and_spares_the_card_text() {
        let (root, _sessions) = running_session();
        let sb = Scrollback::new(root);
        let area = Rect::new(0, 0, 80, 10);
        let mut frame = sb.build(80);
        frame.live[0].started =
            Some(std::time::SystemTime::now() - std::time::Duration::from_secs(12));
        let theme = Theme::current();
        let y = frame.live[0].line as u16;
        let used = frame.live[0].used;

        let mut buf = Buffer::empty(area);
        paint_visible_lines(&mut buf, area, &frame.lines, 0);
        paint_live_chrome(&mut buf, area, &frame, 0, false, &theme, 0);
        let row = row_text(&buf, y, area.width);
        assert!(row.trim_end().ends_with("12s"), "{row:?}");
        assert!(row.contains("cargo build"), "card text survives: {row:?}");
        assert!(
            row.replace(' ', "").contains("运行中"),
            "card label survives: {row:?}"
        );
        // Measured in display columns, not symbols: the clock starts past the
        // header, so nothing of the card is overwritten.
        let clock_x = (0..area.width)
            .rev()
            .take_while(|x| buf[(*x, y)].symbol() != " ")
            .last()
            .expect("clock painted");
        assert!(
            clock_x >= used + 2,
            "clock at col {clock_x} overlaps a {used}-column header"
        );
    }

    /// Narrow pane: there is no room for a clock, so there must be no clock —
    /// not a clock painted over the command.
    #[test]
    fn a_narrow_row_drops_the_clock_instead_of_eating_the_card() {
        let (root, _sessions) = running_session();
        let sb = Scrollback::new(root);
        let area = Rect::new(0, 0, 30, 10);
        let mut frame = sb.build(30);
        frame.live[0].started =
            Some(std::time::SystemTime::now() - std::time::Duration::from_secs(12));
        let theme = Theme::current();
        let y = frame.live[0].line as u16;

        let mut bare = Buffer::empty(area);
        paint_visible_lines(&mut bare, area, &frame.lines, 0);
        let expected = row_text(&bare, y, area.width);

        let mut buf = Buffer::empty(area);
        paint_visible_lines(&mut buf, area, &frame.lines, 0);
        paint_live_chrome(&mut buf, area, &frame, 0, false, &theme, 0);
        assert_eq!(row_text(&buf, y, area.width), expected);
    }

    /// The painter has to actually be wired into `render`, not just exist. The
    /// clock is the tick-independent half, so it proves the call site.
    #[test]
    fn render_runs_the_live_painter() {
        let _guard = crate::theme::test_guard();
        let (root, _sessions) = running_session();
        let sb = Scrollback::new(root);
        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(&sb, area, &mut buf);
        let y = sb.last_frame.lock().unwrap().live[0].line as u16;
        let row = row_text(&buf, y, area.width);
        assert!(
            row.trim_end().ends_with('s'),
            "no elapsed clock — is `paint_live_chrome` still called? {row:?}"
        );
    }

    /// Switching palettes must invalidate the transcript. Without the theme in
    /// the key the cached GrokNight spans (#e1e1e1) survive onto GrokDay's
    /// #eeeeee background — a contrast ratio of about 1.06:1.
    #[test]
    fn switching_the_palette_rebuilds_the_transcript() {
        let _guard = crate::theme::test_guard();
        let restore = Theme::current_kind();

        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        root.provide(SESSIONS, sessions.clone()).unwrap();
        sessions.append(LogEvent::LlmStream(LlmOutput {
            text: "## 标题\n\n正文一段".into(),
            ..LlmOutput::default()
        }));
        let sb = Scrollback::new(root);
        let area = Rect::new(0, 0, 80, 10);
        let mut buf = Buffer::empty(area);

        Theme::apply_kind(crate::theme::ThemeKind::GrokNight);
        Widget::render(&sb, area, &mut buf);
        let night_key = *sb.layout_cache_key.lock().unwrap();
        let night = sb.last_frame.lock().unwrap().lines.clone();

        Theme::apply_kind(crate::theme::ThemeKind::GrokDay);
        Widget::render(&sb, area, &mut buf);
        let day_key = *sb.layout_cache_key.lock().unwrap();
        let day = sb.last_frame.lock().unwrap().lines.clone();

        Theme::apply_kind(restore);

        assert_ne!(night_key, day_key, "palette must be in the layout key");
        assert_eq!(plain(&night), plain(&day), "only the colours may differ");
        let fg = |ls: &[Line<'static>]| -> Vec<Option<ratatui::style::Color>> {
            ls.iter()
                .flat_map(|l| l.spans.iter().map(|s| s.style.fg))
                .collect()
        };
        assert_ne!(
            fg(&night),
            fg(&day),
            "markdown must be re-rendered under the new palette, not served \
             from the assistant cache"
        );
    }

    /// The `read` card keeps its own body cache; it has the same hazard.
    #[test]
    fn switching_the_palette_repaints_a_read_card_body() {
        let _guard = crate::theme::test_guard();
        let restore = Theme::current_kind();
        let args = r#"{"target_file":"note.txt"}"#;
        let body = "1→hello\n2→world\n";

        Theme::apply_kind(crate::theme::ThemeKind::GrokNight);
        let night = read::lines(
            args,
            body,
            &Theme::current(),
            80,
            tool::ToolMode::Expanded,
            false,
        );
        Theme::apply_kind(crate::theme::ThemeKind::GrokDay);
        let day = read::lines(
            args,
            body,
            &Theme::current(),
            80,
            tool::ToolMode::Expanded,
            false,
        );
        Theme::apply_kind(restore);

        assert_eq!(plain(&night), plain(&day));
        // Only the body: the header is rebuilt every call, so including it
        // would pass even with a palette-blind body cache. 整行所有 span 都看
        // —— 只盯第一个的话，公共外壳插在最前面的缩进空白（无色）会把差异吃掉。
        let body = |ls: &[Line<'static>]| -> Vec<Option<ratatui::style::Color>> {
            ls[2..]
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.style.fg))
                .collect()
        };
        assert!(!body(&night).is_empty(), "expected numbered body rows");
        assert_ne!(body(&night), body(&day), "body cache ignored the palette");
    }

    /// 旧实现 `pages.saturating_mul(half).max(pages)` 把所有负向翻页夹成
    /// -1：PageDown / Ctrl+D 只滚一行。24 行高、下翻两页应动 24 行。
    #[test]
    fn page_down_scrolls_half_a_screen_not_one_line() {
        let (root, _sessions) = running_session();
        let sb = Scrollback::new(root);
        *sb.last_area.lock().unwrap() = Rect::new(0, 0, 80, 24);
        *sb.offset.lock().unwrap() = 100;
        sb.scroll_page(-1);
        assert_eq!(
            *sb.offset.lock().unwrap(),
            88,
            "one page down = half screen"
        );
        sb.scroll_page(-1);
        assert_eq!(
            *sb.offset.lock().unwrap(),
            76,
            "second page down = full screen"
        );
        sb.scroll_page(1);
        assert_eq!(*sb.offset.lock().unwrap(), 88, "page up still works");
    }

    /// 窗口矮到半屏算不出（height < 2）时退化为一次一行，且两个方向都要动。
    #[test]
    fn page_scroll_in_a_one_line_window_moves_one_line() {
        let (root, _sessions) = running_session();
        let sb = Scrollback::new(root);
        *sb.last_area.lock().unwrap() = Rect::new(0, 0, 80, 1);
        *sb.offset.lock().unwrap() = 10;
        sb.scroll_page(1);
        assert_eq!(*sb.offset.lock().unwrap(), 11);
        sb.scroll_page(-1);
        assert_eq!(*sb.offset.lock().unwrap(), 10);
    }

    /// hover 态时间戳换格式但不能换宽度：正文按 TIMESTAMP_RESERVED=10 让
    /// 位，旧的 `"  %H:%M:%S | %b %d"` 是 19 列，鼠标一靠右就盖掉 9 列字。
    #[test]
    fn hover_timestamp_stays_within_reserved_columns() {
        let at = std::time::SystemTime::UNIX_EPOCH;
        let hover = format_timestamp(at, true);
        assert_eq!(
            unicode_width::UnicodeWidthStr::width(hover.as_str()),
            TIMESTAMP_RESERVED,
            "{hover}"
        );
        let normal = format_timestamp(at, false);
        assert!(
            unicode_width::UnicodeWidthStr::width(normal.as_str()) <= TIMESTAMP_RESERVED,
            "{normal}"
        );
    }

    /// find 用真实宽度（不再是 `.max(40)`）并直接读 last_frame，窄窗下
    /// jump_to_line 落点才是命中行。
    #[test]
    fn find_at_narrow_width_jumps_to_the_matching_line() {
        use cordis::Context;
        use cordis_spine::{LlmOutput, Sessions, SESSIONS};
        use ratatui::widgets::Widget;

        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        root.provide(SESSIONS, sessions.clone()).unwrap();
        for i in 0..40 {
            sessions.append(LogEvent::User(format!("msg-{i}")));
            sessions.append(LogEvent::LlmStream(LlmOutput {
                text: format!("reply-{i} {}", "more text for wrapping ".repeat(8)),
                ..LlmOutput::default()
            }));
        }
        let sb = Scrollback::new(root);
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);
        Widget::render(&sb, area, &mut buf);

        let hits = sb.find("reply-7");
        assert!(!hits.is_empty());
        let (idx, text) = &hits[0];
        assert!(text.contains("reply-7"), "{text}");
        sb.jump_to_line(*idx);

        let frame = sb.last_frame.lock().unwrap();
        let extra = frame.lines.len().saturating_sub(area.height as usize) as u16;
        let offset = *sb.offset.lock().unwrap();
        let top = extra.saturating_sub(offset) as usize;
        let visible: String = (top..(top + area.height as usize).min(frame.lines.len()))
            .map(|i| {
                frame.lines[i]
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(
            visible.contains("reply-7"),
            "jump must land on the match; visible window:\n{visible}"
        );
    }
}

/// #52：外壳的一致性由测试守着，不靠人记。每加一张卡片都要过这一组 —— 只要
/// 有人手搓缩进、自创截断文案、或者忘了折叠符，这里立刻红。
#[cfg(test)]
mod card_shell_tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// 覆盖 `tool_card_lines` 的每一条分支。新增卡片时补一行。
    fn samples() -> Vec<(&'static str, &'static str, &'static str, String)> {
        let body: String = (1..=20).map(|i| format!("正文第 {i} 行\n")).collect();
        vec![
            ("bash", "bash", r#"{"command":"cargo test"}"#, body.clone()),
            ("通用工具", "echo", r#"{"text":"hi"}"#, body.clone()),
            (
                "read",
                "read_file",
                r#"{"target_file":"a.rs"}"#,
                (1..=20)
                    .map(|i| format!("{i}\u{2192}let v{i} = {i};\n"))
                    .collect(),
            ),
            (
                "edit",
                "search_replace",
                r#"{"file_path":"a.rs","old_string":"let x = 1;","new_string":"let x = 2;"}"#,
                String::new(),
            ),
            (
                "list_dir",
                "list_dir",
                r#"{"target_directory":"src"}"#,
                std::iter::once("src\n".to_string())
                    .chain((1..=20).map(|i| format!("f{i}.rs\n")))
                    .collect(),
            ),
            (
                "grep",
                "grep",
                r#"{"pattern":"fn"}"#,
                (1..=20)
                    .map(|i| format!("src/f{i}.rs:{i}:fn x() {{}}\n"))
                    .collect(),
            ),
            ("web", "web_search", r#"{"query":"q"}"#, body.clone()),
            (
                "fetch",
                "web_fetch",
                r#"{"url":"https://x.dev"}"#,
                body.clone(),
            ),
            ("mcp", "mcp_srv__act", r#"{"a":1}"#, body.clone()),
            (
                "search_tool",
                "search_tool",
                r#"{"query":"q"}"#,
                body.clone(),
            ),
            (
                "goal",
                "update_goal",
                r#"{"objective":"统一外壳"}"#,
                r#"{"success":true,"summary":"Goal set"}"#.into(),
            ),
            (
                "plan",
                "exit_plan_mode",
                r##"{"plan":"# 计划\n\n- 一\n- 二"}"##,
                "ok".into(),
            ),
            (
                "sched",
                "scheduler_create",
                r#"{"interval":"5m","prompt":"检查"}"#,
                "已设定 cron-1".into(),
            ),
            (
                "ask",
                "ask_question",
                r#"{"questions":[{"question":"选哪个?"}]}"#,
                "".into(),
            ),
            (
                "todo",
                "todo_write",
                r#"{"merge":false,"todos":[{"id":"1","content":"干活","status":"pending"}]}"#,
                "ok".into(),
            ),
        ]
    }

    fn render(name: &str, args: &str, content: &str, mode: tool::ToolMode) -> Vec<Line<'static>> {
        tool_card_lines(name, args, content, &Theme::current(), 60, mode, false)
    }

    /// 正文一律缩进 [`card::BODY_INDENT`] 列，和标题文字对齐。原先 0 / 2 /
    /// gutter 三套并存，同屏看过去左边缘是锯齿状的。
    #[test]
    fn every_card_body_shares_one_indent() {
        let pad = " ".repeat(card::BODY_INDENT);
        for (label, name, args, content) in samples() {
            for mode in [tool::ToolMode::Truncated, tool::ToolMode::Expanded] {
                let lines = render(name, args, &content, mode);
                for line in lines.iter().skip(1) {
                    let t = text(line);
                    if t.trim().is_empty() {
                        continue;
                    }
                    assert!(
                        t.starts_with(&pad),
                        "{label} 的正文没走公共缩进 ({mode:?}): {t:?}"
                    );
                }
            }
        }
    }

    /// 截断脚注一种写法。原先四种：`… +4 lines` / `… +2 more files` /
    /// `… (6 more lines)` / `… 还有 6 行`。
    #[test]
    fn every_elision_reads_the_same_way() {
        let banned = ["lines", "more", "entries", "+"];
        let mut seen = 0usize;
        for (label, name, args, content) in samples() {
            for line in render(name, args, &content, tool::ToolMode::Truncated) {
                let t = text(&line);
                let Some(rest) = t.trim_start().strip_prefix('\u{2026}') else {
                    continue;
                };
                seen += 1;
                assert!(
                    rest.trim_start().starts_with("还有"),
                    "{label} 的截断脚注没走公共文案: {t:?}"
                );
                for bad in banned {
                    assert!(!rest.contains(bad), "{label} 仍是旧文案: {t:?}");
                }
            }
        }
        assert!(seen >= 5, "样本里应该有好几张卡触发截断，实际 {seen}");
    }

    /// 每张可折叠的卡片都带折叠符，三种状态各不相同。原先只有一半卡片有
    /// 「（点击展开）」，最常见的 Read / Edit / Bash / List / Search 反倒没有。
    #[test]
    fn every_foldable_card_marks_its_fold_state() {
        for (label, name, args, content) in samples() {
            let marks: Vec<String> = [
                tool::ToolMode::Collapsed,
                tool::ToolMode::Truncated,
                tool::ToolMode::Expanded,
            ]
            .into_iter()
            .map(|m| {
                let head = render(name, args, &content, m).remove(0);
                head.spans
                    .last()
                    .map(|s| s.content.to_string())
                    .unwrap_or_default()
            })
            .collect();
            for m in &marks {
                assert_eq!(
                    unicode_width::UnicodeWidthStr::width(m.as_str()),
                    card::FOLD_MARK,
                    "{label} 的标题尾巴不是折叠符: {marks:?}"
                );
            }
            let unique: std::collections::HashSet<&String> = marks.iter().collect();
            assert_eq!(unique.len(), 3, "{label} 的三种折叠态看不出区别: {marks:?}");
        }
    }

    /// 标题行永远画得下，且不顶出面板 —— 折叠符是「这张卡能点」的唯一提示。
    #[test]
    fn a_card_header_fits_its_pane_at_every_width() {
        for (label, name, args, content) in samples() {
            for pane in [24usize, 40, 60, 100] {
                let head = tool_card_lines(
                    name,
                    args,
                    &content,
                    &Theme::current(),
                    pane,
                    tool::ToolMode::Collapsed,
                    false,
                )
                .remove(0);
                let w = unicode_width::UnicodeWidthStr::width(text(&head).as_str());
                assert!(
                    w <= pane,
                    "{label} 在 {pane} 列画出 {w} 列: {:?}",
                    text(&head)
                );
            }
        }
    }

    /// 整张卡都能点，不只是折叠头那一行。
    #[test]
    fn a_whole_card_is_clickable_not_just_its_header() {
        let frame = build_frame(
            &[LogEvent::ToolExecute {
                id: "t1".into(),
                name: "bash".into(),
                arguments: r#"{"command":"cargo test"}"#.into(),
                content: "一\n二\n三\n".into(),
                images: vec![],
            }],
            &[],
            60,
            &HashSet::new(),
            &HashMap::from([("t1".to_string(), tool::ToolMode::Expanded)]),
            false,
            false,
            &[],
            todo::TodoFold::default(),
            None,
            None,
            None,
        );
        let body_rows: Vec<usize> = frame
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| text(l).contains('\u{4e00}'))
            .map(|(i, _)| i)
            .collect();
        assert!(!body_rows.is_empty(), "样本里应该有正文行");
        for row in body_rows {
            assert!(
                frame
                    .tool_headers
                    .iter()
                    .any(|(i, id)| *i == row && id == "t1"),
                "展开后第 {row} 行点不动"
            );
        }
    }

    /// 卡片之间只留一行空白 —— 外壳负责，各卡片不自己推。
    #[test]
    fn cards_are_separated_by_exactly_one_blank_line() {
        let frame = build_frame(
            &[
                LogEvent::ToolExecute {
                    id: "a".into(),
                    name: "echo".into(),
                    arguments: r#"{"text":"hi"}"#.into(),
                    content: String::new(),
                    images: vec![],
                },
                LogEvent::ToolExecute {
                    id: "b".into(),
                    name: "echo".into(),
                    arguments: r#"{"text":"ho"}"#.into(),
                    content: String::new(),
                    images: vec![],
                },
            ],
            &[],
            60,
            &HashSet::new(),
            &HashMap::from([
                ("a".to_string(), tool::ToolMode::Expanded),
                ("b".to_string(), tool::ToolMode::Expanded),
            ]),
            false,
            false,
            &[],
            todo::TodoFold::default(),
            None,
            None,
            None,
        );
        let blanks: Vec<String> = frame.lines.iter().map(|l| text(l)).collect();
        for w in blanks.windows(2) {
            assert!(
                !(w[0].trim().is_empty() && w[1].trim().is_empty()),
                "出现连续两行空白:\n{}",
                blanks.join("\n")
            );
        }
    }
}
