//! Background jobs (`ctx.jobs`). Bash `is_background` and `tool-jobs` use this.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cordis::{plugin, Inject, Plugin};
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::sync::Mutex as AsyncMutex;

use crate::names::{JOBS, SUBAGENTS, TOOLS};
use crate::task::{render_subagent, Subagents};
use crate::tools::{own_registered, tool_result, ToolBody, Tools};
use crate::types::{ToolCall, ToolResult, ToolSpec};

#[derive(Clone, Debug)]
pub struct JobSnapshot {
    pub id: String,
    pub command: String,
    pub done: bool,
    pub output: String,
}

struct Job {
    command: String,
    output: Mutex<String>,
    done: AtomicBool,
    child: AsyncMutex<Option<Child>>,
}

/// Named `"jobs"` service. Live-lookup; do not capture the Arc.
pub struct Jobs {
    seq: AtomicU64,
    inner: Mutex<HashMap<String, Arc<Job>>>,
}

impl Jobs {
    pub fn new() -> Self {
        Self {
            seq: AtomicU64::new(1),
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn start(&self, command: impl Into<String>) -> String {
        let command = command.into();
        let id = format!("job-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        let job = Arc::new(Job {
            command: command.clone(),
            output: Mutex::new(String::new()),
            done: AtomicBool::new(false),
            child: AsyncMutex::new(None),
        });
        self.inner.lock().unwrap().insert(id.clone(), job.clone());
        let spawned_id = id.clone();
        tokio::spawn(async move {
            run_job(job, command).await;
            let _ = spawned_id;
        });
        id
    }

    pub fn list(&self) -> Vec<JobSnapshot> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .map(|(id, job)| JobSnapshot {
                id: id.clone(),
                command: job.command.clone(),
                done: job.done.load(Ordering::Relaxed),
                output: job.output.lock().unwrap().clone(),
            })
            .collect()
    }

    pub fn snapshot(&self, id: &str) -> Option<JobSnapshot> {
        self.inner.lock().unwrap().get(id).map(|job| JobSnapshot {
            id: id.into(),
            command: job.command.clone(),
            done: job.done.load(Ordering::Relaxed),
            output: job.output.lock().unwrap().clone(),
        })
    }

