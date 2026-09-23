//! Subagent coordinator + dock isolate `ChildRunner`.
//!
//! One plugin provides the named service `"subagents"` and registers the whole
//! model-facing surface: `task` (spawn) plus the mailbox tools
//! (`send_message` / `list_agents` / `interrupt_agent`).
//!
//! Every child is continuable: it runs a turn, parks idle, accepts
//! `send_message`, and pushes a turn-end notice to the parent. The parent is
//! never required to poll. Subagents are not jobs: `job` / `kill_task` only
//! see background commands.
//!
//! `send_message` is direction-neutral. Parent and child share one definition;
//! the service authorizes one adjacent edge (parent → direct child, child →
//! the session that started it) from the caller's session identity.

pub mod admission;
pub mod backend;
mod control;
pub mod coordinator;
mod coordinator_state;
mod execute;
mod format;
mod interjection;
mod runner;
mod store;
mod tool_error;
pub mod types;

use std::sync::Arc;
use std::time::{Duration, Instant};

use cordis::{plugin, Inject, Plugin};
use tokio::sync::{mpsc, oneshot};

use crate::names::{SUBAGENTS, TOOLS};
use crate::session::log::ROOT_IDENTITY;
use crate::tools::registry::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::ToolCall;

use admission::SubagentLimits;
use backend::{ChannelBackend, SubagentBackend};
use coordinator::{CoordinatorConfig, SubagentCoordinator};
use runner::DockChildRunner;
use store::ChildStore;
pub use store::WorkflowReport;
use types::{
    SubagentCancelRequest, SubagentCancelTarget, SubagentEvent, SubagentOwner, SubagentRequest,
    SubagentResult, SubagentRuntimeOverrides,
};

/// Copied from Grok `MAX_SUBAGENT_DEPTH`.
pub const MAX_SUBAGENT_DEPTH: u32 = 1;

/// Idle children older than this are disposed (their slot and transcript stay
/// readable, so `resume_from` keeps working).
const IDLE_TTL: Duration = Duration::from_secs(15 * 60);

/// How often the idle sweep runs.
const IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

tokio::task_local! {
    pub(super) static DEPTH: u32;
}

pub(super) fn current_depth() -> u32 {
    DEPTH.try_with(|d| *d).unwrap_or(0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubagentLife {
    Running,
    Idle,
    Done,
}

#[derive(Clone, Debug)]
pub struct SubagentSnap {
    pub id: String,
    pub description: String,
    pub subagent_type: String,
    pub done: bool,
    pub idle: bool,
    pub cancelled: bool,
    pub output: String,
    pub started_at: Instant,
}

impl SubagentSnap {
    pub fn running(&self) -> bool {
        !self.done && !self.idle
    }
}

/// Host-supplied coordinator policy. Read at the composition root (env +
/// config.toml) and injected, so the plugin never re-reads ambient state.
#[derive(Clone, Debug)]
pub struct TaskConfig {
    /// How long a foreground spawn blocks before it is handed off to the
    /// background. Grok's shell uses 600s; a dock turn is interactive, so the
    /// default is shorter.
    pub foreground_budget: Duration,
    /// Session-scoped concurrent spawn limits.
    pub limits: SubagentLimits,
    /// 一次 workflow run 同时在跑的子代理上限。
    ///
    /// workflow 的子代理**不受 [`SubagentLimits`] 约束**（`admission.rs` 对
    /// workflow owner 直接放行，走 run 自己的池），这个值就是那个池的大小。
    /// 运行时还会按机器并行度收窄，见 `workflow::max_concurrent_agents`。
    pub workflow_max_concurrent_agents: usize,
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            foreground_budget: Duration::from_secs(120),
            limits: SubagentLimits::from_env(),
            workflow_max_concurrent_agents: crate::tools::workflow::DEFAULT_MAX_CONCURRENT_AGENTS,
        }
    }
}

