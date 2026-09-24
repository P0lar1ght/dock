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
    /// 这个孩子发给父级的最后一条消息。不进主线程上下文，只做可见进度。
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
    /// 这次 run 里**每一条**子代理上报，按发生顺序，收尾时一起交给主线程。
    ///
    /// 不放进 `snap`：快照在 80ms 心跳里被整表克隆给 TUI，而这堆文本只有收尾
    /// 那一次用得上。行上的 `latest_report` 是给详情页看的**最后一条**，从它
    /// 反推收尾通知会把同一个孩子先前报过的都丢掉。
    reports: Vec<WorkflowReport>,
    /// 超出 [`WORKFLOW_MAX_DONE_REPORTS`] 被丢掉的条数。
    dropped_reports: usize,
}

/// 收尾通知里最多带几条过程上报。
///
/// 一个孩子可以报很多次，`agent_budget` 管得住孩子数管不住上报数；这条通知是
/// 要进模型上下文的，得有个上限。满了丢最旧的，并在通知里说清楚丢了几条。
const WORKFLOW_MAX_DONE_REPORTS: usize = 64;

/// 快照表里最多留几条**已结束**的 run。
///
/// `WorkflowState` 活得和进程一样久，不淘汰的话跑一整天的会话会把每次 run 的
/// 上报与日志都攒着。活跃的 run 一条不动。
const WORKFLOW_MAX_FINISHED_RUNS: usize = 32;

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

    #[cfg(test)]
    pub(super) fn run_count(&self) -> usize {
        self.inner.lock().unwrap().len()
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
        let mut inner = self.inner.lock().unwrap();
        prune_finished(&mut inner);
        inner.insert(
            snap.run_id.clone(),
            RunSlot {
                started: Instant::now(),
                snap,
                cancel,
                reports: Vec::new(),
                dropped_reports: 0,
            },
        );
    }

    /// 攒下一批子代理上报，收尾时一起交给主线程。
    pub(super) fn push_reports(
        &self,
        run_id: &str,
        reports: impl IntoIterator<Item = WorkflowReport>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        let Some(slot) = inner.get_mut(run_id) else {
            return;
        };
        for report in reports {
            slot.reports.push(report);
            if slot.reports.len() > WORKFLOW_MAX_DONE_REPORTS {
                slot.reports.remove(0);
                slot.dropped_reports += 1;
            }
        }
    }

    /// 取走攒下的上报，返回 `(上报, 被丢掉的条数)`。
    pub(super) fn take_reports(&self, run_id: &str) -> (Vec<WorkflowReport>, usize) {
        let mut inner = self.inner.lock().unwrap();
        let Some(slot) = inner.get_mut(run_id) else {
            return (Vec::new(), 0);
        };
        (
            std::mem::take(&mut slot.reports),
            std::mem::take(&mut slot.dropped_reports),
        )
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

/// 只淘汰**已结束**的 run，最旧的先走。活跃的一条不动——它们还在被 host 写。
fn prune_finished(map: &mut HashMap<String, RunSlot>) {
    let mut finished: Vec<(Instant, String)> = map
        .iter()
        .filter(|(_, slot)| slot.snap.status != "active")
        .map(|(id, slot)| (slot.snap.received_at, id.clone()))
        .collect();
    if finished.len() <= WORKFLOW_MAX_FINISHED_RUNS {
        return;
    }
    finished.sort_by_key(|(at, _)| *at);
    let extra = finished.len() - WORKFLOW_MAX_FINISHED_RUNS;
    for (_, id) in finished.into_iter().take(extra) {
        map.remove(&id);
    }
}

/// 本进程的 scratch 根目录。
///
/// `run_id` 是**会话内**序号（`wf_1`、`wf_2`…），两个 dock 同时开着就会各自从
/// `wf_1` 起。直接按 run id 建目录，两次无关的 run 会共用一个 scratch：互相读
/// 到对方的 `report.md`，配额也算在一起。加一段每进程固定的后缀就够——同一次
/// dock 里的所有 run 仍然挨在一起，跨进程再不撞。
///
/// 不删旧目录：`write_scratch_file` 的路径是当着用户面给出去的（deep-research
/// 的报告就在那儿），到期清理该由用户自己决定。
fn scratch_dir(run_id: &str) -> std::path::PathBuf {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let token = TOKEN.get_or_init(|| {
        uuid::Uuid::now_v7()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect()
    });
    cordis_base::config::dock_home()
        .join("scratch")
        .join(format!("{run_id}-{token}"))
}

pub async fn drain_loop_with_ctx(
    state: Arc<WorkflowState>,
    ctx: cordis::Context,
    mut rx: mpsc::UnboundedReceiver<WorkflowLaunchEnvelope>,
) {
    while let Some((req, ack)) = rx.recv().await {
        handle_launch(state.clone(), ctx.clone(), req.input, req.cwd, ack).await;
    }
}

async fn handle_launch(
    state: Arc<WorkflowState>,
    ctx: cordis::Context,
    mut input: WorkflowToolInput,
    cwd: std::path::PathBuf,
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
            scratch: scratch_dir(&run_id),
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
            snap.result_summary = Some(readable_result(result));
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
    // host 一路攒下来的全部上报；兜底再从信箱取一次，最后一个孩子可能在 host
    // 循环退出之后才上报。
    let (mut reports, dropped) = state.take_reports(run_id);
    reports.extend(sub.take_workflow_reports(run_id));
    // 通知里认的是行上的 label（`researcher-0`），不是子代理 UUID：两条来源都
    // 带的是 child id，在这儿统一换一次，别让同一份列表里两种写法混着。
    for report in reports.iter_mut() {
        if let Some(row) = snap.agents.iter().find(|r| r.agent_id == report.agent_id) {
            report.agent_id = row.label.clone();
        }
    }
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
        dropped,
    );
}

