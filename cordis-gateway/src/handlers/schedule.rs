//! `schedule/*`：定时任务（`"cron"`）。任务落盘在 `$DOCK_HOME/schedules.json`，属于某个
//! 会话、跨重启继续跑；到点由 `cordis-app` 的驱动送进那个会话（关着就开页），或者在
//! 那个会话的项目里新开一个会话（`target: "newSession"`）。
//!
//! 触发方式六选一（创建时必给一个，更新时可换）：`interval` / `everySecs`（固定间隔）、
//! `cron`（5 段表达式，本机时区）、`atMs` / `at`（只跑一次）、`watch`（条件命令）、
//! `files`（文件变化）、`manual: true`（只手动）。
//!
//! - `schedule/list {}` → `{ tasks[] }`，全部会话的任务（GUI 的「定时任务」页）。
//! - `schedule/create { threadId, prompt, <触发方式>, fireImmediately?, expiresAtMs?, maxRuns?,
//!   catchUp?, target?, preset?, paused? }` → `{ task }`。`expiresAtMs` 省略 = 默认（比一天
//!   还勤的 7 天后过期），`null` = 不过期。
//! - `schedule/update { id, <触发方式>?, prompt?, expiresAtMs?, maxRuns?, catchUp?, target?,
//!   preset?, paused? }` → `{ task }`。`maxRuns: null` = 不限次数，`preset: null` = 不指定。
//! - `schedule/run { id, input? }` → `{ ok }`：马上跑一次（不占次数、不动排期）。`input`
//!   附在这次的提问上给模型看。
//! - `schedule/delete { id }` → `{ deleted }`。
//! - 推送 `schedule/changed {}`（连接级，不用订阅）：任何一处改了（含模型的
//!   `scheduler_*`、别的 Dock 进程、到点触发），最多 1 秒后到。收到后重拉 `schedule/list`。

use std::time::Duration;

use serde_json::{json, Value};

use cordis_spine::{
    parse_at, parse_interval, session_cwd, AgentPresets, Cron, CronError, CronExpiry, CronJob,
    CronOwner, CronPatch, CronSchedule, CronTarget, FireReason, NewCronTask, WatchWhen,
    AGENT_PRESETS, CRON, DEFAULT_CHECK_EVERY, DEFAULT_FILES_DEBOUNCE, SCHEDULE_CHANGED,
};

use cordis::{plugin, Inject, Plugin};

use crate::handle::GatewayHandle;
use crate::handlers::thread::roster_entries;
use crate::methods::{method, register_methods, GatewayMethods, MethodPolicy, GATEWAY_METHODS};
use crate::protocol::{self, RpcError};
use crate::threads;

