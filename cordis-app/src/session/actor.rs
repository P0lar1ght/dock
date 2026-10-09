//! Session actor run loop. TUI never holds the loop; it sends `SessionCommand`.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cordis::Context;
use cordis_spine::QueuedItem;
use cordis_spine::{
    Compact, Goal, LoopHandle, Sessions, Subagents, TurnControl, TurnOutcome, AGENT_LOOP, COMPACT,
    GOAL, SESSIONS, SUBAGENTS, TURN,
};
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
                        true,
                    )
                    .await
                    {
                        Drive::Shutdown => {
                            let _ = pending.respond_to.send(Err("shutdown".into()));
                            return;
                        }
                        Drive::Done(result) => {
                            flush_side_notes(&ctx);
                            requeue_stranded_steers(&ctx, &mut queue, &queued, &queued_prompts);
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
                        true,
                    )
                    .await
                    {
                        Drive::Shutdown => {
                            let _ = pending.respond_to.send(Err("shutdown".into()));
                            return;
                        }
                        Drive::Done(result) => {
                            flush_side_notes(&ctx);
                            requeue_stranded_steers(&ctx, &mut queue, &queued, &queued_prompts);
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
                        false,
                    )
                    .await
                    {
                        Drive::Shutdown => {
                            let _ = pending.respond_to.send(Err("shutdown".into()));
                            return;
                        }
                        Drive::Done(result) => {
                            flush_side_notes(&ctx);
                            requeue_stranded_steers(&ctx, &mut queue, &queued, &queued_prompts);
                            let _ = pending.respond_to.send(result);
                        }
                    }
                }
            }
            continue;
        }
        // 这一页的信箱：每页只收自己启动的子代理的消息（分页不再落到第 1 页）。
        let page = ctx
            .get::<Sessions>(SESSIONS)
            .map(|s| s.identity().to_string())
            .unwrap_or_else(|| cordis_spine::ROOT_IDENTITY.to_string());
        if ctx
            .get::<Subagents>(SUBAGENTS)
            .is_some_and(|s| s.has_parent_notices(&page))
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
                true,
            )
            .await
            {
                Drive::Shutdown => return,
                Drive::Done(result) => {
                    flush_side_notes(&ctx);
                    requeue_stranded_steers(&ctx, &mut queue, &queued, &queued_prompts);
                    maybe_queue_goal_summary(&ctx, &result, &mut queue, &queued, &queued_prompts);
                    continue;
                }
            }
        }
        let wake = ctx
            .get::<Subagents>(SUBAGENTS)
            .map(|s| s.parent_wake(&page));
        let wait_wake = async {
            match &wake {
                Some(w) => w.notified().await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(wait_wake);
        tokio::select! {
            cmd = cmd_rx.recv() => {
                if apply_cmd(&ctx, cmd, &mut queue, &queued, &queued_prompts, false, false) {
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
                // 只在没有任务在跑时调：插话没有可并的采样。
                if apply_cmd(ctx, Some(cmd), queue, queued, snaps, false, false) {
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

/// 指定 id 的那条排队消息；`None` = 最早那条。
fn queued_prompt_at(queue: &VecDeque<Pending>, id: Option<&str>) -> Option<usize> {
    match id {
        Some(want) => queue
            .iter()
            .position(|p| is_prompt(p) && p.prompt_id == want),
        None => queue.iter().position(is_prompt),
    }
}

fn promote_in_queue(queue: &mut VecDeque<Pending>, id: Option<&str>) -> bool {
    let Some(idx) = queued_prompt_at(queue, id) else {
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

/// 插话没有可并的采样：排到最前，作为下一条消息单独成一轮。图片放进图片槽，
/// 由那一轮的 `User` 带走。
fn queue_front_prompt(
    ctx: &Context,
    queue: &mut VecDeque<Pending>,
    text: String,
    images: Vec<cordis_spine::UserImage>,
) {
    if !images.is_empty() {
        if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
            sessions.queue_user_images(images);
        }
    }
    let (respond_to, _) = oneshot::channel();
    queue.push_front(Pending {
        prompt_id: format!(
            "prompt-steer-{}",
            STEER_FALLBACK_SEQ.fetch_add(1, Ordering::Relaxed)
        ),
        job: Job::Prompt(text),
        respond_to,
    });
    open_subagent_admission(ctx);
}

static STEER_FALLBACK_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 任务在跑时写进来、循环没等到下一个步骤边界就收尾了的侧边聊天笔记：收尾后
/// 落进历史（跟在这一轮后面），不开新的一轮。
fn flush_side_notes(ctx: &Context) {
    if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
        for note in sessions.take_side_notes() {
            sessions.append_side_note(&note);
        }
    }
}

/// 一轮已经收尾才到的插话（循环最后一次取收件箱之后）：转成排在最前的消息，
/// 按到达顺序跑，不能丢。
fn requeue_stranded_steers(
    ctx: &Context,
    queue: &mut VecDeque<Pending>,
    queued: &AtomicUsize,
    snaps: &Mutex<Vec<QueuedItem>>,
) {
    let Some(sessions) = ctx.get::<Sessions>(SESSIONS) else {
        return;
    };
    let stranded = sessions.take_steers();
    if stranded.is_empty() {
        return;
    }
    for steer in stranded.into_iter().rev() {
        queue_front_prompt(ctx, queue, steer.text, steer.images);
    }
    sync_snaps(ctx, queue, queued, snaps);
}

/// 排队的消息改插话时带上它的图。排队时图片放在会话的图片槽里（只有一格），
/// 正文里的 `[Image #N]` 标记说明这条消息带图。
fn take_images_for(ctx: &Context, text: &str) -> Vec<cordis_spine::UserImage> {
    if !text.contains("[Image #") {
        return Vec::new();
    }
    ctx.get::<Sessions>(SESSIONS)
        .map(|s| s.take_pending_images())
        .unwrap_or_default()
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

/// Returns true on shutdown. `steerable`：正在跑一个会采样的任务（用户轮、
/// 目标收尾、信箱续跑），插话可以交给它在步骤边界送达。`busy`：有任务在跑
/// （含压缩），往日志里写东西得等步骤边界或收尾。
#[allow(clippy::too_many_arguments)]
fn apply_cmd(
    ctx: &Context,
    cmd: Option<SessionCommand>,
    queue: &mut VecDeque<Pending>,
    queued: &AtomicUsize,
    snaps: &Mutex<Vec<QueuedItem>>,
    steerable: bool,
    busy: bool,
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
        Some(SessionCommand::Steer { text, images }) => {
            if steerable {
                cordis_spine::steer(ctx, text, images);
            } else {
                queue_front_prompt(ctx, queue, text, images);
                sync_snaps(ctx, queue, queued, snaps);
            }
            false
        }
        Some(SessionCommand::SideNote { note }) => {
            if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
                if busy {
                    sessions.push_side_note(note);
                } else {
                    sessions.append_side_note(&note);
                }
            }
            false
        }
        Some(SessionCommand::SteerQueued { id }) => {
            if steerable {
                // 不用 `take_from_queue`：它的 `None` 是「最后一条」（收回输入框用），
                // 这里的 `None` 是最早那条。
                let taken = queued_prompt_at(queue, id.as_deref()).and_then(|i| queue.remove(i));
                if let Some(Pending {
                    job: Job::Prompt(text),
                    respond_to,
                    ..
                }) = taken
                {
                    let images = take_images_for(ctx, &text);
                    cordis_spine::steer(ctx, text, images);
                    // 这条不再单独成一轮：它的结果并在正在跑的那一轮里。
                    let _ = respond_to.send(Ok(String::new()));
                }
            } else {
                // 没有可并的采样：只挪到最前，不取消正在跑的压缩。
                promote_in_queue(queue, id.as_deref());
            }
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
    steerable: bool,
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
                    apply_cmd(ctx, other, queue, queued, snaps, steerable, true);
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
        ToolCall, Tools, TurnControl, TurnEndStatus, AGENT_PRESETS, GOAL, LLM, SESSIONS, SUBAGENTS,
        TOOLS, TURN,
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

    /// 一页的会话 actor + 一个第一次采样会停住、等放行的模型。
    async fn held_session() -> (Context, SessionHandle, Arc<Notify>, Arc<Notify>) {
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
        (root, handle, started, release)
    }

    async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !f() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("等不到：{what}"));
    }

    fn turn_ends(events: &[LogEvent]) -> Vec<TurnEndStatus> {
        events
            .iter()
            .filter_map(|e| match e {
                LogEvent::TurnEnd(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    /// 插话不取消正在跑的这一轮：采样照常收完，插话在下一个步骤边界并进这一轮，
    /// 整个过程只有一轮、以完成收尾。旧实现（`turn/steer` = send_now）会把第一轮
    /// 停掉（`TurnEnd(Cancelled)`），插话另起一轮。
    #[tokio::test]
    async fn steer_joins_the_running_turn_instead_of_cancelling_it() {
        let (root, handle, started, release) = held_session().await;
        handle.submit("first", false);
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("first turn should start sampling");
        handle.steer("改一下方向", Vec::new());
        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        wait_until("插话进收件箱", || sessions.has_steers()).await;
        assert!(handle.working(), "插话不该让这一轮停下");
        release.notify_waiters();
        wait_until("这一轮收尾", || {
            !turn_ends(&sessions.events()).is_empty()
        })
        .await;
        let events = sessions.events();
        assert_eq!(
            turn_ends(&events),
            vec![TurnEndStatus::Completed],
            "{events:?}"
        );
        let at = events
            .iter()
            .position(|e| matches!(e, LogEvent::User(t) if t == "改一下方向"))
            .expect("插话要落进日志");
        assert!(
            matches!(&events[at - 1], LogEvent::SystemReminder(t) if t == cordis_spine::STEER_REMINDER),
            "插话前要有提示：{events:?}"
        );
        assert!(
            matches!(&events[at - 2], LogEvent::LlmStream(o) if o.text == "mailbox"),
            "被插话的那次采样要完整保留：{events:?}"
        );
        assert!(
            !events[at..].iter().any(|e| matches!(e, LogEvent::PreStep)),
            "插话不是新的一轮：{events:?}"
        );
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
    }

    /// 排队的消息改插话：从队列里拿出来，并进正在跑的这一轮。
    #[tokio::test]
    async fn a_queued_prompt_can_become_a_steer() {
        let (root, handle, started, release) = held_session().await;
        handle.submit("first", false);
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("first turn should start sampling");
        handle.submit("later", false);
        wait_until("排上队", || handle.queued_prompts().len() == 1).await;
        let id = handle.queued_prompts()[0].id.clone();
        handle.steer_queued(Some(id));
        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        wait_until("改成插话", || sessions.has_steers()).await;
        assert!(handle.queued_prompts().is_empty(), "改插话后不该还在队里");
        release.notify_waiters();
        wait_until("这一轮收尾", || {
            !turn_ends(&sessions.events()).is_empty()
        })
        .await;
        let events = sessions.events();
        assert_eq!(
            turn_ends(&events),
            vec![TurnEndStatus::Completed],
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, LogEvent::User(t) if t == "later")),
            "{events:?}"
        );
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
    }

    /// 不指定 id 时改插话的是**最早**那条（`SteerQueued { id: None }` 的约定，
    /// 和不能插话时挪到最前的那条一致）。以前借用了「收回最后一条」的
    /// `take_from_queue`，拿到的是最新排进去的。
    #[tokio::test]
    async fn steering_the_queue_without_an_id_takes_the_oldest() {
        let (root, handle, started, release) = held_session().await;
        handle.submit("first", false);
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("first turn should start sampling");
        handle.submit("older", false);
        handle.submit("newer", false);
        wait_until("排上两条", || handle.queued_prompts().len() == 2).await;
        handle.steer_queued(None);
        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        wait_until("改成插话", || sessions.has_steers()).await;
        let steered: Vec<String> = sessions
            .pending_steers()
            .into_iter()
            .map(|s| s.text)
            .collect();
        let left: Vec<String> = handle
            .queued_prompts()
            .into_iter()
            .map(|q| q.text)
            .collect();
        release.notify_waiters();
        assert_eq!(steered, ["older"]);
        assert_eq!(left, ["newer"]);
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
    }

    fn side_note_at(events: &[LogEvent]) -> Option<usize> {
        events.iter().position(
            |e| matches!(e, LogEvent::SystemReminder(t) if cordis_spine::side_note_text(t).is_some()),
        )
    }

    /// 闲着的页写进侧边聊天笔记：直接落进历史，不开新的一轮。
    #[tokio::test]
    async fn an_idle_side_note_lands_without_starting_a_turn() {
        let (root, handle, _started, _release) = held_session().await;
        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        handle.merge_side_note("旁问结论");
        wait_until("笔记落进历史", || {
            side_note_at(&sessions.events()).is_some()
        })
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let events = sessions.events();
        assert!(!handle.working(), "笔记不该开一轮");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, LogEvent::User(_) | LogEvent::PreStep)),
            "{events:?}"
        );
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
    }

    /// 一轮在跑时写进笔记、循环没等到下一个步骤边界就收尾了：收尾后落进历史，
    /// 跟在这一轮的 TurnEnd 后面；只有这一轮，没有为笔记再开一轮。
    #[tokio::test]
    async fn a_side_note_that_misses_the_step_lands_after_the_turn() {
        let (root, handle, started, release) = held_session().await;
        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        handle.submit("first", false);
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("first turn should start sampling");
        handle.merge_side_note("旁问结论");
        wait_until("笔记进收件箱", || sessions.has_side_notes()).await;
        assert!(
            side_note_at(&sessions.events()).is_none(),
            "在跑时不能直接落：{:?}",
            sessions.events()
        );
        release.notify_waiters();
        wait_until("笔记落进历史", || {
            side_note_at(&sessions.events()).is_some()
        })
        .await;
        let events = sessions.events();
        let end = events
            .iter()
            .position(|e| matches!(e, LogEvent::TurnEnd(_)))
            .expect("这一轮要收尾");
        assert!(side_note_at(&events).unwrap() > end, "{events:?}");
        assert_eq!(
            turn_ends(&events),
            vec![TurnEndStatus::Completed],
            "{events:?}"
        );
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
    }

    /// 空闲时插话没有可并的一轮：当一条普通消息发（不带插话提示）。
    #[tokio::test]
    async fn an_idle_steer_runs_as_a_plain_prompt() {
        let (root, handle, _started, release) = held_session().await;
        handle.steer("hello", Vec::new());
        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        wait_until("开一轮", || {
            sessions
                .events()
                .iter()
                .any(|e| matches!(e, LogEvent::User(t) if t == "hello"))
        })
        .await;
        release.notify_waiters();
        assert!(
            !sessions.events().iter().any(
                |e| matches!(e, LogEvent::SystemReminder(t) if t == cordis_spine::STEER_REMINDER)
            ),
            "{:?}",
            sessions.events()
        );
        assert!(!sessions.has_steers());
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
    }

    /// 这一轮已经收尾、收件箱里却还有插话（循环最后一次取之后才到）：转成
    /// 排在最前的消息，不能丢。
    #[tokio::test]
    async fn a_steer_stranded_after_the_turn_runs_next() {
        let (root, handle, started, release) = held_session().await;
        let sessions = root.require::<Sessions>(SESSIONS).unwrap();
        handle.submit("first", false);
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("first turn should start sampling");
        // 绕过 actor 直接放进收件箱，再让这一轮在循环取它之前收尾：模拟「循环
        // 最后一次检查之后才到」的那个窗口。
        let turn = root.require::<TurnControl>(TURN).unwrap();
        turn.cancel();
        release.notify_waiters();
        wait_until("第一轮收尾", || {
            !turn_ends(&sessions.events()).is_empty()
        })
        .await;
        sessions.push_steer("掉队的".into(), Vec::new());
        // actor 在下一个任务结束时兜底：再发一条让它走一遍收尾。
        handle.submit("second", false);
        wait_until("掉队的插话被发出", || {
            sessions
                .events()
                .iter()
                .any(|e| matches!(e, LogEvent::User(t) if t == "掉队的"))
        })
        .await;
        let _ = handle.cmd_tx.send(SessionCommand::Shutdown);
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
