//! `todo_write` over a named `"todos"` list. Copied merge/replace logic from Grok.

mod logic;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Context, Inject, Plugin};

use crate::names::{
    JOBS, PLAN_MODE, PRE_STEP, SESSIONS, STEP_START, SUBAGENTS, TODOS, TOOLS, TOOLS_EXECUTE,
    TURN_END,
};
use crate::session::log::Sessions;
use crate::tools::jobs::Jobs;
use crate::tools::plan_mode::PlanMode;
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use crate::tools::task::Subagents;
use cordis_base::types::{
    LogEvent, PreStep, StepStart, ToolCall, ToolResult, ToolSpec, TurnEnd, ORDER_STEP_START_TODO,
    ORDER_TURN_END_TODO,
};

/// Turn-end bookkeeping steps per user prompt. One: the bookkeeping step ends
/// the turn, so a second one could only come from a later continuation of the
/// same prompt (goal / mailbox) — and one honest pass is enough.
const MAX_TODO_BOOKKEEPING_FIRES: usize = 1;
/// Sampling steps without a `todo_write` before the mid-turn nudge fires
/// (Grok `TodoNudgeConfig::turns_since_todo_write`, which ships unimplemented).
const TODO_NUDGE_AFTER_STEPS: usize = 6;
/// Stale / just-verified nudges per turn. Two reminders are a hint; five are noise.
const MAX_TODO_NUDGES: usize = 3;
/// The only tool the turn-end bookkeeping step offers.
const BOOKKEEPING_TOOLS: &[&str] = &["todo_write"];
/// Tools that hand the turn to the user. Calling one this turn means the model
/// already stopped to ask: no bookkeeping step on top of it.
const ASK_TOOLS: &[&str] = &["ask_user_question", "handoff_to_user"];

