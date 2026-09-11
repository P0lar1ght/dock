//! `todo_write` over a named `"todos"` list. Copied merge/replace logic from Grok.

mod logic;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Inject, Plugin};

use crate::jobs::Jobs;
use crate::names::{JOBS, PRE_STEP, STEP_START, SUBAGENTS, TODOS, TOOLS, TURN_END};
use crate::task::Subagents;
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{
    PreStep, StepStart, ToolCall, ToolResult, ToolSpec, TurnEnd, ORDER_STEP_START_TODO,
    ORDER_TURN_END_TODO,
};

/// Grok `TodoGateConfig::max_fires_per_prompt`. Bounds the extra inference one
/// user prompt can be forced into when the model stops with open todos.
const MAX_TODO_GATE_FIRES: usize = 2;
/// Sampling steps without a `todo_write` before the mid-turn nudge fires
/// (Grok `TodoNudgeConfig::turns_since_todo_write`, which ships unimplemented).
const TODO_NUDGE_AFTER_STEPS: usize = 6;
/// Nudges per turn. Two reminders are a hint; five are noise.
const MAX_TODO_NUDGES: usize = 3;

pub use logic::{
    stale_reminder, TodoItem, TodoState, TodoStats, TodoStatus, TodoWriteInput, TODO_GATE_SENTINEL,
};

/// Named `"todos"` service. TUI live-looks the list; do not capture the Arc.
pub struct Todos {
    state: Mutex<TodoState>,
    /// Bumped on every applied write. [`TodoWatch`] compares it against the
    /// value it saw last step to tell "the model is keeping the list current"
    /// from "the model forgot the list exists".
    revision: AtomicU64,
}

impl Todos {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(TodoState::default()),
            revision: AtomicU64::new(0),
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

    pub fn stats(&self) -> TodoStats {
        logic::stats(&self.state.lock().unwrap())
    }

    /// Write counter — see [`Todos::revision`].
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    /// Turn-end gate body, or `None` when nothing is left to nag about.
    /// `backing_tasks` = live background jobs + running subagents.
    pub fn gate_reminder(&self, backing_tasks: usize) -> Option<String> {
        logic::gate_reminder(&self.state.lock().unwrap(), backing_tasks)
    }

    pub fn apply(&self, input: TodoWriteInput) -> Result<String, logic::TodoError> {
        let mut state = self.state.lock().unwrap();
        logic::validate_no_duplicate_ids(&input.todos)?;
        if logic::effective_merge(&state, &input) {
            logic::apply_merge(&mut state, &input.todos)?;
        } else {
            logic::apply_replace(&mut state, &input.todos)?;
        }
        self.revision.fetch_add(1, Ordering::Relaxed);
        Ok(logic::summarize_todo_state(&state))
    }
}

impl Default for Todos {
    fn default() -> Self {
        Self::new()
    }
}

/// Grok keeps the `todo_write` blurb to two lines and puts the discipline in a
/// `<task_completion_discipline>` prompt block. Dock has no such block (the
/// base prompt stays identity-only for prefix cache), so the rules live here —
/// the description is the one place the model re-reads every step.
const TODO_WRITE_DESC: &str = "Create and manage a structured task list. The user sees this list live — it is your primary way to show progress.\n\n\
Use for any task with 3+ steps, or as soon as exploration shows the work is bigger than it looked. Skip it for trivial single-step work.\n\n\
Rules:\n\
- Write the list BEFORE starting the work, not after finishing it.\n\
- Exactly one item is `in_progress` at a time. Mark it before you touch anything for that item.\n\
- Mark an item `completed` in the same turn you finish it — never batch completions at the end, and never mark something completed while its verification (tests, build, review) is still pending.\n\
- When exploration changes the plan, update the list in the same turn you learn it: add the steps you discovered, drop the ones that turned out unnecessary (`cancelled`), re-word items that were wrong. A stale list is worse than no list.\n\
- Do not end a turn with pending items and no tool call. Either advance the next item, or state the external blocker and mark the affected items `cancelled`.\n\n\
Send only the items you are changing (`merge` defaults to true); id + status is enough to flip an existing item.";

const PARAMS: &str = r#"{"type":"object","properties":{"merge":{"type":"boolean","description":"When true (default), merge the given items into the list by id — send only what changed. When false, the given items replace the whole list."},"todos":{"type":"array","description":"Items to write. In merge mode, id + status is enough to flip an existing item.","items":{"type":"object","properties":{"id":{"type":"string","description":"Stable identifier, reused across calls to update the same item."},"content":{"type":"string","description":"Imperative one-liner describing the step. Optional when updating an existing item."},"status":{"type":"string","enum":["pending","in_progress","completed","cancelled"],"description":"pending | in_progress (keep exactly one) | completed (finished and verified) | cancelled (dropped or blocked)."}},"required":["id"]}}},"required":["todos"]}"#;