/// 一次 workflow-host `agent()` 调用要落地的全部信息。
///
/// `run_id` 是关键：coordinator 按它给子代理归属（`cancel_workflow_children`），
/// 传错就没法按 run 取消，也没法按 run 归账。
pub struct WorkflowSpawn {
    pub run_id: String,
    /// 子代理 id。由调用方生成，这样它在孩子起跑**之前**就能把这一行挂进自己
    /// 的进度表——孩子的消息回来时才对得上号。
    pub id: String,
    pub prompt: String,
    pub description: String,
    pub subagent_type: String,
    /// 一个**已 dispose** 的子代理 id：从它的 transcript 续跑（schema 重试用）。
    pub resume_from: Option<String>,
    /// 本次 run 的取消令牌。由 host 传进来，这样 run 一取消孩子立刻收到。
    pub cancel_token: tokio_util::sync::CancellationToken,
    pub runtime_overrides: SubagentRuntimeOverrides,
}

/// Named `"subagents"` service. Live-lookup; do not capture the Arc.
pub struct Subagents {
    backend: ChannelBackend,
    store: ChildStore,
    workflow_max_concurrent_agents: usize,
    spawn_parents: runner::SpawnParents,
}

impl Subagents {
    pub fn list(&self) -> Vec<SubagentSnap> {
        self.store.list()
    }

    pub fn snapshot(&self, id: &str) -> Option<SubagentSnap> {
        self.store.snapshot(id)
    }

    /// Child conversation log. Live while the isolate is up; snapshot after.
    pub fn events(&self, id: &str) -> Vec<cordis_base::types::LogEvent> {
        self.store.events(id)
    }