/// 把 `complete()` 的结果摊成人和模型都读得下去的文本。
///
/// 原样 `to_string()` 交出去的是一整行转义过的 JSON：`deep_research.rhai` 的
/// `complete(#{ path: …, report: … })` 会变成 `{"path":"…","report":"# Research
/// result\n\n**Status…"}` —— 报告正文里的换行全成了字面量 `\n`，滚动区那张收尾
/// 卡和主线程收到的通知都只能看见这么一条长得吓人的单行。
fn readable_result(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(key, v)| {
                let text = readable_result(v);
                // 多行的值和嵌套结构另起一行并缩进，`path: /x/y` 这种标量留在
                // 同一行。不缩进的话 `output: ok: true` 读起来像两层键挤在一起。
                let nested = matches!(
                    v,
                    serde_json::Value::Object(_) | serde_json::Value::Array(_)
                );
                if text.is_empty() {
                    format!("{key}:")
                } else if nested || text.contains('\n') {
                    format!("{key}:\n{}", indent_block(&text))
                } else {
                    format!("{key}: {text}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Array(items) => items
            .iter()
            .map(readable_result)
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

fn indent_block(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(run_id: &str, status: &str, at: Instant) -> WorkflowRunSnap {
        WorkflowRunSnap {
            run_id: run_id.into(),
            name: run_id.into(),
            objective: String::new(),
            status: status.into(),
            current_phase: None,
            elapsed_ms: 0,
            received_at: at,
            builtin: false,
            pause_message: None,
            result_summary: None,
            agents_running: 0,
            agent_budget: 1,
            agents_used: 0,
            logs: Arc::new(Vec::new()),
            phases: Arc::new(Vec::new()),
            agents: Arc::new(Vec::new()),
        }
    }

    fn report(output: &str) -> WorkflowReport {
        WorkflowReport {
            agent_id: "a".into(),
            output: output.into(),
        }
    }

    /// 跑一天的会话不该把每次 run 的上报都攒着，但**在跑的**一条都不能动。
    #[test]
    fn finished_runs_are_evicted_oldest_first_and_active_ones_are_kept() {
        let state = WorkflowState::new();
        let base = Instant::now();
        let live = snap("live", "active", base);
        state.insert(live, CancellationToken::new());
        for i in 0..(WORKFLOW_MAX_FINISHED_RUNS + 5) {
            let at = base + std::time::Duration::from_millis(i as u64 + 1);
            state.insert(
                snap(&format!("wf_{i}"), "complete", at),
                CancellationToken::new(),
            );
        }
        // 最后插进去的那条还没结束，不参与淘汰计数。
        let rows = state.list();
        assert!(
            rows.iter().any(|r| r.run_id == "live"),
            "活跃的 run 被淘汰了：{:?}",
            rows.iter().map(|r| &r.run_id).collect::<Vec<_>>()
        );
        assert!(
            state.run_count() <= WORKFLOW_MAX_FINISHED_RUNS + 2,
            "已结束的 run 没有被淘汰：{}",
            state.run_count()
        );
        assert!(!rows.iter().any(|r| r.run_id == "wf_0"), "最旧的那条该先走");
    }

    /// 攒下的上报有上限，丢掉几条要算清楚——通知里会如实说一句。
    #[test]
    fn the_done_report_buffer_drops_the_oldest_and_counts_them() {
        let state = WorkflowState::new();
        state.insert(
            snap("wf_1", "active", Instant::now()),
            CancellationToken::new(),
        );
        for i in 0..(WORKFLOW_MAX_DONE_REPORTS + 3) {
            state.push_reports("wf_1", [report(&format!("第 {i} 条"))]);
        }
        let (reports, dropped) = state.take_reports("wf_1");
        assert_eq!(reports.len(), WORKFLOW_MAX_DONE_REPORTS);
        assert_eq!(dropped, 3);
        assert_eq!(reports[0].output, "第 3 条", "丢的该是最旧的");
        let (again, dropped) = state.take_reports("wf_1");
        assert!(again.is_empty() && dropped == 0, "取过一次就清空");
    }

    /// `complete()` 的结果要摊成读得下去的文本，不是一整行转义 JSON。
    ///
    /// `deep_research.rhai` 收尾写的是 `complete(#{ path: …, report: … })`。
    /// 原样 `to_string()` 交出去，报告正文里的换行全成了字面量 `\n`，收尾卡和
    /// 主线程收到的通知都只剩一条长得吓人的单行。
    #[test]
    fn a_complete_result_is_readable_not_escaped_json() {
        let value = serde_json::json!({
            "path": "/Users/polar/.dock/scratch/wf_1-abc/report.md",
            "report": "# Research result\n\n**Status: Partial**\n\n没有可支撑的结论。",
        });
        let text = readable_result(&value);
        assert!(!text.contains("\\n"), "换行不该是字面量：{text:?}");
        assert!(text.contains("# Research result"), "{text}");
        assert!(text.contains("**Status: Partial**"), "{text}");
        assert!(
            text.contains("path: /Users/polar/.dock/scratch/wf_1-abc/report.md"),
            "标量留在同一行：{text}"
        );

        // 纯字符串结果原样交出去，别再套一层引号。
        assert_eq!(readable_result(&serde_json::json!("done")), "done");
        // 嵌套结构缩进，不然 `output: ok: true` 像两层键挤在一起。
        let nested = readable_result(&serde_json::json!({ "output": { "ok": true } }));
        assert_eq!(nested, "output:\n  ok: true");
    }

    /// 两个 dock 同时开着，各自的 `wf_1` 不能共用一个 scratch 目录。
    #[test]
    fn scratch_dirs_are_scoped_to_this_process() {
        let a = scratch_dir("wf_1");
        assert_eq!(a, scratch_dir("wf_1"), "同一次 run 每次都得是同一个目录");
        assert_ne!(a, scratch_dir("wf_2"));
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("wf_1-"), "{name}");
        assert!(
            name.len() > "wf_1-".len(),
            "光有 run id 会跨进程撞车：{name}"
        );
    }
}
