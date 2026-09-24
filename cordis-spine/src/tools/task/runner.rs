//! Dock `ChildRunner`: Grok coordinator seam; child body is isolate
//! `"sessions"`+`"turn"`+`"agentPresets"` + [`GrokStep`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cordis::Context;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::agent::presets::AgentPresets;
use crate::agent::runtime::{GrokStep, LoopHandle};
use crate::agent::turn::TurnControl;
use crate::names::{AGENT_PRESETS, CAPABILITY, MODEL_OVERRIDE, SESSIONS, TURN};
use crate::session::log::Sessions;
use cordis_base::types::TurnOutcome;

use super::coordinator::{
    ChildCompletion, ChildControl, ChildRunOutput, ChildRunRequest, ChildRunner, SendBoxFuture,
    StartedChild,
};
use super::interjection::format_interjection;
use super::store::ChildStore;
use super::types::{SubagentResult, SubagentValidateTypeOutcome};
use super::{current_depth, SubagentLife, DEPTH};

/// Session id carried by every child spawn (see [`crate::session::log::ROOT_IDENTITY`]).
pub(super) const PARENT_SESSION_ID: &str = crate::session::log::ROOT_IDENTITY;

pub(super) struct DockChildControl {
    turn: Arc<TurnControl>,
    cancelled: Arc<AtomicBool>,
    token: CancellationToken,
}

impl ChildControl for DockChildControl {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.turn.cancel();
        self.token.cancel();
    }
}

/// Page context captured when `task` is called, keyed by the child id.
///
/// The runner itself is built on the root ctx (one coordinator). The child
/// has to be isolated from the calling page, or `goal` / `todos` / `planMode`
/// — not re-isolated for subagents — resolve to page 1.
#[derive(Clone, Default)]
pub(super) struct SpawnParents(Arc<Mutex<HashMap<String, Context>>>);

impl SpawnParents {
    pub(super) fn note(&self, id: &str, parent: Context) {
        self.0.lock().unwrap().insert(id.to_string(), parent);
    }

    fn take(&self, id: &str) -> Option<Context> {
        self.0.lock().unwrap().remove(id)
    }
}

pub(super) struct DockChildRunner {
    pub ctx: Context,
    pub store: ChildStore,
    pub spawn_parents: SpawnParents,
}

impl ChildRunner for DockChildRunner {
    type Control = DockChildControl;
    type RunFuture = SendBoxFuture<ChildRunOutput>;
    type ValidateFuture = SendBoxFuture<SubagentValidateTypeOutcome>;

    fn run(&self, run: ChildRunRequest<Self::Control>) -> Self::RunFuture {
        let parent = self.parent_for(&run.request.id);
        let store = self.store.clone();
        Box::pin(async move { run_dock_child(parent, store, run).await })
    }

    fn validate_type(
        &self,
        subagent_type: String,
        _parent_session_id: String,
    ) -> Self::ValidateFuture {
        let ctx = self.ctx.clone();
        Box::pin(async move { validate_roster(&ctx, &subagent_type) })
    }

    fn on_completed(&self, completion: ChildCompletion) {
        if let Some(s) = self.store.snapshot(&completion.request.id) {
            if !s.done {
                // `run()` returns after the first turn; drive_child still owns
                // the slot and has already parked / continued.
                return;
            }
        }
        self.store
            .set_output(&completion.request.id, completion.result.output.to_string());
    }
}

impl DockChildRunner {
    /// Calling page when `task` noted one; otherwise the root ctx this runner
    /// was built with (tests and spawns that have no executing page).
    fn parent_for(&self, child_id: &str) -> Context {
        self.spawn_parents
            .take(child_id)
            .unwrap_or_else(|| self.ctx.clone())
    }
}

fn validate_roster(ctx: &Context, subagent_type: &str) -> SubagentValidateTypeOutcome {
    let Some(presets) = ctx.get::<AgentPresets>(AGENT_PRESETS) else {
        return SubagentValidateTypeOutcome::Unknown {
            available: Vec::new(),
        };
    };
    let roster = presets.current_roster();
    if roster.contains_key(subagent_type) {
        SubagentValidateTypeOutcome::Ok
    } else {
        SubagentValidateTypeOutcome::Unknown {
            available: roster.keys().cloned().collect(),
        }
    }
}

