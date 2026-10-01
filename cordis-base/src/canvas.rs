//! 画布：agent 写的自包含 HTML，GUI 放进沙箱 iframe 跑。纯文件引擎，不含插件。
//!
//! 落盘在会话目录下：
//!
//! ```text
//! <session dir>/canvas/<id>/
//!   meta.json    标题、版本列表（每版一句说明）
//!   v1.html …    每版一份完整 HTML，旧版不删，回滚就是拷一份成新版
//!   data.json    数据单独一份，不分版本；改数据不出新版
//! ```
//!
//! `id` 是 `canvas-<n>`，按会话递增。读写都先校验 id，挡住 `..` 之类的路径。
//! 同一进程里的读改写用一把全局锁串起来（模型工具和网关的 `canvas/setData` 会撞）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DIR: &str = "canvas";
/// 单版 HTML 上限。画布是一页，不是一个站点。
pub const HTML_LIMIT: usize = 2 * 1024 * 1024;
/// `data.json` 上限。
pub const DATA_LIMIT: usize = 4 * 1024 * 1024;
/// 标题最长字符数，超出截断。
const TITLE_LIMIT: usize = 80;
const ID_PREFIX: &str = "canvas-";

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub n: u32,
    /// 这一版改了什么，一句话。
    pub note: String,
    pub created_ms: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub id: String,
    pub title: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    /// 数据最后一次改动；没有数据为 0。
    #[serde(default)]
    pub data_updated_ms: u64,
    pub versions: Vec<Version>,
}

impl Meta {
    pub fn latest(&self) -> u32 {
        self.versions.last().map(|v| v.n).unwrap_or(0)
    }
}

/// 读出来的一版。
#[derive(Debug, Clone)]
pub struct Canvas {
    pub meta: Meta,
    pub version: u32,
    pub html: String,
    /// `data.json`；没有就是 `Value::Null`。
    pub data: Value,
}

/// `edit` 的两种改法。
pub enum Edit<'a> {
    /// 局部替换：`old` 必须在当前版里恰好出现一次。
    Replace { old: &'a str, new: &'a str },
    /// 整页重写。
    Rewrite { html: &'a str },
}

pub fn root(session_dir: &Path) -> PathBuf {
    session_dir.join(DIR)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `canvas-<n>`，其余一律拒绝（防路径穿越）。
pub fn valid_id(id: &str) -> bool {
    id.strip_prefix(ID_PREFIX)
        .is_some_and(|n| !n.is_empty() && n.len() <= 9 && n.bytes().all(|b| b.is_ascii_digit()))
}

fn canvas_dir(session_dir: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err(format!("画布 id 不对：{id}（应形如 canvas-1）"));
    }
    let dir = root(session_dir).join(id);
    if !dir.join("meta.json").is_file() {
        return Err(format!("没有这个画布：{id}"));
    }
    Ok(dir)
}

