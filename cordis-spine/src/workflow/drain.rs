//! SessionActor-shaped drain: oneshot ack then `xai_workflow::run_workflow`.
//! Host `SpawnAgent` live-looks `"subagents"` (`ChannelBackend::spawn`, `await_to_completion`).

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use xai_workflow::{
    run_workflow, validate_script, AgentResult, BudgetState, HostError, Journal,
    WorkflowHostRequest, WorkflowOutcome, WorkflowRunParams,
};

use super::grok_tool::{
    WorkflowLaunchAck, WorkflowLaunchEnvelope, WorkflowSource, WorkflowToolInput,
};
use super::registry::{resolve_by_name, resolve_by_path, resolve_inline, ResolveError};
use crate::names::SUBAGENTS;
use crate::task::Subagents;

#[derive(Clone, Debug)]
pub struct WorkflowRunSnap {
    pub run_id: String,
    pub name: String,
    pub objective: String,
    pub status: String,
    pub current_phase: Option<String>,
    pub elapsed_ms: u64,
    pub received_at: Instant,
    pub builtin: bool,
    pub pause_message: Option<String>,
    pub result_summary: Option<String>,
    pub agents_running: u32,
}

struct RunSlot {
    snap: WorkflowRunSnap,
    started: Instant,
    cancel: CancellationToken,
}

pub struct WorkflowState {
    inner: Mutex<HashMap<String, RunSlot>>,
    seq: Mutex<u64>,
}

impl WorkflowState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            seq: Mutex::new(1),
        }
    }

    pub fn list(&self) -> Vec<WorkflowRunSnap> {
        let mut rows: Vec<_> = self
            .inner
            .lock()
            .unwrap()
            .values()
            .map(|s| {
                let mut snap = s.snap.clone();
                if snap.status == "active" {
                    snap.elapsed_ms = s.started.elapsed().as_millis() as u64;
                    snap.received_at = Instant::now();
                }
                snap
            })
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.received_at));
        rows
    }

    fn alloc_name(&self, base: &str) -> String {
        let taken: Vec<String> = self
            .inner
            .lock()
            .unwrap()
            .values()
            .map(|s| s.snap.name.clone())
            .collect();
        if !taken.iter().any(|n| n == base) {
            return base.to_string();
        }
        for i in 2..10_000 {
            let candidate = format!("{base}-{i}");
            if !taken.iter().any(|n| n == &candidate) {
                return candidate;
            }
        }
        format!("{base}-x")
    }

    fn insert(&self, snap: WorkflowRunSnap, cancel: CancellationToken) {
        self.inner.lock().unwrap().insert(
            snap.run_id.clone(),
            RunSlot {
                started: Instant::now(),
                snap,
                cancel,
            },
        );
    }

    fn patch(&self, run_id: &str, f: impl FnOnce(&mut WorkflowRunSnap)) {
        if let Some(slot) = self.inner.lock().unwrap().get_mut(run_id) {
            f(&mut slot.snap);
        }
    }
}

pub async fn drain_loop_with_ctx(
    state: Arc<WorkflowState>,
    ctx: cordis::Context,
    mut rx: mpsc::UnboundedReceiver<WorkflowLaunchEnvelope>,
) {
    while let Some((req, ack)) = rx.recv().await {
        handle_launch(state.clone(), ctx.clone(), req.input, ack).await;
    }
}

