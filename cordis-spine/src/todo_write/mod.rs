//! `todo_write` over a named `"todos"` list. Copied merge/replace logic from Grok.

mod logic;

use std::sync::Mutex;

use cordis::{Inject, Plugin, plugin};

use crate::names::{TODOS, TOOLS};
use crate::tools::{ToolBody, Tools, own_registered, tool_result};
use crate::types::{ToolCall, ToolResult, ToolSpec};

pub use logic::{TodoItem, TodoState, TodoStatus, TodoWriteInput};

/// Named `"todos"` service. TUI live-looks the list; do not capture the Arc.
pub struct Todos {
    state: Mutex<TodoState>,
}

impl Todos {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(TodoState::default()),
        }
    }

    pub fn snapshot(&self) -> Vec<(String, TodoItem)> {
        self.state
            .lock()
            .unwrap()
            .todo_items_with_ids()
            .map(|(id, item)| (id.clone(), item.clone()))
            .collect()
    }

    pub fn apply(&self, input: TodoWriteInput) -> Result<String, logic::TodoError> {
        let mut state = self.state.lock().unwrap();
        logic::validate_no_duplicate_ids(&input.todos)?;
        if input.merge {
            logic::apply_merge(&mut state, &input.todos)?;
        } else {
            logic::apply_replace(&mut state, &input.todos)?;
        }
        Ok(logic::summarize_todo_state(&state))
    }
}

impl Default for Todos {
    fn default() -> Self {
        Self::new()
    }
}

const PARAMS: &str = r#"{"type":"object","properties":{"merge":{"type":"boolean","description":"When true (default), merge by id. When false, replace the list."},"todos":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"content":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed","cancelled"]}},"required":["id"]}}},"required":["todos"]}"#;

pub fn tool_todo() -> Plugin {
    plugin("tool-todo", Inject::from([TOOLS]), |ctx, _: &()| {
        ctx.provide(TODOS, Todos::new())?;
        let tools = ctx.require::<Tools>(TOOLS)?;
        let body: ToolBody = {
            let ctx = ctx.clone();
            std::sync::Arc::new(move |call| {
                let ctx = ctx.clone();
                Box::pin(async move { write_todos(&ctx, call) })
            })
        };
        own_registered(
            ctx,
            vec![tools.register(
                ToolSpec {
                    name: "todo_write".into(),
                    description: "Create and manage a structured task list. The user sees this list live — it is your primary way to show progress.\n\nUse for any task with 3+ steps. Skip for trivial single-step work.".into(),
                    parameters_json: PARAMS.into(),
                },
                body,
            )?],
        )?;
        Ok(None)
    })
}

fn write_todos(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let input: TodoWriteInput = match serde_json::from_str(&call.arguments) {
        Ok(v) => v,
        Err(e) => return tool_result(call, format!("Error: invalid todo_write args ({e})")),
    };
    let Some(todos) = ctx.get::<Todos>(TODOS) else {
        return tool_result(call, "Error: todos is not mounted");
    };
    match todos.apply(input) {
        Ok(summary) => tool_result(call, summary),
        Err(e) => tool_result(call, e.to_string()),
    }
}
