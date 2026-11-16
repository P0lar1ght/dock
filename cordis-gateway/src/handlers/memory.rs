//! `memory/*`：记忆图谱与笔记管理（GUI 的「记忆」页）。
//!
//! 记忆笔记是 `$DOCK_HOME/memory/` 下的 Markdown，笔记之间用 `[[文件名]]` 互相链接。
//! 这里把它投成一张图（笔记是点、链接是边、内容相近的是「建议」边），并提供在界面里
//! 管理笔记的方法。引擎在 `dock-memory`（[`dock_memory::graph`] / [`dock_memory::notes`]）。
//!
//! **全部只认受信连接**：记忆是跨项目的私人内容，配对来的网页不能读、更不能改。
//! 记忆功能关着（`[memory] enabled = false`）也能看、能改——文件就在盘上，开关管的是
//! agent 用不用它（回包里的 `enabled` / `forcedOff` 给界面画提示）。
//!
//! 笔记 id 是 `<scope>/<相对路径>`（`global/topics/prefs.md`、
//! `workspace-dock-b4be1135/observations/_inbox/flush-….md`），`scope` 就是记忆目录下
//! 那一层的目录名。
//!
//! - `memory/graph { cwd? | threadId?, scopes?, similar? }` → `{ enabled, forcedOff, home,
//!   currentScope, scopes[], notes[], edges[], unresolved[], similarSkipped }`。
//!   `scopes`：`"current"`（默认：全局 + `cwd` / `threadId` 那个项目）、`"all"`，或点名的
//!   scope id 数组。`edges[].kind`：`link`（正文里的 `[[…]]`，有方向）/ `similar`
//!   （内容相近、还没链上，`score` 0–1）；`similar: false` 不算相似度。`unresolved` 是指向
//!   不存在的笔记的链接。只读，不建目录。
//! - `memory/search { query, cwd? | threadId?, scopes?, limit? }` → `{ hits: [{ id, line,
//!   snippet }] }`：按内容找（每个词都要出现），范围同 `memory/graph`。
//! - `memory/note/get { id, scopes?, cwd? | threadId? }` → `{ note, links: { outgoing[],
//!   backlinks[], unresolved[], targets[] } }`：全文 + `hash` + 出链 / 反链（带链接所在的
//!   那一行）；`targets` 是正文里每种 `[[写法]]` 指向的笔记 id（悬空为 `null`）。
//! - `memory/note/save { id, text, baseHash? }` → `{ note }`：`baseHash` 对不上回 `conflict`。
//! - `memory/note/create { scope, kind, title, text? }` → `{ note }`：文件名取自标题。
//! - `memory/note/rename { id, title?, name? }` → `{ note, oldId, relinked[] }`：`title` 换
//!   正文的一级标题，`name` 换文件名；指向它的链接跟着改写。
//! - `memory/note/move { id, scope?, kind? }` → `{ note, oldId, relinked[] }`：挪到别的
//!   scope / 观察升为主题。
//! - `memory/note/forget { id, hash }` → `{ archivedTo }`：归档 + 墓碑（同 `/memory` 的遗忘）。
//! - `memory/link { from, to }` / `memory/unlink { from, to | target }` → `{ note, changed }`：
//!   在 `from` 里加 / 去掉指向 `to` 的链接；`target` 是悬空链接里写的名字。
//! - 推送 `memory/changed {}`（连接级，只推受信连接）：有笔记变了（不管是界面、agent 还是
//!   外部编辑器改的），合并在 [`CHANGE_DEBOUNCE`] 内。第一次 `memory/graph` 之后才开始监听。
//!
//! 错误码：`invalid_params`、`not_found`、`read_only`（嵌套目录 / 旧版平铺的观察）、
//! `conflict`（期间被改过）、`too_large`、`write_failed`。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use cordis::{plugin, Context, Disposable, Inject, Plugin};
use cordis_spine::{session_cwd, Memory, MEMORY};
use dock_memory::graph::{
    build_graph, note_links, search_notes, GraphQuery, LinkedNote, ScopeSelection,
};
use dock_memory::notes::{self, LinkChange, LinkTarget, NoteContent, NoteError, Relocated};
use dock_memory::similarity::SimilarConfig;
use dock_memory::vault::{
    scope_label, workspace_scope_id, LoadedNote, MemoryVault, NoteEntry, NoteKind, VaultError,
    GLOBAL_SCOPE_ID,
};
use dock_memory::{AccessError, MemoryChangeWatcher, MemoryScope};

