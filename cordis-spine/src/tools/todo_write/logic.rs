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

    /// The write would leave more than one item `in_progress`. Nothing was
    /// applied.
    #[error(
        "Error: only one item may be in_progress at a time, but this write would leave {0} in_progress. \
         Nothing was changed. First mark the previous item completed, cancelled or awaiting_user \
         (in the same todo_write call is fine), then mark the next one in_progress."
    )]
    TooManyInProgress(String),
}

/// `todo_write` keeps exactly one item in flight. Checked on the state the
/// write would produce, so a merge that starts `b` while `a` is still
/// `in_progress` is rejected as well as a list that sends two at once.
pub(crate) fn validate_single_in_progress(state: &TodoState) -> Result<(), TodoError> {
    let in_progress: Vec<&str> = state
        .todo_items_with_ids()
        .filter(|(_, t)| t.status == TodoStatus::InProgress)
        .map(|(id, _)| id.as_str())
        .collect();
    if in_progress.len() > 1 {
        return Err(TodoError::TooManyInProgress(in_progress.join(", ")));
    }
    Ok(())
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

/// Per-status counts. Drives the tool-result nudge, the turn-end bookkeeping
/// and the TUI header.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TodoStats {
    pub pending: usize,
    pub in_progress: usize,
    pub completed: usize,
    pub cancelled: usize,
    /// Parked on the user's answer. Not settled, but not something the model
    /// owes either: every todo reminder skips it.
    pub awaiting_user: usize,
}

impl TodoStats {
    pub fn total(&self) -> usize {
        self.pending + self.in_progress + self.completed + self.cancelled + self.awaiting_user
    }

    /// Closed items — `completed` plus `cancelled`, the header's numerator.
    pub fn settled(&self) -> usize {
        self.completed + self.cancelled
    }

    /// Anything the model still owes: pending + in_progress. `awaiting_user`
    /// is not counted — it waits on the user, not on the model.
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
            TodoStatus::AwaitingUser => s.awaiting_user += 1,
        }
    }
    s
}

/// Tool-result line for a list with nothing left and at least one item
/// `completed` (the rest `cancelled`).
pub const TODO_ALL_DONE: &str = "清单已全部完成。";

/// Tool-result line for a list where every item was `cancelled`: nothing was
/// done, so「完成」would be a lie.
pub const TODO_ALL_CLOSED: &str = "清单已全部关闭。";

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
    let awaiting = if s.awaiting_user > 0 {
        format!(", {} awaiting_user", s.awaiting_user)
    } else {
        String::new()
    };
    writeln!(
        &mut out,
        "\n{}/{} done ({} in_progress, {} pending{awaiting}).",
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
/// the shape we want (exactly one item in flight). Asks for an honest list, not
/// for nonstop work: stopping to report, to discuss or to wait is fine as long
/// as the statuses say so.
fn state_hint(s: &TodoStats) -> Option<&'static str> {
    if s.total() > 0 && s.settled() == s.total() {
        if s.completed == 0 {
            return Some("清单已全部关闭。Every item was cancelled. Start a new list (merge:false) if new work comes up.");
        }
        return Some("清单已全部完成。Do not re-open the list for work you already finished.");
    }
    if s.open() == 0 {
        return Some(
            "Only awaiting_user items are left: wait for the user. When the user answers, switch them back to pending / in_progress, or mark them completed.",
        );
    }
    if s.in_progress == 0 {
        return Some(
            "Nothing is in_progress. Mark an item in_progress when you actually start it. If you are stopping here (to report, to discuss the plan, or to wait for the user), that is fine — keep the statuses honest instead of working on just to empty the list.",
        );
    }
    None
}

/// Leads the turn-end bookkeeping reminder. Rhai hooks and tests look for it.
pub const TODO_BOOKKEEPING_SENTINEL: &str =
    "Todo bookkeeping only — update statuses, do not start new work.";

/// `pending` / unbacked `in_progress` contents, in list order. `backing_tasks`
/// = live background jobs + running subagents: the first that many
/// `in_progress` items count as backed ("kick off a job, report back" is a
/// legitimate way to end a turn). `awaiting_user` is never included.
fn owed_items(state: &TodoState, backing_tasks: usize) -> (Vec<&str>, Vec<&str>) {
    let mut pending: Vec<&str> = Vec::new();
    let mut in_progress: Vec<&str> = Vec::new();
    for t in state.todo_items() {
        match t.status {
            TodoStatus::Pending => pending.push(&t.content),
            TodoStatus::InProgress => in_progress.push(&t.content),
            TodoStatus::Completed | TodoStatus::Cancelled | TodoStatus::AwaitingUser => {}
        }
    }
    let unbacked = in_progress.split_off(in_progress.len().min(backing_tasks));
    (unbacked, pending)
}

