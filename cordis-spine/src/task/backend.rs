//! Backend trait abstracting how subagent operations are dispatched.
//!
//! `SubagentBackend` decouples the tool implementations (`TaskTool`,
//! `TaskOutputTool`, `KillTaskTool`) from the transport mechanism used to
//! communicate with the subagent coordinator.
//!
//! All hosts use [`ChannelBackend`].
//! The receiver is owned by the shared single-writer coordinator actor; only
//! the child runner plugged into that actor differs by host.

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use super::tool_error::ToolError;
use super::types::{
    SubagentCancelOutcome, SubagentCancelRequest, SubagentCancelTarget, SubagentEvent,
    SubagentRequest, SubagentResult, SubagentSpawnRequest, SubagentValidateTypeOutcome,
    SubagentValidateTypeRequest,
};

/// Abstraction over the mechanism used to spawn, query, and cancel subagents.
///
/// Injected into `Resources` as [`SubagentBackendResource`] so that
/// `TaskTool`, `TaskOutputTool`, and `KillTaskTool` can operate
/// identically regardless of the underlying transport.
#[async_trait::async_trait]
pub trait SubagentBackend: Send + Sync + 'static {
    /// Spawn a subagent and await its result.
    ///
    /// For blocking mode the caller awaits the returned future directly.
    /// For background mode the caller spawns a tokio task around this call
    /// and drops the receiver immediately.
    async fn spawn(&self, request: SubagentRequest) -> Result<SubagentResult, ToolError>;

    /// Validate a subagent type synchronously before spawning.
    /// Returns `ValidationUnavailable` on channel close / responder drop / timeout.
    async fn validate_type(
        &self,
        subagent_type: &str,
        parent_session_id: &str,
    ) -> SubagentValidateTypeOutcome;
}

/// Wraps a single `mpsc::UnboundedSender<SubagentEvent>` that carries
/// spawn, query, and cancel messages to the coordinator. The oneshot for
/// `spawn` is created inside the backend so callers never manage it.
#[derive(Clone)]
pub struct ChannelBackend {
    tx: mpsc::UnboundedSender<SubagentEvent>,
    parent_session_id: Option<Arc<str>>,
}

impl ChannelBackend {
    /// Bind model-facing operations to one parent session.
    pub fn for_session(
        tx: mpsc::UnboundedSender<SubagentEvent>,
        parent_session_id: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            tx,
            parent_session_id: Some(parent_session_id.into()),
        }
    }

    fn parent_session_id(&self) -> Option<String> {
        self.parent_session_id.as_deref().map(str::to_owned)
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<SubagentEvent> {
        self.tx.clone()
    }

    /// Fire-and-forget ParentSession cancel used by the shell Stop path.
    /// Returns false when the backend is unbound or the channel is closed.
    pub fn request_cancel_parent_session(
        &self,
        respond_to: oneshot::Sender<SubagentCancelOutcome>,
    ) -> bool {
        let Some(parent_session_id) = self.parent_session_id() else {
            return false;
        };
        self.tx
            .send(SubagentEvent::Cancel(SubagentCancelRequest {
                parent_session_id: Some(parent_session_id),
                target: SubagentCancelTarget::ParentSession,
                respond_to,
            }))
            .is_ok()
    }

    /// 同 [`Self::request_cancel_parent_session`]，但取消**指定**会话的孩子。
    ///
    /// 分页各是一条并列主线（`main#2`…），Stop / 新会话只该动自己那一页的；
    /// 绑定在 backend 上的那个 id 只是缺省值。
    pub fn request_cancel_session(
        &self,
        parent_session_id: &str,
        respond_to: oneshot::Sender<SubagentCancelOutcome>,
    ) -> bool {
        self.tx
            .send(SubagentEvent::Cancel(SubagentCancelRequest {
                parent_session_id: Some(parent_session_id.to_owned()),
                target: SubagentCancelTarget::ParentSession,
                respond_to,
            }))
            .is_ok()
    }

    /// Re-open Task spawns after a prior ParentSession stop (start of next turn).
    pub fn open_spawn_admission(&self) -> bool {
        let Some(parent_session_id) = self.parent_session_id() else {
            return false;
        };
        self.tx
            .send(SubagentEvent::OpenSpawnAdmission { parent_session_id })
            .is_ok()
    }

    /// 同 [`Self::open_spawn_admission`]，但针对**指定**会话。
    pub fn open_spawn_admission_for(&self, parent_session_id: &str) -> bool {
        self.tx
            .send(SubagentEvent::OpenSpawnAdmission {
                parent_session_id: parent_session_id.to_owned(),
            })
            .is_ok()
    }

    /// Delete-path teardown: cancel `parent_session_id`'s children and wait, up
    /// to `budget`, for the coordinator to drain them. Owns the event shape and
    /// the wait policy so the host does not rebuild them. Best-effort: on a
    /// closed channel or an elapsed budget it logs and returns (the coordinator
    /// keeps admission closed until its own backstop deadline).
    pub async fn teardown_session_and_drain(
        &self,
        parent_session_id: &str,
        budget: std::time::Duration,
    ) {
        let (respond_to, response_rx) = oneshot::channel();
        if self
            .tx
            .send(SubagentEvent::TeardownSession {
                parent_session_id: parent_session_id.to_owned(),
                respond_to: Some(respond_to),
            })
            .is_err()
        {
            return;
        }
        // Err = budget elapsed with children still finishing; Ok = drained or
        // the responder was dropped.
        if tokio::time::timeout(budget, response_rx).await.is_err() {
            tracing::warn!(
                parent_session_id,
                budget_ms = budget.as_millis() as u64,
                "subagents still running after session-delete drain budget"
            );
        }
    }
}

