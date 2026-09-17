//! SessionActor-shaped drain: oneshot ack then `xai_workflow::run_workflow`.
//! 每次 run 的 host 服务在 [`super::host`]。

#![allow(dead_code)] // Grok-copied API kept for later wiring.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use xai_workflow::{run_workflow, validate_script, Journal, WorkflowOutcome, WorkflowRunParams};

use super::grok_tool::{
    WorkflowLaunchAck, WorkflowLaunchEnvelope, WorkflowSource, WorkflowToolInput,
};
use super::host::{self, WorkflowHostParams, DEFAULT_AGENT_BUDGET};
use super::registry::{resolve_by_name, resolve_by_path, resolve_inline, ResolveError};
use crate::names::{SESSIONS, SUBAGENTS};
use crate::session::log::Sessions;
use crate::tools::task::{Subagents, WorkflowReport};
use cordis_base::types::{LogEvent, NoticeKind};

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
    /// 本次 run 的子代理调用上限（`workflow` 工具的 `agent_budget`）。
    pub agent_budget: u64,
    /// 已预留 + 已用掉的子代理调用数。预留即扣减，引擎负责回退。
    pub agents_used: u64,
    /// 脚本 `log()` 的最近几条，新的在后。
    ///
    /// 用 `Arc` 是因为快照在 80ms 心跳里被整表克隆（`any_live_task`），这里
    /// 只该是一次引用计数。
    pub logs: Arc<Vec<String>>,
    /// 脚本 `meta.phases` 声明的阶段（标题，说明）。整条 run 不变。
    pub phases: Arc<Vec<(String, String)>>,
    /// 这次 run 起过的子代理，按启动顺序。
    pub agents: Arc<Vec<WorkflowAgentRow>>,
}

/// run 里一个子代理的状态行。overlay 详情页右栏按它渲染。
#[derive(Clone, Debug)]
pub struct WorkflowAgentRow {
    pub agent_id: String,
    /// `agent(label:)`，没给就是角色名。
    pub label: String,
    /// `agent(phase:)`——这个孩子属于哪个阶段。
    pub phase: Option<String>,
    /// `running` / `done` / `failed` / `cancelled`。
    pub state: String,
    pub tokens_used: u64,
    pub duration_ms: u64,
    /// 这个孩子 `report` 的最后一条。不进主线程上下文，只做可见进度。
    pub latest_report: Option<String>,
}

impl WorkflowRunSnap {
    /// 最新一条脚本进度，给任务条和 overlay 行用。
    pub fn latest_log(&self) -> Option<&str> {
        self.logs.last().map(String::as_str)
    }

    /// 还在跑的子代理数。
    pub fn agents_active(&self) -> u32 {
        self.agents.iter().filter(|a| a.state == "running").count() as u32
    }
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

    pub(super) fn patch(&self, run_id: &str, f: impl FnOnce(&mut WorkflowRunSnap)) {
        if let Some(slot) = self.inner.lock().unwrap().get_mut(run_id) {
            f(&mut slot.snap);
        }
    }

    /// 取消一次 run。引擎自己会以 `Cancelled` 收尾并落状态，host 循环随后把它
    /// 的子代理收干净，所以这里只负责触发。
    ///
    /// 返回 `false` 表示没有这个 run，或者它已经结束了。
    pub(super) fn cancel(&self, run_id: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        let Some(slot) = inner.get(run_id) else {
            return false;
        };
        if slot.cancel.is_cancelled() || slot.snap.status != "active" {
            return false;
        }
        slot.cancel.cancel();
        true
    }