use crate::handle::GatewayHandle;
use crate::methods::{method, register_methods, GatewayMethods, MethodPolicy, GATEWAY_METHODS};
use crate::protocol::{self, RpcError};
use crate::threads;

/// 文件变动合并成一次推送的间隔：一次保存会连着触发好几个文件事件
/// （临时文件换上去、`MEMORY.md` 重新生成、改名时被改写的反链）。
const CHANGE_DEBOUNCE: Duration = Duration::from_millis(250);

const SEARCH_DEFAULT: usize = 50;
const SEARCH_MAX: usize = 200;

/// 记忆这一块：把 `memory/*` 登记进网关的方法表。都读盘（`memory/graph` 还可能跑一次
/// git 算工作区 slug），放到连接锁外跑。
pub fn gateway_memory() -> Plugin {
    plugin(
        "gateway.memory",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            let policy = MethodPolicy {
                detached: true,
                trusted_only: true,
                opens_thread: false,
            };
            let feed = Arc::new(ChangeFeed::default());
            let graph_feed = feed.clone();
            register_methods(
                ctx,
                vec![
                    (
                        protocol::MEMORY_GRAPH,
                        policy,
                        method(move |gw, params| {
                            let feed = graph_feed.clone();
                            async move { graph(&gw, params, &feed).await }
                        }),
                    ),
                    (
                        protocol::MEMORY_SEARCH,
                        policy,
                        method(|gw, params| async move { search(&gw, params).await }),
                    ),
                    (
                        protocol::MEMORY_NOTE_GET,
                        policy,
                        method(|gw, params| async move { note_get(&gw, params).await }),
                    ),
                    (
                        protocol::MEMORY_NOTE_SAVE,
                        policy,
                        method(
                            |_, params| async move { blocking(move || note_save(&params)).await },
                        ),
                    ),
                    (
                        protocol::MEMORY_NOTE_CREATE,
                        policy,
                        method(
                            |_, params| async move { blocking(move || note_create(&params)).await },
                        ),
                    ),
                    (
                        protocol::MEMORY_NOTE_RENAME,
                        policy,
                        method(
                            |_, params| async move { blocking(move || note_rename(&params)).await },
                        ),
                    ),
                    (
                        protocol::MEMORY_NOTE_MOVE,
                        policy,
                        method(
                            |_, params| async move { blocking(move || note_move(&params)).await },
                        ),
                    ),
                    (
                        protocol::MEMORY_NOTE_FORGET,
                        policy,
                        method(
                            |_, params| async move { blocking(move || note_forget(&params)).await },
                        ),
                    ),
                    (
                        protocol::MEMORY_LINK,
                        policy,
                        method(|_, params| async move { blocking(move || link(&params)).await }),
                    ),
                    (
                        protocol::MEMORY_UNLINK,
                        policy,
                        method(|_, params| async move { blocking(move || unlink(&params)).await }),
                    ),
                ],
            )?;
            ctx.effect("gateway.memory.changes", move |scope| {
                scope.own(Disposable::from_fn(move || feed.stop()));
                Ok(())
            })?;
            Ok(None)
        },
    )
}

/// 笔记变动 → `memory/changed`。监听和转发任务都是第一次有人看图时才起：只用 TUI 的
/// 进程不为它开文件监听。
#[derive(Default)]
struct ChangeFeed {
    running: Mutex<Option<Running>>,
}

struct Running {
    _watcher: MemoryChangeWatcher,
    pump: tokio::task::JoinHandle<()>,
}

impl ChangeFeed {
    /// 记忆目录还不存在时起不来（没东西可监听）：下一次看图再试。
    fn ensure_running(&self, ctx: &Context, vault: &MemoryVault) {
        let mut running = self.running.lock().unwrap();
        if running.is_some() {
            return;
        }
        let signal = Arc::new(tokio::sync::Notify::new());
        let wake = signal.clone();
        let Some(watcher) = MemoryChangeWatcher::start(vault.home(), move || wake.notify_one())
        else {
            return;
        };
        // 方法表在推送时现取，不把它关进任务里。
        let live = ctx.clone();
        let pump = tokio::spawn(async move {
            loop {
                signal.notified().await;
                // 等这一阵写完再推：期间再来的事件并进下一次。
                tokio::time::sleep(CHANGE_DEBOUNCE).await;
                if let Some(table) = live.get::<GatewayMethods>(GATEWAY_METHODS) {
                    table.notify_trusted(protocol::MEMORY_CHANGED, json!({}));
                }
            }
        });
        *running = Some(Running {
            _watcher: watcher,
            pump,
        });
    }

