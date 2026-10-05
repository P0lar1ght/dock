//! `fs/*`：只读看会话工作区里的文件（GUI 的文件面板）。
//!
//! - `fs/list { threadId?, path?, hidden? }` → `{ root, path, entries: [{ name, path, kind,
//!   size?, modifiedMs? }], truncated }`。`kind` 是 `dir` / `file` / `symlink`。
//! - `fs/read { threadId?, path, maxBytes? }` → `{ path, size, modifiedMs?, kind, ... }`：
//!   `kind:"text"` 带 `text` 与 `truncated`；`kind:"image"` 带 `mime` 与 base64 `data`；
//!   `kind:"binary"` 只有大小。
//! - `fs/find { threadId?, query, limit? }` → `{ paths }`：按名字找文件（快速打开）。
//!
//! 路径都相对会话的 cwd，出了这个目录就拒绝（见 [`cordis_base::workspace_files`]）。
//! 只认开着的会话（要从页上拿 cwd）。读盘放到阻塞线程里，不占网关的异步线程。

use base64::Engine;
use cordis_base::workspace_files::{self as files, Content};
use serde_json::{json, Value};

use crate::handle::GatewayHandle;
use crate::protocol::{self, RpcError};
use crate::threads;

use cordis::{plugin, Inject, Plugin};

use crate::methods::{method, register_methods, MethodPolicy, GATEWAY_METHODS};

const FIND_DEFAULT: usize = 50;
const FIND_MAX: usize = 500;

pub async fn dispatch(
    gateway: &GatewayHandle,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let root = cordis_spine::session_cwd(&page.ctx);
    let method = method.to_string();
    tokio::task::spawn_blocking(move || match method.as_str() {
        protocol::FS_LIST => list(&root, &params),
        protocol::FS_READ => read(&root, &params),
        protocol::FS_FIND => find(&root, &params),
        other => Err(RpcError::method_not_found(other)),
    })
    .await
    .map_err(|e| RpcError::app("internal", format!("读文件的任务没跑完：{e}")))?
}

fn path_param(params: &Value) -> &str {
    params.get("path").and_then(Value::as_str).unwrap_or("")
}

fn list(root: &std::path::Path, params: &Value) -> Result<Value, RpcError> {
    let hidden = params
        .get("hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let listing =
        files::list(root, path_param(params), hidden).map_err(RpcError::invalid_params)?;
    let entries: Vec<Value> = listing
        .entries
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "path": e.path,
                "kind": e.kind.as_str(),
                "size": e.size,
                "modifiedMs": e.modified_ms,
            })
        })
        .collect();
    Ok(json!({
        "root": root.display().to_string(),
        "path": listing.path,
        "entries": entries,
        "truncated": listing.truncated,
    }))
}

fn read(root: &std::path::Path, params: &Value) -> Result<Value, RpcError> {
    let path = path_param(params);
    if path.is_empty() {
        return Err(RpcError::invalid_params("path is required"));
    }
    let max = params
        .get("maxBytes")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).clamp(1, files::TEXT_LIMIT))
        .unwrap_or(files::TEXT_LIMIT);
    let file = files::read(root, path, max).map_err(RpcError::invalid_params)?;
    let mut out = json!({
        "path": file.path,
        "size": file.size,
        "modifiedMs": file.modified_ms,
    });
    match file.content {
        Content::Text { text, truncated } => {
            out["kind"] = json!("text");
            out["text"] = json!(text);
            out["truncated"] = json!(truncated);
            if let Some(mime) = file.mime {
                out["mime"] = json!(mime);
            }
        }
        Content::Image { mime, bytes } => {
            out["kind"] = json!("image");
            out["mime"] = json!(mime);
            out["data"] = json!(base64::engine::general_purpose::STANDARD.encode(bytes));
        }
        Content::Binary => out["kind"] = json!("binary"),
    }
    Ok(out)
}

fn find(root: &std::path::Path, params: &Value) -> Result<Value, RpcError> {
    let query = params.get("query").and_then(Value::as_str).unwrap_or("");
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).clamp(1, FIND_MAX))
        .unwrap_or(FIND_DEFAULT);
    let paths = files::find(root, query, limit).map_err(RpcError::invalid_params)?;
    Ok(json!({ "paths": paths }))
}

/// 工作区文件这一块：`fs/list|read|find`。读大仓库要走很多目录，放到连接锁外跑。
pub fn gateway_fs() -> Plugin {
    plugin(
        "gateway.fs",
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
                    entry(protocol::FS_LIST),
                    entry(protocol::FS_READ),
                    entry(protocol::FS_FIND),
                ],
            )?;
            Ok(None)
        },
    )
}