fn todo_state_block(out: &mut String, in_progress: &[&str], pending: &[&str]) {
    out.push_str("<todo-state>\n");
    for c in in_progress {
        writeln!(out, "in_progress: {c}").ok();
    }
    for c in pending {
        writeln!(out, "pending: {c}").ok();
    }
    out.push_str("</todo-state>\n");
}

/// Turn-end bookkeeping: the model ended a turn with text while items are
/// still owed. The loop gives it **one** step that only offers `todo_write`,
/// then ends the turn — this settles "forgot to tick it off" without pushing
/// the model into work the user did not ask for (e.g. after "let's discuss
/// the plan first").
///
/// `None` means there is nothing to settle (empty list, everything closed,
/// only `awaiting_user` left, or every `in_progress` backed by a live task).
pub fn bookkeeping_reminder(state: &TodoState, backing_tasks: usize) -> Option<String> {
    let (in_progress, pending) = owed_items(state, backing_tasks);
    if pending.is_empty() && in_progress.is_empty() {
        return None;
    }
    let mut body = String::from("<system-reminder>\n");
    todo_state_block(&mut body, &in_progress, &pending);
    body.push('\n');
    body.push_str(TODO_BOOKKEEPING_SENTINEL);
    body.push('\n');
    body.push_str(
        "按实际进度更新上面这些项的状态：做完的标 completed，在等用户回复或确认的标 awaiting_user，\
         确定不做的标 cancelled，还没开始的保持 pending。这一步只能调用 todo_write：\
         不要开始新的工作，也不用再回复用户；记完账这一轮就结束。\n</system-reminder>",
    );
    Some(body)
}

/// `正在做「a」；下一项「b」` for the current list, or `None` when nothing is
/// owed. `awaiting_user` items are not "next".
fn focus_line(state: &TodoState) -> Option<String> {
    let current = state
        .todo_items()
        .find(|t| t.status == TodoStatus::InProgress)
        .map(|t| t.content.as_str());
    let next = state
        .todo_items()
        .find(|t| t.status == TodoStatus::Pending)
        .map(|t| t.content.as_str());
    match (current, next) {
        (None, None) => None,
        (Some(c), Some(n)) => Some(format!("正在做「{c}」；下一项「{n}」。")),
        (Some(c), None) => Some(format!("正在做「{c}」；这是最后一项。")),
        (None, Some(n)) => Some(format!("没有进行中的项；下一项「{n}」。")),
    }
}

/// Mid-turn, right after the list changed: where the model stands, worded as
/// a suggestion. `None` when nothing is owed.
pub fn focus_reminder(state: &TodoState) -> Option<String> {
    let focus = focus_line(state)?;
    Some(format!(
        "<system-reminder>\n当前清单：{focus}\n\
         （建议：开始一项前先标 in_progress，做完就标 completed。清单只是记录，用户的话优先。）\n\
         </system-reminder>"
    ))
}

/// Mid-turn nudge: the model has been running tools for a while without
/// touching the list, so completed items are probably still showing as
/// `in_progress` and newly-discovered work is missing.
pub fn stale_reminder(steps: usize, state: &TodoState) -> String {
    let focus = focus_line(state)
        .map(|f| format!("当前清单：{f}\n"))
        .unwrap_or_default();
    format!(
        "<system-reminder>\n\
         {focus}todo 列表已经 {steps} 步没有更新。做完的项用 todo_write 标 completed，\
         下一项标 in_progress；探索中发现范围变了就同步增删列表项。\
         不要为此单独回一轮，把 todo_write 和下一步工作放在同一轮里。\n\
         </system-reminder>"
    )
}

/// Mid-turn nudge right after a test / build command ran (see
/// [`is_test_or_build_command`]) while an item is `in_progress` and the list
/// did not move in that step: verification is usually the last thing before
/// "done".
pub fn verified_reminder(command: &str, in_progress: &str) -> String {
    let command: String = command.chars().take(80).collect();
    format!(
        "<system-reminder>\n刚跑完测试 / 构建（`{command}`），「{in_progress}」还是 in_progress。\
         这项做完了就用 todo_write 标 completed（再把下一项标 in_progress）；还没做完就不用理会这条。\n\
         </system-reminder>"
    )
}