fn child_prompt(subagent_type: &str, description: &str, prompt: &str) -> String {
    format!("[{subagent_type}] {description}\n\n{prompt}")
}

async fn run_dock_child(
    parent: Context,
    store: ChildStore,
    run: ChildRunRequest<DockChildControl>,
) -> ChildRunOutput {
    let wall = Instant::now();
    let id = run.request.id.clone();
    let typ = run.request.subagent_type.clone();
    let desc = run.request.description.clone();
    let parent_id = run.request.parent_session_id.clone();
    store.ensure_owned(
        &id,
        desc.clone(),
        typ.clone(),
        run.request.owner.clone(),
        &parent_id,
    );

    let resume = run
        .request
        .resume_from
        .as_deref()
        .map(|src| store.events(src))
        .unwrap_or_default();
    let mut prompt = child_prompt(&typ, &desc, &run.request.prompt);

    let parent_presets = parent.get::<AgentPresets>(AGENT_PRESETS);
    let def = parent_presets.as_ref().and_then(|p| p.subagent(&typ));
    let Some(def) = def else {
        return failed(
            &id,
            &store,
            wall,
            format!("unknown subagent type {typ}"),
            false,
        );
    };

    let mut child = parent
        .isolate("sessions")
        .isolate("turn")
        .isolate("agentPresets");
    // 这两项要**先隔离再 provide**：没隔离的名字 provide 进的是共用注册表，
    // 第二个同样收窄的孩子会撞上「service 已注册」直接起不来——一次
    // `parallel(jobs)` 起四个 read-only researcher 就是四个全挂。
    //
    // 只在真要收窄时隔离：不隔离才继承得到父会话那一份（虽然 `MAX_SUBAGENT_DEPTH`
    // 目前不允许孙子，但别让这条依赖埋在这里）。
    if run.request.runtime_overrides.capability_mode.is_some() {
        child = child.isolate(CAPABILITY);
    }
    if !run.request.runtime_overrides.llm.is_empty() {
        child = child.isolate(MODEL_OVERRIDE);
    }
    let child = child;
    let sessions = Sessions::isolated_as(child.clone(), id.clone());
    if let Some(page) = parent.get::<Sessions>(SESSIONS).and_then(|s| s.ui_page()) {
        sessions.pin_page_home(page);
    }
    // 子代理在父会话的工作目录里干活；父会话没钉就一起跟随进程 cwd。
    if let Some(cwd) = parent
        .get::<Sessions>(SESSIONS)
        .and_then(|s| s.workspace_cwd())
    {
        sessions.pin_workspace_cwd(cwd);
    }
    if !resume.is_empty() {
        sessions.seed(resume);
    }
    let mut hold = Vec::new();
    match child.provide(SESSIONS, sessions.clone()) {
        Ok(d) => hold.push(d),
        Err(e) => {
            return failed(&id, &store, wall, format!("child sessions: {e}"), false);
        }
    }
    match child.provide(TURN, TurnControl::new()) {
        Ok(d) => hold.push(d),
        Err(e) => {
            return failed(&id, &store, wall, format!("child turn: {e}"), false);
        }
    }
    // 能力档位只在收窄时才挂：没挂等于不设限，主会话永远没有这一项。
    if let Some(mode) = run.request.runtime_overrides.capability_mode {
        match child.provide(CAPABILITY, mode) {
            Ok(d) => hold.push(d),
            Err(e) => {
                return failed(&id, &store, wall, format!("child capability: {e}"), false);
            }
        }
    }
    // 采样覆写同理：没点名就一项都不挂，采样照 `"settings"` 走。
    if !run.request.runtime_overrides.llm.is_empty() {
        let over = run.request.runtime_overrides.llm.clone();
        match child.provide(MODEL_OVERRIDE, over) {
            Ok(d) => hold.push(d),
            Err(e) => {
                return failed(
                    &id,
                    &store,
                    wall,
                    format!("child model override: {e}"),
                    false,
                );
            }
        }
    }
    let preset = def.to_preset(&typ);
    // 工具表的排序依据从父会话继承：子代理那张表要和主会话那张共用同一个分组，
    // 才会是它的真前缀，公共头（system + tools）才有得命中。
    let order = parent_presets
        .as_ref()
        .map(|p| p.universal_tools())
        .unwrap_or_default();
    match child.provide(
        AGENT_PRESETS,
        AgentPresets::overlay_with_order(preset, order),
    ) {
        Ok(d) => hold.push(d),
        Err(e) => {
            return failed(&id, &store, wall, format!("child agentPresets: {e}"), false);
        }
    }
    // 「做完要回报」写进初始任务，不进人设或工具描述：那两处排在请求头里，
    // 子代理专属的一段会让它的头和父级分叉。角色拿不到 `send_message` 就不写，
    // 别让它去调一颗看不见的工具。
    if child
        .get::<AgentPresets>(AGENT_PRESETS)
        .is_some_and(|p| p.allows(super::control::SEND_TOOL_NAME))
    {
        super::format::append_reply_instruction(&mut prompt, &parent_id);
    }
    let Some(turn) = child.get::<TurnControl>(TURN) else {
        return failed(&id, &store, wall, "child turn missing".into(), false);
    };
    store.set_turn(&id, turn.clone());
    store.set_hold(&id, hold);
    store.set_sessions(&id, sessions.clone());

    let cancelled = Arc::new(AtomicBool::new(false));
    let promoted = run
        .reporter
        .started(StartedChild {
            child_session_id: id.clone(),
            control: DockChildControl {
                turn: turn.clone(),
                cancelled: cancelled.clone(),
                token: run.cancellation.clone(),
            },
        })
        .await;
    if !promoted {
        turn.cancel();
        return failed(&id, &store, wall, "cancelled before start".into(), true);
    }

    let (first_tx, first_rx) = oneshot::channel();
    let store_w = store.clone();
    let child_w = child.clone();
    let id_w = id.clone();
    let parent_w = parent.clone();
    let coord_cancel = run.cancellation.clone();
    let surface_completion = run.request.surface_completion;
    tokio::spawn(async move {
        drive_child(
            parent_w,
            child_w,
            store_w,
            id_w,
            prompt,
            cancelled,
            coord_cancel,
            surface_completion,
            first_tx,
        )
        .await;
    });

    match first_rx.await {
        Ok(out) => out,
        Err(_) => failed(&id, &store, wall, "child worker dropped".into(), false),
    }
}

