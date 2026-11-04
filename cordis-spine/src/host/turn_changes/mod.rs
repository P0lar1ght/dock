//! 每一轮改了哪些文件：`search_replace` / `write_file` 第一次碰一个文件前存下原文，每次改完
//! 记下它现在的样子；这一轮的改动就是两者之差（增删行数、统一 diff），也能撤销回原文。
//! 照 Codex（`turn_diff_tracker`）和 Claude Code 的做法：只认这两颗编辑工具，bash 里改的
//! （`sed -i`、脚本生成）不算。
//!
//! - 轮次按这一页日志里第几条用户消息算（`ordinal`，从 0 起；和网关投影的 `user_turns` 同序）。
//!   记录里另存那条消息的时刻：撤回（rewind）之后同一个序号换了人，旧记录作废、删掉。
//! - 子代理（含 workflow 派的）改的算进派它那一页当前这一轮。
//! - 落盘在会话目录 `turn_changes/<ordinal>.json`（原文和改后全文都在里面，撤销要用）；
//!   会话还没落盘就只在内存里。单个文件超过 [`MAX_FILE_BYTES`] 或不是 UTF-8 只记「改过」，
//!   不存原文，也撤销不了。
//! - 撤销：文件现在还是这一轮改完的样子才恢复；之后又被改过的（用户、后面的轮次）不动，
//!   报冲突。
//!
//! 有变化发 [`crate::TURN_CHANGES_CHANGED`]（[`TurnChangesChanged`]），网关推成
//! `turn/changes/updated`。

pub mod diff;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use cordis::{plugin, Context, Inject, Plugin};
use cordis_base::types::{is_main_identity, LogEvent, PreExecute, ToolResult};
use serde::{Deserialize, Serialize};

use crate::host::tabs::Tabs;
use crate::names::{
    SESSIONS, SUBAGENTS, TABS, TOOLS_EXECUTE, TOOLS_PRE_EXECUTE, TURN_CHANGES, TURN_CHANGES_CHANGED,
};
use crate::session::log::Sessions;
use crate::tools::task::Subagents;

pub use diff::{diff_stat, unified_diff, DiffStat};

/// 会改文件、要记的工具。
const EDIT_TOOLS: &[&str] = &["search_replace", "write_file"];
/// 单个文件超过这么大不存原文（只记「改过」）。
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// 一轮最多记这么多个文件。
const MAX_FILES: usize = 200;
const RECORD_VERSION: u32 = 1;
const DIR: &str = "turn_changes";

/// 文件某一刻的样子。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Snapshot {
    /// 文件不存在。
    Missing,
    Text {
        text: String,
    },
    /// 太大 / 不是文本：只知道它在，不存内容。`len` / `modified_ms` 用来判断它变没变。
    Skipped {
        reason: String,
        len: u64,
        modified_ms: u64,
    },
}

