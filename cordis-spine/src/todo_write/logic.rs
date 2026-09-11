//! TodoWrite — new-architecture implementation.
//!
//! Reuses the core logic (`validate_no_duplicate_ids`, `apply_replace`,
//! `apply_merge`, `summarize_todo_state`) from the old `implementations::todo`
//! module. State is stored as `State<TodoState>` in Resources instead of
//! `ToolState.todo_state`.

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::fmt::Write;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(thiserror::Error, Debug)]
pub enum TodoError {
    #[error("Missing Todo content in mode: {0}")]
    MissingTodoContent(String),

    #[error("Missing Todo ID in mode: {0}")]
    MissingTodoID(String),

    #[error("Duplicate Todo ID in response: {0}")]
    DuplicateTodoID(String),
}

pub(crate) fn validate_no_duplicate_ids(updates: &[TodoUpdate]) -> Result<(), TodoError> {
    use std::collections::HashSet;
    let mut seen = HashSet::with_capacity(updates.len());
    if let Some(dup) = updates.iter().map(|u| &u.id).find(|id| !seen.insert(*id)) {
        return Err(TodoError::DuplicateTodoID(dup.to_owned()));
    }
    Ok(())
}

/// `merge=false`: the incoming list fully replaces the existing todo state.
/// If `content` is omitted for an item, the `id` is used as a fallback.
/// If `status` is omitted, it defaults to `Pending`.
pub(crate) fn apply_replace(
    state: &mut TodoState,
    updates: &[TodoUpdate],
) -> Result<(), TodoError> {
    state.clear();
    for u in updates {
        let content = if u.has_no_content() {
            u.id.clone()
        } else {
            u.content.clone().unwrap()
        };
        let status = u.status.unwrap_or(TodoStatus::Pending);
        state.push(
            u.id.clone(),
            TodoItem {
                content,
                priority: TodoPriority::default(),
                status,
                meta: None,
            },
        );
    }
    Ok(())
}

/// `merge=true`: updates are merged into the existing state.
/// - **Existing items**: `content` is optional — if omitted the previous
///   value is kept. This lets the model mark an item from `in_progress` →
///   `completed` without echoing the content back.
/// - **New items** (id not yet in state): if `content` is omitted the `id`
///   is used as a fallback so the tool never errors on a merge call. This
///   makes the tool resilient to state being lost between calls.
pub(crate) fn apply_merge(state: &mut TodoState, updates: &[TodoUpdate]) -> Result<(), TodoError> {
    for u in updates {
        if state.update(&u.id, u.content.as_deref(), u.status) {
            // Existing item – partial update succeeded, content was optional.
            continue;
        }
        let content = if u.has_no_content() {
            u.id.clone()
        } else {
            u.content.clone().unwrap()
        };
        let status = u.status.unwrap_or(TodoStatus::Pending);
        state.push(
            u.id.clone(),
            TodoItem {
                content,
                priority: TodoPriority::default(),
                status,
                meta: None,
            },
        );
    }
    Ok(())
}

/// Grok `effective_merge`: the model forgot `merge: true` but clearly meant a
/// partial update (state is non-empty and every item targets an existing id
/// without content). Replacing there would wipe every content string to its
/// id, so treat it as a merge.
pub(crate) fn effective_merge(state: &TodoState, input: &TodoWriteInput) -> bool {
    input.merge
        || (!state.is_empty()
            && !input.todos.is_empty()
            && input
                .todos
                .iter()
                .all(|u| u.has_no_content() && state.has_id(&u.id)))
}

/// Per-status counts. Drives the tool-result nudge, the turn-end gate and the
/// TUI header.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TodoStats {
    pub pending: usize,
    pub in_progress: usize,
    pub completed: usize,
    pub cancelled: usize,
}

impl TodoStats {
    pub fn total(&self) -> usize {
        self.pending + self.in_progress + self.completed + self.cancelled
    }

    /// Closed items — `completed` plus `cancelled`, the header's numerator.
    pub fn settled(&self) -> usize {
        self.completed + self.cancelled
    }

    /// Anything the model still owes: pending + in_progress.
    pub fn open(&self) -> usize {
        self.pending + self.in_progress
    }
}