/// Start of the next user turn: the previous turn left items owed (not
/// counting `awaiting_user`), and/or some are `awaiting_user` — the user's
/// new message may be the answer. Soft: the new message wins.
pub fn next_turn_reminder(state: &TodoState) -> Option<String> {
    let (in_progress, pending) = owed_items(state, 0);
    let awaiting: Vec<&str> = state
        .todo_items()
        .filter(|t| t.status == TodoStatus::AwaitingUser)
        .map(|t| t.content.as_str())
        .collect();
    let owed = in_progress.len() + pending.len();
    if owed == 0 && awaiting.is_empty() {
        return None;
    }
    let mut body = String::from("<system-reminder>\n");
    if owed > 0 {
        writeln!(&mut body, "上轮清单还剩 {owed} 项未完成：").ok();
        todo_state_block(&mut body, &in_progress, &pending);
        body.push_str(
            "用户的新消息优先：如果它改变了方向，先用 todo_write 更新或清空清单（不再做的标 cancelled，\
             或用 merge:false 换成新清单）；仍然相关再接着推进。\n",
        );
    }
    if !awaiting.is_empty() {
        writeln!(
            &mut body,
            "这几项在等用户（awaiting_user）：{}。看用户这条消息是否回答了它们：回答了就改回 pending / in_progress，或直接标 completed。",
            awaiting.iter().map(|c| format!("「{c}」")).collect::<Vec<_>>().join("、")
        )
        .ok();
    }
    body.push_str("</system-reminder>");
    Some(body)
}

/// Commands that count as "a test or build just ran" for
/// [`verified_reminder`]. A command segment (split on `&&`, `||`, `;`, `|`
/// and newlines, leading `VAR=value` assignments dropped) matches when it is
/// one of these words or starts with one of them followed by a space.
pub const TEST_BUILD_COMMANDS: &[&str] = &[
    "cargo test",
    "cargo nextest",
    "cargo build",
    "cargo check",
    "cargo clippy",
    "npm test",
    "npm run test",
    "npm run build",
    "pnpm test",
    "pnpm build",
    "pnpm run test",
    "pnpm run build",
    "yarn test",
    "yarn build",
    "npx vitest",
    "npx jest",
    "npx tsc",
    "vitest",
    "jest",
    "tsc",
    "pytest",
    "python -m pytest",
    "python3 -m pytest",
    "go test",
    "go build",
    "make",
    "cmake --build",
    "ctest",
    "mvn",
    "gradle",
    "./gradlew",
    "swift build",
    "swift test",
    "xcodebuild",
];

/// See [`TEST_BUILD_COMMANDS`].
pub fn is_test_or_build_command(command: &str) -> bool {
    command
        .split(['\n', ';', '|', '&'])
        .map(|segment| {
            let words: Vec<&str> = segment
                .split_whitespace()
                .skip_while(|w| {
                    w.split_once('=')
                        .is_some_and(|(k, _)| !k.is_empty() && !k.starts_with('-'))
                })
                .collect();
            words.join(" ")
        })
        .any(|segment| {
            TEST_BUILD_COMMANDS.iter().any(|cmd| {
                segment == *cmd
                    || segment
                        .strip_prefix(cmd)
                        .is_some_and(|rest| rest.starts_with(' '))
            })
        })
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

/// On disk this is the `status` string inside `todo_write` arguments and the
/// `[status]` tag in its result (both in `chat_history.jsonl`).
/// `awaiting_user` is newer than the other four: files without it read as
/// before. 旧版显示成和 pending 一样（GUI 是空心圆，TUI 是 `□`）—— an older
/// build parses an unknown status as `pending`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
    /// Waiting on the user's answer or approval. Not owed by the model, not
    /// settled either; todo reminders skip it.
    AwaitingUser,
}

