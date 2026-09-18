//! Session actor run loop. TUI never holds the loop; it sends `SessionCommand`.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cordis::Context;
use cordis_spine::{
    Compact, Goal, LoopHandle, Sessions, Subagents, TurnControl, TurnOutcome, AGENT_LOOP, COMPACT,
    GOAL, SESSIONS, SUBAGENTS, TURN,
};
use cordis_tui::QueuedItem;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::oneshot;

use super::commands::{PromptTurnResult, SessionCommand};

enum Job {
    Prompt(String),
    Compact(String),
    /// Hidden Grok GoalSummary: no composer bubble, not shown in the queue pane.
    GoalSummary,
}

struct Pending {
    prompt_id: String,
    job: Job,
    respond_to: tokio::sync::oneshot::Sender<PromptTurnResult>,
}

enum Drive {
    Done(PromptTurnResult),
    Shutdown,
}

pub(super) async fn run_session(
    ctx: Context,
    mut cmd_rx: mpsc::UnboundedReceiver<SessionCommand>,
    current_prompt_id: Arc<Mutex<Option<String>>>,
    queued: Arc<AtomicUsize>,
    queued_prompts: Arc<Mutex<Vec<QueuedItem>>>,
) {
    let mut queue: VecDeque<Pending> = VecDeque::new();
    let mut current: Option<Pending> = None;

    loop {
        if drain_cmds(&ctx, &mut cmd_rx, &mut queue, &queued, &queued_prompts) {
            return;
        }
        if current.is_none() {
            if let Some(next) = queue.pop_front() {
                sync_snaps(&ctx, &queue, &queued, &queued_prompts);
                current = Some(next);
            }
        }
        if let Some(pending) = current.take() {
            let prompt_id = pending.prompt_id.clone();
            match pending.job {
                Job::Prompt(text) => {
                    let turn = run_turn(&ctx, text);
                    tokio::pin!(turn);
                    match drive_job(
                        &ctx,
                        &current_prompt_id,
                        prompt_id,
                        turn,
                        &mut cmd_rx,
                        &mut queue,
                        &queued,
                        &queued_prompts,
                    )
                    .await
                    {
                        Drive::Shutdown => {
                            let _ = pending.respond_to.send(Err("shutdown".into()));
                            return;
                        }
                        Drive::Done(result) => {
                            maybe_queue_goal_summary(
                                &ctx,
                                &result,
                                &mut queue,
                                &queued,
                                &queued_prompts,
                            );
                            let _ = pending.respond_to.send(result);
                        }
                    }
                }
                Job::GoalSummary => {
                    let turn = run_goal_summary(&ctx);
                    tokio::pin!(turn);
                    match drive_job(
                        &ctx,
                        &current_prompt_id,
                        prompt_id,
                        turn,
                        &mut cmd_rx,
                        &mut queue,
                        &queued,
                        &queued_prompts,
                    )
                    .await
                    {
                        Drive::Shutdown => {
                            let _ = pending.respond_to.send(Err("shutdown".into()));
                            return;
                        }
                        Drive::Done(result) => {
                            maybe_queue_goal_summary(
                                &ctx,
                                &result,
                                &mut queue,
                                &queued,
                                &queued_prompts,
                            );
                            let _ = pending.respond_to.send(result);
                        }
                    }
                }
                Job::Compact(context) => {
                    let job = run_compact(&ctx, context);
                    tokio::pin!(job);
                    match drive_job(
                        &ctx,
                        &current_prompt_id,
                        prompt_id,
                        job,
                        &mut cmd_rx,
                        &mut queue,
                        &queued,
                        &queued_prompts,
                    )
                    .await
                    {
                        Drive::Shutdown => {
                            let _ = pending.respond_to.send(Err("shutdown".into()));
                            return;
                        }
                        Drive::Done(result) => {
                            let _ = pending.respond_to.send(result);
                        }
                    }
                }
            }
            continue;
        }
        if ctx
            .get::<Subagents>(SUBAGENTS)
            .is_some_and(|s| s.has_parent_notices())
        {
            let mailbox = run_mailbox(&ctx);
            tokio::pin!(mailbox);
            match drive_job(
                &ctx,
                &current_prompt_id,
                "mailbox".into(),
                mailbox,
                &mut cmd_rx,
                &mut queue,
                &queued,
                &queued_prompts,
            )
            .await
            {
                Drive::Shutdown => return,
                Drive::Done(result) => {
                    maybe_queue_goal_summary(&ctx, &result, &mut queue, &queued, &queued_prompts);
                    continue;
                }
            }
        }
        let wake = ctx.get::<Subagents>(SUBAGENTS).map(|s| s.parent_wake());
        let wait_wake = async {
            match &wake {
                Some(w) => w.notified().await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(wait_wake);
        tokio::select! {
            cmd = cmd_rx.recv() => {
                if apply_cmd(&ctx, cmd, &mut queue, &queued, &queued_prompts) {
                    return;
                }
            }
            _ = &mut wait_wake => {}
        }
    }
}

/// Returns true if the actor should shut down.
fn drain_cmds(
    ctx: &Context,
    cmd_rx: &mut mpsc::UnboundedReceiver<SessionCommand>,
    queue: &mut VecDeque<Pending>,
    queued: &AtomicUsize,
    snaps: &Mutex<Vec<QueuedItem>>,
) -> bool {
    loop {
        match cmd_rx.try_recv() {
            Ok(cmd) => {
                if apply_cmd(ctx, Some(cmd), queue, queued, snaps) {
                    return true;
                }
            }
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => return true,
        }
    }
}

fn sync_snaps(
    ctx: &Context,
    queue: &VecDeque<Pending>,
    queued: &AtomicUsize,
    snaps: &Mutex<Vec<QueuedItem>>,
) {
    let followups = queue.iter().filter(|p| is_prompt(p)).count();
    queued.store(followups, Ordering::Relaxed);
    let items: Vec<QueuedItem> = queue
        .iter()
        .filter_map(|p| match &p.job {
            Job::Prompt(text) => Some(QueuedItem {
                id: p.prompt_id.clone(),
                text: text.clone(),
            }),
            Job::Compact(_) | Job::GoalSummary => None,
        })
        .collect();
    *snaps.lock().unwrap() = items;
    if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
        sessions.set_queued_followups(followups);
    }
}

fn is_prompt(pending: &Pending) -> bool {
    matches!(pending.job, Job::Prompt(_))
}

fn promote_in_queue(queue: &mut VecDeque<Pending>, id: Option<&str>) -> bool {
    let idx = match id {
        Some(want) => queue
            .iter()
            .position(|p| is_prompt(p) && p.prompt_id == want),
        None => queue.iter().position(is_prompt),
    };
    let Some(idx) = idx else {
        return false;
    };
    if let Some(item) = queue.remove(idx) {
        queue.push_front(item);
        true
    } else {
        false
    }
}

fn take_from_queue(queue: &mut VecDeque<Pending>, id: Option<&str>) -> Option<Pending> {
    let idx = match id {
        Some(want) => queue
            .iter()
            .position(|p| is_prompt(p) && p.prompt_id == want),
        None => queue.iter().rposition(is_prompt),
    }?;
    queue.remove(idx)
}

fn request_cancel(ctx: &Context) {
    if let Some(gate) = ctx.get::<TurnControl>(TURN) {
        gate.cancel();
    }
    // Stop also cancels this session's subagents; admission re-opens on the
    // next prompt (see `open_subagent_admission`).
    //
    // `"subagents"` 是全局一份、按 parent session 分账：带上自己这一页的身份，
    // 否则第 2 页按 Stop 会把第 1 页的子代理一起收了。没有会话时退回旧行为。
    if let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) {
        match ctx.get::<Sessions>(SESSIONS) {
            Some(sessions) => sub.cancel_session(sessions.identity()),
            None => sub.cancel_all(),
        }
    }
}

