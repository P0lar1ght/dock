//! Grok-shaped session folders under `$DOCK_HOME/sessions/<cwd-key>/<id>/`.
//!
//! Each session is `meta.json` + `chat_history.jsonl` (+ optional `compact.json`
//! so resume keeps the sampler prefix while the pager transcript stays full).
//! Fail-open: IO errors leave the in-memory log alone. Usage ledgers stay out
//! of these files (Grok: a new process resume starts a fresh `/usage` book).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::session::log::ArchivedSession;
use cordis_base::config::dock_home;
use cordis_base::types::{LlmOutput, LogEvent, ToolCall, TurnEndStatus};

const HISTORY: &str = "chat_history.jsonl";
const META: &str = "meta.json";
const COMPACT: &str = "compact.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MetaFile {
    id: String,
    title: String,
    cwd: String,
    #[serde(default)]
    updated_unix: u64,
    /// Agent preset active when this session was saved. Absent on old sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preset_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CompactFile {
    from: usize,
    prefix: Vec<WireEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HistoryLine {
    ts: u64,
    #[serde(flatten)]
    event: WireEvent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum WireEvent {
    User {
        text: String,
    },
    PreStep,
    Prompt {
        text: String,
    },
    #[serde(rename = "system-reminder")]
    SystemReminder {
        text: String,
    },
    /// 只给用户看的卡片（`LogEvent::Notice`）。`kind` 是自由字符串：认不得的值
    /// 读成 `NoticeKind::Other`，不会让整条会话读不出来。
    Notice {
        /// 不能叫 `kind`：那是整个枚举的内部 tag 名。
        #[serde(default)]
        notice_kind: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        body: String,
    },
    Llm {
        #[serde(default)]
        text: String,
        #[serde(default)]
        reasoning: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<WireToolCall>,
        /// Responses API 的原始 reasoning item，回放推理链要用。旧文件没有这个
        /// 字段，`default` 让它们照常读出来（只是不再有推理链可回放）。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reasoning_items: Vec<serde_json::Value>,
        /// 本轮采样的失败详情。旧文件没有这个字段，`default` 让它们照常读出来。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Tool {
        id: String,
        name: String,
        #[serde(default)]
        arguments: String,
        #[serde(default)]
        content: String,
        /// Filesystem paths for tool images (never base64 in JSONL).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_paths: Vec<String>,
        /// 新记录总写（true / false）；缺失 = 旧会话，读回时按
        /// `tool_output_looks_failed` 推断。只在 true 时写会让工具明确的 false
        /// 在读回时被兜底规则改掉。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    /// 一轮结束。`status` 是 completed / cancelled / failed，认不得的值读成
    /// completed；`error` 只在 failed 时写。更早的会话没有这一行，旧版本读到
    /// 它会整行跳过。
    TurnEnd {
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WireToolCall {
    id: String,
    name: String,
    #[serde(default)]
    arguments: String,
}

pub fn new_id() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

/// Grok `encode_cwd_dirname`: one path segment, reversible enough via `meta.cwd`.
pub fn encode_cwd_dirname(cwd: &Path) -> String {
    let raw = cwd.to_string_lossy();
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '/' | '\\' | ':' => out.push('-'),
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' => out.push(c),
            _ => out.push('-'),
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "workspace".into()
    } else {
        out
    }
}

pub fn sessions_cwd_dir(cwd: &Path) -> PathBuf {
    dock_home().join("sessions").join(encode_cwd_dirname(cwd))
}

pub fn save(item: &ArchivedSession, cwd: &Path) -> std::io::Result<()> {
    if item.id.trim().is_empty() || item.events.is_empty() {
        return Ok(());
    }
    let dir = sessions_cwd_dir(cwd).join(&item.id);
    fs::create_dir_all(&dir)?;
    let updated = item.times.last().copied().unwrap_or_else(SystemTime::now);
    let meta = MetaFile {
        id: item.id.clone(),
        title: item.title.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
        updated_unix: unix(updated),
        preset_id: item.preset_id.clone(),
    };
    atomic_write(
        &dir.join(META),
        serde_json::to_vec_pretty(&meta).unwrap_or_default(),
    )?;
    let mut body = String::new();
    let now = SystemTime::now();
    for (i, event) in item.events.iter().enumerate() {
        let ts = unix(item.times.get(i).copied().unwrap_or(now));
        if let Some(wire) = to_wire(event) {
            let line = HistoryLine { ts, event: wire };
            if let Ok(json) = serde_json::to_string(&line) {
                body.push_str(&json);
                body.push('\n');
            }
        }
    }
    atomic_write(&dir.join(HISTORY), body.into_bytes())?;
    save_compact(&dir, item);
    crate::session::search::index_session(&crate::session::search::SessionDoc::from_session(
        &item.id,
        cwd,
        &item.title,
        unix(updated),
        &item.events,
    ));
    Ok(())
}

pub fn save_title(id: &str, title: &str, cwd: &Path) -> std::io::Result<()> {
    let path = sessions_cwd_dir(cwd).join(id).join(META);
    let mut meta = match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<MetaFile>(&raw).unwrap_or(MetaFile {
            id: id.into(),
            title: String::new(),
            cwd: cwd.to_string_lossy().into_owned(),
            updated_unix: unix(SystemTime::now()),
            preset_id: None,
        }),
        Err(_) => MetaFile {
            id: id.into(),
            title: String::new(),
            cwd: cwd.to_string_lossy().into_owned(),
            updated_unix: unix(SystemTime::now()),
            preset_id: None,
        },
    };
    meta.title = title.into();
    meta.updated_unix = unix(SystemTime::now());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(&path, serde_json::to_vec_pretty(&meta).unwrap_or_default())
}

pub fn remove(id: &str, cwd: &Path) -> std::io::Result<()> {
    let dir = sessions_cwd_dir(cwd).join(id);
    if dir.is_dir() {
        fs::remove_dir_all(dir)?;
    }
    crate::session::search::evict_session(id);
    Ok(())
}

pub fn load_cwd(cwd: &Path) -> Vec<ArchivedSession> {
    let root = sessions_cwd_dir(cwd);
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut rows: Vec<(u64, ArchivedSession)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if !path.is_dir() {
                return None;
            }
            load_one(&path)
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.id.cmp(&a.1.id)));
    rows.into_iter().map(|(_, s)| s).collect()
}