    fn stop(&self) {
        if let Some(running) = self.running.lock().unwrap().take() {
            running.pump.abort();
        }
    }
}

async fn blocking(
    work: impl FnOnce() -> Result<Value, RpcError> + Send + 'static,
) -> Result<Value, RpcError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| RpcError::app("internal", format!("记忆操作没跑完：{e}")))?
}

fn text<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn required<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    text(params, key).ok_or_else(|| RpcError::invalid_params(format!("{key} is required")))
}

/// 「当前项目」的目录：`threadId` 取那一页的 cwd，否则用 `cwd`（绝对路径）。都没有就是
/// 没有当前项目（只看全局）。
///
/// `cwd` 不要求是有会话的项目：侧栏里刚加、还没开过会话的项目也该能看它的记忆。它只
/// 用来算工作区 slug，而且这些方法只给受信连接。
fn project_dir(gateway: &GatewayHandle, params: &Value) -> Result<Option<PathBuf>, RpcError> {
    if let Some(thread) = text(params, "threadId") {
        return Ok(Some(session_cwd(&threads::resolve(gateway, thread)?.ctx)));
    }
    match text(params, "cwd") {
        None => Ok(None),
        Some(raw) => {
            let path = PathBuf::from(raw);
            if !path.is_absolute() {
                return Err(RpcError::invalid_params("cwd 必须是绝对路径"));
            }
            Ok(Some(path))
        }
    }
}