    /// 按 run id 或显示名找一次 run，活跃的优先。
    ///
    /// 显示名是给用户和 `/workflow` 管理用的（`alloc_name` 会加 `-2` 后缀去
    /// 重），run id 保持内部，两边都该能点名。
    pub(super) fn resolve(&self, needle: &str) -> Option<String> {
        let needle = needle.trim();
        if needle.is_empty() {
            return None;
        }
        let inner = self.inner.lock().unwrap();
        if inner.contains_key(needle) {
            return Some(needle.to_owned());
        }
        let mut fallback = None;
        for slot in inner.values() {
            if slot.snap.name != needle {
                continue;
            }
            if slot.snap.status == "active" {
                return Some(slot.snap.run_id.clone());
            }
            fallback.get_or_insert_with(|| slot.snap.run_id.clone());
        }
        fallback
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
    let agent_budget = input.agent_budget.unwrap_or(DEFAULT_AGENT_BUDGET);
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
        agent_budget,
        agents_used: 0,
        logs: Arc::new(Vec::new()),
        phases: Arc::new(
            resolved
                .meta
                .phases
                .iter()
                .map(|p| (p.title.clone(), p.detail.clone().unwrap_or_default()))
                .collect(),
        ),
        agents: Arc::new(Vec::new()),
    };
    state.insert(snap, cancel.clone());
    let _ = ack.send(WorkflowLaunchAck::Started {
        run_id: run_id.clone(),
        task_id: run_id.clone(),
        name: display,
        script_path: None,
    });

    // 并发上限 live-look `"subagents"`：workflow 子代理不走会话限流，只受这里
    // 挂的池约束。服务没挂载时退到出厂值，spawn 自己会再报错。
    let configured = ctx
        .get::<Subagents>(SUBAGENTS)
        .map(|sub| sub.workflow_max_concurrent_agents())
        .unwrap_or(host::DEFAULT_MAX_CONCURRENT_AGENTS);
    let (host_tx, host_rx) = mpsc::unbounded_channel();
    tokio::spawn(host::run(
        WorkflowHostParams {
            run_id: run_id.clone(),
            ctx: ctx.clone(),
            state: state.clone(),
            cancel: cancel.clone(),
            agent_budget,
            max_concurrent_agents: host::max_concurrent_agents(configured),
            scratch: cordis_base::config::dock_home()
                .join("scratch")
                .join(&run_id),
        },
        host_rx,
    ));

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
    notify_done(&ctx, &state, &run_id);
}

/// run 收尾：把结果与过程交给主线程，并叫醒它一次。
///
/// **整条 run 只有这一次唤醒。** 中间那些「某个子代理跑完了」既不完整也不可
/// 行动（脚本还在往下算），推给主线程只会让它在半份结果上烧一轮。
fn notify_done(ctx: &cordis::Context, state: &Arc<WorkflowState>, run_id: &str) {
    let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
        return;
    };
    let Some(snap) = state.list().into_iter().find(|r| r.run_id == run_id) else {
        return;
    };
    // 兜底再取一次：最后一个孩子可能在 host 循环退出之后才上报。
    let mut reports: Vec<_> = snap
        .agents
        .iter()
        .filter_map(|row| {
            row.latest_report.as_ref().map(|output| WorkflowReport {
                agent_id: row.label.clone(),
                output: output.clone(),
            })
        })
        .collect();
    reports.extend(sub.take_workflow_reports(run_id));
    let summary = snap
        .result_summary
        .clone()
        .or_else(|| snap.pause_message.clone())
        .unwrap_or_else(|| "（没有结果）".into());
    // 滚动区的收尾卡：用户该看见 run 结束了、结果是什么。它不进模型历史，
    // 模型走的是下面那条 `WorkflowDone` 通知。
    if let Some(sessions) = ctx.get::<Sessions>(SESSIONS) {
        sessions.append(LogEvent::Notice {
            kind: NoticeKind::WorkflowDone,
            title: format!("工作流 {} · {}", snap.name, status_text(&snap.status)),
            body: summary.clone(),
        });
    }
    sub.notify_workflow_done(
        snap.name.clone(),
        snap.status.clone(),
        snap.elapsed_ms,
        summary,
        reports,
    );
}

fn status_text(status: &str) -> &str {
    match status {
        "complete" => "完成",
        "cancelled" => "已停止",
        "failed" => "失败",
        other => other,
    }
}

fn resolve_detail(err: ResolveError) -> String {
    err.to_string()
}