pub fn stats(state: &TodoState) -> TodoStats {
    let mut s = TodoStats::default();
    for t in state.todo_items() {
        match t.status {
            TodoStatus::Pending => s.pending += 1,
            TodoStatus::InProgress => s.in_progress += 1,
            TodoStatus::Completed => s.completed += 1,
            TodoStatus::Cancelled => s.cancelled += 1,
        }
    }
    s
}

/// The list plus a one-line directive about what the state implies. Returned
/// verbatim as the `todo_write` result, so the model sees the discipline right
/// where it just wrote — the cheapest place to correct a drifting list.
pub(crate) fn summarize_todo_state(state: &TodoState) -> String {
    if state.is_empty() {
        return "No tasks currently tracked.".into();
    }
    let mut out = String::new();
    for (id, t) in state.todo_items_with_ids() {
        writeln!(&mut out, "- {} {id}: {}", t.status.tag(), t.content).ok();
    }
    let s = stats(state);
    writeln!(
        &mut out,
        "\n{}/{} done ({} in_progress, {} pending).",
        s.settled(),
        s.total(),
        s.in_progress,
        s.pending
    )
    .ok();
    if let Some(hint) = state_hint(&s) {
        out.push_str(hint);
        out.push('\n');
    }
    out
}

/// State-derived nudge appended to the tool result. `None` when the list is in
/// the shape we want (exactly one item in flight).
fn state_hint(s: &TodoStats) -> Option<&'static str> {
    if s.open() == 0 {
        return Some(
            "All tracked work is closed. Do not re-open the list for work you already finished.",
        );
    }
    if s.in_progress == 0 {
        return Some(
            "Nothing is in_progress. Mark the next item in_progress and start it in this same turn — do not answer the user with pending work left.",
        );
    }
    if s.in_progress > 1 {
        return Some(
            "More than one item is in_progress. Keep exactly one in flight; move the rest back to pending.",
        );
    }
    None
}

/// Grok `TODO_GATE_IN_FLIGHT`: the sentinel the turn-end reminder leads with,
/// mirroring `GOAL_CONTINUATION_SENTINEL`.
pub const TODO_GATE_SENTINEL: &str = "Todos NOT complete — continue working. Next step:";

/// Grok `evaluate_todo_gate`: the model ended a turn with text while items are
/// still open. `backing_tasks` is the number of live background jobs /
/// subagents — the first that many `in_progress` items (insertion order) count
/// as backed and are not nagged about, so "kick off a job, report back" stays a
/// legitimate way to end a turn.
///
/// `None` means the turn may end.
pub fn gate_reminder(state: &TodoState, backing_tasks: usize) -> Option<String> {
    let mut pending: Vec<&str> = Vec::new();
    let mut in_progress: Vec<&str> = Vec::new();
    for t in state.todo_items() {
        match t.status {
            TodoStatus::Pending => pending.push(&t.content),
            TodoStatus::InProgress => in_progress.push(&t.content),
            TodoStatus::Completed | TodoStatus::Cancelled => {}
        }
    }
    let unbacked = in_progress.split_off(in_progress.len().min(backing_tasks));
    if pending.is_empty() && unbacked.is_empty() {
        return None;
    }
    let mut body = String::from("<system-reminder>\n<todo-state>\n");
    for c in &unbacked {
        writeln!(&mut body, "in_progress: {c}").ok();
    }
    for c in &pending {
        writeln!(&mut body, "pending: {c}").ok();
    }
    body.push_str("</todo-state>\n\n");
    body.push_str(TODO_GATE_SENTINEL);
    body.push('\n');
    body.push_str(
        "别停在这里向用户汇报。把做完的项用 todo_write 标 completed，把下一项标 in_progress，\
         然后在同一轮里直接开工。真被外部因素挡住（缺凭据、权限被拒、网络不通）就明说原因，\
         并把受影响的项标 cancelled。\n</system-reminder>",
    );
    Some(body)
}

/// Mid-turn nudge: the model has been running tools for a while without
/// touching the list, so completed items are probably still showing as
/// `in_progress` and newly-discovered work is missing.
pub fn stale_reminder(steps: usize) -> String {
    format!(
        "<system-reminder>\n\
         todo 列表已经 {steps} 步没有更新。做完的项立刻 todo_write 标 completed，\
         下一项标 in_progress；探索中发现范围变了就同步增删列表项。\
         不要为此单独回一轮，把 todo_write 和下一步工作放在同一轮里。\n\
         </system-reminder>"
    )
}

pub type TodoId = String;