pub use logic::{
    is_test_or_build_command, verified_reminder, TodoItem, TodoState, TodoStats, TodoStatus,
    TodoWriteInput, TEST_BUILD_COMMANDS, TODO_ALL_CLOSED, TODO_ALL_DONE, TODO_BOOKKEEPING_SENTINEL,
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

    /// Turn-end bookkeeping body, or `None` when nothing is owed.
    /// `backing_tasks` = live background jobs + running subagents.
    pub fn bookkeeping_reminder(&self, backing_tasks: usize) -> Option<String> {
        logic::bookkeeping_reminder(&self.state.lock().unwrap(), backing_tasks)
    }

    /// Old name of [`Todos::bookkeeping_reminder`]. The body is now a
    /// bookkeeping request (starts with [`TODO_BOOKKEEPING_SENTINEL`]), not a
    /// "continue working" gate.
    #[deprecated(
        note = "renamed to `bookkeeping_reminder`; the body no longer asks to continue working"
    )]
    pub fn gate_reminder(&self, backing_tasks: usize) -> Option<String> {
        self.bookkeeping_reminder(backing_tasks)
    }

    /// Start-of-turn reminder about what the previous turn left behind.
    pub fn next_turn_reminder(&self) -> Option<String> {
        logic::next_turn_reminder(&self.state.lock().unwrap())
    }

    fn stale_reminder(&self, steps: usize) -> String {
        logic::stale_reminder(steps, &self.state.lock().unwrap())
    }

    fn in_progress(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .todo_items()
            .find(|t| t.status == TodoStatus::InProgress)
            .map(|t| t.content.clone())
    }

    /// Apply one `todo_write`. Validated on a copy first: a write that would
    /// leave two items `in_progress` changes nothing.
    pub fn apply(&self, input: TodoWriteInput) -> Result<String, logic::TodoError> {
        let mut state = self.state.lock().unwrap();
        logic::validate_no_duplicate_ids(&input.todos)?;
        let mut next = state.clone();
        if logic::effective_merge(&next, &input) {
            logic::apply_merge(&mut next, &input.todos)?;
        } else {
            logic::apply_replace(&mut next, &input.todos)?;
        }
        logic::validate_single_in_progress(&next)?;
        *state = next;
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
- Exactly one item is `in_progress` at a time — a write that would leave two is rejected. Mark the previous item `completed` / `cancelled` / `awaiting_user` first (in the same call is fine). Mark an item `in_progress` before you touch anything for it.\n\
- Mark an item `completed` in the same turn you finish it — never batch completions at the end, and never mark something completed while its verification (tests, build, review) is still pending.\n\
- When exploration changes the plan, update the list in the same turn you learn it: add the steps you discovered, drop the ones that turned out unnecessary (`cancelled`), re-word items that were wrong. A stale list is worse than no list.\n\
- Send `todo_write` in the same reply as the next work calls, not as a reply by itself.\n\
- Keep the list honest when a turn ends: finished items `completed`, items that wait on the user's answer or approval `awaiting_user`, dropped or blocked ones `cancelled`. Open items are not a reason to keep working — if the user asked to discuss, review or wait, stop and say so. When the user's next message answers an `awaiting_user` item, move it back to `pending` / `in_progress` or complete it.";

const PARAMS: &str = r#"{"type":"object","properties":{"merge":{"type":"boolean","description":"When true (default), merge the given items into the list by id — send only what changed. When false, the given items replace the whole list."},"todos":{"type":"array","description":"Items to write. In merge mode, id + status is enough to flip an existing item.","items":{"type":"object","properties":{"id":{"type":"string","description":"Stable identifier, reused across calls to update the same item."},"content":{"type":"string","description":"Imperative one-liner describing the step. Optional when updating an existing item."},"status":{"type":"string","enum":["pending","in_progress","completed","cancelled","awaiting_user"],"description":"pending | in_progress (at most one) | completed (finished and verified) | cancelled (dropped or blocked) | awaiting_user (waiting on the user's answer or approval)."}},"required":["id"]}}},"required":["todos"]}"#;

/// Named `"todos"` service + the waterfall wiring. Mounted once per isolate
/// subtree that wants its own list: the root session and every tab page. The
/// `todo_write` tool itself is registered separately by
/// [`todo_tool_registration`], once, against the global `"tools"` table.
pub fn todo_service() -> Plugin {
    plugin("tool-todo", Inject::new(), |ctx, _: &()| {
        ctx.provide(TODOS, Todos::new())?;
        wire_todo_waterfalls(ctx);
        Ok(None)
    })
}

/// Register the single `todo_write` tool against the global `"tools"` table.
/// Registered once; the body dispatches on the **executing** context so a call
/// from any tab page reaches that page's own `"todos"` service.
pub fn todo_tool_registration() -> Plugin {
    plugin("tool-todo.tools", Inject::from([TOOLS]), |ctx, _: &()| {
        let tools = ctx.require::<Tools>(TOOLS)?;
        let root_ctx = ctx.clone();
        let body: ToolBody = {
            std::sync::Arc::new(move |call| {
                // `TODOS` is isolated per tab page, so resolve the caller's
                // subtree, not the (root) one that registered the tool.
                let ctx = crate::tools::registry::exec_ctx().unwrap_or_else(|| root_ctx.clone());
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

/// Backwards-compatible single mount: service + tool registration together.
/// New code mounts [`todo_service`] per isolate subtree and
/// [`todo_tool_registration`] once at the root.
pub fn tool_todo() -> Plugin {
    plugin("tool-todo", Inject::from([TOOLS]), |ctx, _: &()| {
        ctx.provide(TODOS, Todos::new())?;
        wire_todo_waterfalls(ctx);
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

/// Per-page listeners. Same identity rule as goal: a waterfall is process-global,
/// and each tab is its own main session.
fn wire_todo_waterfalls(ctx: &Context) {
    // Policy state of these handlers (not part of the named service). `Todos`
    // outlives the turn, so the per-prompt bits are reset in `agent/pre-step`.
    let fires = Arc::new(AtomicUsize::new(0));
    // This page called `ask_user_question` / `handoff_to_user` since the user's
    // last prompt: the model already handed the turn back.
    let asked = Arc::new(AtomicBool::new(false));
    // Start-of-turn reminder computed when the prompt arrives, injected on the
    // first step of that turn.
    let carried = Arc::new(Mutex::new(None::<String>));
    // [`Todos::revision`] right after the last bookkeeping step, so a list the
    // model left as it was does not get a bookkeeping step every turn (each one
    // samples with a different tool list and breaks the prefix cache).
    // `bookkept` marks "a bookkeeping step ran; read the revision once it is
    // over" — the step itself may write, so the vote's on_win is too early.
    let bookkept_at = Arc::new(Mutex::new(None::<u64>));
    let bookkept = Arc::new(AtomicBool::new(false));
    {
        let (fires, asked, carried, ctx_pre) =
            (fires.clone(), asked.clone(), carried.clone(), ctx.clone());
        let (bookkept_at, bookkept) = (bookkept_at.clone(), bookkept.clone());
        let _ = ctx.on_waterfall(PRE_STEP, move |step: PreStep, args| {
            let next = args.next::<PreStep>().unwrap_or(step);
            // The quota belongs to this page's prompt. Another page, or a child,
            // must not refill it.
            if next.enter && Sessions::turn_is_this_page(&ctx_pre, &next.identity) {
                fires.store(0, Ordering::Relaxed);
                asked.store(false, Ordering::Relaxed);
                let todos = ctx_pre.get::<Todos>(TODOS);
                if bookkept.swap(false, Ordering::Relaxed) {
                    *bookkept_at.lock().unwrap() = todos.as_ref().map(|t| t.revision());
                }
                *carried.lock().unwrap() = todos.and_then(|t| t.next_turn_reminder());
            }
            next
        });
    }
    {
        let (asked, ctx_exec, bookkept_at) = (asked.clone(), ctx.clone(), bookkept_at.clone());
        let _ = ctx.on_waterfall(TOOLS_EXECUTE, move |result: ToolResult, args| {
            let result = args.next::<ToolResult>().unwrap_or(result);
            if result.name == "todo_write" || !call_is_this_page(&ctx_exec) {
                return result;
            }
            // Any other tool on this page is work the list may now be behind
            // on: the "unchanged since the last bookkeeping" skip no longer
            // holds. (The bookkeeping step itself only runs todo_write;
            // refused calls there never reach `tools/execute`.)
            *bookkept_at.lock().unwrap() = None;
            // A refused / failed ask never reached the user.
            if !result.is_error && ASK_TOOLS.contains(&result.name.as_str()) {
                asked.store(true, Ordering::Relaxed);
            }
            result
        });
    }
    // Mid-turn watchdog. Rearmed from the payload (step 0) instead of by the loop.
    let watch = Arc::new(Mutex::new(TodoWatch::default()));
    let ctx_step = ctx.clone();
    let _ = ctx.on_waterfall(STEP_START, move |start: StepStart, args| {
        let mut next = args.next::<StepStart>().unwrap_or(start);
        // A child sharing this page's list must not be nagged about it, and
        // must not reset this page's counters. Another tab has its own watch.
        if !Sessions::turn_is_this_page(&ctx_step, &next.identity) {
            return next;
        }
        let Some(todos) = ctx_step.get::<Todos>(TODOS) else {
            return next;
        };
        if let Some(body) = carried.lock().unwrap().take() {
            next.remind(ORDER_STEP_START_TODO, body);
        }
        let mut watch = watch.lock().unwrap();
        if next.step == 0 {
            *watch = TodoWatch::armed_at(todos.revision());
            return next;
        }
        let verified = ctx_step
            .get::<Sessions>(SESSIONS)
            .and_then(|s| s.with_log(|events, _| last_step_test_or_build(events)));
        if let Some(body) = watch.step(&todos, verified.as_deref()) {
            next.remind(ORDER_STEP_START_TODO, body);
        }
        next
    });
    let ctx_end = ctx.clone();
    let _ = ctx.on_waterfall(TURN_END, move |end: TurnEnd, args| {
        let mut next = args.next::<TurnEnd>().unwrap_or(end);
        if !gate_applies(&ctx_end, &next)
            || fires.load(Ordering::Relaxed) >= MAX_TODO_BOOKKEEPING_FIRES
            || asked.load(Ordering::Relaxed)
            || in_plan_mode(&ctx_end)
        {
            return next;
        }
        let Some(todos) = ctx_end.get::<Todos>(TODOS) else {
            return next;
        };
        // Nothing changed since the last bookkeeping step and no tool ran
        // since (any non-todo_write call clears `bookkept_at`): the model
        // already had its chance to settle exactly this list.
        if *bookkept_at.lock().unwrap() == Some(todos.revision()) {
            return next;
        }
        if let Some(body) = todos.bookkeeping_reminder(backing_tasks(&ctx_end)) {
            // Charged only when the loop actually takes the step
            // (`TurnEndRound::charge`): a handler in a lower slot can win the
            // round, or the step can be skipped (no todo_write, Stop, compact
            // threshold) — then neither the quota nor `bookkept` moves.
            let (spent, ran) = (fires.clone(), bookkept.clone());
            next.bookkeep_with(ORDER_TURN_END_TODO, body, BOOKKEEPING_TOOLS, move || {
                spent.fetch_add(1, Ordering::Relaxed);
                ran.store(true, Ordering::Relaxed);
            });
        }
        next
    });
}

/// The tool call finishing in `tools/execute` came from this page's own turn —
/// not a subagent (own isolated `"sessions"`), not another tab. `"todos"` is
/// shared with subagents, so the session identity is what tells them apart.
fn call_is_this_page(page: &Context) -> bool {
    let Some(exec) = crate::tools::registry::exec_ctx() else {
        return false;
    };
    match (
        page.get::<Sessions>(SESSIONS),
        exec.get::<Sessions>(SESSIONS),
    ) {
        (Some(here), Some(there)) => here.identity() == there.identity(),
        _ => false,
    }
}

fn in_plan_mode(ctx: &Context) -> bool {
    ctx.get::<PlanMode>(PLAN_MODE).is_some_and(|p| p.active())
}

/// The `bash` command of the step that just finished (the tool rows after the
/// last model output), when it is a test / build ([`is_test_or_build_command`])
/// and the same step did not also write the list.
fn last_step_test_or_build(events: &[LogEvent]) -> Option<String> {
    let mut command = None;
    for event in events.iter().rev() {
        match event {
            LogEvent::LlmStream(_) => break,
            LogEvent::ToolExecute { name, .. } if name == "todo_write" => return None,
            LogEvent::ToolExecute {
                name, arguments, ..
            } if name == "bash" && command.is_none() => {
                let cmd = serde_json::from_str::<serde_json::Value>(arguments)
                    .ok()
                    .and_then(|v| v.get("command")?.as_str().map(str::to_string));
                command = cmd.filter(|c| is_test_or_build_command(c));
            }
            _ => {}
        }
    }
    command
}

/// Mid-turn `todo_write` watchdog. One turn's worth of state —
/// `agent/step-start` step 0 rearms it. Per step it says at most one thing, in
/// this order:
///
/// 1. a test / build just ran with an item `in_progress` and no update in that
///    step → "mark it completed if it is done" ([`verified_reminder`]);
/// 2. [`TODO_NUDGE_AFTER_STEPS`] steps without a write → [`logic::stale_reminder`].
///
/// 1 and 2 share [`MAX_TODO_NUDGES`]. Nothing fires while nothing is owed
/// (`awaiting_user` does not count). A step that changed the list says
/// nothing: the write's own result already lists every item and its status,
/// so a reminder would only repeat it.
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

    /// Call once per sampling step after step 0. `verified` = the test / build
    /// command the previous step ran, if any.
    fn step(&mut self, todos: &Todos, verified: Option<&str>) -> Option<String> {
        let revision = todos.revision();
        if self.seen_revision != Some(revision) {
            self.seen_revision = Some(revision);
            self.steps_since_write = 0;
            return None;
        }
        self.steps_since_write += 1;
        if self.nudges >= MAX_TODO_NUDGES || todos.stats().open() == 0 {
            return None;
        }
        if let (Some(command), Some(current)) = (verified, todos.in_progress()) {
            self.nudges += 1;
            self.steps_since_write = 0;
            return Some(verified_reminder(command, &current));
        }
        if self.steps_since_write < TODO_NUDGE_AFTER_STEPS {
            return None;
        }
        self.nudges += 1;
        let steps = std::mem::take(&mut self.steps_since_write);
        Some(todos.stale_reminder(steps))
    }
}

/// Whether this turn end is one the todo bookkeeping has any business in.
///
/// - Only a text ending: a blown step budget means the model is stuck in a tool
///   loop; bookkeeping will not unstick it.
/// - Not while the user has queued the next message — they steer.
/// - This page only. `"todos"` is **not** isolated per subagent (the child
///   runner isolates `sessions` / `turn` / `agentPresets`), so a child would
///   otherwise be gated by its parent's list. Another tab is a different main
///   session; `is_main_session` does not separate them. The identity rides on
///   the payload because a waterfall handler cannot see the executing context.
fn gate_applies(ctx: &Context, end: &TurnEnd) -> bool {
    end.ended_with_text && !end.queued_followups && Sessions::turn_is_this_page(ctx, &end.identity)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write(todos: &Todos, args: &str) -> Result<String, logic::TodoError> {
        todos.apply(serde_json::from_str(args).unwrap())
    }

    #[test]
    fn two_in_progress_items_are_rejected() {
        let todos = Todos::new();
        let err = write(
            &todos,
            r#"{"merge":false,"todos":[
                {"id":"a","content":"读代码","status":"in_progress"},
                {"id":"b","content":"改实现","status":"in_progress"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.starts_with(
                "Error: only one item may be in_progress at a time, but this write would leave a, b in_progress."
            ),
            "{err}"
        );
        assert!(
            err.contains("completed, cancelled or awaiting_user"),
            "{err}"
        );
        assert!(
            todos.snapshot().is_empty(),
            "a rejected write changes nothing"
        );
        assert_eq!(todos.revision(), 0);

        // A merge that starts `b` while `a` is still in flight is rejected too,
        // and leaves the list as it was.
        write(
            &todos,
            r#"{"merge":false,"todos":[
                {"id":"a","content":"读代码","status":"in_progress"},
                {"id":"b","content":"改实现","status":"pending"}]}"#,
        )
        .unwrap();
        let err = write(&todos, r#"{"todos":[{"id":"b","status":"in_progress"}]}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("would leave a, b in_progress"), "{err}");
        let statuses: Vec<_> = todos
            .snapshot()
            .into_iter()
            .map(|(_, t)| t.status)
            .collect();
        assert_eq!(statuses, [TodoStatus::InProgress, TodoStatus::Pending]);
        assert_eq!(todos.revision(), 1);

        // Handing over in one call is fine.
        let ok = write(
            &todos,
            r#"{"todos":[{"id":"a","status":"completed"},{"id":"b","status":"in_progress"}]}"#,
        )
        .unwrap();
        assert!(ok.contains("1/2 done (1 in_progress, 0 pending)."), "{ok}");
    }

    #[test]
    fn closing_every_item_reports_the_list_done() {
        let todos = Todos::new();
        let out = write(
            &todos,
            r#"{"merge":false,"todos":[
                {"id":"a","content":"读代码","status":"completed"},
                {"id":"b","content":"改实现","status":"completed"}]}"#,
        )
        .unwrap();
        assert!(
            out.contains("2/2 done (0 in_progress, 0 pending)."),
            "{out}"
        );
        assert!(out.contains("清单已全部完成"), "{out}");
    }

    #[test]
    fn the_description_asks_for_honest_bookkeeping_not_nonstop_work() {
        assert!(!TODO_WRITE_DESC.contains("Do not end a turn with pending items"));
        assert!(TODO_WRITE_DESC.contains("Open items are not a reason to keep working"));
        assert!(TODO_WRITE_DESC.contains("awaiting_user"));
        assert!(PARAMS.contains(r#""awaiting_user""#));
    }

    #[test]
    fn a_test_step_is_found_in_the_last_tool_rows_only() {
        let bash = |cmd: &str| LogEvent::ToolExecute {
            id: "1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({ "command": cmd }).to_string(),
            content: String::new(),
            images: Vec::new(),
            is_error: false,
        };
        let sample = || LogEvent::LlmStream(Default::default());
        let events = vec![sample(), bash("cargo test"), sample(), bash("ls")];
        assert_eq!(
            last_step_test_or_build(&events),
            None,
            "only the last step counts"
        );
        let events = vec![sample(), bash("ls"), bash("npm run build")];
        assert_eq!(
            last_step_test_or_build(&events).as_deref(),
            Some("npm run build")
        );
        let mut events = vec![sample(), bash("cargo test")];
        events.push(LogEvent::ToolExecute {
            id: "2".into(),
            name: "todo_write".into(),
            arguments: "{}".into(),
            content: String::new(),
            images: Vec::new(),
            is_error: false,
        });
        assert_eq!(
            last_step_test_or_build(&events),
            None,
            "the same step already updated the list"
        );
    }
}