fn read_meta(dir: &Path) -> Result<Meta, String> {
    let raw =
        std::fs::read_to_string(dir.join("meta.json")).map_err(|e| format!("读画布失败：{e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("画布 meta.json 坏了：{e}"))
}

/// 先写临时文件再改名，读的一方不会看到半个文件。
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("写画布失败：{e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("写画布失败：{e}"))
}

fn write_meta(dir: &Path, meta: &Meta) -> Result<(), String> {
    let raw = serde_json::to_vec_pretty(meta).map_err(|e| e.to_string())?;
    write_atomic(&dir.join("meta.json"), &raw)
}

fn check_html(html: &str) -> Result<(), String> {
    if html.trim().is_empty() {
        return Err("HTML 不能为空".into());
    }
    if html.len() > HTML_LIMIT {
        return Err(format!(
            "HTML 太大：{} 字节，上限 {HTML_LIMIT}。把数据放进 data，库用 dock:lib/ 或 CDN 引用",
            html.len()
        ));
    }
    Ok(())
}

fn write_data(dir: &Path, data: &Value) -> Result<(), String> {
    let raw = serde_json::to_vec(data).map_err(|e| e.to_string())?;
    if raw.len() > DATA_LIMIT {
        return Err(format!("数据太大：{} 字节，上限 {DATA_LIMIT}", raw.len()));
    }
    write_atomic(&dir.join("data.json"), &raw)
}

fn clean_title(title: &str) -> String {
    let title = title.trim();
    let title = if title.is_empty() {
        "未命名画布"
    } else {
        title
    };
    title.chars().take(TITLE_LIMIT).collect()
}

fn push_version(dir: &Path, meta: &mut Meta, html: &str, note: &str) -> Result<(), String> {
    let n = meta.latest() + 1;
    write_atomic(&dir.join(format!("v{n}.html")), html.as_bytes())?;
    let now = now_ms();
    meta.versions.push(Version {
        n,
        note: note.trim().to_string(),
        created_ms: now,
        bytes: html.len() as u64,
    });
    meta.updated_ms = now;
    write_meta(dir, meta)
}

/// 本会话所有画布，最近改过的在前。坏掉的目录跳过。
pub fn list(session_dir: &Path) -> Vec<Meta> {
    let Ok(entries) = std::fs::read_dir(root(session_dir)) else {
        return Vec::new();
    };
    let mut out: Vec<Meta> = entries
        .flatten()
        .filter(|e| valid_id(&e.file_name().to_string_lossy()))
        .filter_map(|e| read_meta(&e.path()).ok())
        .collect();
    out.sort_by(|a, b| {
        b.updated_ms
            .max(b.data_updated_ms)
            .cmp(&a.updated_ms.max(a.data_updated_ms))
    });
    out
}

/// 新建画布，得到 v1。
pub fn create(
    session_dir: &Path,
    title: &str,
    html: &str,
    data: Option<&Value>,
) -> Result<Meta, String> {
    check_html(html)?;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let base = root(session_dir);
    std::fs::create_dir_all(&base).map_err(|e| format!("建画布目录失败：{e}"))?;
    let next = std::fs::read_dir(&base)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .strip_prefix(ID_PREFIX)
                        .and_then(|n| n.parse::<u32>().ok())
                })
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
        + 1;
    let id = format!("{ID_PREFIX}{next}");
    let dir = base.join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("建画布目录失败：{e}"))?;
    let now = now_ms();
    let mut meta = Meta {
        id,
        title: clean_title(title),
        created_ms: now,
        updated_ms: now,
        data_updated_ms: 0,
        versions: Vec::new(),
    };
    if let Some(data) = data.filter(|d| !d.is_null()) {
        write_data(&dir, data)?;
        meta.data_updated_ms = now;
    }
    push_version(&dir, &mut meta, html, "初版")?;
    Ok(meta)
}

/// 改 HTML，出一个新版。`title` 给了就顺带改标题。
pub fn edit(
    session_dir: &Path,
    id: &str,
    edit: Edit<'_>,
    note: &str,
    title: Option<&str>,
) -> Result<Meta, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = canvas_dir(session_dir, id)?;
    let mut meta = read_meta(&dir)?;
    let html = match edit {
        Edit::Rewrite { html } => html.to_string(),
        Edit::Replace { old, new } => {
            if old.is_empty() {
                return Err("old_string 不能为空".into());
            }
            let current = read_version(&dir, meta.latest())?;
            match current.matches(old).count() {
                0 => return Err(format!("v{} 里没找到 old_string", meta.latest())),
                1 => current.replacen(old, new, 1),
                n => {
                    return Err(format!(
                        "old_string 在 v{} 里出现了 {n} 次，要恰好一次；带上更多上下文",
                        meta.latest()
                    ))
                }
            }
        }
    };
    check_html(&html)?;
    if let Some(title) = title {
        meta.title = clean_title(title);
    }
    push_version(&dir, &mut meta, &html, note)?;
    Ok(meta)
}

/// 换掉数据，不出新版。
pub fn set_data(session_dir: &Path, id: &str, data: &Value) -> Result<Meta, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = canvas_dir(session_dir, id)?;
    let mut meta = read_meta(&dir)?;
    write_data(&dir, data)?;
    meta.data_updated_ms = now_ms();
    write_meta(&dir, &meta)?;
    Ok(meta)
}

/// 回滚：把 `to` 那一版拷成新的最新版，历史不丢。
pub fn rollback(session_dir: &Path, id: &str, to: u32) -> Result<Meta, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = canvas_dir(session_dir, id)?;
    let mut meta = read_meta(&dir)?;
    if to == meta.latest() {
        return Err(format!("v{to} 已经是最新版"));
    }
    let html = read_version(&dir, to)?;
    push_version(&dir, &mut meta, &html, &format!("回滚到 v{to}"))?;
    Ok(meta)
}