async fn handle_launch(
    state: Arc<WorkflowState>,
    ctx: cordis::Context,
    mut input: WorkflowToolInput,
    ack: tokio::sync::oneshot::Sender<WorkflowLaunchAck>,
) {
    input.normalize();
    if let Err(detail) = input.validate() {
        let _ = ack.send(WorkflowLaunchAck::Rejected {
            code: "workflow_invalid_input",
            detail,
        });
        return;
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let resolved = match &input.source {
        WorkflowSource::Name { name } => resolve_by_name(name, Some(&cwd)).map_err(resolve_detail),
        WorkflowSource::Script { script } => resolve_inline(script.clone()).map_err(resolve_detail),
        WorkflowSource::ScriptPath { script_path } => {
            resolve_by_path(std::path::Path::new(script_path), &cwd, None).map_err(resolve_detail)
        }
        WorkflowSource::Resume { resume_from_run_id } => {
            let _ = ack.send(WorkflowLaunchAck::Rejected {
                code: "workflow_resume_unsupported",
                detail: format!(
                    "resume_from_run_id {resume_from_run_id} is not available after process restart"
                ),
            });
            return;
        }
    };
    let resolved = match resolved {
        Ok(r) => r,
        Err(detail) => {
            let _ = ack.send(WorkflowLaunchAck::Rejected {
                code: "workflow_resolve_failed",
                detail,
            });
            return;
        }
    };

    if input.validate_only {
        match validate_script(&resolved.script, input.args.clone()) {
            Ok(report) => {
                let _ = ack.send(WorkflowLaunchAck::Validated {
                    name: report.name,
                    phases: report.phases,
                    summary: report.outcome_summary,
                });
            }
            Err(err) => {
                let _ = ack.send(WorkflowLaunchAck::Rejected {
                    code: "workflow_validate_failed",
                    detail: err.to_string(),
                });
            }
        }
        return;
    }

    let n = {
        let mut seq = state.seq.lock().unwrap();
        let n = *seq;
        *seq += 1;
        n
    };
    let run_id = format!("wf_{n}");
    let display = state.alloc_name(&resolved.meta.name);
    let cancel = CancellationToken::new();
    let snap = WorkflowRunSnap {
        run_id: run_id.clone(),
        name: display.clone(),
        objective: resolved.meta.description.clone(),
        status: "active".into(),
        current_phase: None,
        elapsed_ms: 0,
        received_at: Instant::now(),
        builtin: matches!(resolved.source, super::registry::WorkflowSource::Builtin),
        pause_message: None,
        result_summary: None,
        agents_running: 0,
    };
    state.insert(snap, cancel.clone());
    let _ = ack.send(WorkflowLaunchAck::Started {
        run_id: run_id.clone(),
        task_id: run_id.clone(),
        name: display,
        script_path: None,
    });

    let (host_tx, host_rx) = mpsc::unbounded_channel();
    let host_state = state.clone();
    let host_id = run_id.clone();
    let host_ctx = ctx.clone();
    let host_cancel = cancel.clone();
    tokio::spawn(async move {
        host_loop(host_ctx, host_state, host_id, host_rx, host_cancel).await;
    });

    let script = resolved.script;
    let args = input.args.unwrap_or(serde_json::json!({}));
    let engine_cancel = cancel.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        run_workflow(WorkflowRunParams {
            script,
            args,
            journal: Journal::new(None),
            host_tx,
            cancel: engine_cancel,
            max_ops: WorkflowRunParams::DEFAULT_MAX_OPS,
        })
    })
    .await
    .unwrap_or_else(|e| WorkflowOutcome::Failed {
        error: format!("workflow engine join: {e}"),
    });

    state.patch(&run_id, |snap| match &outcome {
        WorkflowOutcome::Completed { result } => {
            snap.status = "complete".into();
            snap.result_summary = Some(result.to_string());
        }
        WorkflowOutcome::Paused { kind, message } => {
            snap.status = format!("{}_paused", kind.as_str());
            snap.pause_message = Some(message.clone());
        }
        WorkflowOutcome::BudgetExceeded { message } => {
            snap.status = "failed".into();
            snap.pause_message = Some(message.clone());
        }
        WorkflowOutcome::Cancelled => {
            snap.status = "cancelled".into();
        }
        WorkflowOutcome::Failed { error } => {
            snap.status = "failed".into();
            snap.pause_message = Some(error.clone());
        }
    });
}

fn resolve_detail(err: ResolveError) -> String {
    err.to_string()
}

async fn host_loop(
    ctx: cordis::Context,
    state: Arc<WorkflowState>,
    run_id: String,
    mut rx: mpsc::UnboundedReceiver<WorkflowHostRequest>,
    cancel: CancellationToken,
) {
    let spent = 0u64;
    let mut reserved = 0u64;
    let scratch = cordis_base::config::dock_home()
        .join("scratch")
        .join(&run_id);
    let _ = std::fs::create_dir_all(&scratch);
    loop {
        let req = tokio::select! {
            req = rx.recv() => match req {
                Some(req) => req,
                None => break,
            },
            _ = cancel.cancelled() => {
                while let Ok(req) = rx.try_recv() {
                    reply_cancelled(req);
                }
                break;
            }
        };
        match req {
            WorkflowHostRequest::ReserveAgentCalls { count, reply } => {
                reserved = reserved.saturating_add(count);
                let _ = reply.send(Ok(()));
            }
            WorkflowHostRequest::ReleaseAgentCalls { count, reply } => {
                reserved = reserved.saturating_sub(count);
                let _ = reply.send(Ok(()));
            }
            WorkflowHostRequest::SpawnAgent { opts, reply } => {
                let ctx = ctx.clone();
                let state = state.clone();
                let run_id = run_id.clone();
                tokio::spawn(async move {
                    state.patch(&run_id, |s| {
                        s.agents_running = s.agents_running.saturating_add(1)
                    });
                    let result = spawn_agent(&ctx, opts).await;
                    state.patch(&run_id, |s| {
                        s.agents_running = s.agents_running.saturating_sub(1)
                    });
                    let _ = reply.send(result);
                });
            }
            WorkflowHostRequest::Phase { title, .. } => {
                state.patch(&run_id, |s| s.current_phase = Some(title));
            }
            WorkflowHostRequest::Log { .. } | WorkflowHostRequest::Telemetry { .. } => {}
            WorkflowHostRequest::BudgetQuery { reply } => {
                let _ = reply.send(Ok(BudgetState {
                    total: None,
                    spent,
                    reserved,
                    remaining: None,
                }));
            }
            WorkflowHostRequest::RenderTemplate { reply, .. } => {
                let _ = reply.send(Err(HostError::Unsupported(
                    "render_template is not available".into(),
                )));
            }
            WorkflowHostRequest::WriteScratchFile {
                name,
                content,
                reply,
            } => {
                let path = scratch.join(sanitize_scratch(&name));
                let out = std::fs::write(&path, content)
                    .map(|_| path.display().to_string())
                    .map_err(|e| HostError::Failed(e.to_string()));
                let _ = reply.send(out);
            }
            WorkflowHostRequest::ReadScratchFile { name, reply } => {
                let path = scratch.join(sanitize_scratch(&name));
                let out =
                    std::fs::read_to_string(&path).map_err(|e| HostError::Failed(e.to_string()));
                let _ = reply.send(out);
            }
            WorkflowHostRequest::GitDiffSince { reply, .. } => {
                let _ = reply.send(Err(HostError::Unsupported(
                    "git_diff_since is not available".into(),
                )));
            }
        }
        let _ = spent;
    }
}

