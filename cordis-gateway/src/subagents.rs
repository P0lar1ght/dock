//! 子代理投影：`subagent/updated` / `subagent/list` 里一个子代理的样子，以及它
//! 停下时怎么收尾。子代理的对话事件由 [`crate::transcript::Transcript::for_child`]
//! 投影。
//!
//! 子代理挂在启动它的那一页（`SubagentSnap::parent`）：只有那一页的订阅者收到它的
//! 推送，`subagent/*` 方法也只认那一页的线程。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cordis::Context;
use cordis_spine::{
    AgentPresets, LogEvent, SubagentSnap, Subagents, TurnEndStatus, AGENT_PRESETS, SUBAGENTS,
};
use serde_json::{json, Value};

/// `output` 最多带这么多字（卡片只显示两行摘要，面板显示最后的回报）。
const OUTPUT_CHARS: usize = 4000;

/// running / idle / completed / failed / cancelled（规则在 [`SubagentSnap::status`]）。
pub fn status(snap: &SubagentSnap) -> &'static str {
    snap.status()
}

/// 子代理停下时它那一轮的收尾状态（子代理的会话不记 `TurnEnd`）。
pub fn turn_status(snap: &SubagentSnap, events: &[LogEvent]) -> TurnEndStatus {
    match status(snap) {
        "failed" => TurnEndStatus::Failed(error(snap, events).unwrap_or_default()),
        "cancelled" => TurnEndStatus::Cancelled,
        _ => TurnEndStatus::Completed,
    }
}

/// 一个子代理：`subagent/updated` 的 `agent`、`subagent/list` 的一项。
pub fn info(snap: &SubagentSnap, events: &[LogEvent], role: Option<String>) -> Value {
    // 墙钟启动时刻：同 id 复活后沿用第一次的，列表里不挪位置。
    let started_at = snap.started_wall;
    let mut v = json!({
        "agentId": snap.id,
        "toolCallId": snap.tool_call_id,
        "subagentType": snap.subagent_type,
        "role": role.unwrap_or_else(|| snap.subagent_type.clone()),
        "description": snap.description,
        "status": status(snap),
        "startedAt": unix_ms(started_at),
        "durationMs": duration_ms(snap.elapsed()),
        "toolCalls": tool_calls(events),
        "output": cap(snap.output.trim(), OUTPUT_CHARS),
        // 显式带上：和只在盘上的 `persisted: true` 对称，客户端不用靠「缺省即 false」。
        "persisted": false,
    });
    if snap.running() {
        if let Some(activity) = activity(events) {
            v["activity"] = activity;
        }
    }
    if let Some(error) = (status(snap) == "failed")
        .then(|| error(snap, events))
        .flatten()
    {
        v["error"] = json!(error);
    }
    v
}

/// 只在盘上的子代理（不在内存里）：和 [`info`] 同形，多一个 `persisted: true`。
///
/// 盘上记的是最后一次变化时的状态。人已经不在内存里了，还写着 running 的是进程中途
/// 没了（算 cancelled），idle 的是停在那等下一句、后来被回收或 Dock 重启了（算 completed）。
pub fn persisted_info(meta: &cordis_spine::SubagentMeta, role: Option<String>) -> Value {
    let status = match meta.status.as_str() {
        "running" => "cancelled",
        "idle" => "completed",
        other => other,
    };
    let output = meta.output.trim();
    let mut v = json!({
        "agentId": meta.id,
        "toolCallId": meta.tool_call_id,
        "subagentType": meta.subagent_type,
        "role": role.unwrap_or_else(|| meta.subagent_type.clone()),
        "description": meta.description,
        "status": status,
        "startedAt": meta.started_ms,
        "durationMs": meta.settled_ms.map(|s| s.saturating_sub(meta.started_ms)).unwrap_or(0),
        "toolCalls": meta.tool_calls,
        "output": cap(output, OUTPUT_CHARS),
        "persisted": true,
    });
    if status == "failed" && !output.is_empty() {
        v["error"] = json!(output.strip_prefix("failed: ").unwrap_or(output));
    }
    v
}