    pub async fn wait(&self, ids: &[String], timeout_ms: u64) -> Vec<SubagentSnap> {
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms.max(1));
        loop {
            let snaps: Vec<_> = ids.iter().filter_map(|id| self.snapshot(id)).collect();
            if snaps.iter().all(|s| !s.running()) || tokio::time::Instant::now() >= deadline {
                return if snaps.is_empty() {
                    ids.iter()
                        .map(|id| SubagentSnap {
                            id: id.clone(),
                            description: String::new(),
                            subagent_type: String::new(),
                            done: true,
                            idle: false,
                            cancelled: false,
                            output: format!(
                                "Task or subagent {id} not found. No background tasks or subagents exist in this session."
                            ),
                            started_at: Instant::now(),
                        })
                        .collect()
                } else {
                    snaps
                };
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    pub fn kill(&self, id: &str) -> Option<String> {
        let snap = self.snapshot(id)?;
        if snap.done {
            return Some(format!("{id} already finished"));
        }
        self.store.mark_cancelled(id);
        let (respond_to, _rx) = oneshot::channel();
        let _ = self
            .backend
            .sender()
            .send(SubagentEvent::Cancel(SubagentCancelRequest {
                parent_session_id: Some(ROOT_IDENTITY.into()),
                target: SubagentCancelTarget::SubagentId(id.to_owned()),
                respond_to,
            }));
        Some(format!("killed {id}"))
    }

    /// Cancel every live child of **one** session（Stop / 新会话）。
    /// Spawn admission stays closed until [`Self::open_admission_for`].
    ///
    /// 分页是并列主线：第 2 页按 Stop 不该把第 1 页的子代理一起收了，所以取消
    /// 按会话身份走，而不是 backend 上绑的那个缺省 id。
    pub fn cancel_session(&self, parent_session_id: &str) {
        let (respond_to, _rx) = oneshot::channel();
        let _ = self
            .backend
            .request_cancel_session(parent_session_id, respond_to);
    }

    /// 同 [`Self::open_admission`]，但针对指定会话。
    pub fn open_admission_for(&self, parent_session_id: &str) {
        let _ = self.backend.open_spawn_admission_for(parent_session_id);
    }

    /// Cancel every live child of this session (user Stop, session switch).
    /// Spawn admission stays closed until [`Self::open_admission`].
    pub fn cancel_all(&self) {
        let (respond_to, _rx) = oneshot::channel();
        let _ = self.backend.request_cancel_parent_session(respond_to);
    }

    /// Re-open spawns after a Stop: called at the start of each user turn.
    pub fn open_admission(&self) {
        let _ = self.backend.open_spawn_admission();
    }

    /// Session delete / switch: cancel this session's children and wait up to
    /// `budget` for them to drain.
    pub async fn teardown_and_drain(&self, budget: Duration) {
        self.backend
            .teardown_session_and_drain(ROOT_IDENTITY, budget)
            .await;
    }

    /// Dispose idle children past the sweep TTL.
    pub fn sweep_idle(&self, ttl: Duration) -> Vec<String> {
        self.store.sweep_idle(ttl)
    }

    pub fn has_parent_notices(&self) -> bool {
        self.store.has_parent_notices()
    }

    /// Enqueue a parent-facing notice and wake the session actor.
    pub fn enqueue_parent_report(&self, from: &str, output: &str) {
        self.store.push_report(from, output);
    }

    /// 取走某次 workflow run 攒下的子代理上报。
    ///
    /// 这些上报**不会**叫醒主线程（[`store::ChildStore::push_report`] 按 owner
    /// 分流）；workflow host 取走后折进 run 快照，收尾时随最终结果一并交付。
    pub fn take_workflow_reports(&self, run_id: &str) -> Vec<WorkflowReport> {
        self.store.take_workflow_reports(run_id)
    }

    /// 一次 workflow run 收尾：把结果与过程交给主线程，并叫醒它。
    ///
    /// **整条 run 只有这一次唤醒**。中间那些「某个子代理跑完了」既不完整也不
    /// 可行动，推给主线程只会让它在半份结果上烧一轮。过程上报已经在 overlay
    /// 和滚动区 `Notice` 里给用户看过；这一条是给主模型的完整交卷。
    pub fn notify_workflow_done(
        &self,
        name: String,
        status: String,
        elapsed_ms: u64,
        summary: String,
        reports: Vec<WorkflowReport>,
        dropped_reports: usize,
    ) {
        self.store
            .push_workflow_done(name, status, elapsed_ms, summary, reports, dropped_reports);
    }

    pub fn parent_wake(&self) -> Arc<tokio::sync::Notify> {
        self.store.parent_wake()
    }

    /// 配置的 workflow 并发上限（未按机器并行度收窄）。workflow host 在挂载
    /// 每次 run 的并发池时 live-look 这个值。
    pub fn workflow_max_concurrent_agents(&self) -> usize {
        self.workflow_max_concurrent_agents
    }

    /// Spawn one workflow-host child and wait for it.
    ///
    /// owner 必须带**真实 run id**（所以 [`Self::cancel_workflow_and_drain`]
    /// 能按 run 收人），返回**原件** [`SubagentResult`] 而不是快照（快照没有
    /// `success` / `error`，失败会被读成成功）。
    pub async fn spawn_for_workflow(&self, spawn: WorkflowSpawn) -> SubagentResult {
        let (id, result) = self.spawn_workflow_child(spawn).await;
        result.unwrap_or_else(|error| SubagentResult {
            success: false,
            output: Arc::from(""),
            error: Some(error),
            subagent_id: id.clone(),
            child_session_id: id,
            ..Default::default()
        })
    }

    /// 共用的落地路径，返回（子代理 id，后端结果）。
    async fn spawn_workflow_child(
        &self,
        spawn: WorkflowSpawn,
    ) -> (String, Result<SubagentResult, String>) {
        let id = spawn.id;
        let owner = SubagentOwner::workflow(&spawn.run_id);
        if let Some(parent) = crate::tools::registry::exec_ctx() {
            self.note_spawn_parent(&id, parent);
        }
        self.remember(
            &id,
            spawn.description.clone(),
            spawn.subagent_type.clone(),
            owner.clone(),
            ROOT_IDENTITY,
        );
        let request = SubagentRequest {
            id: id.clone(),
            prompt: spawn.prompt,
            description: spawn.description,
            subagent_type: spawn.subagent_type,
            parent_session_id: ROOT_IDENTITY.into(),
            resume_from: spawn.resume_from,
            runtime_overrides: spawn.runtime_overrides,
            run_in_background: false,
            surface_completion: false,
            await_to_completion: true,
            owner,
            cancel_token: spawn.cancel_token,
        };
        match self.backend.spawn(request).await {
            Ok(result) => (id, Ok(result)),
            Err(e) => {
                let message = e.to_string();
                self.store.finish(&id, message.clone(), false);
                (id, Err(message))
            }
        }
    }

    /// 取消一次 workflow run 的全部子代理，最多等 `budget` 让它们收尾。
    ///
    /// 形状照 [`Self::teardown_and_drain`]：coordinator 先真的取消，再在最后一
    /// 个孩子落地时回执（`resolve_workflow_cancel_waiters`）。
    pub async fn cancel_workflow_and_drain(&self, run_id: &str, budget: Duration) {
        self.backend.cancel_workflow_and_drain(run_id, budget).await;
    }

    pub(super) fn backend(&self) -> ChannelBackend {
        self.backend.clone()
    }

    /// Remember which page called `task`, so the child isolate is cut from
    /// that page rather than from the root ctx the runner captured.
    pub(crate) fn note_spawn_parent(&self, id: &str, parent: cordis::Context) {
        self.spawn_parents.note(id, parent);
    }

    pub(super) fn remember(
        &self,
        id: &str,
        description: String,
        subagent_type: String,
        owner: SubagentOwner,
        parent: &str,
    ) {
        self.store
            .ensure_owned(id, description, subagent_type, owner, parent);
    }
}

pub fn tool_task() -> Plugin {
    plugin(
        "tool-task",
        Inject::from([TOOLS]),
        |ctx, cfg: &TaskConfig| {
            let (tx, rx) = mpsc::unbounded_channel();
            let store = ChildStore::new();
            let backend = ChannelBackend::for_session(tx, ROOT_IDENTITY);
            let spawn_parents = runner::SpawnParents::default();
            let runner = DockChildRunner {
                ctx: ctx.clone(),
                store: store.clone(),
                spawn_parents: spawn_parents.clone(),
            };
            let coord = SubagentCoordinator::new(
                rx,
                runner,
                CoordinatorConfig {
                    foreground_budget: cfg.foreground_budget,
                    limits: cfg.limits,
                },
            );
            tokio::spawn(async move {
                coord.run().await;
            });

            let sweep_store = store.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(IDLE_SWEEP_INTERVAL).await;
                    for id in sweep_store.sweep_idle(IDLE_TTL) {
                        tracing::debug!(subagent_id = %id, "disposed idle subagent");
                    }
                }
            });

            ctx.provide(
                SUBAGENTS,
                Subagents {
                    backend,
                    store,
                    workflow_max_concurrent_agents: cfg.workflow_max_concurrent_agents,
                    spawn_parents,
                },
            )?;
            let tools = ctx.require::<Tools>(TOOLS)?;

            let task_body: ToolBody = {
                let ctx = ctx.clone();
                Arc::new(move |call: ToolCall| {
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
                            return tool_result(call, "Error: subagents is not mounted");
                        };
                        execute::run_task(&ctx, &sub, call).await
                    })
                })
            };
            let send_body: ToolBody = {
                let ctx = ctx.clone();
                Arc::new(move |call: ToolCall| {
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
                            return tool_result(call, "Error: subagents is not mounted");
                        };
                        control::run_send(&sub, caller_identity(&ctx), call).await
                    })
                })
            };
            let list_body: ToolBody = {
                let ctx = ctx.clone();
                Arc::new(move |call: ToolCall| {
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
                            return tool_result(call, "Error: subagents is not mounted");
                        };
                        control::run_list(&sub, caller_identity(&ctx), call).await
                    })
                })
            };
            let interrupt_body: ToolBody = {
                let ctx = ctx.clone();
                Arc::new(move |call: ToolCall| {
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
                            return tool_result(call, "Error: subagents is not mounted");
                        };
                        control::run_interrupt(&sub, caller_identity(&ctx), call).await
                    })
                })
            };
            own_registered(
                ctx,
                vec![
                    tools.register(execute::spec(), task_body)?,
                    tools.register(control::send_spec(), send_body)?,
                    tools.register(control::list_spec(), list_body)?,
                    tools.register(control::interrupt_spec(), interrupt_body)?,
                ],
            )?;
            Ok(None)
        },
    )
}