/// 一条会话名册项：**不解析整份 transcript**。
///
/// [`load_cwd`] 走 [`load_one`]，那条路把 `chat_history.jsonl` 整份读出来再逐行
/// 反序列化——用来恢复一个会话没问题，用来列全机器上的几十上百个会话就不行了。
/// 名册只要抬头，所以只读 `meta.json` 加 jsonl 的**尾部一小段**。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterEntry {
    pub id: String,
    pub title: String,
    /// 会话的工作目录。**只能从 `meta.json` 读**，不能从目录名反解——
    /// [`encode_cwd_dirname`] 把 `/` `\` `:` 和所有非字母数字都压成 `-`，再折叠
    /// 连续的 `-`，是有损的。
    pub cwd: PathBuf,
    pub updated: SystemTime,
    /// 末条事件的一行摘要，读不出来就是空串。
    pub summary: String,
    /// 落盘时生效的 Agent 预设（`meta.json` 的 `preset_id`）；老会话没有。
    pub preset_id: Option<String>,
}

/// 只在 jsonl 尾部读这么多字节找最后一行。一条 `chat_history.jsonl` 可以有几
/// MB（长会话 + 工具输出），名册为了一行摘要没必要整份读进来。
const TAIL_SCAN_BYTES: u64 = 64 * 1024;

/// 扫 `$DOCK_HOME/sessions/*/*/`，按更新时间新到旧。
///
/// 跨 cwd：`load_cwd` 只看当前工作目录那一个子目录，名册要看全部。失败的条目
/// 直接跳过（fail-open，和这个模块其余部分一致）。
pub fn load_roster() -> Vec<RosterEntry> {
    let root = dock_home().join("sessions");
    let Ok(cwd_dirs) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut rows: Vec<RosterEntry> = Vec::new();
    for cwd_dir in cwd_dirs.flatten() {
        let path = cwd_dir.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(sessions) = fs::read_dir(&path) else {
            continue;
        };
        rows.extend(
            sessions
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter_map(|p| roster_entry(&p)),
        );
    }
    rows.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
    rows
}

fn roster_entry(dir: &Path) -> Option<RosterEntry> {
    let dir_id = dir.file_name()?.to_string_lossy().into_owned();
    if dir_id.starts_with('.') {
        return None;
    }
    let history = dir.join(HISTORY);
    // 没有 transcript 的目录不算一个会话——`save` 对空 `events` 直接返回，所以
    // 这种目录只会是写了一半或被手动动过的残留。
    if !history.is_file() {
        return None;
    }
    let meta: Option<MetaFile> = fs::read_to_string(dir.join(META))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let last = last_history_line(&history);
    let updated = meta
        .as_ref()
        .map(|m| m.updated_unix)
        .filter(|n| *n > 0)
        .or_else(|| last.as_ref().map(|l| l.ts))
        .map(from_unix)
        .unwrap_or(UNIX_EPOCH);
    let title = meta
        .as_ref()
        .map(|m| m.title.clone())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| dir_id.clone());
    let cwd = meta
        .as_ref()
        .map(|m| PathBuf::from(&m.cwd))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_default();
    Some(RosterEntry {
        id: meta
            .as_ref()
            .map(|m| m.id.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or(dir_id),
        title,
        cwd,
        updated,
        summary: last.map(|l| wire_summary(&l.event)).unwrap_or_default(),
        preset_id: meta.and_then(|m| m.preset_id),
    })
}

