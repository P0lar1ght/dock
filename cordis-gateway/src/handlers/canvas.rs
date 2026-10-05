//! `canvas/*`：会话的画布（模型用 `canvas_*` 工具写的 HTML），GUI 的画布面板。
//!
//! - `canvas/list { threadId? }` → `{ canvases: [Meta] }`，最近改过的在前。
//! - `canvas/get { threadId?, canvasId, version? }` → `{ canvas: Meta, version, html, data,
//!   path }`：默认最新版；`data` 没有就是 `null`；`path` 是画布目录（本机「在访达中显示」）。
//! - `canvas/setData { threadId?, canvasId, data }` → `{ canvas: Meta }`：用户在画布里改的
//!   数据（`dock.setData`）。不出新版。
//! - `canvas/rollback { threadId?, canvasId, version }` → `{ canvas: Meta }`：把那一版拷成
//!   新的最新版，历史不丢。
//!
//! `Meta` = `{ id, title, createdMs, updatedMs, dataUpdatedMs, latest, versions: [{ n, note,
//! createdMs, bytes }] }`。
//!
//! 画布跟着会话目录走（见 [`cordis_base::canvas`]）：开着的页用它正在写的会话，关着的
//! 会话从名册找目录，所以历史会话的画布也看得到。读写放到阻塞线程里。

use std::path::PathBuf;

use cordis_base::canvas::{self, Meta};
use serde_json::{json, Value};

use crate::handle::GatewayHandle;
use crate::handlers::thread::fresh_roster;
use crate::protocol::{self, RpcError};
use crate::threads;

use cordis::{plugin, Inject, Plugin};

use crate::methods::{method, register_methods, MethodPolicy, GATEWAY_METHODS};

pub async fn dispatch(
    gateway: &GatewayHandle,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    let dir = session_dir(gateway, &params)?;
    let method = method.to_string();
    tokio::task::spawn_blocking(move || {
        let Some(dir) = dir else {
            // 还没落过盘的新会话：一张画布都不会有。
            return match method.as_str() {
                protocol::CANVAS_LIST => Ok(json!({ "canvases": [] })),
                _ => Err(RpcError::app("not_found", "这个会话还没有画布")),
            };
        };
        match method.as_str() {
            protocol::CANVAS_LIST => {
                let all: Vec<Value> = canvas::list(&dir).iter().map(meta_value).collect();
                Ok(json!({ "canvases": all }))
            }
            protocol::CANVAS_GET => get(&dir, &params),
            protocol::CANVAS_SET_DATA => {
                let data = params
                    .get("data")
                    .ok_or_else(|| RpcError::invalid_params("data is required"))?;
                let meta = canvas::set_data(&dir, canvas_id(&params)?, data).map_err(app_error)?;
                Ok(json!({ "canvas": meta_value(&meta) }))
            }
            protocol::CANVAS_ROLLBACK => {
                let to = params
                    .get("version")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| RpcError::invalid_params("version is required"))?;
                let meta =
                    canvas::rollback(&dir, canvas_id(&params)?, to as u32).map_err(app_error)?;
                Ok(json!({ "canvas": meta_value(&meta) }))
            }
            other => Err(RpcError::method_not_found(other)),
        }
    })
    .await
    .map_err(|e| RpcError::app("internal", format!("读画布的任务没跑完：{e}")))?
}

/// 会话目录。开着但还没落过盘的页是 `None`（不替它新建目录）。
fn session_dir(gateway: &GatewayHandle, params: &Value) -> Result<Option<PathBuf>, RpcError> {
    let id = threads::thread_param(params);
    if let Ok(page) = threads::resolve(gateway, &id) {
        let sessions = page.sessions()?;
        if sessions.live_session_id().is_empty() {
            return Ok(None);
        }
        return Ok(sessions.disk_session_dir());
    }
    let entry = fresh_roster(gateway)
        .list()
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| RpcError::app("not_found", format!("thread {id} not found")))?;
    Ok(Some(
        cordis_spine::sessions_cwd_dir(&entry.cwd).join(&entry.id),
    ))
}

fn canvas_id(params: &Value) -> Result<&str, RpcError> {
    params
        .get("canvasId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params("canvasId is required"))
}

fn app_error(msg: String) -> RpcError {
    let code = if msg.starts_with("没有") {
        "not_found"
    } else {
        "invalid"
    };
    RpcError::app(code, msg)
}

fn meta_value(meta: &Meta) -> Value {
    let mut v = serde_json::to_value(meta).unwrap_or_else(|_| json!({}));
    v["latest"] = json!(meta.latest());
    v
}

fn get(dir: &std::path::Path, params: &Value) -> Result<Value, RpcError> {
    let id = canvas_id(params)?;
    let version = params
        .get("version")
        .and_then(Value::as_u64)
        .map(|n| n as u32);
    let c = canvas::read(dir, id, version).map_err(app_error)?;
    Ok(json!({
        "canvas": meta_value(&c.meta),
        "version": c.version,
        "html": c.html,
        "data": c.data,
        "path": canvas::root(dir).join(id).display().to_string(),
    }))
}

/// 画布这一块：`canvas/list|get|setData|rollback`。读写会话目录下的文件，放到连接锁外跑。
pub fn gateway_canvas() -> Plugin {
    plugin(
        "gateway.canvas",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            let entry = |name: &'static str| {
                (
                    name,
                    MethodPolicy::detached(),
                    method(move |gw, params| async move { dispatch(&gw, name, params).await }),
                )
            };
            register_methods(
                ctx,
                vec![
                    entry(protocol::CANVAS_LIST),
                    entry(protocol::CANVAS_GET),
                    entry(protocol::CANVAS_SET_DATA),
                    entry(protocol::CANVAS_ROLLBACK),
                ],
            )?;
            Ok(None)
        },
    )
}