/// 定时任务这一块：把 `schedule/*` 登记进网关的方法表，并把 `"cron"` 的变动转成连接级
/// 推送 `schedule/changed`。
pub fn gateway_schedule() -> Plugin {
    plugin(
        "gateway.schedule",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            register_methods(
                ctx,
                vec![
                    (
                        protocol::SCHEDULE_LIST,
                        MethodPolicy::default(),
                        method(|gw, params| async move { list(&gw, params) }),
                    ),
                    (
                        protocol::SCHEDULE_CREATE,
                        MethodPolicy::default(),
                        method(|gw, params| async move { create(&gw, params) }),
                    ),
                    (
                        protocol::SCHEDULE_UPDATE,
                        MethodPolicy::default(),
                        method(|gw, params| async move { update(&gw, params) }),
                    ),
                    (
                        protocol::SCHEDULE_DELETE,
                        MethodPolicy::default(),
                        method(|gw, params| async move { delete(&gw, params) }),
                    ),
                    (
                        protocol::SCHEDULE_RUN,
                        MethodPolicy::default(),
                        method(|gw, params| async move { run(&gw, params) }),
                    ),
                ],
            )?;
            // 方法表在推送时现取，不把它关进监听闭包。
            let live = ctx.clone();
            let listener = ctx.on(SCHEDULE_CHANGED, move |_: &()| {
                if let Some(table) = live.get::<GatewayMethods>(GATEWAY_METHODS) {
                    table.notify(protocol::SCHEDULE_CHANGED, json!({}));
                }
            })?;
            ctx.effect("gateway.schedule.changed", |scope| {
                scope.own(listener);
                Ok(())
            })?;
            Ok(None)
        },
    )
}

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
    let schedule = schedule(&params)?.ok_or_else(|| {
        RpcError::invalid_params(
            "interval / everySecs / cron / atMs / at / watch / files / manual 必须给一个",
        )
    })?;
    let (owner, owner_preset) = owner_of(gateway, &params)?;
    let flag = |key: &str| params.get(key).and_then(Value::as_bool);
    let target = target(&params)?.unwrap_or_default();
    let preset = match params.get("preset") {
        Some(Value::String(id)) if !id.trim().is_empty() => Some(id.trim().to_string()),
        // 没指定：新开会话照这个会话现在的预设开。
        _ if target == CronTarget::NewSession => owner_preset,
        _ => None,
    };
    let id = cron
        .create(NewCronTask {
            owner: Some(owner),
            fire_immediately: flag("fireImmediately").unwrap_or(false),
            expiry: expiry(&params)?.unwrap_or_default(),
            max_runs: max_runs(&params)?.flatten(),
            catch_up: flag("catchUp"),
            target,
            preset,
            paused: flag("paused").unwrap_or(false),
            ..NewCronTask::new(schedule, prompt)
        })
        .map_err(cron_error)?;
    let job = cron
        .get(&id)
        .ok_or_else(|| RpcError::app("internal", "刚建的任务读不回来"))?;
    Ok(json!({ "task": task_json(&job, None) }))
}

pub fn update(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let cron = cron(gateway)?;
    let id = text(&params, "id").ok_or_else(|| RpcError::invalid_params("id is required"))?;
    let preset = match params.get("preset") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(id)) => Some(Some(id.trim().to_string()).filter(|s| !s.is_empty())),
        Some(_) => return Err(RpcError::invalid_params("preset 是预设 id 或 null")),
    };
    let patch = CronPatch {
        schedule: schedule(&params)?,
        prompt: text(&params, "prompt"),
        expiry: expiry(&params)?,
        max_runs: max_runs(&params)?,
        catch_up: params.get("catchUp").and_then(Value::as_bool),
        target: target(&params)?,
        preset,
        paused: params.get("paused").and_then(Value::as_bool),
    };
    if patch.is_empty() {
        return Err(RpcError::invalid_params("没有要改的字段"));
    }
    let job = cron.patch(&id, None, patch).map_err(cron_error)?;
    Ok(json!({ "task": task_json(&job, None) }))
}

/// 马上跑一次。持有它的 Dock（可能是别的进程）一秒内送出去。
pub fn run(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let cron = cron(gateway)?;
    let id = text(&params, "id").ok_or_else(|| RpcError::invalid_params("id is required"))?;
    let input = text(&params, "input").map(|text| format!("Input passed to this run:\n{text}"));
    cron.trigger(&id, None, FireReason::Manual, input)
        .map_err(cron_error)?;
    Ok(json!({ "ok": true }))
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
/// 第二个值是那个会话现在的预设（新开会话时照它开）。
fn owner_of(
    gateway: &GatewayHandle,
    params: &Value,
) -> Result<(CronOwner, Option<String>), RpcError> {
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
        let preset = page
            .ctx
            .get::<AgentPresets>(AGENT_PRESETS)
            .map(|p| p.current_id())
            .filter(|id| !id.is_empty());
        return Ok((
            CronOwner {
                session,
                cwd: session_cwd(&page.ctx),
            },
            preset,
        ));
    }
    let entry = roster_entries(gateway)
        .into_iter()
        .find(|e| e.id == thread)
        .ok_or_else(|| RpcError::app("not_found", format!("thread {thread} not found")))?;
    Ok((
        CronOwner {
            session: entry.id,
            cwd: entry.cwd,
        },
        entry.preset_id,
    ))
}

