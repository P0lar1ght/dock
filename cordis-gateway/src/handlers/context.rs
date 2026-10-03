//! `thread/context/get`：下一次请求的上下文窗口按类别拆开（GUI 点上下文圆环看的详情）。
//!
//! 和 TUI `/context` 同一份数：`ContextBook::window_on` 的快照 + `detail_on` 的明细。
//! `slices` 互不重叠、加起来是整个窗口（已用 + 空闲）；技能 / 工作流已算在系统提示里，
//! 工程规约 / 记忆已算在消息里，所以只作为所在那一片的 `includes` 列出，不另占一片。
//! MCP 与本地按需工具不进窗口（按需加载），单列在 `onDemand`。

use serde_json::{json, Value};

use cordis_spine::{
    snapshot_context, ContextBook, ContextCategory, ContextSnapshot, DetailGroup, OccupancyKind,
    CONTEXT,
};

use crate::handle::GatewayHandle;
use crate::protocol::RpcError;
use crate::threads;

pub fn get(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let thread_id = threads::thread_param(&params);
    let page = threads::resolve(gateway, &thread_id)?;
    let ctx = &page.ctx;
    let book = ctx.get::<ContextBook>(CONTEXT);
    let snap = book
        .as_ref()
        .map(|b| b.window_on(ctx))
        .unwrap_or_else(|| snapshot_context(ctx));
    let detail = |kind: OccupancyKind| -> Vec<DetailGroup> {
        match &book {
            Some(b) => b.detail_on(ctx, kind).groups,
            None => cordis_spine::occupancy_detail(ctx, kind).groups,
        }
    };
    let trigger = snap.auto_compact_threshold_percent as u64;
    Ok(json!({
        "threadId": thread_id,
        "model": snap.model,
        "usedTokens": snap.used,
        "maxContextTokens": snap.total,
        "usagePercent": snap.usage_pct,
        "compactionTriggerPercent": trigger,
        "compactionTriggerTokens": snap.total.saturating_mul(trigger) / 100,
        "turnCount": snap.turn_count,
        "toolCallCount": snap.tool_call_count,
        "compactionCount": snap.compaction_count,
        "slices": slices(&snap, &detail),
        "onDemand": on_demand(&snap, &detail),
    }))
}

fn slices(snap: &ContextSnapshot, detail: &dyn Fn(OccupancyKind) -> Vec<DetailGroup>) -> Value {
    // 已用 = 系统提示 + 消息 + 其余开销（工具定义、推理、图片、上游真账与估算之差）。
    // 工具定义从「其余」里拆出来单列：它和系统提示一样每轮原样重发。
    let overhead = snap.used.saturating_sub(
        snap.system_prompt_tokens
            .saturating_add(snap.message_tokens),
    );
    let tools = snap.tool_definitions_tokens.min(overhead);
    let rest = overhead - tools;
    let tools_title = OccupancyKind::Tools.title();
    // 「其余」的明细去掉工具定义那一行：它已经是单独一片。
    let rest_detail: Vec<DetailGroup> = detail(OccupancyKind::Overhead)
        .into_iter()
        .map(|mut g| {
            g.rows.retain(|r| r.label != tools_title);
            g
        })
        .collect();
    let mut out = vec![
        slice(
            "system",
            OccupancyKind::System,
            "prefix",
            snap.system_prompt_tokens,
            None,
            includes(snap, &[OccupancyKind::Skills, OccupancyKind::Workflows]),
            detail(OccupancyKind::System),
        ),
        slice(
            "tools",
            OccupancyKind::Tools,
            "prefix",
            tools,
            Some(format!("{} 个", snap.tool_definitions_count)),
            Vec::new(),
            detail(OccupancyKind::Tools),
        ),
        slice(
            "messages",
            OccupancyKind::Messages,
            "session",
            snap.message_tokens,
            Some(format!(
                "{} 轮 · {} 次工具调用",
                snap.turn_count, snap.tool_call_count
            )),
            includes(snap, &[OccupancyKind::Instructions, OccupancyKind::Memory]),
            detail(OccupancyKind::Messages),
        ),
    ];
    if rest > 0 {
        out.push(slice(
            "overhead",
            OccupancyKind::Overhead,
            "session",
            rest,
            None,
            Vec::new(),
            rest_detail,
        ));
    }
    out.push(slice(
        "free",
        OccupancyKind::Free,
        "free",
        snap.free_tokens,
        None,
        Vec::new(),
        Vec::new(),
    ));
    Value::Array(out)
}

