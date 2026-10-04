//! `schedule/*`：定时任务（`"cron"`）。任务落盘在 `$DOCK_HOME/schedules.json`，属于某个
//! 会话、跨重启继续跑；到点由 `cordis-app` 的驱动送进那个会话（关着就开页）。
//!
//! - `schedule/list {}` → `{ tasks[] }`，全部会话的任务（GUI 的「定时任务」页）。
//! - `schedule/create { threadId, interval | everySecs, prompt, fireImmediately? }` → `{ task }`。
//! - `schedule/update { id, interval? | everySecs?, prompt? }` → `{ task }`。
//! - `schedule/delete { id }` → `{ deleted }`。
//! - 推送 `schedule/changed {}`（连接级，不用订阅）：任何一处改了（含模型的
//!   `scheduler_*`、别的 Dock 进程、到点触发），最多 1 秒后到。收到后重拉 `schedule/list`。

use std::time::Duration;

use serde_json::{json, Value};

use cordis_spine::{
    interval_to_human, parse_interval, session_cwd, Cron, CronError, CronJob, CronOwner, CRON,
};

use crate::handle::GatewayHandle;
use crate::handlers::thread::roster_entries;
use crate::protocol::RpcError;
use crate::threads;

/// 和 `scheduler_create` 同一个下限。
const MIN_EVERY_SECS: u64 = 60;

fn cron(gateway: &GatewayHandle) -> Result<std::sync::Arc<Cron>, RpcError> {
    gateway
        .ctx()
        .get::<Cron>(CRON)
        .ok_or_else(|| RpcError::app("unavailable", "定时任务服务没有挂载"))
}

pub fn list(gateway: &GatewayHandle, _params: Value) -> Result<Value, RpcError> {
    let cron = cron(gateway)?;
    let roster = roster_entries(gateway);
    let tasks: Vec<Value> = cron
        .list()
        .iter()
        .map(|job| {
            let title = job.owner.as_ref().and_then(|o| {
                roster
                    .iter()
                    .find(|e| e.id == o.session)
                    .map(|e| e.title.clone())
            });
            task_json(job, title)
        })
        .collect();
    Ok(json!({ "tasks": tasks }))
}

pub fn create(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let cron = cron(gateway)?;
    let prompt =
        text(&params, "prompt").ok_or_else(|| RpcError::invalid_params("prompt 不能为空"))?;
    let every =
        every(&params)?.ok_or_else(|| RpcError::invalid_params("interval 或 everySecs 必填"))?;
    let owner = owner_of(gateway, &params)?;
    let fire_now = params
        .get("fireImmediately")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let id = cron
        .add_owned(Some(owner), every, prompt, fire_now)
        .map_err(cron_error)?;
    let job = cron
        .get(&id)
        .ok_or_else(|| RpcError::app("internal", "刚建的任务读不回来"))?;
    Ok(json!({ "task": task_json(&job, None) }))
}

pub fn update(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let cron = cron(gateway)?;
    let id = text(&params, "id").ok_or_else(|| RpcError::invalid_params("id is required"))?;
    let every = every(&params)?;
    let prompt = text(&params, "prompt");
    if every.is_none() && prompt.is_none() {
        return Err(RpcError::invalid_params(
            "interval / everySecs / prompt 至少给一个",
        ));
    }
    let job = cron
        .update(&id, every, prompt.as_deref())
        .map_err(cron_error)?;
    Ok(json!({ "task": task_json(&job, None) }))
}

pub fn delete(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let cron = cron(gateway)?;
    let id = text(&params, "id").ok_or_else(|| RpcError::invalid_params("id is required"))?;
    if !cron.cancel(&id).map_err(cron_error)? {
        return Err(RpcError::app("not_found", format!("没有定时任务 {id}")));
    }
    Ok(json!({ "deleted": id }))
}

/// 任务属于哪个会话：开着的页（含 `live`）取它正在写的会话；关着的按会话列表找 cwd。
fn owner_of(gateway: &GatewayHandle, params: &Value) -> Result<CronOwner, RpcError> {
    let thread =
        text(params, "threadId").ok_or_else(|| RpcError::invalid_params("threadId is required"))?;
    if let Ok(page) = threads::resolve(gateway, &thread) {
        let session = page.session_id();
        if session.is_empty() {
            return Err(RpcError::app(
                "invalid_params",
                "这个线程还没落盘，先发一条消息",
            ));
        }
        return Ok(CronOwner {
            session,
            cwd: session_cwd(&page.ctx),
        });
    }
    let entry = roster_entries(gateway)
        .into_iter()
        .find(|e| e.id == thread)
        .ok_or_else(|| RpcError::app("not_found", format!("thread {thread} not found")))?;
    Ok(CronOwner {
        session: entry.id,
        cwd: entry.cwd,
    })
}

fn every(params: &Value) -> Result<Option<Duration>, RpcError> {
    if let Some(raw) = text(params, "interval") {
        let secs = parse_interval(&raw).map_err(|e| RpcError::invalid_params(e.to_string()))?;
        return Ok(Some(Duration::from_secs(secs)));
    }
    match params.get("everySecs") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let secs = v
                .as_u64()
                .ok_or_else(|| RpcError::invalid_params("everySecs 是正整数"))?;
            if secs < MIN_EVERY_SECS {
                return Err(RpcError::invalid_params(format!(
                    "间隔最短 {MIN_EVERY_SECS} 秒"
                )));
            }
            Ok(Some(Duration::from_secs(secs)))
        }
    }
}

fn text(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn cron_error(e: CronError) -> RpcError {
    match e {
        CronError::UnknownId(id) => RpcError::app("not_found", format!("没有定时任务 {id}")),
        CronError::LimitReached(n) => {
            RpcError::app("limit_reached", format!("最多 {n} 个定时任务"))
        }
        CronError::EmptyPrompt => RpcError::invalid_params("prompt 不能为空"),
        CronError::Store(msg) => RpcError::app("store_failed", msg),
    }
}

fn task_json(job: &CronJob, title: Option<String>) -> Value {
    let secs = job.every.as_secs();
    json!({
        "id": job.id,
        "threadId": job.owner.as_ref().map(|o| o.session.clone()),
        "threadTitle": title,
        "cwd": job.owner.as_ref().map(|o| o.cwd.display().to_string()),
        "prompt": job.prompt,
        "everySecs": secs,
        "intervalLabel": interval_to_human(secs),
        "createdAtMs": job.created_at_ms,
        "nextAtMs": job.next_at_ms,
        "expiresAtMs": job.expires_at_ms,
        "lastFiredAtMs": job.last_fired_at_ms,
        "lastError": job.last_error,
        "heldHere": job.held_here,
    })
}