/// `target`：`session`（默认）/ `newSession`。
fn target(params: &Value) -> Result<Option<CronTarget>, RpcError> {
    match params.get("target").and_then(Value::as_str) {
        None => Ok(None),
        Some("session") => Ok(Some(CronTarget::Session)),
        Some("newSession") => Ok(Some(CronTarget::NewSession)),
        Some(other) => Err(RpcError::invalid_params(format!(
            "target 是 session 或 newSession，收到了 {other}"
        ))),
    }
}

/// 触发方式：`interval` / `everySecs` / `cron` / `atMs` / `at` / `watch` / `files` / `manual`
/// 里的一个。都没给是 `None`。
fn schedule(params: &Value) -> Result<Option<CronSchedule>, RpcError> {
    let given: Vec<&str> = [
        "interval",
        "everySecs",
        "cron",
        "atMs",
        "at",
        "watch",
        "files",
        "manual",
    ]
    .into_iter()
    .filter(|k| match params.get(*k) {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(_) => true,
    })
    .collect();
    if given.len() > 1 {
        return Err(RpcError::invalid_params(format!(
            "触发方式只能给一个，收到了 {}",
            given.join("、")
        )));
    }
    if let Some(raw) = text(params, "interval") {
        let secs = parse_interval(&raw).map_err(|e| RpcError::invalid_params(e.to_string()))?;
        return Ok(Some(CronSchedule::Every(Duration::from_secs(secs))));
    }
    if let Some(v) = params.get("everySecs").filter(|v| !v.is_null()) {
        return Ok(Some(CronSchedule::Every(every_secs(v, "everySecs")?)));
    }
    if let Some(expr) = text(params, "cron") {
        return CronSchedule::cron(&expr).map(Some).map_err(cron_error);
    }
    if let Some(v) = params.get("atMs").filter(|v| !v.is_null()) {
        let at = v
            .as_u64()
            .ok_or_else(|| RpcError::invalid_params("atMs 是毫秒时间戳"))?;
        return Ok(Some(CronSchedule::At(at)));
    }
    if let Some(raw) = text(params, "at") {
        return parse_at(&raw, now_ms())
            .map(|at| Some(CronSchedule::At(at)))
            .map_err(cron_error);
    }
    if let Some(watch) = params.get("watch").filter(|v| !v.is_null()) {
        let command = text(watch, "command")
            .ok_or_else(|| RpcError::invalid_params("watch.command 不能为空"))?;
        let every = match watch.get("everySecs").filter(|v| !v.is_null()) {
            Some(v) => every_secs(v, "watch.everySecs")?,
            None => DEFAULT_CHECK_EVERY,
        };
        let when = match text(watch, "when").as_deref().unwrap_or("changed") {
            "changed" => WatchWhen::Changed,
            "succeeds" => WatchWhen::Succeeds,
            "fails" => WatchWhen::Fails,
            "matches" => WatchWhen::Matches {
                pattern: text(watch, "pattern").ok_or_else(|| {
                    RpcError::invalid_params("watch.when 是 matches 时要给 watch.pattern")
                })?,
            },
            other => {
                return Err(RpcError::invalid_params(format!(
                    "watch.when 是 changed / succeeds / fails / matches，收到了 {other}"
                )))
            }
        };
        return CronSchedule::watch(&command, every, when)
            .map(Some)
            .map_err(cron_error);
    }
    if let Some(files) = params.get("files").filter(|v| !v.is_null()) {
        let paths: Vec<String> = files
            .get("paths")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let debounce = match files.get("debounceSecs").and_then(Value::as_u64) {
            Some(secs) => Duration::from_secs(secs),
            None => DEFAULT_FILES_DEBOUNCE,
        };
        return CronSchedule::files(paths, debounce)
            .map(Some)
            .map_err(cron_error);
    }
    if params.get("manual").and_then(Value::as_bool) == Some(true) {
        return Ok(Some(CronSchedule::Manual));
    }
    Ok(None)
}

fn every_secs(v: &Value, key: &str) -> Result<Duration, RpcError> {
    let secs = v
        .as_u64()
        .ok_or_else(|| RpcError::invalid_params(format!("{key} 是正整数")))?;
    if secs < MIN_EVERY_SECS {
        return Err(RpcError::invalid_params(format!(
            "间隔最短 {MIN_EVERY_SECS} 秒"
        )));
    }
    Ok(Duration::from_secs(secs))
}

