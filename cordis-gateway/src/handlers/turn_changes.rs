//! `turn/changes/*`：一个线程每轮改了哪些文件（Dock 的 [`cordis_spine::TurnChanges`]，只认
//! `search_replace` / `write_file`）。轮次用投影里的 `turnId`（`t3`），和 `item/user_message`
//! 的 `turnId` 是同一个。
//!
//! - `list { threadId }` → `{ turns: [Changes] }`：有改动的轮次，先发生的在前。
//! - `diff { threadId, turnId }` → `{ diff }`：这一轮的统一 diff。
//! - `revert { threadId, turnId }`（受信连接）→ `{ restored, conflicts, changes }`。
//! - 推送 `turn/changes/updated { turnId, changes }`：这一轮又改了 / 撤销了。

use serde_json::{json, Value};

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{record_user_edit, TurnChanges, TurnSummary, TURN_CHANGES};

use crate::handle::GatewayHandle;
use crate::methods::{method, register_methods, MethodPolicy, GATEWAY_METHODS};
use crate::protocol::{self, RpcError};
use crate::threads::{self, Page};

pub fn gateway_turn_changes() -> Plugin {
    plugin(
        "gateway.turn-changes",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            let thread = MethodPolicy {
                opens_thread: true,
                ..MethodPolicy::default()
            };
            register_methods(
                ctx,
                vec![
                    (
                        protocol::TURN_CHANGES_LIST,
                        thread,
                        method(|gw, params| async move { list(&gw, params) }),
                    ),
                    (
                        protocol::TURN_CHANGES_DIFF,
                        thread,
                        method(|gw, params| async move { diff(&gw, params) }),
                    ),
                    (
                        protocol::TURN_CHANGES_REVERT,
                        // 会写工作区的文件。
                        MethodPolicy {
                            trusted_only: true,
                            ..thread
                        },
                        method(|gw, params| async move { revert(&gw, params) }),
                    ),
                ],
            )?;
            Ok(None)
        },
    )
}

/// 一轮的改动：`turn/changes/list` 的一项、`turn/changes/updated` 的 `changes`。没有改动
/// （全撤成原样了）给 `files: []`。
pub fn changes_json(turn_id: &str, summary: Option<&TurnSummary>) -> Value {
    let files: Vec<Value> = summary
        .map(|s| {
            s.files
                .iter()
                .map(|f| {
                    let mut v = json!({
                        "path": f.path,
                        "absPath": f.abs.display().to_string(),
                        "status": f.status,
                        "added": f.added,
                        "removed": f.removed,
                    });
                    if let Some(reason) = &f.skipped {
                        v["skipped"] = json!(reason);
                    }
                    v
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "turnId": turn_id,
        "files": files,
        "added": summary.map_or(0, |s| s.added),
        "removed": summary.map_or(0, |s| s.removed),
        "reverted": summary.is_some_and(|s| s.reverted),
    })
}

fn list(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let turns: Vec<Value> = service(&page)?
        .list(&page.ctx)
        .iter()
        .filter_map(|s| {
            let turn_id = gateway.turn_id_at(&page.identity, s.ordinal)?;
            Some(changes_json(&turn_id, Some(s)))
        })
        .collect();
    Ok(json!({ "turns": turns }))
}

fn diff(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let (page, ordinal) = turn(gateway, &params)?;
    let diff = service(&page)?
        .diff(&page.ctx, ordinal)
        .ok_or_else(|| RpcError::app("not_found", "这一轮没有记下的文件改动"))?;
    Ok(json!({ "diff": diff }))
}

fn revert(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let (page, ordinal) = turn(gateway, &params)?;
    let changes = service(&page)?;
    let outcome = changes
        .revert(&page.ctx, ordinal)
        .map_err(|e| RpcError::app("revert_failed", e))?;
    // 不是 agent 改的：下一步提醒它，别拿旧内容覆盖回去。
    for path in &outcome.restored {
        record_user_edit(
            &page.ctx,
            format!("撤销了一轮改动：{path} 恢复成那一轮之前的样子"),
        );
    }
    let turn_id = text(&params, "turnId")?;
    Ok(json!({
        "restored": outcome.restored,
        "conflicts": outcome
            .conflicts
            .iter()
            .map(|(path, reason)| json!({ "path": path, "reason": reason }))
            .collect::<Vec<_>>(),
        "changes": changes_json(turn_id, changes.summary(&page.ctx, ordinal).as_ref()),
    }))
}

fn service(page: &Page) -> Result<std::sync::Arc<TurnChanges>, RpcError> {
    page.ctx
        .get::<TurnChanges>(TURN_CHANGES)
        .ok_or_else(|| RpcError::app("unavailable", "turn-changes is not mounted"))
}

/// 参数里的线程和 `turnId` → 那一页、第几条用户消息。
fn turn(gateway: &GatewayHandle, params: &Value) -> Result<(Page, usize), RpcError> {
    let page = threads::resolve_param(gateway, params)?;
    let turn_id = text(params, "turnId")?;
    let ordinal = gateway
        .user_ordinal(&page.identity, turn_id)
        .ok_or_else(|| RpcError::app("not_found", format!("没有轮次 {turn_id}")))?;
    Ok((page, ordinal))
}

fn text<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("{key} 不能为空")))
}