/// 发起这次工具调用的会话身份：主线 `main` / `main#2`，或子代理自己的 id。
///
/// 工具体捕获的是根 ctx，身份要问执行期 ctx（`Tools::execute_on` 递下来的那个）；
/// 拿不到时退回根 ctx 上的会话（也就是 `main`）。
fn caller_identity(ctx: &cordis::Context) -> String {
    crate::tools::registry::exec_ctx()
        .and_then(|c| c.get::<crate::session::log::Sessions>(crate::names::SESSIONS))
        .or_else(|| ctx.get::<crate::session::log::Sessions>(crate::names::SESSIONS))
        .map(|s| s.identity().to_string())
        .unwrap_or_else(|| ROOT_IDENTITY.to_string())
}

#[cfg(test)]
mod coordinator_spawn_tests {
    use super::*;
    use crate::tools::task::coordinator::{
        ChildControl, ChildRunOutput, ChildRunRequest, ChildRunner, SendBoxFuture, StartedChild,
    };
    use crate::tools::task::types::{SubagentResult, SubagentValidateTypeOutcome};

    struct NopControl;

    impl ChildControl for NopControl {
        fn cancel(&self) {}
    }

    struct ImmediateRunner;

    impl ChildRunner for ImmediateRunner {
        type Control = NopControl;
        type RunFuture = SendBoxFuture<ChildRunOutput>;
        type ValidateFuture = SendBoxFuture<SubagentValidateTypeOutcome>;

