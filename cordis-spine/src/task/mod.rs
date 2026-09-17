//! Subagent coordinator + dock isolate `ChildRunner`.
//!
//! One plugin provides the named service `"subagents"` and registers the whole
//! model-facing surface: `task` (spawn) plus the mailbox tools
//! (`send_message` / `list_agents` / `interrupt_agent` / `report`).
//!
//! Every child is continuable: it runs a turn, parks idle, accepts
//! `send_message`, and pushes a turn-end notice to the parent. The parent is
//! never required to poll.

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
use crate::session::ROOT_IDENTITY;
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use cordis_base::types::ToolCall;

use admission::SubagentLimits;
use backend::{ChannelBackend, SubagentBackend};
use coordinator::{CoordinatorConfig, SubagentCoordinator};
use runner::DockChildRunner;
use store::ChildStore;
use types::{
    SubagentCancelRequest, SubagentCancelTarget, SubagentEvent, SubagentOwner, SubagentRequest,
    SubagentRuntimeOverrides,
};

/// Copied from Grok `MAX_SUBAGENT_DEPTH`.
pub const MAX_SUBAGENT_DEPTH: u32 = 1;

/// Idle children older than this are disposed (their slot and transcript stay
/// readable, so `get_task_output` and `resume_from` keep working).
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
}

impl Default for TaskConfig {
    fn default() -> Self {
        Self {
            foreground_budget: Duration::from_secs(120),
            limits: SubagentLimits::from_env(),
        }
    }
}

/// Named `"subagents"` service. Live-lookup; do not capture the Arc.
pub struct Subagents {
    backend: ChannelBackend,
    store: ChildStore,
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

    pub fn parent_wake(&self) -> Arc<tokio::sync::Notify> {
        self.store.parent_wake()
    }

    /// Spawn a child and wait. Workflow host `SpawnAgent` live-looks `"subagents"`.
    pub async fn spawn_and_wait(
        &self,
        prompt: String,
        description: String,
        subagent_type: String,
    ) -> SubagentSnap {
        self.spawn_and_wait_with(
            prompt,
            description,
            subagent_type,
            SubagentRuntimeOverrides::default(),
        )
        .await
    }

    /// Same as [`Self::spawn_and_wait`], with the runtime overrides a
    /// workflow-host `agent()` call can express.
    pub async fn spawn_and_wait_with(
        &self,
        prompt: String,
        description: String,
        subagent_type: String,
        runtime_overrides: SubagentRuntimeOverrides,
    ) -> SubagentSnap {
        let id = uuid::Uuid::now_v7().to_string();
        self.remember(
            &id,
            description.clone(),
            subagent_type.clone(),
            SubagentOwner::workflow("host"),
        );
        let request = SubagentRequest {
            id: id.clone(),
            prompt,
            description,
            subagent_type,
            parent_session_id: ROOT_IDENTITY.into(),
            resume_from: None,
            runtime_overrides,
            run_in_background: false,
            surface_completion: false,
            await_to_completion: true,
            owner: SubagentOwner::workflow("host"),
            cancel_token: tokio_util::sync::CancellationToken::new(),
        };
        match self.backend.spawn(request).await {
            Ok(_) => self.snapshot(&id).unwrap_or_else(|| missing_snap(&id)),
            Err(e) => {
                self.store.finish(&id, e.to_string(), false);
                self.snapshot(&id).unwrap_or_else(|| missing_snap(&id))
            }
        }
    }

    pub(super) fn backend(&self) -> ChannelBackend {
        self.backend.clone()
    }

    pub(super) fn remember(
        &self,
        id: &str,
        description: String,
        subagent_type: String,
        owner: SubagentOwner,
    ) {
        self.store
            .ensure_owned(id, description, subagent_type, owner);
    }
}

fn missing_snap(id: &str) -> SubagentSnap {
    SubagentSnap {
        id: id.into(),
        description: String::new(),
        subagent_type: String::new(),
        done: true,
        idle: false,
        cancelled: false,
        output: String::new(),
        started_at: Instant::now(),
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
            let runner = DockChildRunner {
                ctx: ctx.clone(),
                store: store.clone(),
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

            ctx.provide(SUBAGENTS, Subagents { backend, store })?;
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
                        control::run_send(&sub, call).await
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
                        control::run_list(&sub, call).await
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
                        control::run_interrupt(&sub, call).await
                    })
                })
            };
            let report_body: ToolBody = {
                let ctx = ctx.clone();
                Arc::new(move |call: ToolCall| {
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
                            return tool_result(call, "Error: subagents is not mounted");
                        };
                        let sessions = crate::tools::exec_ctx()
                            .and_then(|c| c.get::<crate::session::Sessions>(crate::names::SESSIONS))
                            .or_else(|| {
                                ctx.get::<crate::session::Sessions>(crate::names::SESSIONS)
                            });
                        control::run_report(&sub, sessions, call).await
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
                    tools.register(control::report_spec(), report_body)?,
                ],
            )?;
            Ok(None)
        },
    )
}

pub fn render_subagent(s: &SubagentSnap) -> String {
    let status = if s.cancelled {
        "cancelled"
    } else if s.done {
        "done"
    } else if s.idle {
        "idle"
    } else {
        "running"
    };
    format!(
        "[{status}] {} [{}] {}\n{}",
        s.id, s.subagent_type, s.description, s.output
    )
}

#[cfg(test)]
mod coordinator_spawn_tests {
    use super::*;
    use crate::task::coordinator::{
        ChildControl, ChildRunOutput, ChildRunRequest, ChildRunner, SendBoxFuture, StartedChild,
    };
    use crate::task::types::{SubagentResult, SubagentValidateTypeOutcome};

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

        fn on_completed(&self, _completion: crate::task::coordinator::ChildCompletion) {}
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
}