/// `ctx` 那一页预设里角色的显示名（如「探索」「岑」）。
pub fn role_label(ctx: &Context, subagent_type: &str) -> Option<String> {
    ctx.get::<AgentPresets>(AGENT_PRESETS)
        .and_then(|p| p.role_label(subagent_type))
}

/// 关着的线程用它自己落盘时的预设（`meta.json` 的 `preset_id`，按它的项目目录读）找角色
/// 显示名；老会话没记预设、或预设已经不在了，退回根上的。
pub fn role_label_at(
    ctx: &Context,
    cwd: &std::path::Path,
    preset_id: Option<&str>,
    subagent_type: &str,
) -> Option<String> {
    let presets = ctx.get::<AgentPresets>(AGENT_PRESETS)?;
    let from_session = preset_id
        .and_then(|id| presets.get_at(cwd, id))
        .and_then(|preset| {
            preset.agents.get(subagent_type).map(|def| {
                let name = def.name.trim();
                if name.is_empty() {
                    subagent_type.to_string()
                } else {
                    name.to_string()
                }
            })
        });
    from_session.or_else(|| presets.role_label(subagent_type))
}

/// `identity` 那一页启动的子代理，先启动的在前。
pub fn children_of(ctx: &Context, identity: &str) -> Vec<SubagentSnap> {
    let Some(sub) = ctx.get::<Subagents>(SUBAGENTS) else {
        return Vec::new();
    };
    let mut kids: Vec<_> = sub
        .list()
        .into_iter()
        .filter(|s| s.parent == identity)
        .collect();
    kids.sort_by_key(|s| s.started_at);
    kids
}

/// 调过的工具数：做完的，加上最后一次采样里已经发起、还没出结果的。
fn tool_calls(events: &[LogEvent]) -> usize {
    let done = events
        .iter()
        .filter(|e| matches!(e, LogEvent::ToolExecute { .. }))
        .count();
    done + pending_calls(events).len()
}

/// 最后一次采样发起、还没出结果的工具调用。
fn pending_calls(events: &[LogEvent]) -> Vec<&cordis_spine::ToolCall> {
    let Some(at) = events
        .iter()
        .rposition(|e| matches!(e, LogEvent::LlmStream(_)))
    else {
        return Vec::new();
    };
    let LogEvent::LlmStream(out) = &events[at] else {
        return Vec::new();
    };
    out.tool_calls
        .iter()
        .filter(|call| {
            !events[at..]
                .iter()
                .any(|e| matches!(e, LogEvent::ToolExecute { id, .. } if *id == call.id))
        })
        .collect()
}

/// 在跑的子代理此刻在做什么：调工具（带参数，客户端按工具画成「正在读 …」）、
/// 回复、思考。说不上来就不带。
fn activity(events: &[LogEvent]) -> Option<Value> {
    if let Some(call) = pending_calls(events).last() {
        let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(json!({}));
        return Some(json!({ "kind": "tool", "toolName": call.name, "arguments": arguments }));
    }
    match events.last()? {
        LogEvent::LlmStream(out) if !out.text.is_empty() => Some(json!({ "kind": "replying" })),
        LogEvent::LlmStream(out) if !out.reasoning.is_empty() => {
            Some(json!({ "kind": "thinking" }))
        }
        _ => None,
    }
}

/// 失败的原因：最后一次采样的错误，否则是循环报的错（`output` 里 `failed: …`）。
fn error(snap: &SubagentSnap, events: &[LogEvent]) -> Option<String> {
    let sampled = events.iter().rev().find_map(|e| match e {
        LogEvent::LlmStream(out) => Some(out.error.clone()),
        _ => None,
    });
    sampled.flatten().or_else(|| {
        let out = snap.output.trim();
        (!out.is_empty()).then(|| out.strip_prefix("failed: ").unwrap_or(out).to_string())
    })
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

fn duration_ms(d: Duration) -> u64 {
    d.as_millis() as u64
}