        fn run(&self, run: ChildRunRequest<Self::Control>) -> Self::RunFuture {
            Box::pin(async move {
                let _ = run
                    .reporter
                    .started(StartedChild {
                        child_session_id: run.request.id.clone(),
                        control: NopControl,
                    })
                    .await;
                ChildRunOutput {
                    result: SubagentResult {
                        success: true,
                        output: Arc::from("ok"),
                        subagent_id: run.request.id.clone(),
                        child_session_id: run.request.id,
                        ..Default::default()
                    },
                }
            })
        }

        fn validate_type(
            &self,
            _subagent_type: String,
            _parent_session_id: String,
        ) -> Self::ValidateFuture {
            Box::pin(async { SubagentValidateTypeOutcome::Ok })
        }

        fn on_completed(&self, _completion: crate::tools::task::coordinator::ChildCompletion) {}
    }

    #[tokio::test]
    async fn channel_backend_spawn_goes_through_coordinator() {
        let (tx, rx) = mpsc::unbounded_channel();
        let coord = SubagentCoordinator::new(rx, ImmediateRunner, CoordinatorConfig::default());
        tokio::spawn(async move {
            coord.run().await;
        });
        let backend = ChannelBackend::for_session(tx, ROOT_IDENTITY);
        let result = backend
            .spawn(SubagentRequest {
                id: "sa-coord".into(),
                prompt: "hi".into(),
                description: "coord test".into(),
                subagent_type: "general-purpose".into(),
                parent_session_id: "dock".into(),
                resume_from: None,
                runtime_overrides: SubagentRuntimeOverrides::default(),
                run_in_background: false,
                surface_completion: true,
                await_to_completion: true,
                owner: SubagentOwner::Task,
                cancel_token: tokio_util::sync::CancellationToken::new(),
            })
            .await
            .expect("spawn");
        assert!(result.success, "{:?}", result.error);
        assert_eq!(result.output.as_ref(), "ok");
        assert_eq!(result.subagent_id, "sa-coord");
    }

    /// 一直跑到取消为止的 runner，用来观察取消的排空过程。
    struct GatedRunner;

    struct TokenControl(tokio_util::sync::CancellationToken);

    impl ChildControl for TokenControl {
        fn cancel(&self) {
            self.0.cancel();
        }
    }

    impl ChildRunner for GatedRunner {
        type Control = TokenControl;
        type RunFuture = SendBoxFuture<ChildRunOutput>;
        type ValidateFuture = SendBoxFuture<SubagentValidateTypeOutcome>;