/// 最后一条能解出来、有内容的 `HistoryLine`（跳过 `turn-end`：名册要的是
/// 对话本身的末条）。只读文件尾部 [`TAIL_SCAN_BYTES`]。
fn last_history_line(path: &Path) -> Option<HistoryLine> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from = len.saturating_sub(TAIL_SCAN_BYTES);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    // 从尾部截进来时第一行多半是被切断的半行，所以从后往前找第一条解得开的。
    text.lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<HistoryLine>(l).ok())
        .find(|l| !matches!(l.event, WireEvent::TurnEnd { .. }))
}

/// 末条事件压成一行给名册用。
fn wire_summary(event: &WireEvent) -> String {
    let raw = match event {
        WireEvent::User { text } | WireEvent::Prompt { text } => text.as_str(),
        WireEvent::Notice { title, .. } => title.as_str(),
        WireEvent::SystemReminder { .. } | WireEvent::PreStep | WireEvent::TurnEnd { .. } => "",
        WireEvent::Llm {
            text, tool_calls, ..
        } => {
            if !text.trim().is_empty() {
                text.as_str()
            } else {
                return tool_calls
                    .first()
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
            }
        }
        WireEvent::Tool { name, .. } => name.as_str(),
    };
    let one_line: String = raw
        .trim()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    one_line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 按 id 读**一个**会话的完整 transcript。
///
/// [`load_cwd`] 会把同一个 cwd 下的每个会话都整份解出来——面板 peek 只要选中的
/// 那一个，用它等于为了一条读几十条。
pub fn load_session(id: &str, cwd: &Path) -> Option<ArchivedSession> {
    let dir = sessions_cwd_dir(cwd).join(id);
    if !dir.is_dir() {
        return None;
    }
    load_one(&dir).map(|(_, session)| session)
}

fn load_one(dir: &Path) -> Option<(u64, ArchivedSession)> {
    let id = dir.file_name()?.to_string_lossy().into_owned();
    if id.starts_with('.') {
        return None;
    }
    let meta_raw = fs::read_to_string(dir.join(META)).ok();
    let meta: Option<MetaFile> = meta_raw
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let history = fs::read_to_string(dir.join(HISTORY)).unwrap_or_default();
    let mut events = Vec::new();
    let mut times = Vec::new();
    for line in history.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<HistoryLine>(line) else {
            continue;
        };
        events.push(from_wire(row.event));
        times.push(from_unix(row.ts));
    }
    if events.is_empty() {
        return None;
    }
    let (compact_prefix, compact_from) = load_compact(dir, events.len());
    let title = meta
        .as_ref()
        .map(|m| m.title.clone())
        .filter(|t| !t.trim().is_empty())
        .or_else(|| first_user_title(&events))
        .unwrap_or_else(|| id.clone());
    let updated = meta
        .as_ref()
        .map(|m| m.updated_unix)
        .filter(|n| *n > 0)
        .unwrap_or_else(|| times.last().map(|t| unix(*t)).unwrap_or(0));
    Some((
        updated,
        ArchivedSession {
            id: meta
                .as_ref()
                .map(|m| m.id.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or(id),
            title,
            events,
            times,
            compact_prefix,
            compact_from,
            preset_id: meta.as_ref().and_then(|m| m.preset_id.clone()),
        },
    ))
}

fn save_compact(dir: &Path, item: &ArchivedSession) {
    let path = dir.join(COMPACT);
    match &item.compact_prefix {
        Some(prefix) if !prefix.is_empty() => {
            let file = CompactFile {
                from: item.compact_from,
                prefix: prefix.iter().filter_map(to_wire).collect(),
            };
            if let Ok(bytes) = serde_json::to_vec_pretty(&file) {
                let _ = atomic_write(&path, bytes);
            }
        }
        _ => {
            let _ = fs::remove_file(path);
        }
    }
}

fn load_compact(dir: &Path, event_len: usize) -> (Option<Vec<LogEvent>>, usize) {
    let Ok(raw) = fs::read_to_string(dir.join(COMPACT)) else {
        return (None, 0);
    };
    let Ok(file) = serde_json::from_str::<CompactFile>(&raw) else {
        return (None, 0);
    };
    if file.prefix.is_empty() {
        return (None, 0);
    }
    let prefix: Vec<LogEvent> = file.prefix.into_iter().map(from_wire).collect();
    (Some(prefix), file.from.min(event_len))
}

fn first_user_title(events: &[LogEvent]) -> Option<String> {
    events.iter().find_map(|e| match e {
        LogEvent::User(text) => {
            let t = text.trim();
            (!t.is_empty()).then(|| t.chars().take(40).collect())
        }
        _ => None,
    })
}

fn tool_image_blob_dir() -> PathBuf {
    dock_home().join("tool-images")
}

/// Write in-memory tool images to `$DOCK_HOME/tool-images/<call>-N.ext` and
/// return those paths for the JSONL wire. Skips when empty.
fn persist_tool_images(call_id: &str, images: &[cordis_base::types::UserImage]) -> Vec<String> {
    if images.is_empty() {
        return Vec::new();
    }
    let dir = tool_image_blob_dir();
    let _ = fs::create_dir_all(&dir);
    let safe: String = call_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut paths = Vec::with_capacity(images.len());
    for (i, img) in images.iter().enumerate() {
        let ext = match img.mime.as_str() {
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "png",
        };
        let path = dir.join(format!("{safe}-{i}.{ext}"));
        if fs::write(&path, img.data.as_ref()).is_ok() {
            paths.push(path.display().to_string());
        }
    }
    paths
}

/// Only reload images whose paths resolve under `$DOCK_HOME/tool-images/`.
/// Out-of-bounds / missing paths are skipped (fail-open).
fn load_tool_images(paths: &[String]) -> Vec<cordis_base::types::UserImage> {
    let root = tool_image_blob_dir();
    let root_canon = root.canonicalize().unwrap_or(root);
    paths
        .iter()
        .filter_map(|p| {
            let path = Path::new(p);
            let canon = path.canonicalize().ok()?;
            if !canon.starts_with(&root_canon) {
                return None;
            }
            crate::tools::tool_images::user_image_from_path(&canon)
        })
        .collect()
}

fn to_wire(event: &LogEvent) -> Option<WireEvent> {
    Some(match event {
        LogEvent::User(text) => WireEvent::User { text: text.clone() },
        LogEvent::PreStep => WireEvent::PreStep,
        LogEvent::Prompt(text) => WireEvent::Prompt { text: text.clone() },
        LogEvent::SystemReminder(text) => WireEvent::SystemReminder { text: text.clone() },
        LogEvent::Notice { kind, title, body } => WireEvent::Notice {
            notice_kind: kind.as_str().to_string(),
            title: title.clone(),
            body: body.clone(),
        },
        LogEvent::LlmStream(out) => WireEvent::Llm {
            text: out.text.clone(),
            reasoning: out.reasoning.clone(),
            reasoning_ms: out.reasoning_ms,
            reasoning_items: out.reasoning_items.clone(),
            error: out.error.clone(),
            tool_calls: out
                .tool_calls
                .iter()
                .map(|c| WireToolCall {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    arguments: c.arguments.clone(),
                })
                .collect(),
        },
        LogEvent::ToolExecute {
            id,
            name,
            arguments,
            content,
            images,
            is_error,
        } => WireEvent::Tool {
            id: id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
            content: content.clone(),
            image_paths: persist_tool_images(id, images),
            is_error: Some(*is_error),
        },
        LogEvent::TurnEnd(status) => WireEvent::TurnEnd {
            status: status.as_str().to_string(),
            error: match status {
                TurnEndStatus::Failed(error) => Some(error.clone()),
                _ => None,
            },
        },
    })
}

fn from_wire(event: WireEvent) -> LogEvent {
    match event {
        WireEvent::User { text } => LogEvent::User(text),
        WireEvent::PreStep => LogEvent::PreStep,
        WireEvent::Prompt { text } => LogEvent::Prompt(text),
        WireEvent::SystemReminder { text } => LogEvent::SystemReminder(text),
        WireEvent::Notice {
            notice_kind,
            title,
            body,
        } => LogEvent::Notice {
            kind: cordis_base::types::NoticeKind::from_str_lenient(&notice_kind),
            title,
            body,
        },
        WireEvent::Llm {
            text,
            reasoning,
            reasoning_ms,
            tool_calls,
            reasoning_items,
            error,
        } => LogEvent::LlmStream(LlmOutput {
            text,
            reasoning,
            reasoning_ms,
            reasoning_items,
            error,
            tool_calls: tool_calls
                .into_iter()
                .map(|c| ToolCall {
                    id: c.id,
                    name: c.name,
                    arguments: c.arguments,
                })
                .collect(),
        }),
        WireEvent::Tool {
            id,
            name,
            arguments,
            content,
            image_paths,
            is_error,
        } => LogEvent::ToolExecute {
            id,
            name,
            arguments,
            is_error: is_error
                .unwrap_or_else(|| cordis_base::types::tool_output_looks_failed(&content)),
            content,
            images: load_tool_images(&image_paths),
        },
        WireEvent::TurnEnd { status, error } => LogEvent::TurnEnd(match status.as_str() {
            "cancelled" => TurnEndStatus::Cancelled,
            "failed" => TurnEndStatus::Failed(error.unwrap_or_default()),
            _ => TurnEndStatus::Completed,
        }),
    }
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn from_unix(secs: u64) -> SystemTime {
    UNIX_EPOCH + std::time::Duration::from_secs(secs)
}

fn atomic_write(path: &Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)
}

/// 这份记录里有没有「对话」：用户消息、模型输出、工具结果、通知卡。只有后台
/// 记账（开场提醒、提示词组装、轮次结束）的会话不落盘——比如刚开页就收到
/// 「MCP 已连接」，没人说过话，写下来只会在会话列表里多一个空会话。
pub fn has_conversation(events: &[LogEvent]) -> bool {
    events.iter().any(|e| match e {
        LogEvent::User(_) | LogEvent::ToolExecute { .. } | LogEvent::Notice { .. } => true,
        LogEvent::LlmStream(out) => !out.text.is_empty() || !out.tool_calls.is_empty(),
        LogEvent::PreStep
        | LogEvent::Prompt(_)
        | LogEvent::SystemReminder(_)
        | LogEvent::TurnEnd(_) => false,
    })
}

pub fn empty_llm_placeholder(events: &[LogEvent]) -> bool {
    matches!(
        events.last(),
        Some(LogEvent::LlmStream(out))
            if out.text.is_empty() && out.reasoning.is_empty() && out.tool_calls.is_empty()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::log::ArchivedSession;
    use cordis_base::types::LogEvent;

    #[test]
    fn encode_collapses_path_separators() {
        let p = Path::new("/Users/polar/Desktop/AILab/dock");
        let enc = encode_cwd_dirname(p);
        assert!(enc.starts_with("Users-polar"), "{enc}");
        assert!(!enc.contains('/'), "{enc}");
    }

    #[test]
    fn roundtrip_preset_id_and_old_meta_without_field() {
        let _home = cordis_base::test_env::scoped().home();
        let cwd = Path::new("/tmp/dock-persist-preset-test");
        let _ = remove("p1", cwd);
        let _ = remove("old", cwd);
        let item = ArchivedSession {
            id: "p1".into(),
            title: "with preset".into(),
            events: vec![LogEvent::User("hi".into())],
            times: vec![SystemTime::now()],
            compact_prefix: None,
            compact_from: 0,
            preset_id: Some("warden".into()),
        };
        save(&item, cwd).unwrap();
        let loaded = load_cwd(cwd);
        assert_eq!(loaded[0].preset_id.as_deref(), Some("warden"));

        let dir = sessions_cwd_dir(cwd).join("old");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(META),
            br#"{"id":"old","title":"legacy","cwd":"/tmp","updated_unix":1}"#,
        )
        .unwrap();
        fs::write(
            dir.join(HISTORY),
            "{\"ts\":1,\"kind\":\"user\",\"text\":\"legacy\"}\n",
        )
        .unwrap();
        let loaded = load_cwd(cwd);
        let old = loaded.iter().find(|s| s.id == "old").expect("old session");
        assert_eq!(old.preset_id, None, "missing field must stay None");
        remove("p1", cwd).unwrap();
        remove("old", cwd).unwrap();
    }

    #[test]
    fn roundtrip_user_and_tool() {
        let _home = cordis_base::test_env::scoped().home();
        let cwd = Path::new("/tmp/dock-persist-test");
        let item = ArchivedSession {
            id: "abc123".into(),
            title: "hello persist".into(),
            events: vec![
                LogEvent::User("hello persist".into()),
                LogEvent::ToolExecute {
                    id: "c1".into(),
                    name: "bash".into(),
                    arguments: "{\"cmd\":\"pwd\"}".into(),
                    content: "/tmp".into(),

                    images: Vec::new(),
                    is_error: false,
                },
            ],
            times: vec![SystemTime::now(), SystemTime::now()],
            compact_prefix: None,
            compact_from: 0,
            preset_id: None,
        };
        save(&item, cwd).unwrap();
        let loaded = load_cwd(cwd);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "abc123");
        assert_eq!(loaded[0].title, "hello persist");
        assert_eq!(loaded[0].events, item.events);
        remove("abc123", cwd).unwrap();
        assert!(load_cwd(cwd).is_empty());
    }

    /// 采样失败详情要跟着会话落盘，`/resume` 之后还看得见「上次为什么断的」；
    /// 旧文件没有这个字段也得照常读。
    #[test]
    fn roundtrip_llm_error_and_reads_old_rows() {
        let _home = cordis_base::test_env::scoped().home();
        let cwd = Path::new("/tmp/dock-persist-llm-error-test");
        let detail = "[连接失败] error sending request ← connection reset by peer";
        let item = ArchivedSession {
            id: "e1".into(),
            title: "err".into(),
            events: vec![LogEvent::LlmStream(LlmOutput {
                error: Some(detail.into()),
                ..LlmOutput::default()
            })],
            times: vec![SystemTime::now()],
            compact_prefix: None,
            compact_from: 0,
            preset_id: None,
        };
        save(&item, cwd).unwrap();
        let loaded = load_cwd(cwd);
        assert_eq!(loaded[0].events, item.events);
        remove("e1", cwd).unwrap();

        let old: WireEvent = serde_json::from_str(r#"{"kind":"llm","text":"hi"}"#).unwrap();
        let LogEvent::LlmStream(out) = from_wire(old) else {
            panic!("expected llm row");
        };
        assert_eq!(out.error, None);
    }

    /// 工具的 `is_error` 原样落盘、读回；旧行没有这个字段时按兜底规则推断，
    /// 新行里工具明确的 false 不能被规则改掉。
    #[test]
    fn tool_error_flag_round_trips_and_old_rows_are_inferred() {
        let tool = |content: &str, is_error: bool| LogEvent::ToolExecute {
            id: "c1".into(),
            name: "bash".into(),
            arguments: "{}".into(),
            content: content.into(),
            images: Vec::new(),
            is_error,
        };
        for event in [
            tool("ok", false),
            tool("boom", true),
            tool("Error: 但工具说没事", false),
        ] {
            let back = from_wire(to_wire(&event).unwrap());
            assert_eq!(back, event);
        }
        let old = |content: &str| {
            let row =
                serde_json::json!({"kind": "tool", "id": "c1", "name": "bash", "content": content});
            match from_wire(serde_json::from_value(row).unwrap()) {
                LogEvent::ToolExecute { is_error, .. } => is_error,
                other => panic!("expected tool row, got {other:?}"),
            }
        };
        assert!(old("exit status: 1\nboom"));
        assert!(old("Error: nope"));
        assert!(!old("fine"));
    }

    /// 一轮的结果落盘、读回（停止、出错带原文）；名册摘要跳过这一行，还是对话
    /// 的末条；认不得的状态读成完成。
    #[test]
    fn turn_end_rows_round_trip_and_stay_out_of_the_roster_summary() {
        let _home = cordis_base::test_env::scoped().home();
        let cwd = Path::new("/tmp/dock-persist-turn-end-test");
        let item = ArchivedSession {
            id: "te1".into(),
            title: "turn end".into(),
            events: vec![
                LogEvent::User("hi".into()),
                LogEvent::LlmStream(LlmOutput {
                    text: "answer".into(),
                    ..LlmOutput::default()
                }),
                LogEvent::TurnEnd(TurnEndStatus::Cancelled),
                LogEvent::User("again".into()),
                LogEvent::TurnEnd(TurnEndStatus::Failed("HTTP 404".into())),
            ],
            times: vec![SystemTime::now(); 5],
            compact_prefix: None,
            compact_from: 0,
            preset_id: None,
        };
        save(&item, cwd).unwrap();
        assert_eq!(load_cwd(cwd)[0].events, item.events);
        let entry = roster_entry(&sessions_cwd_dir(cwd).join("te1")).unwrap();
        assert_eq!(entry.summary, "again");
        remove("te1", cwd).unwrap();

        let row: WireEvent =
            serde_json::from_str(r#"{"kind":"turn-end","status":"paused"}"#).unwrap();
        assert_eq!(from_wire(row), LogEvent::TurnEnd(TurnEndStatus::Completed));
    }

    /// Responses 的推理链要跨会话活下来；旧文件没有这个字段也得读得出来。
    #[test]
    fn roundtrip_reasoning_items_and_reads_old_rows() {
        let _home = cordis_base::test_env::scoped().home();
        let cwd = Path::new("/tmp/dock-persist-reasoning-test");
        let item = ArchivedSession {
            id: "rs1".into(),
            title: "reasoning".into(),
            events: vec![LogEvent::LlmStream(LlmOutput {
                text: "answer".into(),
                reasoning: "plan".into(),
                reasoning_items: vec![serde_json::json!({
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{"type": "summary_text", "text": "plan"}],
                })],
                ..LlmOutput::default()
            })],
            times: vec![SystemTime::now()],
            compact_prefix: None,
            compact_from: 0,
            preset_id: None,
        };
        save(&item, cwd).unwrap();
        let loaded = load_cwd(cwd);
        assert_eq!(loaded[0].events, item.events);
        remove("rs1", cwd).unwrap();

        // 旧格式（没有 reasoning_items）照常读，只是没推理链可回放。
        let old: WireEvent =
            serde_json::from_str(r#"{"kind":"llm","text":"hi","reasoning":"plan"}"#).unwrap();
        let LogEvent::LlmStream(out) = from_wire(old) else {
            panic!("expected llm row");
        };
        assert_eq!(out.text, "hi");
        assert!(out.reasoning_items.is_empty());
    }

    #[test]
    fn roundtrip_tool_images_bytes() {
        use crate::tools::tool_images::user_image_from_bytes;
        let _home = cordis_base::test_env::scoped().home();
        let cwd = Path::new("/tmp/dock-persist-image-test");
        let png = vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f, 0x00, 0x05, 0xfe, 0x02, 0xfe, 0xa7, 0x35, 0x81,
            0x84, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let img = user_image_from_bytes(png.clone(), Some("image/png")).expect("png");
        let item = ArchivedSession {
            id: "img123".into(),
            title: "image persist".into(),
            events: vec![
                LogEvent::User("see shot".into()),
                LogEvent::ToolExecute {
                    id: "shot1".into(),
                    name: "browser_screenshot".into(),
                    arguments: "{}".into(),
                    content: "Image content included inline".into(),
                    images: vec![img.clone()],
                    is_error: false,
                },
            ],
            times: vec![SystemTime::now(), SystemTime::now()],
            compact_prefix: None,
            compact_from: 0,
            preset_id: None,
        };
        save(&item, cwd).unwrap();
        // image_paths should land under DOCK_HOME/tool-images/
        let blob_dir = cordis_base::config::dock_home().join("tool-images");
        assert!(blob_dir.is_dir(), "{blob_dir:?}");
        let loaded = load_cwd(cwd);
        assert_eq!(loaded.len(), 1);
        match &loaded[0].events[1] {
            LogEvent::ToolExecute { images, .. } => {
                assert_eq!(images.len(), 1, "{images:?}");
                assert_eq!(images[0].mime, "image/png");
                assert_eq!(images[0].data.as_ref(), img.data.as_ref());
                assert_eq!(images[0].data.as_ref(), png.as_slice());
            }
            other => panic!("expected ToolExecute, got {other:?}"),
        }
        remove("img123", cwd).unwrap();
    }

    #[test]
    fn load_tool_images_skips_out_of_bounds_paths() {
        let _home = cordis_base::test_env::scoped().home();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), b"not an image under tool-images").unwrap();
        let loaded = load_tool_images(&[outside.path().display().to_string()]);
        assert!(
            loaded.is_empty(),
            "must skip paths outside tool-images: {loaded:?}"
        );
    }

    /// 回归：只收到一条后台提醒（如「MCP 已连接」）、没人说过话的会话也被写了盘，
    /// 在会话列表里多出一个空会话。只有真有内容（用户消息、模型输出、工具结果、
    /// 通知卡）才落盘；有了之后照常写。
    #[tokio::test]
    async fn a_session_with_only_bookkeeping_is_not_saved() {
        let _home = cordis_base::test_env::scoped().home();
        let saved = || {
            walk_files(&dock_home().join("sessions"))
                .into_iter()
                .filter(|p| p.ends_with(HISTORY))
                .count()
        };
        let sessions = crate::session::log::Sessions::new(cordis::Context::new());
        sessions.attach_disk();
        sessions.append(LogEvent::SystemReminder("MCP 服务器已连接".into()));
        sessions.append(LogEvent::PreStep);
        assert_eq!(saved(), 0, "只有后台提醒的会话不该落盘");

        sessions.append(LogEvent::User("hi".into()));
        assert_eq!(saved(), 1, "有了用户消息就该落盘");
    }

    fn walk_files(dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .flat_map(|e| {
                let p = e.path();
                if p.is_dir() {
                    walk_files(&p)
                } else {
                    vec![p]
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn attach_disk_reloads_archived_session() {
        let _home = cordis_base::test_env::scoped().home();
        let sessions = crate::session::log::Sessions::new(cordis::Context::new());
        sessions.attach_disk();
        sessions.append(LogEvent::User("disk hello".into()));
        let id = sessions
            .archive_current()
            .expect("archive writes a folder")
            .id;
        let again = crate::session::log::Sessions::new(cordis::Context::new());
        again.attach_disk();
        assert!(
            again.archived().iter().any(|s| s.id == id),
            "{:?}",
            again
                .archived()
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>()
        );
        assert!(again.restore(&id));
        assert!(matches!(
            again.events().first(),
            Some(LogEvent::User(t)) if t == "disk hello"
        ));
    }

    /// A blank tab adopts the folder and later turns land back in it.
    /// Archiving the empty log first would mint another id and drop this one.
    #[tokio::test]
    async fn adopt_archived_continues_the_same_folder() {
        let _home = cordis_base::test_env::scoped().home();
        let sessions = crate::session::log::Sessions::new(cordis::Context::new());
        sessions.attach_disk();
        sessions.append(LogEvent::User("keep me".into()));
        let id = sessions
            .archive_current()
            .expect("archive writes a folder")
            .id;

        let tab = crate::session::log::Sessions::tab(cordis::Context::new(), 2);
        assert!(tab.adopt_archived(&id), "empty tab should adopt {id}");
        assert_eq!(tab.live_session_id(), id);
        assert!(tab.on_disk());
        assert!(matches!(
            tab.events().first(),
            Some(LogEvent::User(t)) if t == "keep me"
        ));
        tab.append(LogEvent::User("and this".into()));

        let again = crate::session::log::Sessions::new(cordis::Context::new());
        again.attach_disk();
        assert!(again.restore(&id));
        let texts: Vec<String> = again
            .events()
            .iter()
            .filter_map(|e| match e {
                LogEvent::User(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["keep me", "and this"]);

        let busy = crate::session::log::Sessions::tab(cordis::Context::new(), 3);
        busy.append(LogEvent::User("already talking".into()));
        assert!(
            !busy.adopt_archived(&id),
            "a page that already has a log must not be replaced"
        );
        assert!(!busy.adopt_archived("missing"), "unknown id");
        assert_eq!(busy.live_session_id(), "");
    }

    fn reload() -> crate::session::log::Sessions {
        let sessions = crate::session::log::Sessions::new(cordis::Context::new());
        sessions.attach_disk();
        sessions
    }

    #[tokio::test]
    async fn clear_drops_live_folder_so_attach_disk_does_not_resurrect() {
        let _home = cordis_base::test_env::scoped().home();
        let sessions = reload();
        sessions.append(LogEvent::User("gone after clear".into()));
        sessions.clear();
        let again = reload();
        assert!(
            again.archived().is_empty(),
            "clear must delete the live folder, got {:?}",
            again
                .archived()
                .iter()
                .map(|s| s.title.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn rewind_persists_truncated_log() {
        let _home = cordis_base::test_env::scoped().home();
        let sessions = reload();
        sessions.append(LogEvent::User("keep".into()));
        sessions.append(LogEvent::LlmStream(cordis_base::types::LlmOutput {
            text: "reply".into(),
            ..cordis_base::types::LlmOutput::default()
        }));
        sessions.append(LogEvent::User("undo me".into()));
        sessions.begin_llm();
        sessions
            .rewind_inflight_user()
            .expect("no-output user rewinds");
        let again = reload();
        let item = again.archived().into_iter().next().expect("rewound live");
        assert!(again.restore(&item.id));
        let events = again.events();
        assert_eq!(events.len(), 2, "{events:?}");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, LogEvent::User(t) if t == "undo me")),
            "{events:?}"
        );
    }

    #[tokio::test]
    async fn seal_persists_interrupted_tool_stub() {
        let _home = cordis_base::test_env::scoped().home();
        let sessions = reload();
        sessions.append(LogEvent::User("hi".into()));
        sessions.append(LogEvent::LlmStream(cordis_base::types::LlmOutput {
            tool_calls: vec![cordis_base::types::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: "{}".into(),
            }],
            ..cordis_base::types::LlmOutput::default()
        }));
        sessions.seal_incomplete_tool_calls();
        let again = reload();
        let item = again.archived().into_iter().next().expect("sealed live");
        assert!(again.restore(&item.id));
        assert!(
            again.events().iter().any(|e| matches!(
                e,
                LogEvent::ToolExecute { content, .. }
                    if content == cordis_base::types::INTERRUPTED_TOOL_RESULT
            )),
            "{:?}",
            again.events()
        );
    }

    #[tokio::test]
    async fn compact_keeps_transcript_and_restores_model_prefix() {
        let _home = cordis_base::test_env::scoped().home();
        let sessions = reload();
        sessions.append(LogEvent::User("keep visible".into()));
        sessions.append(LogEvent::ToolExecute {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
            content: "full tool body".into(),

            images: Vec::new(),
            is_error: false,
        });
        sessions.replace_compacted(vec![
            LogEvent::User("keep visible".into()),
            LogEvent::SystemReminder("compacted earlier turns".into()),
            LogEvent::LlmStream(cordis_base::types::LlmOutput {
                text: cordis_base::types::COMPACT_NOTICE.into(),
                ..cordis_base::types::LlmOutput::default()
            }),
        ]);
        let id = sessions
            .archive_current()
            .expect("archive writes a folder")
            .id;
        let again = reload();
        assert!(again.restore(&id));
        assert!(
            again.events().iter().any(|e| matches!(
                e,
                LogEvent::ToolExecute { content, .. } if content.contains("full tool body")
            )),
            "resume must keep the pager transcript: {:?}",
            again.events()
        );
        assert!(!again.model_history().iter().any(|e| matches!(
            e,
            LogEvent::ToolExecute { content, .. } if content.contains("full tool body")
        )));
        assert!(again.model_history().iter().any(|e| matches!(
            e,
            LogEvent::SystemReminder(t) if t.contains("compacted earlier")
        )));
    }

    #[tokio::test]
    async fn notice_round_trips_and_stays_out_of_model_history() {
        let _home = cordis_base::test_env::scoped().home();
        let sessions = reload();
        sessions.append(LogEvent::User("hi".into()));
        sessions.append(LogEvent::Notice {
            kind: cordis_base::types::NoticeKind::WorkflowDone,
            title: "工作流 deep-research · 完成".into(),
            body: "report.md".into(),
        });
        let id = sessions
            .archive_current()
            .expect("archive writes a folder")
            .id;
        let again = reload();
        assert!(again.restore(&id));
        assert!(
            again.events().iter().any(|e| matches!(
                e,
                LogEvent::Notice { title, body, .. }
                    if title.contains("deep-research") && body == "report.md"
            )),
            "{:?}",
            again.events()
        );
        assert!(
            !again
                .model_history()
                .iter()
                .any(|e| matches!(e, LogEvent::Notice { .. })),
            "{:?}",
            again.model_history()
        );
    }
}