/// A new user turn re-opens spawns after a prior Stop.
fn open_subagent_admission(ctx: &Context) {
    if let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) {
        match ctx.get::<Sessions>(SESSIONS) {
            Some(sessions) => sub.open_admission_for(sessions.identity()),
            None => sub.open_admission(),
        }
    }
}

/// Returns true on shutdown.
fn apply_cmd(
    ctx: &Context,
    cmd: Option<SessionCommand>,
    queue: &mut VecDeque<Pending>,
    queued: &AtomicUsize,
    snaps: &Mutex<Vec<QueuedItem>>,
) -> bool {
    match cmd {
        Some(SessionCommand::Shutdown) | None => true,
        Some(SessionCommand::Cancel) => false,
        Some(SessionCommand::Prompt {
            prompt_id,
            text,
            send_now,
            respond_to,
        }) => {
            let next = Pending {
                prompt_id,
                job: Job::Prompt(text),
                respond_to,
            };
            if send_now {
                queue.push_front(next);
                // 先 cancel（cancel_all 会关掉 spawn admission），再重开，
                // 这一轮才有 spawn 面 —— 顺序反了会让本轮一直 spawn_blocked。
                request_cancel(ctx);
                open_subagent_admission(ctx);
            } else {
                open_subagent_admission(ctx);
                queue.push_back(next);
            }
            sync_snaps(ctx, queue, queued, snaps);
            false
        }
        Some(SessionCommand::Compact {
            context,
            respond_to,
        }) => {
            queue.push_back(Pending {
                prompt_id: "compact".into(),
                job: Job::Compact(context),
                respond_to,
            });
            sync_snaps(ctx, queue, queued, snaps);
            false
        }
        Some(SessionCommand::Promote { id }) => {
            promote_in_queue(queue, id.as_deref());
            sync_snaps(ctx, queue, queued, snaps);
            request_cancel(ctx);
            // 同 send_now：cancel_all 关掉 admission，给被提升的那一轮重开。
            open_subagent_admission(ctx);
            false
        }
        Some(SessionCommand::Take { id }) => {
            let _ = take_from_queue(queue, id.as_deref());
            sync_snaps(ctx, queue, queued, snaps);
            false
        }
    }
}

