//! 工作流投影：`workflow/updated` / `workflow/list` 里一条 run 的样子。
//!
//! run 挂在起它的那一页（`WorkflowRunSnap::page`）：只有那一页的订阅者收到推送，
//! `workflow/*` 方法也只认那一页的线程。run 只活在 Dock 的内存里（不落盘）；它派的
//! 子代理照常走 `subagent/*`（`toolCallId` 为 `null`），按 `agents[].agentId` 对上。

use std::time::{SystemTime, UNIX_EPOCH};

use cordis::Context;
use cordis_spine::{WorkflowRunSnap, Workflows, WORKFLOWS};
use serde_json::{json, Value};

/// 推送里最多带几条脚本日志（新的在后）。快照里留 50 条、每条 4KB，每次变化都整份推太重。
const LOG_LINES: usize = 8;
/// 一条日志最多这么多字。
const LOG_CHARS: usize = 500;
/// 一个子代理的最近一条上报最多这么多字（面板里只看摘要，全文在它自己的对话里）。
const REPORT_CHARS: usize = 2000;
/// 结果 / 错误最多这么多字。
const RESULT_CHARS: usize = 8000;
/// 结构化结果（`complete` 的 `data`）序列化后超过这么大就不带（只标 `resultDataOmitted`）。
const RESULT_DATA_BYTES: usize = 512 * 1024;

/// running / completed / failed / cancelled / paused。
pub fn status(snap: &WorkflowRunSnap) -> &'static str {
    match snap.status.as_str() {
        "active" => "running",
        "complete" => "completed",
        "cancelled" => "cancelled",
        s if s.ends_with("_paused") => "paused",
        _ => "failed",
    }
}

/// 一条 run：`workflow/updated` 的 `run`、`workflow/list` 的一项。
pub fn info(snap: &WorkflowRunSnap) -> Value {
    let status = status(snap);
    let logs: Vec<String> = snap
        .logs
        .iter()
        .skip(snap.logs.len().saturating_sub(LOG_LINES))
        .map(|l| cap(l, LOG_CHARS))
        .collect();
    let agents: Vec<Value> = snap
        .agents
        .iter()
        .map(|row| {
            json!({
                "agentId": row.agent_id,
                "label": row.label,
                "phase": row.phase,
                "status": match row.state.as_str() {
                    "done" => "completed",
                    other => other,
                },
                "durationMs": row.duration_ms,
                "tokensUsed": row.tokens_used,
                "latestReport": row.latest_report.as_deref().map(|r| cap(r.trim(), REPORT_CHARS)),
            })
        })
        .collect();
    let phases: Vec<Value> = snap
        .phases
        .iter()
        .map(|(title, detail)| json!({ "title": title, "detail": detail }))
        .collect();
    let mut v = json!({
        "runId": snap.run_id,
        "toolCallId": snap.tool_call_id,
        "revision": snap.revision,
        "name": snap.name,
        "description": snap.objective,
        "builtin": snap.builtin,
        "status": status,
        "phases": phases,
        "currentPhase": snap.current_phase,
        "startedAt": unix_ms(snap.started_wall),
        "durationMs": snap.elapsed_ms,
        "agentBudget": snap.agent_budget,
        "agentsUsed": snap.agents_used,
        "agents": agents,
        "logs": logs,
    });
    if let Some(result) = snap
        .result_summary
        .as_deref()
        .filter(|_| status == "completed")
    {
        v["result"] = json!(cap(result.trim(), RESULT_CHARS));
    }
    if let Some(data) = snap.result_data.as_deref() {
        let size = serde_json::to_vec(data)
            .map(|b| b.len())
            .unwrap_or(usize::MAX);
        if size <= RESULT_DATA_BYTES {
            v["resultData"] = data.clone();
        } else {
            v["resultDataOmitted"] = json!(true);
        }
    }
    if let Some(message) = snap
        .pause_message
        .as_deref()
        .filter(|_| matches!(status, "failed" | "paused"))
    {
        v["error"] = json!(cap(message.trim(), RESULT_CHARS));
    }
    v
}

/// `identity` 那一页起的 run，先起的在前。
pub fn runs_of(ctx: &Context, identity: &str) -> Vec<WorkflowRunSnap> {
    let Some(wf) = ctx.get::<Workflows>(WORKFLOWS) else {
        return Vec::new();
    };
    let mut runs: Vec<_> = wf
        .list()
        .into_iter()
        .filter(|r| r.page == identity)
        .collect();
    runs.sort_by_key(|r| r.started_wall);
    runs
}

/// 按 run id 取一条 run 的当前快照。
pub fn run(ctx: &Context, run_id: &str) -> Option<WorkflowRunSnap> {
    ctx.get::<Workflows>(WORKFLOWS)?
        .list()
        .into_iter()
        .find(|r| r.run_id == run_id)
}

fn cap(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}
