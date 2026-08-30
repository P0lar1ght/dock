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

pub(crate) fn summarize_todo_state(state: &TodoState) -> String {
    if state.is_empty() {
        "No tasks currently tracked.".into()
    } else {
        let mut out = String::new();
        for (id, t) in state.todo_items_with_ids() {
            writeln!(&mut out, "- {} {id}: {}", t.status.tag(), t.content).ok();
        }
        out
    }
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
}