#[allow(clippy::too_many_arguments)] // 绘制 / 布局 / 注册参数天然多，抽结构体只是把参数搬个家，留给需要时再拆
async fn drive_job<F>(
    ctx: &Context,
    current_prompt_id: &Mutex<Option<String>>,
    job_id: String,
    mut job: std::pin::Pin<&mut F>,
    cmd_rx: &mut mpsc::UnboundedReceiver<SessionCommand>,
    queue: &mut VecDeque<Pending>,
    queued: &AtomicUsize,
    snaps: &Mutex<Vec<QueuedItem>>,
) -> Drive
where
    F: Future<Output = PromptTurnResult>,
{
    *current_prompt_id.lock().unwrap() = Some(job_id);
    if let Some(turn) = ctx.get::<TurnControl>(TURN) {
        turn.reset();
    }
    let result = loop {
        tokio::select! {
            outcome = &mut job => break Drive::Done(outcome),
            cmd = cmd_rx.recv() => match cmd {
                Some(SessionCommand::Shutdown) | None => {
                    request_cancel(ctx);
                    *current_prompt_id.lock().unwrap() = None;
                    return Drive::Shutdown;
                }
                Some(SessionCommand::Cancel) => {
                    request_cancel(ctx);
                }
                other => {
                    apply_cmd(ctx, other, queue, queued, snaps);
                }
            }
        }
    };
    *current_prompt_id.lock().unwrap() = None;
    result
}

async fn run_compact(ctx: &Context, extra: String) -> PromptTurnResult {
    let compact = ctx
        .get::<Compact>(COMPACT)
        .ok_or_else(|| "压缩服务未挂载".to_string())?;
    let extra = extra.trim();
    compact
        .run_on(ctx, (!extra.is_empty()).then_some(extra))
        .await
        .map(|_| "已压缩上下文。".into())
        .map_err(|e| e.to_string())
}