impl TodoStatus {
    pub const fn tag(&self) -> &str {
        match self {
            Self::Pending => "[pending]",
            Self::InProgress => "[in_progress]",
            Self::Completed => "[completed]",
            Self::Cancelled => "[cancelled]",
            Self::AwaitingUser => "[awaiting_user]",
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
    fn a_write_that_leaves_two_in_progress_fails_validation() {
        let mut state = seeded();
        apply_merge(&mut state, &[upd("ui", None, Some(TodoStatus::InProgress))]).unwrap();
        let err = validate_single_in_progress(&state).unwrap_err();
        assert!(
            matches!(&err, TodoError::TooManyInProgress(ids) if ids == "explore, ui"),
            "{err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.starts_with("Error: only one item may be in_progress"),
            "{msg}"
        );
        assert!(
            msg.contains("completed, cancelled or awaiting_user"),
            "{msg}"
        );
    }

    #[test]
    fn every_item_completed_says_the_list_is_done() {
        let mut state = seeded();
        for id in ["explore", "ui", "docs"] {
            apply_merge(&mut state, &[upd(id, None, Some(TodoStatus::Completed))]).unwrap();
        }
        let summary = summarize_todo_state(&state);
        assert!(
            summary.contains("3/3 done (0 in_progress, 0 pending)."),
            "{summary}"
        );
        assert!(summary.contains(TODO_ALL_DONE), "{summary}");
        assert!(summary.contains("清单已全部完成"), "{summary}");
    }

    #[test]
    fn every_item_cancelled_says_the_list_is_closed_not_done() {
        let mut state = seeded();
        for id in ["explore", "ui", "docs"] {
            apply_merge(&mut state, &[upd(id, None, Some(TodoStatus::Cancelled))]).unwrap();
        }
        let summary = summarize_todo_state(&state);
        assert!(summary.contains(TODO_ALL_CLOSED), "{summary}");
        assert!(!summary.contains(TODO_ALL_DONE), "{summary}");
    }

    #[test]
    fn the_hint_asks_for_honest_statuses_not_nonstop_work() {
        let mut state = seeded();
        apply_merge(
            &mut state,
            &[upd("explore", None, Some(TodoStatus::Completed))],
        )
        .unwrap();
        let summary = summarize_todo_state(&state);
        assert!(summary.contains("keep the statuses honest"), "{summary}");
        assert!(!summary.contains("do not answer the user"), "{summary}");
        assert!(!summary.contains(TODO_ALL_DONE), "{summary}");
    }

    #[test]
    fn bookkeeping_lists_owed_items_and_clears_when_done() {
        let mut state = seeded();
        let reminder =
            bookkeeping_reminder(&state, 0).expect("owed items must ask for bookkeeping");
        assert!(reminder.contains(TODO_BOOKKEEPING_SENTINEL), "{reminder}");
        assert!(
            reminder.contains("in_progress: 摸清 todo 现状"),
            "{reminder}"
        );
        assert!(reminder.contains("pending: 同步文档"), "{reminder}");
        assert!(reminder.contains("这一步只能调用 todo_write"), "{reminder}");
        assert!(reminder.contains("不要开始新的工作"), "{reminder}");
        assert!(!reminder.contains("continue working"), "{reminder}");

        for id in ["explore", "ui", "docs"] {
            apply_merge(&mut state, &[upd(id, None, Some(TodoStatus::Completed))]).unwrap();
        }
        assert!(bookkeeping_reminder(&state, 0).is_none());
    }

    #[test]
    fn awaiting_user_items_are_not_owed() {
        let mut state = seeded();
        for id in ["explore", "ui", "docs"] {
            apply_merge(&mut state, &[upd(id, None, Some(TodoStatus::AwaitingUser))]).unwrap();
        }
        assert_eq!(bookkeeping_reminder(&state, 0), None);
        assert_eq!(focus_reminder(&state), None);
        let s = stats(&state);
        assert_eq!(
            (s.awaiting_user, s.open(), s.settled(), s.total()),
            (3, 0, 0, 3)
        );
        let summary = summarize_todo_state(&state);
        assert!(
            summary.contains("- [awaiting_user] ui: 折叠 + 美化"),
            "{summary}"
        );
        assert!(
            summary.contains("0/3 done (0 in_progress, 0 pending, 3 awaiting_user)."),
            "{summary}"
        );
        assert!(
            summary.contains("Only awaiting_user items are left"),
            "{summary}"
        );
    }

    /// Grok's backed/unbacked partition: an `in_progress` item covered by a
    /// live background task is a legitimate reason to end the turn.
    #[test]
    fn bookkeeping_ignores_in_progress_backed_by_a_live_task() {
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
        assert!(bookkeeping_reminder(&state, 1).is_none());
        assert!(bookkeeping_reminder(&state, 0).is_some());
    }

    #[test]
    fn bookkeeping_stays_quiet_on_an_empty_list() {
        assert!(bookkeeping_reminder(&TodoState::default(), 0).is_none());
        assert!(next_turn_reminder(&TodoState::default()).is_none());
    }

    #[test]
    fn focus_names_the_current_and_the_next_item() {
        let state = seeded();
        assert_eq!(
            focus_reminder(&state).unwrap(),
            "<system-reminder>\n当前清单：正在做「摸清 todo 现状」；下一项「折叠 + 美化」。\n\
             （建议：开始一项前先标 in_progress，做完就标 completed。清单只是记录，用户的话优先。）\n\
             </system-reminder>"
        );
    }

    #[test]
    fn next_turn_reminder_counts_owed_items_and_names_awaiting_ones() {
        let mut state = seeded();
        apply_merge(
            &mut state,
            &[upd("docs", None, Some(TodoStatus::AwaitingUser))],
        )
        .unwrap();
        let body = next_turn_reminder(&state).unwrap();
        assert!(body.contains("上轮清单还剩 2 项未完成"), "{body}");
        assert!(
            body.contains("in_progress: 摸清 todo 现状\npending: 折叠 + 美化\n"),
            "{body}"
        );
        assert!(body.contains("用户的新消息优先"), "{body}");
        assert!(
            body.contains("在等用户（awaiting_user）：「同步文档」"),
            "{body}"
        );
        assert!(body.contains("改回 pending / in_progress"), "{body}");
    }

    #[test]
    fn test_and_build_commands_are_recognised() {
        for yes in [
            "cargo test -p cordis-spine",
            "cd dock && cargo build --release",
            "RUST_LOG=debug cargo nextest run",
            "npm run build 2>&1 | tail -20",
            "pytest -q tests/",
            "python -m pytest",
            "make",
            "go test ./...",
        ] {
            assert!(is_test_or_build_command(yes), "{yes}");
        }
        for no in [
            "cargo fmt",
            "ls -la",
            "git status && git diff",
            "makeself foo",
            "echo cargo test",
            "rg 'cargo test' docs",
        ] {
            assert!(!is_test_or_build_command(no), "{no}");
        }
    }

    /// `awaiting_user` is a new on-disk status string. It must round-trip, and
    /// rows written before it existed must still read.
    #[test]
    fn awaiting_user_survives_a_write_and_read_back() {
        let item = TodoItem {
            content: "等用户确认方案".into(),
            priority: TodoPriority::default(),
            status: TodoStatus::AwaitingUser,
            meta: None,
        };
        let json = serde_json::to_string(&item).unwrap();
        assert_eq!(
            json,
            r#"{"content":"等用户确认方案","priority":"medium","status":"awaiting_user"}"#
        );
        let back: TodoItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, TodoStatus::AwaitingUser);

        let input: TodoWriteInput =
            serde_json::from_str(r#"{"todos":[{"id":"a","status":"awaiting_user"}]}"#).unwrap();
        assert_eq!(input.todos[0].status, Some(TodoStatus::AwaitingUser));
    }

    #[test]
    fn the_old_format_still_reads() {
        let old = r#"{"merge":false,"todos":[
            {"id":"a","content":"读代码","status":"in_progress"},
            {"id":"b","content":"改实现","status":"pending"},
            {"id":"c","content":"写测试","status":"completed"},
            {"id":"d","content":"发版","status":"cancelled"}]}"#;
        let input: TodoWriteInput = serde_json::from_str(old).unwrap();
        let statuses: Vec<_> = input.todos.iter().map(|t| t.status.unwrap()).collect();
        assert_eq!(
            statuses,
            [
                TodoStatus::InProgress,
                TodoStatus::Pending,
                TodoStatus::Completed,
                TodoStatus::Cancelled
            ]
        );
        let item: TodoItem =
            serde_json::from_str(r#"{"content":"x","status":"completed"}"#).unwrap();
        assert_eq!(item.status, TodoStatus::Completed);
    }

    #[test]
    fn stats_counts_every_status() {
        let mut state = seeded();
        apply_merge(
            &mut state,
            &[upd("docs", None, Some(TodoStatus::Cancelled))],
        )
        .unwrap();
        apply_merge(
            &mut state,
            &[upd("ui", None, Some(TodoStatus::AwaitingUser))],
        )
        .unwrap();
        let s = stats(&state);
        assert_eq!(
            (s.in_progress, s.pending, s.cancelled, s.awaiting_user),
            (1, 0, 1, 1)
        );
        assert_eq!((s.total(), s.settled(), s.open()), (3, 1, 1));
    }
}
