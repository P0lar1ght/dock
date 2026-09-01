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
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::grok::mermaid::{self, AffordanceKind};
use crate::theme::Theme;

mod ask;
mod assistant;
mod bg_task;
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
mod subagent;
mod text_selection;
mod thinking;
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
    last_frame: Mutex<Frame>,
    /// When [`Self::layout_key`] matches, [`Self::last_frame`] is reused.
    layout_cache_key: Mutex<Option<u64>>,
    last_area: Mutex<Rect>,
    mouse: Mutex<Option<(u16, u16)>>,
    selection: Mutex<SelectionState>,
}

impl Scrollback {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            offset: Mutex::new(0),
            expanded: Mutex::new(HashSet::new()),
            tool_fold: Mutex::new(HashMap::new()),
            last_frame: Mutex::new(Frame {
                lines: Vec::new(),
                tool_headers: Vec::new(),
                mermaid: Vec::new(),
                stamps: Vec::new(),
            }),
            layout_cache_key: Mutex::new(None),
            last_area: Mutex::new(Rect::default()),
            mouse: Mutex::new(None),
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
        self.scroll(pages.saturating_mul(height as i16 / 2).max(pages));
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
        let Some(drag) = sel.drag.take() else {
            return None;
        };
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
            return match mermaid::hit_kind(column.saturating_sub(area.x)) {
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
        if id.starts_with("think:") {
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
        let needle = q.to_ascii_lowercase();
        let width = self.last_area.lock().unwrap().width.max(40) as usize;
        let frame = self.build(width);
        frame
            .lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                text.to_ascii_lowercase()
                    .contains(&needle)
                    .then_some((i, text.trim().to_string()))
            })
            .collect()
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
                std::mem::discriminant(&item.status).hash(&mut h);
            }
        }
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
            if matches!(e, LogEvent::User(_)) {
                if !seen_user {
                    seen_user = true;
                    return false;
                }
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
        None,
        None,
        None,
    )
    .lines
}

/// Grok `timestamp_reserved`: `"  12:30 PM"` is 10 columns.
const TIMESTAMP_RESERVED: usize = 10;

fn message_wrap(width: usize, show_ts: bool) -> usize {
    if show_ts {
        width.saturating_sub(TIMESTAMP_RESERVED).max(20)
    } else {
        width
    }
}

fn build_frame(
    events: &[LogEvent],
    times: &[SystemTime],
    width: usize,
    expanded: &HashSet<String>,
    tool_fold: &HashMap<String, tool::ToolMode>,
    working: bool,
    show_ts: bool,
    todos: &[(String, cordis_spine::TodoItem)],
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
    for (i, event) in events.iter().enumerate() {
        let stamp = times.get(i).copied();
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
            } => {
                push_tool_card(
                    &mut lines,
                    &mut tool_headers,
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
    if !todos.is_empty() {
        lines.extend(todo_lines(todos, &theme, width));
        lines.push(Line::from(""));
    }
    Frame {
        lines,
        tool_headers,
        mermaid,
        stamps,
    }
}

fn push_tool_card(
    lines: &mut Vec<Line<'static>>,
    tool_headers: &mut Vec<(usize, String)>,
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
            role: if subagent::is_subagent_tool(name) {
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
            } else {
                None
            },
        });
        lines.extend(subagent::lines(arguments, content, live, theme, width));
        for i in header_at..lines.len() {
            tool_headers.push((i, hid.clone()));
        }
    } else if let Some(jid) = bg_task::open_id(&hid) {
        let snap = jobs.and_then(|j| j.snapshot(jid));
        lines.extend(bg_task::lines(
            name,
            arguments,
            content,
            snap.as_ref(),
            theme,
            width,
        ));
        for i in header_at..lines.len() {
            tool_headers.push((i, hid.clone()));
        }
    } else {
        let mode = tool_fold
            .get(id)
            .copied()
            .unwrap_or(tool::ToolMode::Collapsed);
        lines.extend(tool_card_lines(
            name, arguments, content, theme, width, mode, running,
        ));
        tool_headers.push((header_at, hid));
    }
    lines.push(Line::from(""));
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

fn todo_lines(
    todos: &[(String, cordis_spine::TodoItem)],
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let style = crate::grok::todo_pane::TodoPaneStyle::default();
    let mut lines = vec![Line::from(Span::styled(
        "待办",
        Style::default().fg(theme.accent_user),
    ))];
    for (id, item) in todos {
        let st = style.for_status(item.status);
        let mark = crate::grok::todo_pane::todo_icon(item.status);
        let content = format!(" {id} {}", item.content);
        let clipped: String = content
            .chars()
            .take(width.saturating_sub(2).max(6))
            .collect();
        lines.push(Line::from(vec![
            Span::styled(mark.to_string(), Style::default().fg(st.icon_fg)),
            Span::styled(clipped, st.text_style),
        ]));
    }
    lines
}

fn format_timestamp(at: SystemTime, expanded: bool) -> String {
    let dt: DateTime<Local> = at.into();
    if expanded {
        dt.format("  %H:%M:%S | %b %d").to_string()
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
        let hover = format_timestamp(at, true);
        assert!(hover.contains('|'), "{hover}");
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
        }]);
        let text = plain(&lines);
        assert!(text.contains("Read "), "{text}");
        assert!(text.contains("src/lib.rs"), "{text}");
        assert!(text.contains("(1-20)"), "{text}");
        assert!(!text.contains("pub fn"), "collapsed hides body: {text}");
    }

    #[test]
    fn update_goal_card_uses_goal_header() {
        let lines = lines_from_events(&[LogEvent::ToolExecute {
            id: "g1".into(),
            name: "update_goal".into(),
            arguments: r#"{"objective":"理解并分析 TUI"}"#.into(),
            content: r#"{"success":true,"summary":"Goal set: 理解并分析 TUI."}"#.into(),
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
            name: "subagent".into(),
            arguments: r#"{"prompt":"x","description":"观 观察","subagent_type":"观"}"#.into(),
            content: "Subagent started in background.\n         subagent_id: kid-2\n         type: 观\n         description: 观 观察\n".into(),
        }]);
        let text = plain(&lines);
        assert!(text.contains("子代理"), "{text}");
        assert!(text.contains("观"), "{text}");
        assert!(text.contains("点击查看"), "{text}");
        let hid = super::subagent::header_id("s1", "subagent", "subagent_id: kid-2\n", "{}", &[]);
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

    #[test]
    fn tool_collapsed_by_default_hides_body() {
        let lines = lines_from_events(&[LogEvent::ToolExecute {
            id: "1".into(),
            name: "list_dir".into(),
            arguments: r#"{"target_directory":"."}"#.into(),
            content: "MARKER.txt\nother".into(),
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
        assert!(text.contains("… +7 lines"), "{text}");
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

        let root = Context::new();
        let sessions = Sessions::new(root.clone());
        root.provide(SESSIONS, sessions.clone()).unwrap();
        for i in 0..40 {
            sessions.append(LogEvent::User(format!("msg-{i}")));
            sessions.append(LogEvent::LlmStream(LlmOutput {
                text: format!(
                    "{}{}",
                    format!("reply-{i} "),
                    "more text for wrapping ".repeat(8)
                ),
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
}
