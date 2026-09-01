//! Grok `task` coordinator + dock isolate `ChildRunner`.
//!
//! Named service `"subagents"` is provided by `tool-task`. Continuable spawn
//! is a separate plugin: [`tool_subagent`].

mod admission;
pub mod backend;
mod control;
pub mod coordinator;
mod coordinator_state;
mod execute;
mod format;
mod interjection;
mod runner;
mod store;
mod subagent_tool;
mod tool_error;
pub mod types;

pub use subagent_tool::tool_subagent;

use std::sync::Arc;
use std::time::Instant;

use cordis::{plugin, Inject, Plugin};
use tokio::sync::{mpsc, oneshot};

use crate::names::{SUBAGENTS, TOOLS};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::ToolCall;

use backend::{ChannelBackend, SubagentBackend};
use coordinator::{CoordinatorConfig, SubagentCoordinator};
use runner::{DockChildRunner, PARENT_SESSION_ID};
use store::ChildStore;
use types::{
    SubagentCancelOutcome, SubagentCancelRequest, SubagentCancelTarget, SubagentEvent,
    SubagentOwner, SubagentRequest, SubagentRuntimeOverrides,
};

/// Copied from Grok `MAX_SUBAGENT_DEPTH`.
pub const MAX_SUBAGENT_DEPTH: u32 = 1;

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
    /// Spawned with the `subagent` tool (mailbox). Not `get_task_output`.
    pub mailbox: bool,
}

impl SubagentSnap {
    pub fn running(&self) -> bool {
        !self.done && !self.idle
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
    pub fn events(&self, id: &str) -> Vec<crate::types::LogEvent> {
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
                            mailbox: false,
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
                parent_session_id: Some(PARENT_SESSION_ID.into()),
                target: SubagentCancelTarget::SubagentId(id.to_owned()),
                respond_to,
            }));
        let _ = SubagentCancelOutcome::Cancelled;
        Some(format!("killed {id}"))
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

    /// Same as [`Self::spawn_and_wait`], with capability / schema overrides
    /// from a Rhai `agent()` call.
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
            parent_session_id: PARENT_SESSION_ID.into(),
            parent_prompt_id: None,
            resume_from: None,
            cwd: None,
            runtime_overrides,
            run_in_background: false,
            surface_completion: false,
            await_to_completion: true,
            fork_context: false,
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

    pub fn mailbox_child(&self, id: &str) -> bool {
        self.snapshot(id).is_some_and(|s| s.mailbox)
    }

    pub fn has_parent_notices(&self) -> bool {
        self.store.has_parent_notices()
    }

    /// Enqueue a parent mailbox notice and wake the session actor.
    pub fn enqueue_parent_report(&self, from: &str, output: &str) {
        self.store.push_report(from, output);
    }

    pub fn parent_wake(&self) -> std::sync::Arc<tokio::sync::Notify> {
        self.store.parent_wake()
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
        mailbox: false,
    }
}

pub fn tool_task() -> Plugin {
    plugin("tool-task", Inject::from([TOOLS]), |ctx, _: &()| {
        let (tx, rx) = mpsc::unbounded_channel();
        let store = ChildStore::new();
        let backend = ChannelBackend::new(tx);
        let runner = DockChildRunner {
            ctx: ctx.clone(),
            store: store.clone(),
        };
        let coord = SubagentCoordinator::new(rx, runner, CoordinatorConfig::default());
        tokio::spawn(async move {
            coord.run().await;
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
        own_registered(ctx, vec![tools.register(execute::spec(), task_body)?])?;
        Ok(None)
    })
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
        SubagentProgress,
    };
    use crate::task::types::{
        SubagentDescribeOutcome, SubagentResult, SubagentTypeSummary, SubagentValidateTypeOutcome,
    };

    struct NopControl;

    impl ChildControl for NopControl {
        type ProgressFuture = std::future::Ready<SubagentProgress>;

        fn progress(&self) -> Self::ProgressFuture {
            std::future::ready(SubagentProgress::default())
        }

        fn cancel(&self) {}
    }

    struct ImmediateRunner;

    impl ChildRunner for ImmediateRunner {
        type Control = NopControl;
        type CompletionData = ();
        type RunFuture = SendBoxFuture<ChildRunOutput<()>>;
        type ValidateFuture = SendBoxFuture<SubagentValidateTypeOutcome>;
        type DescribeFuture = SendBoxFuture<SubagentDescribeOutcome>;

        fn run(&self, run: ChildRunRequest<Self::Control>) -> Self::RunFuture {
            Box::pin(async move {
                let _ = run
                    .reporter
                    .started(StartedChild {
                        child_session_id: run.request.id.clone(),
                        persona: None,
                        resumed_from: None,
                        child_cwd: String::new(),
                        worktree_path: None,
                        effective_model_id: String::new(),
                        definition_background: false,
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
                    completion_data: (),
                    snapshot_ref: None,
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

        fn describe_type(
            &self,
            _subagent_type: String,
            _harness_agent_type: Option<String>,
            _parent_session_id: String,
        ) -> Self::DescribeFuture {
            Box::pin(async { SubagentDescribeOutcome::Ok(SubagentTypeSummary::default()) })
        }

        fn on_completed(&self, _completion: crate::task::coordinator::ChildCompletion<()>) {}
    }

    #[tokio::test]
    async fn channel_backend_spawn_goes_through_coordinator() {
        let (tx, rx) = mpsc::unbounded_channel();
        let coord = SubagentCoordinator::new(rx, ImmediateRunner, CoordinatorConfig::default());
        tokio::spawn(async move {
            coord.run().await;
        });
        let backend = ChannelBackend::new(tx);
        let result = backend
            .spawn(SubagentRequest {
                id: "sa-coord".into(),
                prompt: "hi".into(),
                description: "coord test".into(),
                subagent_type: "general-purpose".into(),
                parent_session_id: "dock".into(),
                parent_prompt_id: None,
                resume_from: None,
                cwd: None,
                runtime_overrides: SubagentRuntimeOverrides::default(),
                run_in_background: false,
                surface_completion: true,
                await_to_completion: true,
                fork_context: false,
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