fn read_version(dir: &Path, n: u32) -> Result<String, String> {
    std::fs::read_to_string(dir.join(format!("v{n}.html"))).map_err(|_| format!("没有 v{n} 这一版"))
}

/// 读一版（默认最新）连同数据。
pub fn read(session_dir: &Path, id: &str, version: Option<u32>) -> Result<Canvas, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = canvas_dir(session_dir, id)?;
    let meta = read_meta(&dir)?;
    let version = version.unwrap_or_else(|| meta.latest());
    let html = read_version(&dir, version)?;
    let data = std::fs::read(dir.join("data.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or(Value::Null);
    Ok(Canvas {
        meta,
        version,
        html,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn create_edit_rollback_keep_every_version() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path();
        let a = create(s, "销售", "<h1>A</h1>", Some(&json!({"n": 1}))).unwrap();
        assert_eq!(a.id, "canvas-1");
        assert_eq!(a.latest(), 1);
        let b = create(s, "", "<p>b</p>", None).unwrap();
        assert_eq!(b.id, "canvas-2");
        assert_eq!(b.title, "未命名画布");

        let a = edit(
            s,
            "canvas-1",
            Edit::Replace { old: "A", new: "B" },
            "改标题",
            None,
        )
        .unwrap();
        assert_eq!(a.latest(), 2);
        assert_eq!(read(s, "canvas-1", None).unwrap().html, "<h1>B</h1>");
        assert_eq!(read(s, "canvas-1", Some(1)).unwrap().html, "<h1>A</h1>");

        let a = rollback(s, "canvas-1", 1).unwrap();
        assert_eq!(a.latest(), 3);
        assert_eq!(a.versions[2].note, "回滚到 v1");
        let c = read(s, "canvas-1", None).unwrap();
        assert_eq!(c.html, "<h1>A</h1>");
        assert_eq!(c.data, json!({"n": 1}));
        assert!(rollback(s, "canvas-1", 3).is_err(), "已是最新版");
    }

    #[test]
    fn replace_needs_exactly_one_match() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path();
        create(s, "t", "<i>x</i><i>x</i>", None).unwrap();
        let many = edit(
            s,
            "canvas-1",
            Edit::Replace { old: "x", new: "y" },
            "",
            None,
        );
        assert!(many.unwrap_err().contains("2 次"));
        let none = edit(
            s,
            "canvas-1",
            Edit::Replace { old: "z", new: "y" },
            "",
            None,
        );
        assert!(none.unwrap_err().contains("没找到"));
        assert_eq!(list(s)[0].latest(), 1, "失败的编辑不出新版");
    }

    #[test]
    fn data_changes_without_a_new_version() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path();
        create(s, "t", "<p></p>", None).unwrap();
        assert_eq!(read(s, "canvas-1", None).unwrap().data, Value::Null);
        let meta = set_data(s, "canvas-1", &json!([1, 2])).unwrap();
        assert_eq!(meta.latest(), 1);
        assert!(meta.data_updated_ms > 0);
        assert_eq!(read(s, "canvas-1", None).unwrap().data, json!([1, 2]));
    }

    #[test]
    fn ids_cannot_escape_the_canvas_dir() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path();
        create(s, "t", "<p></p>", None).unwrap();
        for bad in [
            "../canvas-1",
            "canvas-1/..",
            "canvas-",
            "canvas-x",
            "x-1",
            "",
        ] {
            assert!(read(s, bad, None).is_err(), "{bad} 应被拒绝");
        }
        assert!(read(s, "canvas-9", None)
            .unwrap_err()
            .contains("没有这个画布"));
    }

    #[test]
    fn list_skips_garbage_and_sorts_by_recent_change() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path();
        create(s, "old", "<p></p>", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        create(s, "new", "<p></p>", None).unwrap();
        std::fs::create_dir_all(root(s).join("canvas-7")).unwrap();
        std::fs::create_dir_all(root(s).join("notes")).unwrap();
        let titles: Vec<_> = list(s).into_iter().map(|m| m.title).collect();
        assert_eq!(titles, ["new", "old"]);
        std::thread::sleep(std::time::Duration::from_millis(5));
        set_data(s, "canvas-1", &json!({})).unwrap();
        assert_eq!(list(s)[0].title, "old");
    }
}