fn selection(params: &Value) -> Result<ScopeSelection, RpcError> {
    match params.get("scopes") {
        None | Some(Value::Null) => Ok(ScopeSelection::Current),
        Some(Value::String(s)) if s == "current" => Ok(ScopeSelection::Current),
        Some(Value::String(s)) if s == "all" => Ok(ScopeSelection::All),
        Some(Value::Array(ids)) => ids
            .iter()
            .map(|id| id.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .map(ScopeSelection::Only)
            .ok_or_else(|| RpcError::invalid_params("scopes 数组里只能是 scope id")),
        Some(_) => Err(RpcError::invalid_params(
            "scopes 是 \"current\"、\"all\" 或 scope id 数组",
        )),
    }
}

/// 图上放哪些笔记：解析 `scopes` 与当前项目。算 slug 要跑 git，调用方放在阻塞线程里。
fn query_of(project: Option<PathBuf>, selection: ScopeSelection, similar: bool) -> GraphQuery {
    GraphQuery {
        current_workspace: project
            .map(|dir| workspace_scope_id(&dock_memory::workspace_slug(&dir))),
        selection,
        similar: SimilarConfig {
            per_note: if similar {
                SimilarConfig::default().per_note
            } else {
                0
            },
            ..SimilarConfig::default()
        },
    }
}

async fn graph(
    gateway: &GatewayHandle,
    params: Value,
    feed: &ChangeFeed,
) -> Result<Value, RpcError> {
    let project = project_dir(gateway, &params)?;
    let selection = selection(&params)?;
    let similar = params
        .get("similar")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    // 开关状态看记忆服务（含会话里临时切的）；没挂就看配置。
    let (enabled, forced_off) = match gateway.ctx().get::<Memory>(MEMORY) {
        Some(memory) => (memory.enabled(), memory.config().force_disabled),
        None => {
            let config = cordis_base::config::load_memory_config();
            (
                config.enabled && !config.force_disabled,
                config.force_disabled,
            )
        }
    };
    let vault = MemoryVault::open_default();
    feed.ensure_running(gateway.ctx(), &vault);
    blocking(move || {
        let query = query_of(project, selection, similar);
        let graph = build_graph(&vault, &query);
        Ok(json!({
            "enabled": enabled,
            "forcedOff": forced_off,
            "home": vault.home().display().to_string(),
            "currentScope": query.current_workspace,
            "scopes": graph.scopes.iter().map(|scope| json!({
                "id": scope.id,
                "kind": scope_kind(scope.scope),
                "label": scope_label(&scope.id),
                "current": scope.current,
                "selected": scope.selected,
                "topics": scope.topics,
                "observations": scope.observations,
            })).collect::<Vec<_>>(),
            "notes": graph.notes.iter().map(loaded_json).collect::<Vec<_>>(),
            "edges": graph.edges.iter().map(|edge| json!({
                "source": edge.source,
                "target": edge.target,
                "kind": edge.kind.as_str(),
                "score": edge.score,
            })).collect::<Vec<_>>(),
            "unresolved": graph.unresolved.iter().map(|link| json!({
                "source": link.source,
                "target": link.target,
            })).collect::<Vec<_>>(),
            "similarSkipped": graph.similar_skipped,
        }))
    })
    .await
}

/// `scopes` 选中的那几个 scope 的笔记（连正文）。
fn load_selected(vault: &MemoryVault, query: &GraphQuery) -> Vec<LoadedNote> {
    let on_disk = vault.scope_ids();
    let selected: Vec<String> = match &query.selection {
        ScopeSelection::All => on_disk,
        ScopeSelection::Only(ids) => on_disk.into_iter().filter(|id| ids.contains(id)).collect(),
        ScopeSelection::Current => on_disk
            .into_iter()
            .filter(|id| {
                id == GLOBAL_SCOPE_ID || Some(id.as_str()) == query.current_workspace.as_deref()
            })
            .collect(),
    };
    vault.load(&selected)
}

async fn search(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let project = project_dir(gateway, &params)?;
    let selection = selection(&params)?;
    let query_text = required(&params, "query")?.to_string();
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(SEARCH_DEFAULT)
        .clamp(1, SEARCH_MAX);
    blocking(move || {
        let vault = MemoryVault::open_default();
        let notes = load_selected(&vault, &query_of(project, selection, false));
        let hits: Vec<Value> = search_notes(&notes, &query_text, limit)
            .into_iter()
            .map(|hit| json!({ "id": hit.id, "line": hit.line, "snippet": hit.snippet }))
            .collect();
        Ok(json!({ "hits": hits }))
    })
    .await
}

async fn note_get(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let project = project_dir(gateway, &params)?;
    // 反链默认在所有项目里找：别的项目引用了它也该看得到。
    let selection = match params.get("scopes") {
        None | Some(Value::Null) => ScopeSelection::All,
        Some(_) => selection(&params)?,
    };
    let id = required(&params, "id")?.to_string();
    blocking(move || {
        let vault = MemoryVault::open_default();
        let note = notes::read_note(&vault, &id).map_err(note_error)?;
        let loaded = load_selected(&vault, &query_of(project, selection, false));
        let links = note_links(&loaded, &id);
        Ok(json!({
            "note": content_json(&note),
            "links": {
                "outgoing": links.outgoing.iter().map(linked_json).collect::<Vec<_>>(),
                "backlinks": links.backlinks.iter().map(linked_json).collect::<Vec<_>>(),
                "unresolved": links.unresolved,
                "targets": links.targets.iter().map(|t| json!({
                    "target": t.target,
                    "id": t.id,
                })).collect::<Vec<_>>(),
            },
        }))
    })
    .await
}

fn note_save(params: &Value) -> Result<Value, RpcError> {
    let id = required(params, "id")?;
    // 正文原样收：空串是合法的（清空一篇笔记），不能像别的参数那样 trim 掉。
    let body = params
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("text is required"))?;
    let note = notes::save_note(
        &MemoryVault::open_default(),
        id,
        body,
        text(params, "baseHash"),
    )
    .map_err(note_error)?;
    Ok(json!({ "note": content_json(&note) }))
}

fn kind_param(params: &Value) -> Result<Option<NoteKind>, RpcError> {
    match text(params, "kind") {
        None => Ok(None),
        Some(raw) => NoteKind::parse(raw)
            .map(Some)
            .ok_or_else(|| RpcError::invalid_params("kind 是 topic 或 observation")),
    }
}

fn note_create(params: &Value) -> Result<Value, RpcError> {
    let scope = required(params, "scope")?;
    let kind = kind_param(params)?.unwrap_or(NoteKind::Topic);
    let title = required(params, "title")?;
    let body = params.get("text").and_then(Value::as_str).unwrap_or("");
    let note = notes::create_note(&MemoryVault::open_default(), scope, kind, title, body)
        .map_err(note_error)?;
    Ok(json!({ "note": content_json(&note) }))
}

fn note_rename(params: &Value) -> Result<Value, RpcError> {
    let id = required(params, "id")?;
    let (title, name) = (text(params, "title"), text(params, "name"));
    if title.is_none() && name.is_none() {
        return Err(RpcError::invalid_params("title / name 至少给一个"));
    }
    notes::rename_note(&MemoryVault::open_default(), id, title, name)
        .map(|moved| relocated_json(&moved))
        .map_err(note_error)
}