struct CancelResultReceiverOnDrop {
    cancel_token: tokio_util::sync::CancellationToken,
    armed: bool,
}

impl Drop for CancelResultReceiverOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.cancel_token.cancel();
        }
    }
}

#[async_trait::async_trait]
impl SubagentBackend for ChannelBackend {
    async fn spawn(&self, mut request: SubagentRequest) -> Result<SubagentResult, ToolError> {
        if let Some(parent_session_id) = self.parent_session_id.as_deref() {
            request.parent_session_id = parent_session_id.to_owned();
        }
        let (respond_to, response_rx) = oneshot::channel();
        let cancel_on_receiver_drop = request.owner.is_workflow();
        let cancel_token = request.cancel_token.clone();
        self.tx
            .send(SubagentEvent::Spawn(SubagentSpawnRequest {
                request: Box::new(request),
                result_tx: respond_to,
            }))
            .map_err(|_| {
                ToolError::custom(
                    "channel_closed",
                    "Subagent coordinator channel closed — cannot spawn subagent",
                )
            })?;

        let mut receiver_guard = cancel_on_receiver_drop.then(|| CancelResultReceiverOnDrop {
            cancel_token: cancel_token.clone(),
            armed: true,
        });
        let result = response_rx.await;
        if result.is_ok() {
            if let Some(guard) = receiver_guard.as_mut() {
                guard.armed = false;
            }
        } else if cancel_on_receiver_drop {
            cancel_token.cancel();
        }
        result.map_err(|_| {
            ToolError::custom(
                "channel_closed",
                "Subagent result channel dropped — child session may have crashed",
            )
        })
    }

    async fn validate_type(
        &self,
        subagent_type: &str,
        parent_session_id: &str,
    ) -> SubagentValidateTypeOutcome {
        let parent_session_id = self
            .parent_session_id
            .as_deref()
            .unwrap_or(parent_session_id);
        let (respond_to, response_rx) = oneshot::channel();
        if self
            .tx
            .send(SubagentEvent::ValidateType(SubagentValidateTypeRequest {
                subagent_type: subagent_type.to_string(),
                parent_session_id: parent_session_id.to_string(),
                respond_to,
            }))
            .is_err()
        {
            tracing::warn!(
                subagent_type,
                "coordinator validation channel closed, treating as ValidationUnavailable",
            );
            return SubagentValidateTypeOutcome::ValidationUnavailable;
        }
        let timeout = validate_type_timeout();
        match tokio::time::timeout(timeout, response_rx).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => {
                tracing::warn!(
                    subagent_type,
                    "coordinator validation responder dropped, treating as ValidationUnavailable",
                );
                SubagentValidateTypeOutcome::ValidationUnavailable
            }
            Err(_) => {
                tracing::warn!(
                    subagent_type,
                    timeout_ms = timeout.as_millis() as u64,
                    "coordinator validation timed out, treating as ValidationUnavailable",
                );
                SubagentValidateTypeOutcome::ValidationUnavailable
            }
        }
    }
}

/// Default `validate_type` timeout. Override via [`VALIDATE_TYPE_TIMEOUT_ENV_VAR`].
pub const VALIDATE_TYPE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Env-var override for [`VALIDATE_TYPE_TIMEOUT`] (positive milliseconds).
pub const VALIDATE_TYPE_TIMEOUT_ENV_VAR: &str = "XAI_VALIDATE_TYPE_TIMEOUT_MS";

/// Validation timeout, honoring the env-var override.
pub fn validate_type_timeout() -> std::time::Duration {
    let value = std::env::var(VALIDATE_TYPE_TIMEOUT_ENV_VAR).ok();
    parse_timeout_ms(value.as_deref())
        .map(std::time::Duration::from_millis)
        .unwrap_or(VALIDATE_TYPE_TIMEOUT)
}

/// Parse a positive `u64` millisecond value; `None` for unset, invalid, or zero.
pub(crate) fn parse_timeout_ms(value: Option<&str>) -> Option<u64> {
    value.and_then(|raw| {
        let n = raw.parse::<u64>().ok()?;
        (n > 0).then_some(n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ROOT_IDENTITY;

    /// 分页各是一条并列主线：取消 / 放行都要打到**指定**会话，不能回落到
    /// backend 上绑的那个缺省 id —— 那样第 2 页按 Stop 会收掉第 1 页的孩子。
    #[tokio::test]
    async fn per_session_cancel_and_admission_target_the_given_id() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let backend = ChannelBackend::for_session(tx, ROOT_IDENTITY);

        let (respond_to, _rx) = oneshot::channel();
        assert!(backend.request_cancel_session("main#2", respond_to));
        match rx.recv().await.expect("cancel event") {
            SubagentEvent::Cancel(request) => {
                assert_eq!(request.parent_session_id.as_deref(), Some("main#2"));
                assert!(matches!(
                    request.target,
                    SubagentCancelTarget::ParentSession
                ));
            }
            _ => panic!("expected Cancel"),
        }

        assert!(backend.open_spawn_admission_for("main#2"));
        match rx.recv().await.expect("admission event") {
            SubagentEvent::OpenSpawnAdmission { parent_session_id } => {
                assert_eq!(parent_session_id, "main#2");
            }
            _ => panic!("expected OpenSpawnAdmission"),
        }

        // 不带 id 的那两个仍然走绑定值，老行为不变。
        let (respond_to, _rx) = oneshot::channel();
        assert!(backend.request_cancel_parent_session(respond_to));
        match rx.recv().await.expect("bound cancel") {
            SubagentEvent::Cancel(request) => {
                assert_eq!(request.parent_session_id.as_deref(), Some(ROOT_IDENTITY));
            }
            _ => panic!("expected Cancel"),
        }
    }
}