async fn spawn_agent(
    ctx: &cordis::Context,
    opts: xai_workflow::AgentOpts,
) -> Result<AgentResult, HostError> {
    let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
        return Err(HostError::Failed("subagents is not mounted".into()));
    };
    let mut prompt = opts.prompt;
    if prompt.trim().is_empty() {
        return Err(HostError::Failed("agent prompt is empty".into()));
    }
    if let Some(schema) = &opts.output_schema {
        prompt = append_output_schema(&prompt, schema);
    }
    let description = opts
        .label
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "workflow-agent".into());
    let subagent_type = opts
        .agent_type
        .as_deref()
        .unwrap_or("general-purpose")
        .to_string();
    if opts.model.is_some()
        || opts.effort.is_some()
        || opts.capability_mode.is_some()
        || opts.max_output_tokens.is_some()
    {
        tracing::debug!(
            "workflow agent() model/effort/capability/max_output_tokens are ignored; \
             dock children share the parent model and full toolset"
        );
    }
    let started = Instant::now();
    let snap = sub.spawn_and_wait(prompt, description, subagent_type).await;
    Ok(AgentResult {
        agent_id: snap.id,
        success: !snap.cancelled && snap.done,
        output: agent_output_value(&snap.output),
        cancelled: snap.cancelled,
        tokens_used: 0,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn append_output_schema(prompt: &str, schema: &serde_json::Value) -> String {
    format!(
        "{prompt}\n\n<output-schema>\n{schema}\n</output-schema>\nReply with JSON only that matches the schema. Do not wrap it in markdown fences."
    )
}

fn agent_output_value(raw: &str) -> serde_json::Value {
    let trimmed = raw.trim();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return value;
    }
    if let Some(fenced) = extract_fenced_json(trimmed) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&fenced) {
            return value;
        }
    }
    serde_json::json!(raw)
}

fn extract_fenced_json(raw: &str) -> Option<String> {
    let after_open = raw.split_once("```")?.1;
    let body = after_open
        .strip_prefix("json")
        .or_else(|| after_open.strip_prefix("JSON"))
        .unwrap_or(after_open);
    let body = body
        .strip_prefix('\n')
        .or_else(|| body.strip_prefix("\r\n"))
        .unwrap_or(body);
    let inner = body.split_once("```")?.0.trim();
    if inner.starts_with('{') || inner.starts_with('[') {
        Some(inner.to_string())
    } else {
        None
    }
}

fn sanitize_scratch(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .take(255)
        .collect()
}

fn reply_cancelled(req: WorkflowHostRequest) {
    use WorkflowHostRequest as R;
    match req {
        R::ReserveAgentCalls { reply, .. } | R::ReleaseAgentCalls { reply, .. } => {
            let _ = reply.send(Err(HostError::Cancelled));
        }
        R::SpawnAgent { reply, .. } => {
            let _ = reply.send(Err(HostError::Cancelled));
        }
        R::BudgetQuery { reply } => {
            let _ = reply.send(Err(HostError::Cancelled));
        }
        R::RenderTemplate { reply, .. }
        | R::WriteScratchFile { reply, .. }
        | R::ReadScratchFile { reply, .. }
        | R::GitDiffSince { reply, .. } => {
            let _ = reply.send(Err(HostError::Cancelled));
        }
        R::Phase { .. } | R::Log { .. } | R::Telemetry { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_output_parses_json_object_or_fence() {
        let obj = agent_output_value(r#"{"questions":["a"]}"#);
        assert_eq!(obj["questions"][0], "a");
        let fenced = agent_output_value("here\n```json\n{\"claim\":\"x\"}\n```\n");
        assert_eq!(fenced["claim"], "x");
        let plain = agent_output_value("not json");
        assert_eq!(plain, serde_json::json!("not json"));
    }
}