#[allow(clippy::too_many_arguments)] // 绘制 / 布局 / 注册参数天然多，抽结构体只是把参数搬个家，留给需要时再拆
async fn drive_child(
    parent: Context,
    child: Context,
    store: ChildStore,
    id: String,
    mut prompt: String,
    cancelled: Arc<AtomicBool>,
    coord_cancel: CancellationToken,
    // `SubagentRequest::surface_completion`：false 时回合结束通知不入父信箱。
    surface_completion: bool,
    first_tx: oneshot::Sender<ChildRunOutput>,
) {
    let wall = Instant::now();
    let handle = LoopHandle::new(child.clone(), Arc::new(GrokStep));
    let mut first_tx = Some(first_tx);
    // Child usage already billed to the parent.
    let mut folded = cordis_base::usage::UsageLedger::default();

    loop {
        if store
            .get(&id)
            .is_some_and(|s| s.dispose.load(Ordering::Relaxed))
            || (first_tx.is_some() && coord_cancel.is_cancelled())
        {
            let out = failed(&id, &store, wall, "cancelled".into(), true);
            if let Some(tx) = first_tx.take() {
                let _ = tx.send(out);
            }
            store.dispose(&id, "cancelled".into(), true);
            return;
        }

        if let Some(turn) = child.get::<TurnControl>(TURN) {
            turn.reset();
        }
        store.reset_reported(&id);
        store
            .get(&id)
            .inspect(|s| s.set_life(SubagentLife::Running));

        let outcome = if first_tx.is_some() {
            tokio::select! {
                _ = coord_cancel.cancelled() => Err("cancelled".into()),
                out = DEPTH.scope(current_depth() + 1, handle.run(prompt.clone())) => match out {
                    Ok(TurnOutcome::Text(t)) => Ok(t),
                    Err(e) => Err(format!("{e}")),
                },
            }
        } else {
            match DEPTH
                .scope(current_depth() + 1, handle.run(prompt.clone()))
                .await
            {
                Ok(TurnOutcome::Text(t)) => Ok(t),
                Err(e) => Err(format!("{e}")),
            }
        };
        if let Some(s) = child.get::<Sessions>(SESSIONS) {
            store.set_events(&id, s.events());
        }
        if let (Some(child_s), Some(parent_s)) = (
            child.get::<Sessions>(SESSIONS),
            parent.get::<Sessions>(SESSIONS),
        ) {
            // The child keeps one cumulative ledger across turns, so fold only
            // what this turn added — re-folding the total would bill the parent
            // once per turn.
            let child_ledger = child_s.ledger();
            let mut delta = child_ledger.delta_since(&folded);
            if cancelled.load(Ordering::Relaxed) && delta.totals.model_calls > 0 {
                delta.mark_incomplete();
            }
            parent_s.fold_subagent_ledger(&delta);
            folded = child_ledger;
        }

        let slot = store.get(&id);
        let dispose = slot
            .as_ref()
            .is_some_and(|s| s.dispose.load(Ordering::Relaxed))
            || (first_tx.is_some() && coord_cancel.is_cancelled());
        let send_now = slot
            .as_ref()
            .is_some_and(|s| s.send_now.swap(false, Ordering::Relaxed));
        let interrupt = slot
            .as_ref()
            .is_some_and(|s| s.interrupt.swap(false, Ordering::Relaxed));

        if dispose {
            let msg = match &outcome {
                Ok(t) => t.clone(),
                Err(e) => e.clone(),
            };
            let out = failed(&id, &store, wall, msg.clone(), true);
            if let Some(tx) = first_tx.take() {
                let _ = tx.send(out);
            }
            store.dispose(&id, msg, true);
            return;
        }

        if send_now {
            if let Some(msg) = store.take_urgent(&id) {
                prompt = format_interjection(&msg);
                continue;
            }
        }

        let duration_ms = wall.elapsed().as_millis() as u64;
        let was_cancelled = cancelled.load(Ordering::Relaxed)
            || interrupt
            || matches!(&outcome, Err(e) if e.contains("cancel"));
        let (success, output) = match &outcome {
            Ok(text) => (true, text.clone()),
            Err(e) => (false, format!("failed: {e}")),
        };
        // One parent notice per finished turn. The child's text rides it when
        // it did not message its parent; a caller that already has the result
        // (foreground spawn) drops the notice again.
        let reported = store
            .get(&id)
            .is_some_and(|s| s.reported_this_turn.load(Ordering::Relaxed));
        // `surface_completion: false` 的孩子（workflow 的子代理、以后的 harness
        // 内部子代理）根本不该在父信箱里露面：run 还没结束就推一条"某个孩子跑
        // 完了"，主线程就会在半份结果上开一轮。
        if surface_completion {
            store.push_turn_end(
                &id,
                (!reported).then(|| super::format::cap_turn_text(&output)),
                was_cancelled && interrupt,
            );
        }
        let next_ready = take_inbox(&store, &id);
        if next_ready.is_none() {
            store.park_idle(&id, output.clone(), was_cancelled && interrupt);
        } else {
            store.set_output(&id, output.clone());
        }
        let first_out = ChildRunOutput {
            result: SubagentResult {
                success: success && !interrupt,
                output: Arc::from(output.clone()),
                error: if success { None } else { Some(output.clone()) },
                cancelled: interrupt,
                subagent_id: id.clone(),
                child_session_id: id.clone(),
                duration_ms,
                ..Default::default()
            },
        };
        if let Some(tx) = first_tx.take() {
            let _ = tx.send(first_out);
        }

        let next = match next_ready {
            Some(msg) => Some(msg),
            None => wait_next(&store, &id).await,
        };
        match next {
            None => {
                store.dispose(&id, output, false);
                return;
            }
            // Idle + urgent is a plain next turn. Envelope is only for send-now
            // while a turn is running (the `send_now` branch above).
            Some(NextMsg::Urgent(m)) => prompt = m,
            Some(NextMsg::Queued(m)) => prompt = m,
        }
    }
}