        fn run(&self, run: ChildRunRequest<Self::Control>) -> Self::RunFuture {
            Box::pin(async move {
                let token = tokio_util::sync::CancellationToken::new();
                let _ = run
                    .reporter
                    .started(StartedChild {
                        child_session_id: run.request.id.clone(),
                        control: TokenControl(token.clone()),
                    })
                    .await;
                token.cancelled().await;
                // 真实的孩子收尾要花一点时间；立刻返回就测不出"等没等"。
                tokio::time::sleep(Duration::from_millis(80)).await;
                ChildRunOutput {
                    result: SubagentResult {
                        cancelled: true,
                        output: Arc::from(""),
                        subagent_id: run.request.id.clone(),
                        child_session_id: run.request.id,
                        ..Default::default()
                    },
                }
            })
        }

        fn validate_type(
            &self,
            _subagent_type: String,
            _parent_session_id: String,
        ) -> Self::ValidateFuture {
            Box::pin(async { SubagentValidateTypeOutcome::Ok })
        }

        fn on_completed(&self, _completion: crate::tools::task::coordinator::ChildCompletion) {}
    }

    fn workflow_request(id: &str, run_id: &str) -> SubagentRequest {
        SubagentRequest {
            id: id.into(),
            prompt: "hi".into(),
            description: "drain test".into(),
            subagent_type: "general-purpose".into(),
            parent_session_id: ROOT_IDENTITY.into(),
            resume_from: None,
            runtime_overrides: SubagentRuntimeOverrides::default(),
            run_in_background: false,
            surface_completion: false,
            await_to_completion: true,
            owner: SubagentOwner::workflow(run_id),
            cancel_token: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// 按 run 取消要等孩子真的落地才回执。
    ///
    /// 立刻回执等于告诉调用方"已经排空"，而孩子还在烧 token —— 这条打的就是
    /// `workflow_cancel_waiters` 那套一直没人往里放东西的代码。
    #[tokio::test]
    async fn cancelling_a_workflow_run_waits_for_its_children() {
        let (tx, rx) = mpsc::unbounded_channel();
        let coord = SubagentCoordinator::new(rx, GatedRunner, CoordinatorConfig::default());
        tokio::spawn(async move {
            coord.run().await;
        });
        let backend = ChannelBackend::for_session(tx, ROOT_IDENTITY);

        let child = {
            let backend = backend.clone();
            tokio::spawn(async move { backend.spawn(workflow_request("sa-wf", "wf_1")).await })
        };
        // 等孩子进到 active，否则取消发得太早，等于什么都没取消。
        tokio::time::sleep(Duration::from_millis(50)).await;

        let started = Instant::now();
        backend
            .cancel_workflow_and_drain("wf_1", Duration::from_secs(5))
            .await;
        assert!(
            started.elapsed() >= Duration::from_millis(60),
            "回执早于孩子落地：{:?}",
            started.elapsed()
        );
        let result = child.await.expect("join").expect("spawn");
        assert!(result.cancelled, "{result:?}");
    }

    /// 取消只收自己那一个 run 的孩子，别的 run 不受影响。
    #[tokio::test]
    async fn cancelling_one_run_leaves_another_runs_children_alone() {
        let (tx, rx) = mpsc::unbounded_channel();
        let coord = SubagentCoordinator::new(rx, GatedRunner, CoordinatorConfig::default());
        tokio::spawn(async move {
            coord.run().await;
        });
        let backend = ChannelBackend::for_session(tx, ROOT_IDENTITY);

        let mut children = Vec::new();
        for (id, run_id) in [("sa-a", "wf_1"), ("sa-b", "wf_2")] {
            let backend = backend.clone();
            children.push(tokio::spawn(async move {
                backend.spawn(workflow_request(id, run_id)).await
            }));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;

        backend
            .cancel_workflow_and_drain("wf_1", Duration::from_secs(5))
            .await;
        let mut children = children.into_iter();
        let cancelled = children
            .next()
            .unwrap()
            .await
            .expect("join")
            .expect("spawn");
        assert!(cancelled.cancelled, "wf_1 的孩子应当被取消");

        let survivor = children.next().unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(150), survivor)
                .await
                .is_err(),
            "wf_2 的孩子不该被 wf_1 的取消波及"
        );
    }
}