impl Snapshot {
    pub fn read(path: &Path) -> Self {
        let Ok(meta) = std::fs::metadata(path) else {
            return Self::Missing;
        };
        let skipped = |reason: &str| Self::Skipped {
            reason: reason.into(),
            len: meta.len(),
            modified_ms: meta.modified().map(unix_ms).unwrap_or(0),
        };
        if meta.is_dir() {
            return skipped("是目录");
        }
        if meta.len() > MAX_FILE_BYTES {
            return skipped("文件太大");
        }
        match std::fs::read(path).map(String::from_utf8) {
            Ok(Ok(text)) => Self::Text { text },
            Ok(Err(_)) => skipped("不是文本文件"),
            Err(e) => skipped(&format!("读不了：{e}")),
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }

    fn skipped(&self) -> Option<&str> {
        match self {
            Self::Skipped { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileRecord {
    path: PathBuf,
    before: Snapshot,
    after: Snapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TurnRecord {
    version: u32,
    ordinal: usize,
    /// 开这一轮的那条用户消息的时刻（微秒）。
    user_us: u64,
    files: Vec<FileRecord>,
    #[serde(default)]
    reverted: bool,
}

/// 一轮里改过的一个文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// 显示用：在这一页工作目录里的是相对路径。
    pub path: String,
    pub abs: PathBuf,
    /// `added` / `modified` / `deleted`。
    pub status: &'static str,
    pub added: usize,
    pub removed: usize,
    /// 没存原文的原因（太大、不是文本）：行数不算、撤销不了。
    pub skipped: Option<String>,
}

/// 一轮的改动。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnSummary {
    pub ordinal: usize,
    pub files: Vec<FileChange>,
    pub added: usize,
    pub removed: usize,
    pub reverted: bool,
}

/// 撤销的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RevertOutcome {
    /// 恢复了的文件（显示路径）。
    pub restored: Vec<String>,
    /// 没动的文件和原因。
    pub conflicts: Vec<(String, String)>,
}

/// [`crate::TURN_CHANGES_CHANGED`] 的载荷：`page` 那一页第 `ordinal` 轮的改动变了。
#[derive(Clone, Debug)]
pub struct TurnChangesChanged {
    pub page: Arc<str>,
    pub ordinal: usize,
}

#[derive(Default)]
struct Store {
    loaded: bool,
    turns: BTreeMap<usize, TurnRecord>,
}

/// `before_edit` 记下、等 `after_edit` 认领的一次编辑：归到哪一轮、改哪个文件。
struct Pending {
    ordinal: usize,
    user_us: u64,
    path: PathBuf,
}

/// 各会话的每轮改动。挂在根上一份，按会话（会话目录 / 页身份）分开。
pub struct TurnChanges {
    ctx: Context,
    stores: Mutex<HashMap<String, Store>>,
    /// 跑到一半的编辑，按（调用方身份，工具名）排队。`after_edit` 用 pre 时定下的轮次和文件，
    /// 不再重算：pre 和 post 之间用户又发了一条消息，改动也还归 pre 那一轮。
    pending: Mutex<HashMap<(String, String), VecDeque<Pending>>>,
}

impl TurnChanges {
    pub fn new(ctx: Context) -> Self {
        Self {
            ctx,
            stores: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// 一颗编辑工具要跑了（`exec` 是调用方，可能是子代理）：它要改的文件这一轮还没记过就存原文。
    /// `tool` 是这颗编辑工具的名字，跑完由 [`Self::after_edit`] 按同一个名字认领。
    pub fn before_edit(&self, exec: &Context, tool: &str, arguments: &str) {
        let Some(path) = edit_target(arguments) else {
            return;
        };
        let Some(page) = self.page_of(exec) else {
            return;
        };
        let users = user_times(&page);
        let Some((&user_us, ordinal)) = users.last().map(|t| (t, users.len() - 1)) else {
            return;
        };
        self.pending
            .lock()
            .unwrap()
            .entry(pending_key(exec, tool))
            .or_default()
            .push_back(Pending {
                ordinal,
                user_us,
                path: path.clone(),
            });
        self.with_store(&page, &users, |store, dir| {
            let record = store.turns.entry(ordinal).or_insert_with(|| TurnRecord {
                version: RECORD_VERSION,
                ordinal,
                user_us,
                files: Vec::new(),
                reverted: false,
            });
            if record.files.iter().any(|f| f.path == path) || record.files.len() >= MAX_FILES {
                return;
            }
            let snap = Snapshot::read(&path);
            record.files.push(FileRecord {
                path,
                before: snap.clone(),
                after: snap,
            });
            persist(dir, record);
        });
    }

    /// 一颗编辑工具跑完了：只重新看它 pre 时登记的那一个文件，记进 pre 定下的那一轮。
    pub fn after_edit(&self, exec: &Context, tool: &str) {
        let pending = {
            let mut all = self.pending.lock().unwrap();
            let key = pending_key(exec, tool);
            let next = all.get_mut(&key).and_then(VecDeque::pop_front);
            if all.get(&key).is_some_and(VecDeque::is_empty) {
                all.remove(&key);
            }
            next
        };
        let Some(Pending {
            ordinal,
            user_us,
            path,
        }) = pending
        else {
            return;
        };
        let Some(page) = self.page_of(exec) else {
            return;
        };
        let users = user_times(&page);
        let changed = self.with_store(&page, &users, |store, dir| {
            // 撤回过：那一轮已经作废（`with_store` 删掉了）或换了人，不记。
            let Some(record) = store
                .turns
                .get_mut(&ordinal)
                .filter(|r| r.user_us == user_us)
            else {
                return false;
            };
            let Some(file) = record.files.iter_mut().find(|f| f.path == path) else {
                return false;
            };
            let now = Snapshot::read(&file.path);
            if now == file.after {
                return false;
            }
            file.after = now;
            record.reverted = false;
            persist(dir, record);
            true
        });
        if changed {
            self.notify(&page, ordinal);
        }
    }

    /// `page` 这一页每轮的改动（有改动的轮次，先发生的在前）。
    pub fn list(&self, page: &Context) -> Vec<TurnSummary> {
        let users = user_times(page);
        let cwd = crate::session::cwd::session_cwd(page);
        self.with_store(page, &users, |store, _| {
            store
                .turns
                .values()
                .filter_map(|r| summarize(r, &cwd))
                .collect()
        })
    }

    /// 第 `ordinal` 轮的改动；没有改动是 `None`。
    pub fn summary(&self, page: &Context, ordinal: usize) -> Option<TurnSummary> {
        let users = user_times(page);
        let cwd = crate::session::cwd::session_cwd(page);
        self.with_store(page, &users, |store, _| {
            store.turns.get(&ordinal).and_then(|r| summarize(r, &cwd))
        })
    }

    /// 第 `ordinal` 轮的统一 diff（文件按记下的先后）。没存原文的文件只有文件头和一句说明。
    pub fn diff(&self, page: &Context, ordinal: usize) -> Option<String> {
        let users = user_times(page);
        let cwd = crate::session::cwd::session_cwd(page);
        self.with_store(page, &users, |store, _| {
            let record = store.turns.get(&ordinal)?;
            let mut out = String::new();
            for file in &record.files {
                if file.before == file.after {
                    continue;
                }
                let label = display_path(&file.path, &cwd);
                match (file.before.skipped(), file.after.skipped()) {
                    (None, None) => {
                        out.push_str(&unified_diff(&label, file.before.text(), file.after.text()).0)
                    }
                    _ => out.push_str(&format!(
                        "diff --git a/{label} b/{label}\nBinary files a/{label} and b/{label} differ\n"
                    )),
                }
            }
            Some(out)
        })
    }

    /// 撤销第 `ordinal` 轮：文件现在还是这一轮改完的样子才恢复成原文。
    pub fn revert(&self, page: &Context, ordinal: usize) -> Result<RevertOutcome, String> {
        let users = user_times(page);
        let cwd = crate::session::cwd::session_cwd(page);
        let outcome = self.with_store(page, &users, |store, dir| {
            let record = store
                .turns
                .get_mut(&ordinal)
                .ok_or_else(|| "这一轮没有记下的文件改动".to_string())?;
            if record.reverted {
                return Err("这一轮已经撤销过了".to_string());
            }
            let mut outcome = RevertOutcome::default();
            for file in &record.files {
                if file.before == file.after {
                    continue;
                }
                let label = display_path(&file.path, &cwd);
                if let Some(reason) = file.before.skipped() {
                    outcome
                        .conflicts
                        .push((label, format!("没存原文（{reason}）")));
                    continue;
                }
                if Snapshot::read(&file.path) != file.after {
                    outcome
                        .conflicts
                        .push((label, "之后又被改过，没动它".into()));
                    continue;
                }
                let done = match &file.before {
                    Snapshot::Missing => std::fs::remove_file(&file.path),
                    Snapshot::Text { text } => std::fs::write(&file.path, text),
                    Snapshot::Skipped { .. } => unreachable!("上面已经跳过"),
                };
                match done {
                    Ok(()) => outcome.restored.push(label),
                    Err(e) => outcome.conflicts.push((label, format!("写不回去：{e}"))),
                }
            }
            if !outcome.restored.is_empty() {
                record.reverted = true;
                persist(dir, record);
            }
            Ok(outcome)
        })?;
        if !outcome.restored.is_empty() {
            self.notify(page, ordinal);
        }
        Ok(outcome)
    }

    fn notify(&self, page: &Context, ordinal: usize) {
        let Some(sessions) = page.get::<Sessions>(SESSIONS) else {
            return;
        };
        self.ctx.emit(
            TURN_CHANGES_CHANGED,
            TurnChangesChanged {
                page: sessions.identity().into(),
                ordinal,
            },
        );
    }

    /// 调用方所在的那一页：主会话 / 分页就是它自己；子代理顺着派它的那一级往上找。
    fn page_of(&self, exec: &Context) -> Option<Context> {
        let sessions = exec.get::<Sessions>(SESSIONS)?;
        if sessions.is_main() {
            return Some(exec.clone());
        }
        let subagents = self.ctx.get::<Subagents>(SUBAGENTS)?;
        let mut id = sessions.identity().to_string();
        for _ in 0..8 {
            let parent = subagents.snapshot(&id)?.parent;
            if is_main_identity(&parent) {
                let mut pages = self
                    .ctx
                    .get::<Tabs>(TABS)
                    .map(|t| t.contexts())
                    .unwrap_or_default();
                pages.push(self.ctx.clone());
                return pages.into_iter().find(|p| {
                    p.get::<Sessions>(SESSIONS)
                        .is_some_and(|s| s.identity() == parent)
                });
            }
            id = parent;
        }
        None
    }

    /// 拿这一页当前会话的记录（第一次用时读盘），先清掉撤回作废的。
    fn with_store<R>(
        &self,
        page: &Context,
        users: &[u64],
        f: impl FnOnce(&mut Store, Option<&Path>) -> R,
    ) -> R {
        let sessions = page.get::<Sessions>(SESSIONS);
        let dir = sessions
            .as_ref()
            .and_then(|s| s.existing_session_dir())
            .map(|d| d.join(DIR));
        let key = match (&dir, &sessions) {
            (Some(d), _) => d.display().to_string(),
            (None, Some(s)) => format!("mem:{}", s.identity()),
            (None, None) => "mem:".into(),
        };
        let mut stores = self.stores.lock().unwrap();
        let store = stores.entry(key).or_default();
        if !store.loaded {
            store.loaded = true;
            if let Some(dir) = &dir {
                store.turns = load(dir);
            }
        }
        let stale: Vec<usize> = store
            .turns
            .values()
            .filter(|r| users.get(r.ordinal) != Some(&r.user_us))
            .map(|r| r.ordinal)
            .collect();
        for ordinal in stale {
            store.turns.remove(&ordinal);
            if let Some(dir) = &dir {
                let _ = std::fs::remove_file(dir.join(format!("{ordinal}.json")));
            }
        }
        f(store, dir.as_deref())
    }
}

fn pending_key(exec: &Context, tool: &str) -> (String, String) {
    let identity = exec
        .get::<Sessions>(SESSIONS)
        .map(|s| s.identity().to_string())
        .unwrap_or_default();
    (identity, tool.to_string())
}

/// 编辑工具参数里的目标文件（相对路径按调用方的工作目录展开）。
fn edit_target(arguments: &str) -> Option<PathBuf> {
    let v = crate::tools::fs_common::parse_args(arguments);
    let raw = crate::tools::fs_common::str_field(&v, &["file_path", "target_file", "path"])?;
    Some(crate::tools::fs_common::resolve(&raw))
}

/// 这一页日志里每条用户消息的时刻（微秒，撤回后再发一条也分得开），按先后。
fn user_times(page: &Context) -> Vec<u64> {
    let Some(sessions) = page.get::<Sessions>(SESSIONS) else {
        return Vec::new();
    };
    sessions.with_log(|events, times| {
        events
            .iter()
            .zip(times)
            .filter(|(e, _)| matches!(e, LogEvent::User(_)))
            .map(|(_, t)| unix_us(*t))
            .collect()
    })
}

fn summarize(record: &TurnRecord, cwd: &Path) -> Option<TurnSummary> {
    let files: Vec<FileChange> = record
        .files
        .iter()
        .filter(|f| f.before != f.after)
        .map(|f| {
            let status = match (&f.before, &f.after) {
                (Snapshot::Missing, _) => "added",
                (_, Snapshot::Missing) => "deleted",
                _ => "modified",
            };
            let skipped = f.before.skipped().or(f.after.skipped()).map(str::to_string);
            let stat = if skipped.is_some() {
                DiffStat::default()
            } else {
                diff_stat(f.before.text(), f.after.text())
            };
            FileChange {
                path: display_path(&f.path, cwd),
                abs: f.path.clone(),
                status,
                added: stat.added,
                removed: stat.removed,
                skipped,
            }
        })
        .collect();
    if files.is_empty() {
        return None;
    }
    Some(TurnSummary {
        ordinal: record.ordinal,
        added: files.iter().map(|f| f.added).sum(),
        removed: files.iter().map(|f| f.removed).sum(),
        files,
        reverted: record.reverted,
    })
}

/// 在工作目录里的用相对路径。两边可能一个走了软链接一个没走（macOS 的 `/var` →
/// `/private/var`），对不上时都规范化了再比（文件可能已经删了：规范化它的目录）。
fn display_path(path: &Path, cwd: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(cwd) {
        return rel.display().to_string();
    }
    let canonical = |p: &Path| {
        p.canonicalize().ok().or_else(|| {
            let parent = p.parent()?.canonicalize().ok()?;
            Some(parent.join(p.file_name()?))
        })
    };
    match (canonical(path), cwd.canonicalize().ok()) {
        (Some(p), Some(c)) => match p.strip_prefix(&c) {
            Ok(rel) => rel.display().to_string(),
            Err(_) => path.display().to_string(),
        },
        _ => path.display().to_string(),
    }
}

fn load(dir: &Path) -> BTreeMap<usize, TurnRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    entries
        .flatten()
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<TurnRecord>(&bytes).ok())
        .filter(|r| r.version == RECORD_VERSION)
        .map(|r| (r.ordinal, r))
        .collect()
}

/// 写 `<dir>/<ordinal>.json`（先写临时文件再换名，写一半不会留下坏文件）。
fn persist(dir: Option<&Path>, record: &TurnRecord) {
    let Some(dir) = dir else {
        return;
    };
    let Ok(bytes) = serde_json::to_vec(record) else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = dir.join(format!("{}.json", record.ordinal));
    let tmp = dir.join(format!(".{}.json.tmp", record.ordinal));
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn unix_us(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_micros() as u64
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// 记每轮改动：挂 [`TURN_CHANGES`]，在 `tools/pre-execute` 存原文、`tools/execute` 记改后。
pub fn turn_changes() -> Plugin {
    plugin("turn-changes", Inject::new(), |ctx, _: &()| {
        let service = ctx.provide(TURN_CHANGES, TurnChanges::new(ctx.clone()))?;
        let pre = {
            let ctx = ctx.clone();
            ctx.clone()
                .on_waterfall(TOOLS_PRE_EXECUTE, move |pre: PreExecute, args| {
                    let next = args.next::<PreExecute>().unwrap_or(pre);
                    if next.denial().is_none() && EDIT_TOOLS.contains(&next.name()) {
                        if let (Some(exec), Some(changes)) = (
                            crate::tools::registry::exec_ctx(),
                            ctx.get::<TurnChanges>(TURN_CHANGES),
                        ) {
                            changes.before_edit(&exec, next.name(), next.arguments());
                        }
                    }
                    next
                })?
        };
        let post = {
            let ctx = ctx.clone();
            ctx.clone()
                .on_waterfall(TOOLS_EXECUTE, move |result: ToolResult, args| {
                    let next = args.next::<ToolResult>().unwrap_or(result);
                    if EDIT_TOOLS.contains(&next.name.as_str()) {
                        if let (Some(exec), Some(changes)) = (
                            crate::tools::registry::exec_ctx(),
                            ctx.get::<TurnChanges>(TURN_CHANGES),
                        ) {
                            changes.after_edit(&exec, &next.name);
                        }
                    }
                    next
                })?
        };
        let all = ctx.effect("turn-changes", |scope| {
            scope.own(service);
            scope.own(pre);
            scope.own(post);
            Ok(())
        })?;
        Ok(Some(all))
    })
}