    pub async fn wait(&self, ids: &[String], timeout_ms: u64) -> Vec<JobSnapshot> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms.max(1));
        loop {
            let snaps: Vec<_> = ids.iter().filter_map(|id| self.snapshot(id)).collect();
            if snaps.iter().all(|s| s.done) || tokio::time::Instant::now() >= deadline {
                return if snaps.is_empty() {
                    ids.iter()
                        .map(|id| JobSnapshot {
                            id: id.clone(),
                            command: String::new(),
                            done: true,
                            output: format!("Task {id} not found. No background tasks exist in this session."),
                        })
                        .collect()
                } else {
                    snaps
                };
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    pub async fn kill(&self, id: &str) -> String {
        let job = self.inner.lock().unwrap().get(id).cloned();
        let Some(job) = job else {
            return format!(
                "Task or subagent {id} not found. No background tasks or subagents exist in this session."
            );
        };
        if job.done.load(Ordering::Relaxed) {
            return format!("{id} already finished");
        }
        let mut child = job.child.lock().await;
        if let Some(child) = child.as_mut() {
            let _ = child.kill().await;
        }
        drop(child);
        job.done.store(true, Ordering::Relaxed);
        format!("killed {id}")
    }
}

async fn run_job(job: Arc<Job>, command: String) {
    let spawned = tokio::process::Command::new("bash")
        .arg("-lc")
        .arg(&command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    match spawned {
        Err(e) => {
            *job.output.lock().unwrap() = format!("Error spawning bash: {e}");
            job.done.store(true, Ordering::Relaxed);
        }
        Ok(mut child) => {
            let mut stdout = child.stdout.take();
            let mut stderr = child.stderr.take();
            *job.child.lock().await = Some(child);
            let mut out = Vec::new();
            let mut err = Vec::new();
            if let Some(mut s) = stdout.take() {
                let _ = s.read_to_end(&mut out).await;
            }
            if let Some(mut s) = stderr.take() {
                let _ = s.read_to_end(&mut err).await;
            }
            let mut body = String::from_utf8_lossy(&out).into_owned();
            let err_s = String::from_utf8_lossy(&err);
            if !err_s.is_empty() {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(&err_s);
            }
            if let Some(child) = job.child.lock().await.as_mut() {
                let status = child.wait().await.ok();
                if let Some(st) = status {
                    if !st.success() {
                        body = format!("exit {st}\n{body}");
                    }
                }
            }
            if body.trim().is_empty() {
                body = "(no output)".into();
            }
            *job.output.lock().unwrap() = body;
            job.done.store(true, Ordering::Relaxed);
        }
    }
}

impl Default for Jobs {
    fn default() -> Self {
        Self::new()
    }
}

pub fn jobs() -> Plugin {
    plugin("jobs", Inject::new(), |ctx, _: &()| {
        Ok(Some(ctx.provide(JOBS, Jobs::new())?))
    })
}

const OUTPUT_PARAMS: &str = r#"{"type":"object","properties":{"task_ids":{"type":"array","items":{"type":"string"},"description":"Background task ids."},"timeout_ms":{"type":"integer","description":"Wait up to this many ms; omit or 0 for a snapshot."}}}"#;
const WAIT_PARAMS: &str = r#"{"type":"object","properties":{"task_ids":{"type":"array","items":{"type":"string"}},"timeout_ms":{"type":"integer"}},"required":["task_ids"]}"#;
const KILL_PARAMS: &str = r#"{"type":"object","properties":{"task_id":{"type":"string"}},"required":["task_id"]}"#;

pub fn tool_jobs() -> Plugin {
    plugin(
        "tool-jobs",
        Inject::from([TOOLS, JOBS]),
        |ctx, _: &()| {
            let tools = ctx.require::<Tools>(TOOLS)?;
            let output: ToolBody = {
                let ctx = ctx.clone();
                std::sync::Arc::new(move |call| {
                    let ctx = ctx.clone();
                    Box::pin(async move { job_output(&ctx, call).await })
                })
            };
            let wait: ToolBody = {
                let ctx = ctx.clone();
                std::sync::Arc::new(move |call| {
                    let ctx = ctx.clone();
                    Box::pin(async move { wait_tasks(&ctx, call).await })
                })
            };
            let kill: ToolBody = {
                let ctx = ctx.clone();
                std::sync::Arc::new(move |call| {
                    let ctx = ctx.clone();
                    Box::pin(async move { kill_task(&ctx, call).await })
                })
            };
            own_registered(
                ctx,
                vec![
                    tools.register(
                        ToolSpec {
                            name: "get_task_output".into(),
                            description: "Get output and status from a background terminal command.\n- Pass task_ids with one or more ids from is_background=true commands; omit timeout_ms or pass 0 for a non-blocking snapshot.".into(),
                            parameters_json: OUTPUT_PARAMS.into(),
                        },
                        output,
                    )?,
                    tools.register(
                        ToolSpec {
                            name: "wait_tasks".into(),
                            description: "Wait until background tasks complete. Prefer get_task_output with a positive timeout_ms.".into(),
                            parameters_json: WAIT_PARAMS.into(),
                        },
                        wait,
                    )?,
                    tools.register(
                        ToolSpec {
                            name: "kill_task".into(),
                            description: "Terminate a running background terminal command. Sends SIGTERM/SIGKILL to a background command.".into(),
                            parameters_json: KILL_PARAMS.into(),
                        },
                        kill,
                    )?,
                ],
            )?;
            Ok(None)
        },
    )
}

fn parse_ids(raw: &str) -> (Vec<String>, u64) {
    let v: serde_json::Value = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
    let mut ids = Vec::new();
    if let Some(arr) = v.get("task_ids").and_then(|x| x.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str() {
                if !s.is_empty() {
                    ids.push(s.to_string());
                }
            }
        }
    }
    if ids.is_empty() {
        if let Some(s) = v.get("task_id").and_then(|x| x.as_str()) {
            if !s.is_empty() {
                ids.push(s.to_string());
            }
        }
    }
    let timeout = v
        .get("timeout_ms")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    (ids, timeout)
}

fn render_snap(s: &JobSnapshot) -> String {
    let status = if s.done { "done" } else { "running" };
    format!("[{status}] {} {}\n{}", s.id, s.command, s.output)
}

async fn job_output(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let (ids, timeout) = parse_ids(&call.arguments);
    let jobs = ctx.get::<Jobs>(JOBS);
    let sub = ctx.get::<Subagents>(SUBAGENTS);
    if ids.is_empty() {
        let mut parts = Vec::new();
        if let Some(jobs) = &jobs {
            parts.extend(jobs.list().iter().map(render_snap));
        }
        if let Some(sub) = &sub {
            parts.extend(sub.list().iter().map(render_subagent));
        }
        if parts.is_empty() {
            return tool_result(call, "No background tasks exist in this session.");
        }
        return tool_result(call, parts.join("\n\n"));
    }
    let body = collect_output(jobs.as_deref(), sub.as_deref(), &ids, timeout).await;
    tool_result(call, body)
}

async fn wait_tasks(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let (ids, timeout) = parse_ids(&call.arguments);
    let timeout = if timeout == 0 { 30_000 } else { timeout };
    if ids.is_empty() {
        return tool_result(call, "Error: task_ids is required");
    }
    let jobs = ctx.get::<Jobs>(JOBS);
    let sub = ctx.get::<Subagents>(SUBAGENTS);
    let body = collect_output(jobs.as_deref(), sub.as_deref(), &ids, timeout).await;
    tool_result(call, body)
}

async fn kill_task(ctx: &cordis::Context, call: ToolCall) -> ToolResult {
    let (ids, _) = parse_ids(&call.arguments);
    let Some(id) = ids.first() else {
        return tool_result(call, "Error: task_id is required");
    };
    if let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) {
        if let Some(msg) = sub.kill(id) {
            return tool_result(call, msg);
        }
    }
    let Some(jobs) = ctx.get::<Jobs>(JOBS) else {
        return tool_result(call, "Error: jobs is not mounted");
    };
    let msg = jobs.kill(id).await;
    tool_result(call, msg)
}

async fn collect_output(
    jobs: Option<&Jobs>,
    sub: Option<&Subagents>,
    ids: &[String],
    timeout_ms: u64,
) -> String {
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_millis(if timeout_ms == 0 { 1 } else { timeout_ms });
    loop {
        let mut parts = Vec::new();
        let mut all_done = true;
        for id in ids {
            if let Some(s) = sub.and_then(|s| s.snapshot(id)) {
                if !s.done {
                    all_done = false;
                }
                parts.push(render_subagent(&s));
            } else if let Some(j) = jobs.and_then(|j| j.snapshot(id)) {
                if !j.done {
                    all_done = false;
                }
                parts.push(render_snap(&j));
            } else {
                parts.push(format!(
                    "Task {id} not found. No background tasks or subagents exist in this session."
                ));
            }
        }
        if timeout_ms == 0 || all_done || tokio::time::Instant::now() >= deadline {
            return parts.join("\n\n");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
