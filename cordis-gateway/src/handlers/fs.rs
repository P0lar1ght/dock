//! `fs/*`：只读看会话工作区里的文件（GUI 的文件面板）。
//!
//! - `fs/list { threadId?, path?, hidden? }` → `{ root, path, entries: [{ name, path, kind,
//!   size?, modifiedMs? }], truncated }`。`kind` 是 `dir` / `file` / `symlink`。
//! - `fs/read { threadId?, path, maxBytes? }` → `{ path, size, modifiedMs?, kind, ... }`：
//!   `kind:"text"` 带 `text` 与 `truncated`；`kind:"image"` 带 `mime` 与 base64 `data`；
//!   `kind:"binary"` 只有大小。
//! - `fs/read { threadId?, path, raw: true, offset?, length? }` → `{ path, size, modifiedMs?,
//!   kind:"bytes", offset, data, eof }`：原样读一段字节（base64），大文件分块拉，GUI 自己解析
//!   表格 / 文档。`length` 默认 1 MiB、最多 4 MiB。
//! - `fs/find { threadId?, query, limit? }` → `{ paths }`：按名字找文件（快速打开）。
//!
//! 改（只认受信连接，写盘成功后记一条给这一页的 agent，见 [`cordis_spine::record_user_edit`]）：
//!
//! - `fs/write { threadId?, path, text, baseModifiedMs?, create? }` → `{ path, size, modifiedMs }`：
//!   保存文本（同目录临时文件再换上）；`baseModifiedMs` 对不上回 `conflict`（期间被改过）；
//!   `create: true` 新建，已存在回 `invalid_params`。
//! - `fs/mkdir { threadId?, path }` → `{ path }`：新建文件夹（中间缺的一并建）。
//! - `fs/rename { threadId?, from, to }` → `{ path }`：改名 / 移动，目标已存在就拒绝。
//! - `fs/trash { threadId?, path }` → `{}`：移到系统废纸篓；没有回收站回 `write_failed`，不硬删。
//!
//! 路径都相对会话的 cwd，出了这个目录就拒绝（见 [`cordis_base::workspace_files`]）。
//! 只认开着的会话（要从页上拿 cwd）。读盘放到阻塞线程里，不占网关的异步线程。
//!
//! - `fs/dirs { path?, hidden? }` → `{ path, parent, home, entries: [{ name, path }], truncated }`：
//!   选目录用（远程 GUI 新建会话）。`path` 是绝对路径或 `~` 开头，空着就是主目录；
//!   只列子目录，不看会话、不限在工作区里，所以只认受信连接。

use std::path::{Path, PathBuf};

use base64::Engine;
use cordis_base::workspace_files::{self as files, Content, EditError};
use serde_json::{json, Value};

use crate::handle::GatewayHandle;
use crate::protocol::{self, RpcError};
use crate::threads;

use cordis::{plugin, Inject, Plugin};

use crate::methods::{method, register_methods, MethodPolicy, GATEWAY_METHODS};

const FIND_DEFAULT: usize = 50;
const FIND_MAX: usize = 500;
/// `fs/dirs` 一次最多列这么多个子目录。
const DIRS_LIMIT: usize = 1000;
/// `fs/read { raw }` 一块默认 / 最多多大：base64 后约 1.3 / 5.3 MB 一帧。
const RAW_CHUNK_DEFAULT: usize = 1024 * 1024;
const RAW_CHUNK_MAX: usize = 4 * 1024 * 1024;

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

/// 改工作区的方法：写盘在阻塞线程里，成功后给这一页记一条用户改动。
pub async fn edit(gateway: &GatewayHandle, method: &str, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let root = cordis_spine::session_cwd(&page.ctx);
    let method = method.to_string();
    let (out, note) = tokio::task::spawn_blocking(move || edit_blocking(&root, &method, &params))
        .await
        .map_err(|e| RpcError::app("internal", format!("写文件的任务没跑完：{e}")))??;
    cordis_spine::record_user_edit(&page.ctx, note);
    Ok(out)
}

fn str_param<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("{key} is required")))
}

fn edit_error(e: EditError) -> RpcError {
    match e {
        EditError::Invalid(m) => RpcError::invalid_params(m),
        conflict @ EditError::Conflict { .. } => RpcError::app("conflict", conflict.to_string()),
        EditError::Io(m) => RpcError::app("write_failed", m),
    }
}