async fn run_turn(ctx: &Context, text: String) -> PromptTurnResult {
    let handle = ctx
        .require::<LoopHandle>(AGENT_LOOP)
        .map_err(|e| e.to_string())?;
    match handle.run(text).await {
        Ok(TurnOutcome::Text(reply)) => Ok(reply),
        Err(err) => Err(err.to_string()),
    }
}

async fn run_mailbox(ctx: &Context) -> PromptTurnResult {
    let handle = ctx
        .require::<LoopHandle>(AGENT_LOOP)
        .map_err(|e| e.to_string())?;
    match handle.continue_mailbox().await {
        Ok(TurnOutcome::Text(reply)) => Ok(reply),
        Err(err) => Err(err.to_string()),
    }
}

async fn run_goal_summary(ctx: &Context) -> PromptTurnResult {
    let handle = ctx
        .require::<LoopHandle>(AGENT_LOOP)
        .map_err(|e| e.to_string())?;
    match handle.continue_goal().await {
        Ok(TurnOutcome::Text(reply)) => Ok(reply),
        Err(err) => Err(err.to_string()),
    }
}

fn maybe_queue_goal_summary(
    ctx: &Context,
    result: &PromptTurnResult,
    queue: &mut VecDeque<Pending>,
    queued: &AtomicUsize,
    snaps: &Mutex<Vec<QueuedItem>>,
) {
    if result.is_err() {
        return;
    }
    let Some(goal) = ctx.get::<Goal>(GOAL) else {
        return;
    };
    if !goal.active() {
        return;
    }
    if queue.iter().any(|p| matches!(p.job, Job::GoalSummary)) {
        return;
    }
    let (respond_to, _) = oneshot::channel();
    queue.push_back(Pending {
        prompt_id: "goal-summary".into(),
        job: Job::GoalSummary,
        respond_to,
    });
    sync_snaps(ctx, queue, queued, snaps);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use cordis::Context;
    use cordis_spine::{
        agent_loop, install_without_llm, tool_goal, tool_task, turn, AgentPresets, BoxFuture, Goal,
        Llm, LlmOutput, LogEvent, PromptRequest, Sampler, Sessions, StreamDelta, Subagents,
        ToolCall, Tools, AGENT_PRESETS, GOAL, LLM, SESSIONS, SUBAGENTS, TOOLS,
    };
    use tokio::sync::mpsc;
    use tokio::sync::Notify;

    use crate::session::handle::SessionHandle;

    struct HoldThenText {
        started: Arc<Notify>,
        release: Arc<Notify>,
        holding: Arc<AtomicBool>,
    }

    impl Sampler for HoldThenText {
        fn sample<'a>(
            &'a self,
            _request: PromptRequest,
            _on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            let started = self.started.clone();
            let release = self.release.clone();
            let holding = self.holding.clone();
            Box::pin(async move {
                if holding.swap(false, Ordering::SeqCst) {
                    started.notify_waiters();
                    release.notified().await;
                    return LlmOutput {
                        text: "mailbox".into(),
                        ..LlmOutput::default()
                    };
                }
                LlmOutput {
                    text: "follow-up".into(),
                    ..LlmOutput::default()
                }
            })
        }
    }

    #[tokio::test]
    async fn mailbox_is_working_and_queues_user_prompt() {
        let root = Context::new();
        install_without_llm(&root).await.unwrap();
        root.plugin(turn(), ()).unwrap().wait().await.unwrap();
        root.plugin(tool_task(), cordis_spine::TaskConfig::default())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        root.provide(
            LLM,
            Llm::from_sampler(
                root.clone(),
                Arc::new(HoldThenText {
                    started: started.clone(),
                    release: release.clone(),
                    holding: Arc::new(AtomicBool::new(true)),
                }),
            ),
        )
        .unwrap();
        root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let current_prompt_id = Arc::new(Mutex::new(None));
        let queued = Arc::new(AtomicUsize::new(0));
        let queued_prompts = Arc::new(Mutex::new(Vec::new()));
        let handle = SessionHandle {
            cmd_tx,
            current_prompt_id: current_prompt_id.clone(),
            queued: queued.clone(),
            queued_prompts: queued_prompts.clone(),
        };
        root.require::<Subagents>(SUBAGENTS)
            .unwrap()
            .enqueue_parent_report("child-1", "ping");
        tokio::spawn(run_session(
            root.clone(),
            cmd_rx,
            current_prompt_id,
            queued,
            queued_prompts,
        ));

        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("mailbox should start sampling");
        assert!(
            handle.working(),
            "mailbox continuation must count as working"
        );
        handle.submit("follow-up", false);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if handle.has_queued() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("user prompt must queue while mailbox is sampling");
        assert!(handle.working());
        release.notify_waiters();

        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if sessions
                    .events()
                    .iter()
                    .any(|e| matches!(e, LogEvent::User(t) if t == "follow-up"))
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("queued follow-up should run after mailbox");
        handle.cancel();
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
    }

    /// A background subagent's finished turn reaches the model through the
    /// actor's own mailbox wake — no `job` polling anywhere in
    /// this test.
    #[tokio::test(flavor = "multi_thread")]
    async fn background_subagent_turn_end_wakes_the_actor() {
        let root = Context::new();
        install_without_llm(&root).await.unwrap();
        root.plugin(turn(), ()).unwrap().wait().await.unwrap();
        let home = tempfile::tempdir().unwrap();
        root.provide(AGENT_PRESETS, AgentPresets::load(home.path().to_path_buf()))
            .unwrap();
        root.plugin(tool_task(), cordis_spine::TaskConfig::default())
            .unwrap()
            .wait()
            .await
            .unwrap();
        root.provide(LLM, Llm::from_sampler(root.clone(), Arc::new(EchoLastUser)))
            .unwrap();
        root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let current_prompt_id = Arc::new(Mutex::new(None));
        let queued = Arc::new(AtomicUsize::new(0));
        let queued_prompts = Arc::new(Mutex::new(Vec::new()));
        let handle = SessionHandle {
            cmd_tx,
            current_prompt_id: current_prompt_id.clone(),
            queued: queued.clone(),
            queued_prompts: queued_prompts.clone(),
        };
        tokio::spawn(run_session(
            root.clone(),
            cmd_rx,
            current_prompt_id,
            queued,
            queued_prompts,
        ));

        let tools = root.require::<Tools>(TOOLS).unwrap();
        let started = tools
            .execute(ToolCall {
                id: "bg".into(),
                name: "task".into(),
                arguments: serde_json::json!({
                    "prompt": "CHILD_REPORT_TEXT",
                    "description": "bg smoke",
                    "subagent_type": "general-purpose",
                    "run_in_background": true,
                })
                .to_string(),
            })
            .await;
        assert!(
            started.content.contains("Subagent started in background"),
            "{}",
            started.content
        );

        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let seen = sessions.events().iter().any(|e| {
                    matches!(e, LogEvent::SystemReminder(t)
                        if t.contains("finished its turn and is idle")
                            && t.contains("CHILD_REPORT_TEXT"))
                });
                if seen {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the actor should wake on the child's turn-end notice and inject it");
        handle.cancel();
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
    }

    struct EchoLastUser;

    impl Sampler for EchoLastUser {
        fn sample<'a>(
            &'a self,
            request: PromptRequest,
            mut on_delta: Box<dyn FnMut(StreamDelta) + Send + 'a>,
        ) -> BoxFuture<'a, LlmOutput> {
            Box::pin(async move {
                let text = request
                    .history
                    .iter()
                    .rev()
                    .find_map(|e| match e {
                        LogEvent::User(t) => Some(t.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                if !text.is_empty() {
                    on_delta(StreamDelta::Text(text.clone()));
                }
                LlmOutput {
                    text,
                    ..LlmOutput::default()
                }
            })
        }
    }

    #[tokio::test]
    async fn send_now_cancels_in_flight_and_runs_queued() {
        let root = Context::new();
        install_without_llm(&root).await.unwrap();
        root.plugin(turn(), ()).unwrap().wait().await.unwrap();
        root.plugin(tool_task(), cordis_spine::TaskConfig::default())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        root.provide(
            LLM,
            Llm::from_sampler(
                root.clone(),
                Arc::new(HoldThenText {
                    started: started.clone(),
                    release: release.clone(),
                    holding: Arc::new(AtomicBool::new(true)),
                }),
            ),
        )
        .unwrap();
        root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let current_prompt_id = Arc::new(Mutex::new(None));
        let queued = Arc::new(AtomicUsize::new(0));
        let queued_prompts = Arc::new(Mutex::new(Vec::new()));
        let handle = SessionHandle {
            cmd_tx,
            current_prompt_id: current_prompt_id.clone(),
            queued: queued.clone(),
            queued_prompts: queued_prompts.clone(),
        };
        tokio::spawn(run_session(
            root.clone(),
            cmd_rx,
            current_prompt_id,
            queued,
            queued_prompts,
        ));
        handle.submit("first", false);
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("first turn should start sampling");
        handle.submit("interrupt", true);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if handle
                    .queued_prompts()
                    .iter()
                    .any(|q| q.text == "interrupt")
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("send-now prompt should appear in the queue");
        release.notify_waiters();
        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if sessions
                    .events()
                    .iter()
                    .any(|e| matches!(e, LogEvent::User(t) if t == "interrupt"))
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("send-now should cancel the first turn and run interrupt");
        handle.cancel();
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
    }

    /// send_now 先 cancel（request_cancel → cancel_all 会关掉 spawn
    /// admission）再重开：随后这一轮再 spawn 的子代理必须真的跑起来（快照离开
    /// Running、产出非空），而不是被 "parent session is stopped" 拒掉、空等下一个
    /// prompt。注意不能等 turn-end 通知进父会话事件：父会话采样被 hold，唤醒
    /// 排不进事件流，子代理本身跑完才是准据。
    #[tokio::test(flavor = "multi_thread")]
    async fn send_now_reopens_spawn_admission_for_the_turn_it_starts() {
        let root = Context::new();
        install_without_llm(&root).await.unwrap();
        root.plugin(turn(), ()).unwrap().wait().await.unwrap();
        let home = tempfile::tempdir().unwrap();
        root.provide(AGENT_PRESETS, AgentPresets::load(home.path().to_path_buf()))
            .unwrap();
        root.plugin(tool_task(), cordis_spine::TaskConfig::default())
            .unwrap()
            .wait()
            .await
            .unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        root.provide(
            LLM,
            Llm::from_sampler(
                root.clone(),
                Arc::new(HoldThenText {
                    started: started.clone(),
                    release: release.clone(),
                    holding: Arc::new(AtomicBool::new(true)),
                }),
            ),
        )
        .unwrap();
        root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let current_prompt_id = Arc::new(Mutex::new(None));
        let queued = Arc::new(AtomicUsize::new(0));
        let queued_prompts = Arc::new(Mutex::new(Vec::new()));
        let handle = SessionHandle {
            cmd_tx,
            current_prompt_id: current_prompt_id.clone(),
            queued: queued.clone(),
            queued_prompts: queued_prompts.clone(),
        };
        tokio::spawn(run_session(
            root.clone(),
            cmd_rx,
            current_prompt_id,
            queued,
            queued_prompts,
        ));

        handle.submit("first", false);
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("first turn should start sampling");
        handle.submit("interrupt", true);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if handle
                    .queued_prompts()
                    .iter()
                    .any(|q| q.text == "interrupt")
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("send-now prompt should reach apply_cmd");

        // The send-now turn itself spawns: admission must be open again after
        // apply_cmd's cancel → reopen sequence.
        let tools = root.require::<Tools>(TOOLS).unwrap();
        let out = tools
            .execute(ToolCall {
                id: "after-send-now".into(),
                name: "task".into(),
                arguments: serde_json::json!({
                    "prompt": "SEND_NOW_ADMISSION_CHILD",
                    "description": "admission smoke",
                    "subagent_type": "general-purpose",
                    "run_in_background": true,
                })
                .to_string(),
            })
            .await;
        assert!(
            out.content.contains("Subagent started in background"),
            "{}",
            out.content
        );

        // `remember` writes the snapshot before the coordinator admits the
        // spawn, so a bare snapshot proves nothing: a rejected spawn stays in
        // the initial Running life with empty output. Wait until the child has
        // actually executed a turn (left Running and produced output).
        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        let subs = root.require::<Subagents>(SUBAGENTS).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let ran = subs.list().iter().any(|snap| {
                    snap.description == "admission smoke"
                        && !snap.running()
                        && !snap.output.is_empty()
                });
                if ran {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            for snap in subs.list() {
                eprintln!("SNAP: {snap:?}");
            }
            for e in sessions.events() {
                eprintln!("EVT: {e:?}");
            }
            panic!("child spawned after send-now must actually start");
        });
        handle.cancel();
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
    }

    #[tokio::test]
    async fn goal_summary_queued_after_turn_yields_to_user() {
        let root = Context::new();
        install_without_llm(&root).await.unwrap();
        root.plugin(turn(), ()).unwrap().wait().await.unwrap();
        root.plugin(tool_task(), cordis_spine::TaskConfig::default())
            .unwrap()
            .wait()
            .await
            .unwrap();
        root.plugin(tool_goal(), ()).unwrap().wait().await.unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        root.provide(
            LLM,
            Llm::from_sampler(
                root.clone(),
                Arc::new(HoldThenText {
                    started: started.clone(),
                    release: release.clone(),
                    holding: Arc::new(AtomicBool::new(true)),
                }),
            ),
        )
        .unwrap();
        root.plugin(agent_loop(), ()).unwrap().wait().await.unwrap();
        root.get::<Goal>(GOAL).unwrap().start("ship");

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let current_prompt_id = Arc::new(Mutex::new(None));
        let queued = Arc::new(AtomicUsize::new(0));
        let queued_prompts = Arc::new(Mutex::new(Vec::new()));
        let handle = SessionHandle {
            cmd_tx,
            current_prompt_id: current_prompt_id.clone(),
            queued: queued.clone(),
            queued_prompts: queued_prompts.clone(),
        };
        tokio::spawn(run_session(
            root.clone(),
            cmd_rx,
            current_prompt_id,
            queued,
            queued_prompts,
        ));

        handle.submit("first", false);
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("first turn should start");
        handle.submit("second", false);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if handle.has_queued() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("second prompt queues while goal turn runs");
        release.notify_waiters();

        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let ev = sessions.events();
                let users: Vec<_> = ev
                    .iter()
                    .filter_map(|e| match e {
                        LogEvent::User(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect();
                let reminder = ev.iter().any(|e| {
                    matches!(
                        e,
                        LogEvent::SystemReminder(t) if t.contains("Goal NOT complete")
                    )
                });
                if users.contains(&"first") && users.contains(&"second") && reminder {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("user follow-up then hidden GoalSummary reminder");
        handle.cancel();
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
    }

    #[test]
    fn promote_moves_prompt_to_front() {
        use tokio::sync::oneshot;
        let (tx, _rx) = oneshot::channel();
        let mut queue = VecDeque::from([Pending {
            prompt_id: "a".into(),
            job: Job::Prompt("first".into()),
            respond_to: tx,
        }]);
        let (tx2, _rx2) = oneshot::channel();
        queue.push_back(Pending {
            prompt_id: "b".into(),
            job: Job::Prompt("second".into()),
            respond_to: tx2,
        });
        assert!(promote_in_queue(&mut queue, Some("b")));
        assert_eq!(queue.front().unwrap().prompt_id, "b");
        let taken = take_from_queue(&mut queue, None).unwrap();
        assert_eq!(taken.prompt_id, "a");
    }
}