fn on_demand(snap: &ContextSnapshot, detail: &dyn Fn(OccupancyKind) -> Vec<DetailGroup>) -> Value {
    let rows: Vec<Value> = [
        ("mcp", OccupancyKind::Mcp),
        ("deferred", OccupancyKind::Deferred),
    ]
    .into_iter()
    .filter_map(|(id, kind)| {
        let c = category(snap, kind)?;
        Some(json!({
            "id": id,
            "label": c.label,
            "note": c.detail,
            "detail": groups_json(detail(kind)),
        }))
    })
    .collect();
    Value::Array(rows)
}

fn category(snap: &ContextSnapshot, kind: OccupancyKind) -> Option<&ContextCategory> {
    snap.categories.iter().find(|c| c.label == kind.title())
}

/// 已经算在某一片里的子项（技能在系统提示里、规约在消息里……）。
fn includes(snap: &ContextSnapshot, kinds: &[OccupancyKind]) -> Vec<Value> {
    kinds
        .iter()
        .filter_map(|kind| category(snap, *kind))
        .filter(|c| c.tokens > 0)
        .map(|c| json!({ "label": c.label, "tokens": c.tokens, "note": c.detail }))
        .collect()
}

fn slice(
    id: &str,
    kind: OccupancyKind,
    group: &str,
    tokens: u64,
    note: Option<String>,
    includes: Vec<Value>,
    detail: Vec<DetailGroup>,
) -> Value {
    json!({
        "id": id,
        "label": kind.title(),
        "group": group,
        "tokens": tokens,
        "note": note,
        "includes": includes,
        "detail": groups_json(detail),
    })
}

/// 每组最多给这么多行，其余并成一行：「逐条消息」在长会话里有成百上千行。
const MAX_DETAIL_ROWS: usize = 40;

fn groups_json(groups: Vec<DetailGroup>) -> Value {
    Value::Array(
        groups
            .into_iter()
            .map(|g| {
                let extra = g.rows.len().saturating_sub(MAX_DETAIL_ROWS);
                let mut rows: Vec<Value> = g
                    .rows
                    .iter()
                    .take(MAX_DETAIL_ROWS)
                    .map(|r| json!({ "label": r.label, "tokens": r.tokens, "note": r.note }))
                    .collect();
                if extra > 0 {
                    let rest = &g.rows[MAX_DETAIL_ROWS..];
                    let tokens = rest
                        .iter()
                        .any(|r| r.tokens.is_some())
                        .then(|| rest.iter().filter_map(|r| r.tokens).sum::<u64>());
                    rows.push(json!({
                        "label": format!("其余 {extra} 项"),
                        "tokens": tokens,
                        "note": null,
                    }));
                }
                json!({ "heading": g.heading, "rows": rows })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis_spine::DetailRow;

    #[test]
    fn long_detail_groups_fold_the_tail_into_one_row() {
        let rows = (0..45)
            .map(|i| DetailRow {
                label: format!("第 {i} 条"),
                tokens: Some(2),
                note: None,
            })
            .collect();
        let out = groups_json(vec![DetailGroup {
            heading: "逐条".into(),
            rows,
        }]);
        let rows = out[0]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), MAX_DETAIL_ROWS + 1);
        assert_eq!(rows[MAX_DETAIL_ROWS]["label"], "其余 5 项");
        assert_eq!(rows[MAX_DETAIL_ROWS]["tokens"], 10);
    }
}