/// `expiresAtMs`：省略 = 不改 / 默认，`null` = 不过期，数字 = 那一刻。
fn expiry(params: &Value) -> Result<Option<CronExpiry>, RpcError> {
    match params.get("expiresAtMs") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(CronExpiry::Never)),
        Some(v) => v
            .as_u64()
            .map(|at| Some(CronExpiry::At(at)))
            .ok_or_else(|| RpcError::invalid_params("expiresAtMs 是毫秒时间戳或 null")),
    }
}

/// `maxRuns`：省略 = 不改，`null` = 不限，正整数 = 跑满就停。
fn max_runs(params: &Value) -> Result<Option<Option<u32>>, RpcError> {
    match params.get("maxRuns") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v) => v
            .as_u64()
            .filter(|n| *n >= 1)
            .map(|n| Some(Some(u32::try_from(n).unwrap_or(u32::MAX))))
            .ok_or_else(|| RpcError::invalid_params("maxRuns 是正整数或 null")),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
        CronError::InvalidSchedule(msg) => RpcError::invalid_params(msg),
        CronError::Store(msg) => RpcError::app("store_failed", msg),
    }
}

fn task_json(job: &CronJob, title: Option<String>) -> Value {
    let schedule = match &job.schedule {
        CronSchedule::Every(every) => json!({ "kind": "every", "everySecs": every.as_secs() }),
        CronSchedule::Cron(expr) => json!({ "kind": "cron", "cron": expr }),
        CronSchedule::At(at) => json!({ "kind": "at", "atMs": at }),
        CronSchedule::Watch(spec) => {
            let (when, pattern) = match &spec.when {
                WatchWhen::Changed => ("changed", None),
                WatchWhen::Succeeds => ("succeeds", None),
                WatchWhen::Fails => ("fails", None),
                WatchWhen::Matches { pattern } => ("matches", Some(pattern.clone())),
            };
            json!({
                "kind": "watch",
                "command": spec.command,
                "everySecs": spec.every.as_secs(),
                "when": when,
                "pattern": pattern,
            })
        }
        CronSchedule::Files(spec) => json!({
            "kind": "files",
            "paths": spec.paths,
            "debounceSecs": spec.debounce.as_secs(),
        }),
        CronSchedule::Manual => json!({ "kind": "manual" }),
    };
    let every_secs = match &job.schedule {
        CronSchedule::Every(every) => Some(every.as_secs()),
        _ => None,
    };
    let history: Vec<Value> = job
        .history
        .iter()
        .rev()
        .map(|run| {
            json!({
                "atMs": run.at_ms,
                "reason": run.reason,
                "outcome": run.outcome,
                "threadId": run.session,
                "lateMs": run.late_ms,
                "error": run.error,
            })
        })
        .collect();
    let watch = job.watch.as_ref().map(|w| {
        json!({
            "checkedAtMs": w.checked_at_ms,
            "exit": w.exit,
            "matched": w.matched,
            "error": w.error,
        })
    });
    json!({
        "id": job.id,
        "threadId": job.owner.as_ref().map(|o| o.session.clone()),
        "threadTitle": title,
        "cwd": job.owner.as_ref().map(|o| o.cwd.display().to_string()),
        "prompt": job.prompt,
        "schedule": schedule,
        "scheduleLabel": job.schedule.label(),
        "everySecs": every_secs,
        "intervalLabel": job.schedule.human(),
        "createdAtMs": job.created_at_ms,
        "nextAtMs": job.next_at_ms,
        "expiresAtMs": job.expires_at_ms,
        "lastFiredAtMs": job.last_fired_at_ms,
        "lastError": job.last_error,
        "runs": job.runs,
        "maxRuns": job.max_runs,
        "catchUp": job.catch_up,
        "target": job.target,
        "preset": job.preset,
        "paused": job.paused,
        "pending": job.pending,
        "finished": job.finished(),
        "history": history,
        "watch": watch,
        "heldHere": job.held_here,
        "holderLive": job.holder_live,
    })
}