fn edit_blocking(root: &Path, method: &str, params: &Value) -> Result<(Value, String), RpcError> {
    match method {
        protocol::FS_WRITE => {
            let path = str_param(params, "path")?;
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcError::invalid_params("text is required"))?;
            let create = params.get("create").and_then(Value::as_bool) == Some(true);
            let saved = if create {
                files::create_file(root, path, text)
            } else {
                let base = params.get("baseModifiedMs").and_then(Value::as_u64);
                files::save_text(root, path, text, base)
            }
            .map_err(edit_error)?;
            let note = format!("{} {}", if create { "新建" } else { "编辑" }, saved.path);
            Ok((
                json!({ "path": saved.path, "size": saved.size, "modifiedMs": saved.modified_ms }),
                note,
            ))
        }
        protocol::FS_MKDIR => {
            let path = files::make_dir(root, str_param(params, "path")?).map_err(edit_error)?;
            let note = format!("新建文件夹 {path}/");
            Ok((json!({ "path": path }), note))
        }
        protocol::FS_RENAME => {
            let from = str_param(params, "from")?;
            let path = files::rename(root, from, str_param(params, "to")?).map_err(edit_error)?;
            let note = format!("改名 / 移动 {} → {path}", from.trim_start_matches("./"));
            Ok((json!({ "path": path }), note))
        }
        protocol::FS_TRASH => {
            let path = str_param(params, "path")?;
            files::move_to_trash(root, path).map_err(edit_error)?;
            Ok((
                json!({}),
                format!("移到废纸篓 {}", path.trim_start_matches("./")),
            ))
        }
        other => Err(RpcError::method_not_found(other)),
    }
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
    if params.get("raw").and_then(Value::as_bool) == Some(true) {
        return read_raw(root, path, params);
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

fn read_raw(root: &Path, path: &str, params: &Value) -> Result<Value, RpcError> {
    let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let length = params
        .get("length")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).clamp(1, RAW_CHUNK_MAX))
        .unwrap_or(RAW_CHUNK_DEFAULT);
    let got = files::read_range(root, path, offset, length).map_err(RpcError::invalid_params)?;
    let end = got.offset + got.bytes.len() as u64;
    Ok(json!({
        "path": got.path,
        "size": got.size,
        "modifiedMs": got.modified_ms,
        "kind": "bytes",
        "offset": got.offset,
        "data": base64::engine::general_purpose::STANDARD.encode(&got.bytes),
        "eof": end >= got.size,
    }))
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

fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `fs/dirs` 的目标目录：空着是主目录，`~` / `~/x` 按主目录展开，其余必须是绝对路径。
/// （Windows 上不认 `~\x`，`canonicalize` 也会回 `\\?\C:\…`；远程 GUI 目前只有 macOS。）
fn dirs_target(raw: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let raw = raw.trim();
    let home_or = || {
        home.map(Path::to_path_buf)
            .ok_or("找不到主目录".to_string())
    };
    if raw.is_empty() || raw == "~" {
        return home_or();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return Ok(home_or()?.join(rest));
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(format!("要绝对路径：{raw}"));
    }
    Ok(path)
}

async fn dirs(params: Value) -> Result<Value, RpcError> {
    tokio::task::spawn_blocking(move || list_dirs(&params))
        .await
        .map_err(|e| RpcError::app("internal", format!("列目录的任务没跑完：{e}")))?
}

fn list_dirs(params: &Value) -> Result<Value, RpcError> {
    let hidden = params
        .get("hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let home = home_dir();
    let target =
        dirs_target(path_param(params), home.as_deref()).map_err(RpcError::invalid_params)?;
    let dir = target
        .canonicalize()
        .map_err(|e| RpcError::invalid_params(format!("打不开 {}：{e}", target.display())))?;
    if !dir.is_dir() {
        return Err(RpcError::invalid_params(format!(
            "不是目录：{}",
            dir.display()
        )));
    }
    // 没权限读不是参数错：单给一个码，界面好分「路径填错了」和「读不了」。
    let read = std::fs::read_dir(&dir)
        .map_err(|e| RpcError::app("read_failed", format!("读不了 {}：{e}", dir.display())))?;
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        // 跟着符号链接走：指向目录的链接也能点进去。
        if !entry.path().is_dir() {
            continue;
        }
        entries.push((name, entry.path()));
    }
    // 先全收、排好再截：截出来的是按名字排的前 DIRS_LIMIT 个，而不是 read_dir 顺序里的随便哪些。
    entries.sort_by_key(|(name, _)| name.to_lowercase());
    let truncated = entries.len() > DIRS_LIMIT;
    entries.truncate(DIRS_LIMIT);
    let entries: Vec<Value> = entries
        .into_iter()
        .map(|(name, path)| json!({ "name": name, "path": path.display().to_string() }))
        .collect();
    Ok(json!({
        "path": dir.display().to_string(),
        "parent": dir.parent().map(|p| p.display().to_string()),
        "home": home.map(|h| h.display().to_string()),
        "entries": entries,
        "truncated": truncated,
    }))
}

/// 工作区文件这一块：`fs/list|read|find`、改文件的 `fs/write|mkdir|rename|trash` 和选目录的
/// `fs/dirs`。读大仓库要走很多目录、写盘也是阻塞的，都放到连接锁外跑。
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
            let edit_entry = |name: &'static str| {
                (
                    name,
                    MethodPolicy {
                        trusted_only: true,
                        ..MethodPolicy::detached()
                    },
                    method(move |gw, params| async move { edit(&gw, name, params).await }),
                )
            };
            register_methods(
                ctx,
                vec![
                    entry(protocol::FS_LIST),
                    entry(protocol::FS_READ),
                    entry(protocol::FS_FIND),
                    edit_entry(protocol::FS_WRITE),
                    edit_entry(protocol::FS_MKDIR),
                    edit_entry(protocol::FS_RENAME),
                    edit_entry(protocol::FS_TRASH),
                    (
                        protocol::FS_DIRS,
                        MethodPolicy {
                            trusted_only: true,
                            ..MethodPolicy::detached()
                        },
                        method(|_, params| dirs(params)),
                    ),
                ],
            )?;
            Ok(None)
        },
    )
}