pub fn tool_todo() -> Plugin {
    plugin("tool-todo", Inject::from([TOOLS]), |ctx, _: &()| {
        ctx.provide(TODOS, Todos::new())?;
        // Gate quota. Plugin-local (not on `Todos`) — it is policy state of
        // this handler pair, not part of the named service. `Todos` outlives
        // the turn, so the counter needs an explicit per-prompt reset.
        let fires = Arc::new(AtomicUsize::new(0));
        let fires_pre = fires.clone();
        let _ = ctx.on_waterfall(PRE_STEP, move |step: PreStep, args| {
            let next = args.next::<PreStep>().unwrap_or(step);
            // Main session only: the quota belongs to the main session's
            // prompt, and a child starting a turn must not hand it a refill.
            if next.enter && next.is_main_session() {
                fires_pre.store(0, Ordering::Relaxed);
            }
            next
        });
        // Mid-turn watchdog. Same shape as the gate quota: plugin-local policy
        // state, rearmed from the payload (step 0) instead of by the loop.
        let watch = Arc::new(Mutex::new(TodoWatch::default()));
        let ctx_step = ctx.clone();
        let _ = ctx.on_waterfall(STEP_START, move |start: StepStart, args| {
            let mut next = args.next::<StepStart>().unwrap_or(start);
            // Main session only — see [`gate_applies`]. A child sharing the
            // parent's list must not be nagged about it, and must not disturb
            // the parent's counters while it runs alongside.
            if !next.is_main_session() {
                return next;
            }
            let Some(todos) = ctx_step.get::<Todos>(TODOS) else {
                return next;
            };
            let mut watch = watch.lock().unwrap();
            if next.step == 0 {
                *watch = TodoWatch::armed_at(todos.revision());
            }
            if let Some(body) = watch.stale_nudge(&todos) {
                next.remind(ORDER_STEP_START_TODO, body);
            }
            next
        });
        let ctx_end = ctx.clone();
        let _ = ctx.on_waterfall(TURN_END, move |end: TurnEnd, args| {
            let mut next = args.next::<TurnEnd>().unwrap_or(end);
            if !gate_applies(&next) || fires.load(Ordering::Relaxed) >= MAX_TODO_GATE_FIRES {
                return next;
            }
            let Some(todos) = ctx_end.get::<Todos>(TODOS) else {
                return next;
            };
            if let Some(body) = todos.gate_reminder(backing_tasks(&ctx_end)) {
                // Quota burns on the vote, not on the win. `ORDER_TURN_END_TODO`
                // is the lowest slot, so today a vote is a win; a future handler
                // that outranks it would silently eat this quota.
                fires.fetch_add(1, Ordering::Relaxed);
                next.keep_working(ORDER_TURN_END_TODO, body);
            }
            next
        });
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
                    description: TODO_WRITE_DESC.into(),
                    parameters_json: PARAMS.into(),
                },
                body,
            )?],
        )?;
        Ok(None)
    })
}

/// Mid-turn `todo_write` watchdog. Counts sampling steps since the list last
/// moved; nudges when the model is clearly working but not checking items off.
/// One turn's worth of state — `agent/step-start` step 0 rearms it.
#[derive(Default)]
struct TodoWatch {
    /// [`Todos::revision`] as of the last step we looked.
    seen_revision: Option<u64>,
    steps_since_write: usize,
    nudges: usize,
}

impl TodoWatch {
    fn armed_at(revision: u64) -> Self {
        Self {
            seen_revision: Some(revision),
            ..Self::default()
        }
    }

    /// Call once per sampling step. `Some(reminder)` when the list has gone
    /// stale for [`TODO_NUDGE_AFTER_STEPS`] steps with work still open.
    fn stale_nudge(&mut self, todos: &Todos) -> Option<String> {
        let revision = todos.revision();
        if self.seen_revision != Some(revision) {
            self.seen_revision = Some(revision);
            self.steps_since_write = 0;
            return None;
        }
        self.steps_since_write += 1;
        if self.nudges >= MAX_TODO_NUDGES || self.steps_since_write < TODO_NUDGE_AFTER_STEPS {
            return None;
        }
        if todos.stats().open() == 0 {
            return None;
        }
        self.nudges += 1;
        let steps = std::mem::take(&mut self.steps_since_write);
        Some(stale_reminder(steps))
    }
}

/// Whether this turn end is one the todo gate has any business in.
///
/// - Only a text ending: a blown step budget means the model is stuck in a tool
///   loop, and one more round of "advance your todos" will not unstick it.
/// - Not while the user has queued the next message — they steer.
/// - Main session only. `"todos"` is **not** isolated per subagent (the child
///   runner isolates `sessions` / `turn` / `agentPresets`), so a child would
///   otherwise be gated by its parent's list. The identity rides on the payload
///   because a waterfall handler cannot see the executing context.
fn gate_applies(end: &TurnEnd) -> bool {
    end.ended_with_text && !end.queued_followups && end.is_main_session()
}

/// Live background work that can legitimately back an `in_progress` item —
/// "kick off a job, report back" stays a valid way to end a turn.
fn backing_tasks(ctx: &Context) -> usize {
    let jobs = ctx
        .get::<Jobs>(JOBS)
        .map(|j| j.list().iter().filter(|j| !j.done).count())
        .unwrap_or(0);
    let agents = ctx
        .get::<Subagents>(SUBAGENTS)
        .map(|s| s.list().iter().filter(|a| a.running()).count())
        .unwrap_or(0);
    jobs + agents
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