fn note_move(params: &Value) -> Result<Value, RpcError> {
    let id = required(params, "id")?;
    let (scope, kind) = (text(params, "scope"), kind_param(params)?);
    if scope.is_none() && kind.is_none() {
        return Err(RpcError::invalid_params("scope / kind 至少给一个"));
    }
    notes::move_note(&MemoryVault::open_default(), id, scope, kind)
        .map(|moved| relocated_json(&moved))
        .map_err(note_error)
}

fn note_forget(params: &Value) -> Result<Value, RpcError> {
    let id = required(params, "id")?;
    let hash = required(params, "hash")?;
    let forgotten =
        notes::forget_note(&MemoryVault::open_default(), id, hash).map_err(note_error)?;
    Ok(json!({
        "archivedTo": forgotten.archived_to.map(|p| p.display().to_string()),
    }))
}

fn link(params: &Value) -> Result<Value, RpcError> {
    let from = required(params, "from")?;
    let to = required(params, "to")?;
    notes::link_notes(&MemoryVault::open_default(), from, to)
        .map(|change| link_json(&change))
        .map_err(note_error)
}

fn unlink(params: &Value) -> Result<Value, RpcError> {
    let from = required(params, "from")?;
    let target = match (text(params, "to"), text(params, "target")) {
        (Some(to), None) => LinkTarget::Note(to),
        (None, Some(name)) => LinkTarget::Dangling(name),
        _ => return Err(RpcError::invalid_params("to 和 target 给且只给一个")),
    };
    notes::unlink_notes(&MemoryVault::open_default(), from, target)
        .map(|change| link_json(&change))
        .map_err(note_error)
}

fn scope_kind(scope: MemoryScope) -> &'static str {
    match scope {
        MemoryScope::Global => "global",
        MemoryScope::Workspace => "workspace",
    }
}

fn entry_json(entry: &NoteEntry, title: &str, summary: &str) -> Value {
    json!({
        "id": entry.id,
        "scope": entry.scope_id,
        "kind": entry.kind.as_str(),
        "origin": entry.origin.as_str(),
        "name": entry.stem,
        "title": title,
        "summary": summary,
        "path": entry.path.display().to_string(),
        "bytes": entry.bytes,
        "modifiedMs": entry.modified_ms,
        "writable": entry.writable,
    })
}

/// 图上的一个点（不带正文）。
fn loaded_json(note: &LoadedNote) -> Value {
    let mut value = entry_json(&note.entry, &note.title, &note.summary);
    if note.truncated {
        value["truncated"] = json!(true);
    }
    value
}

/// 一篇笔记的全文：点的那些字段 + `text` + `hash`。
fn content_json(note: &NoteContent) -> Value {
    let mut value = entry_json(&note.entry, &note.title, &note.summary);
    value["text"] = json!(note.text);
    value["hash"] = json!(note.hash);
    value
}

fn linked_json(link: &LinkedNote) -> Value {
    json!({
        "id": link.id,
        "title": link.title,
        "context": link.context,
        "line": link.line,
    })
}

fn relocated_json(moved: &Relocated) -> Value {
    json!({
        "note": content_json(&moved.note),
        "oldId": moved.old_id,
        "relinked": moved.relinked,
    })
}

fn link_json(change: &LinkChange) -> Value {
    json!({ "note": content_json(&change.note), "changed": change.changed })
}

fn note_error(error: NoteError) -> RpcError {
    let message = error.to_string();
    match error {
        NoteError::Vault(VaultError::InvalidId(_) | VaultError::InvalidScope(_))
        | NoteError::Invalid(_) => RpcError::invalid_params(message),
        NoteError::Vault(VaultError::NotFound(_)) => RpcError::app("not_found", message),
        NoteError::Vault(VaultError::TooLarge { .. })
        | NoteError::TooLarge { .. }
        | NoteError::Access(AccessError::TooLarge { .. }) => RpcError::app("too_large", message),
        NoteError::ReadOnly(_)
        | NoteError::Access(
            AccessError::Protected(_) | AccessError::NestedPath(_) | AccessError::Outside(_),
        ) => RpcError::app("read_only", message),
        NoteError::Conflict { .. } => RpcError::app("conflict", message),
        // 遗忘时指纹对不上（界面看到的不是盘上现在这一版）。
        NoteError::Access(AccessError::Io(ref io))
            if io.kind() == std::io::ErrorKind::InvalidInput =>
        {
            RpcError::app("conflict", message)
        }
        NoteError::Vault(VaultError::Io(_))
        | NoteError::Access(_)
        | NoteError::Io(_)
        | NoteError::Index(_) => RpcError::app("write_failed", message),
    }
}