// diff from acp: default to medium
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoPriority {
    High,
    #[default]
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub const fn tag(&self) -> &str {
        match self {
            Self::Pending => "[pending]",
            Self::InProgress => "[in_progress]",
            Self::Completed => "[completed]",
            Self::Cancelled => "[cancelled]",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub content: String,
    #[serde(default)]
    pub priority: TodoPriority,
    pub status: TodoStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodoState {
    todos: IndexMap<TodoId, TodoItem>,
}

impl TodoState {
    pub fn push(&mut self, id: TodoId, todo: TodoItem) {
        self.todos.insert(id, todo);
    }

    pub fn clear(&mut self) {
        self.todos.clear();
    }

    pub fn update(
        &mut self,
        id: &TodoId,
        content: Option<&str>,
        status: Option<TodoStatus>,
    ) -> bool {
        let Some(todo) = self.todos.get_mut(id) else {
            return false;
        };
        if let Some(content) = content {
            if !content.is_empty() {
                todo.content = content.into();
            }
        }
        if let Some(status) = status {
            todo.status = status;
        }
        true
    }

    pub fn todo_items(&self) -> impl Iterator<Item = &TodoItem> + '_ {
        self.todos.values()
    }

    pub fn todo_items_with_ids(&self) -> impl Iterator<Item = (&TodoId, &TodoItem)> + '_ {
        self.todos.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.todos.is_empty()
    }

    pub fn has_id(&self, id: &str) -> bool {
        self.todos.contains_key(id)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoUpdate {
    pub id: String,
    pub content: Option<String>,
    pub status: Option<TodoStatus>,
}

impl TodoUpdate {
    /// True when the update carries no meaningful content (None or empty string).
    fn has_no_content(&self) -> bool {
        self.content.as_deref().is_none_or(str::is_empty)
    }
}

const fn default_merge() -> bool {
    true
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoWriteInput {
    /// When true (the default), merge the provided todos into the existing
    /// list by id. When explicitly set to false, the provided todos replace
    /// the existing list entirely.
    #[serde(default = "default_merge")]
    pub merge: bool,
    pub todos: Vec<TodoUpdate>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upd(id: &str, content: Option<&str>, status: Option<TodoStatus>) -> TodoUpdate {
        TodoUpdate {
            id: id.into(),
            content: content.map(str::to_owned),
            status,
        }
    }

    #[test]
    fn replace_mode_creates_items() {
        let mut state = TodoState::default();
        apply_replace(
            &mut state,
            &[
                upd("1", Some("Task A"), Some(TodoStatus::Pending)),
                upd("2", Some("Task B"), Some(TodoStatus::InProgress)),
            ],
        )
        .unwrap();
        assert_eq!(state.todo_items().count(), 2);
        let summary = summarize_todo_state(&state);
        assert!(summary.contains("Task A"));
        assert!(summary.contains("Task B"));
    }

    #[test]
    fn replace_clears_previous_state() {
        let mut state = TodoState::default();
        apply_replace(
            &mut state,
            &[upd("old", Some("Old task"), Some(TodoStatus::Completed))],
        )
        .unwrap();
        apply_replace(
            &mut state,
            &[upd("new", Some("New task"), Some(TodoStatus::Pending))],
        )
        .unwrap();
        assert_eq!(state.todo_items().count(), 1);
        assert!(state.has_id("new"));
        assert!(!state.has_id("old"));
    }

    #[test]
    fn merge_updates_status_without_content() {
        let mut state = TodoState::default();
        apply_replace(
            &mut state,
            &[upd("1", Some("Task A"), Some(TodoStatus::Pending))],
        )
        .unwrap();
        apply_merge(&mut state, &[upd("1", None, Some(TodoStatus::Completed))]).unwrap();
        let item = state.todo_items().next().unwrap();
        assert_eq!(item.content, "Task A");
        assert_eq!(item.status, TodoStatus::Completed);
    }

    #[test]
    fn duplicate_ids_error() {
        let err =
            validate_no_duplicate_ids(&[upd("1", Some("a"), None), upd("1", Some("b"), None)]);
        assert!(matches!(err, Err(TodoError::DuplicateTodoID(id)) if id == "1"));
    }

    fn seeded() -> TodoState {
        let mut state = TodoState::default();
        apply_replace(
            &mut state,
            &[
                upd(
                    "explore",
                    Some("摸清 todo 现状"),
                    Some(TodoStatus::InProgress),
                ),
                upd("ui", Some("折叠 + 美化"), Some(TodoStatus::Pending)),
                upd("docs", Some("同步文档"), Some(TodoStatus::Pending)),
            ],
        )
        .unwrap();
        state
    }

    /// Regression (Grok `missing_merge_flag_auto_upgrades_when_status_only`):
    /// a status-only write that forgot `merge: true` must not replace every
    /// content string with its id.
    #[test]
    fn missing_merge_flag_auto_upgrades_when_status_only() {
        let state = seeded();
        let input = TodoWriteInput {
            merge: false,
            todos: vec![
                upd("explore", None, Some(TodoStatus::Completed)),
                upd("ui", None, Some(TodoStatus::InProgress)),
            ],
        };
        assert!(effective_merge(&state, &input));

        let mut state = state;
        apply_merge(&mut state, &input.todos).unwrap();
        let by_id: Vec<_> = state.todo_items_with_ids().collect();
        assert_eq!(by_id[0].1.content, "摸清 todo 现状");
        assert_eq!(by_id[0].1.status, TodoStatus::Completed);
        assert_eq!(by_id[2].1.status, TodoStatus::Pending, "第三项不该被清掉");
    }

    #[test]
    fn replace_with_content_stays_a_replace() {
        let state = seeded();
        let input = TodoWriteInput {
            merge: false,
            todos: vec![upd("fresh", Some("全新计划"), Some(TodoStatus::Pending))],
        };
        assert!(!effective_merge(&state, &input));
    }

    #[test]
    fn summary_carries_counts_and_next_step_hint() {
        let mut state = seeded();
        let summary = summarize_todo_state(&state);
        assert!(
            summary.contains("0/3 done (1 in_progress, 2 pending)"),
            "{summary}"
        );
        assert!(!summary.contains("Nothing is in_progress"), "{summary}");

        apply_merge(
            &mut state,
            &[upd("explore", None, Some(TodoStatus::Completed))],
        )
        .unwrap();
        let summary = summarize_todo_state(&state);
        assert!(summary.contains("1/3 done"), "{summary}");
        assert!(summary.contains("Nothing is in_progress"), "{summary}");
    }

    #[test]
    fn summary_flags_two_items_in_flight() {
        let mut state = seeded();
        apply_merge(&mut state, &[upd("ui", None, Some(TodoStatus::InProgress))]).unwrap();
        let summary = summarize_todo_state(&state);
        assert!(
            summary.contains("More than one item is in_progress"),
            "{summary}"
        );
    }

    #[test]
    fn gate_lists_open_items_and_clears_when_done() {
        let mut state = seeded();
        let reminder = gate_reminder(&state, 0).expect("open items must gate the turn");
        assert!(reminder.contains(TODO_GATE_SENTINEL), "{reminder}");
        assert!(
            reminder.contains("in_progress: 摸清 todo 现状"),
            "{reminder}"
        );
        assert!(reminder.contains("pending: 同步文档"), "{reminder}");

        for id in ["explore", "ui", "docs"] {
            apply_merge(&mut state, &[upd(id, None, Some(TodoStatus::Completed))]).unwrap();
        }
        assert!(gate_reminder(&state, 0).is_none());
    }

    /// Grok's backed/unbacked partition: an `in_progress` item covered by a
    /// live background task is a legitimate reason to end the turn.
    #[test]
    fn gate_ignores_in_progress_backed_by_a_live_task() {
        let mut state = TodoState::default();
        apply_replace(
            &mut state,
            &[upd(
                "build",
                Some("等后台构建"),
                Some(TodoStatus::InProgress),
            )],
        )
        .unwrap();
        assert!(gate_reminder(&state, 1).is_none());
        assert!(gate_reminder(&state, 0).is_some());
    }

    #[test]
    fn gate_stays_quiet_on_an_empty_list() {
        assert!(gate_reminder(&TodoState::default(), 0).is_none());
    }

    #[test]
    fn stats_counts_every_status() {
        let mut state = seeded();
        apply_merge(
            &mut state,
            &[upd("docs", None, Some(TodoStatus::Cancelled))],
        )
        .unwrap();
        let s = stats(&state);
        assert_eq!((s.in_progress, s.pending, s.cancelled), (1, 1, 1));
        assert_eq!((s.total(), s.settled(), s.open()), (3, 1, 2));
    }
}