#[derive(Debug)]
enum NextMsg {
    Queued(String),
    Urgent(String),
}

async fn wait_next(store: &ChildStore, id: &str) -> Option<NextMsg> {
    loop {
        let slot = store.get(id)?;
        if slot.dispose.load(Ordering::Relaxed) {
            return None;
        }
        if let Some(msg) = take_inbox(store, id) {
            return Some(msg);
        }
        let notified = slot.wake.notified();
        tokio::pin!(notified);
        // `notified()` is lazy: enable() inserts the waiter before the
        // second inbox check so enqueue+notify cannot be lost.
        if notified.as_mut().enable() {
            continue;
        }
        if slot.dispose.load(Ordering::Relaxed) {
            return None;
        }
        if let Some(msg) = take_inbox(store, id) {
            return Some(msg);
        }
        notified.await;
    }
}

fn take_inbox(store: &ChildStore, id: &str) -> Option<NextMsg> {
    if let Some(u) = store.take_urgent(id) {
        return Some(NextMsg::Urgent(u));
    }
    store.take_queued(id).map(NextMsg::Queued)
}

fn failed(
    id: &str,
    store: &ChildStore,
    wall: Instant,
    msg: String,
    cancelled: bool,
) -> ChildRunOutput {
    store.dispose(id, msg.clone(), cancelled);
    ChildRunOutput {
        result: SubagentResult {
            success: false,
            cancelled,
            output: Arc::from(msg.clone()),
            error: Some(msg),
            subagent_id: id.to_owned(),
            child_session_id: id.to_owned(),
            duration_ms: wall.elapsed().as_millis() as u64,
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod inbox_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn wait_next_sees_message_enqueued_before_wait() {
        let store = ChildStore::new();
        store.ensure("x", "d".into(), "t".into());
        store.enqueue_queued("x", "hello".into()).unwrap();
        store.park_idle("x", "out".into(), false);
        let got = tokio::time::timeout(Duration::from_millis(200), wait_next(&store, "x")).await;
        assert!(
            matches!(got, Ok(Some(NextMsg::Queued(ref s))) if s == "hello"),
            "{got:?}"
        );
    }

    #[tokio::test]
    async fn wait_next_wakes_on_enqueue_while_idle() {
        let store = ChildStore::new();
        store.ensure("x", "d".into(), "t".into());
        store.park_idle("x", "out".into(), false);
        let store2 = store.clone();
        let join = tokio::spawn(async move { wait_next(&store2, "x").await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        store.enqueue_queued("x", "later".into()).unwrap();
        let got = tokio::time::timeout(Duration::from_millis(200), join)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(got, Some(NextMsg::Queued(ref s)) if s == "later"),
            "{got:?}"
        );
    }

    #[test]
    fn enqueue_while_idle_marks_running_before_return() {
        let store = ChildStore::new();
        store.ensure("x", "d".into(), "t".into());
        store.park_idle("x", "out".into(), false);
        assert!(store.snapshot("x").unwrap().idle);
        let was_running = store.enqueue_queued("x", "hello".into()).unwrap();
        assert!(!was_running);
        assert!(store.snapshot("x").unwrap().running());
        assert_eq!(store.inbox_pending("x"), (1, false));
    }

    #[test]
    fn urgent_while_idle_marks_running_before_return() {
        let store = ChildStore::new();
        store.ensure("x", "d".into(), "t".into());
        store.park_idle("x", "out".into(), false);
        let was_running = store.push_urgent("x", "now".into()).unwrap();
        assert!(!was_running);
        assert!(store.snapshot("x").unwrap().running());
        assert_eq!(store.inbox_pending("x"), (0, true));
    }

    #[test]
    fn turn_end_without_report_carries_the_text() {
        let store = ChildStore::new();
        store.ensure("x", "d".into(), "t".into());
        store.push_turn_end("x", Some("FINDINGS".into()), false);
        let rendered: Vec<String> = store
            .drain_notices()
            .iter()
            .map(super::super::format::format_parent_notice)
            .collect();
        let text = rendered.join("\n");
        assert!(text.contains("FINDINGS"), "{text}");
        assert!(text.contains("finished its turn and is idle"), "{text}");
    }

    #[test]
    fn reported_turn_pushes_one_notice_without_forwarded_text() {
        let store = ChildStore::new();
        store.ensure("x", "d".into(), "t".into());
        store.push_report("x", "progress-1");
        store.push_report("x", "progress-2");
        // The runner passes `None` when the child already reported.
        store.push_turn_end("x", None, false);
        let notices = store.drain_notices();
        let text = notices
            .iter()
            .map(super::super::format::format_parent_notice)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("progress-1"), "{text}");
        assert!(text.contains("progress-2"), "{text}");
        assert_eq!(notices.len(), 3, "{text}");
        assert!(matches!(
            notices.last(),
            Some(super::super::store::ParentNotice::TurnEnd { output: None, .. })
        ));
    }

    #[test]
    fn consume_completion_drops_only_that_childs_turn_end() {
        let store = ChildStore::new();
        store.ensure("a", "d".into(), "t".into());
        store.ensure("b", "d".into(), "t".into());
        store.push_turn_end("a", Some("A".into()), false);
        store.push_turn_end("b", Some("B".into()), false);
        store.consume_completion("a");
        let text = store
            .drain_notices()
            .iter()
            .map(super::super::format::format_parent_notice)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("\"a\""), "{text}");
        assert!(text.contains("\"b\""), "{text}");
    }
}

#[cfg(test)]
mod parent_tests {
    use super::*;
    use crate::names::{AGENT_PRESETS, SESSIONS, TODOS};
    use crate::session::log::Sessions;
    use crate::tools::todo_write::Todos;
    use std::sync::Arc;

    /// 第 2 页记下的父 ctx 才是孩子的隔离根。从根隔离的话，没再隔离的
    /// `todos` 会落到第 1 页。
    #[tokio::test]
    async fn child_isolated_from_the_noted_page_sees_that_pages_todos() {
        let root = Context::new();
        root.provide(TODOS, Todos::new()).unwrap();
        let page = root.isolate(SESSIONS).isolate(TODOS);
        page.provide(SESSIONS, Sessions::tab(page.clone(), 2))
            .unwrap();
        page.provide(TODOS, Todos::new()).unwrap();

        let parents = SpawnParents::default();
        parents.note("child-1", page.clone());
        let runner = DockChildRunner {
            ctx: root.clone(),
            store: ChildStore::new(),
            spawn_parents: parents,
        };
        let parent = runner.parent_for("child-1");
        let child = parent
            .isolate(SESSIONS)
            .isolate("turn")
            .isolate(AGENT_PRESETS);
        child
            .provide(SESSIONS, Sessions::isolated_as(child.clone(), "child-1"))
            .unwrap();

        let seen = child.get::<Todos>(TODOS).unwrap();
        assert!(Arc::ptr_eq(&seen, &page.get::<Todos>(TODOS).unwrap()));
        assert!(!Arc::ptr_eq(&seen, &root.get::<Todos>(TODOS).unwrap()));

        let fallback = runner.parent_for("missing");
        assert!(Arc::ptr_eq(
            &fallback.get::<Todos>(TODOS).unwrap(),
            &root.get::<Todos>(TODOS).unwrap()
        ));
    }
}
