//! 看和改工作区文件：列目录、读一个文件、按名字找文件；保存文本、新建、改名 / 移动、移到废纸篓。
//! 给网关的 `fs/*` 用（GUI 的文件面板）。
//!
//! 路径一律相对工作区根（会话的 cwd），用 `/` 分隔；空串是根本身。解析后必须还在根下面：
//! `..`、绝对路径、指到外面的符号链接都拒绝。这不是安全边界（拿到连接的人本来就能让 agent
//! 跑命令），是让面板只看会话自己的目录。
//!
//! 列目录和找文件都照 `.gitignore`（不管是不是 git 仓库），默认不列点开头的文件。

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

/// 一层目录最多列这么多项；再多就截断并标出来。
pub const LIST_LIMIT: usize = 5000;
/// 找文件最多走这么多个文件（大仓库不至于卡住）。
pub const FIND_SCAN_LIMIT: usize = 100_000;
/// 文本默认最多回这么多字节。
pub const TEXT_LIMIT: usize = 512 * 1024;
/// 图片最多回这么大。
pub const IMAGE_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Dir => "dir",
            EntryKind::File => "file",
            EntryKind::Symlink => "symlink",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// 相对工作区根。
    pub path: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub modified_ms: Option<u64>,
}

#[derive(Debug)]
pub struct Listing {
    pub path: String,
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

#[derive(Debug)]
pub enum Content {
    Text {
        text: String,
        truncated: bool,
    },
    Image {
        mime: &'static str,
        bytes: Vec<u8>,
    },
    /// 二进制或太大的图片：只给大小，不给内容。
    Binary,
}

#[derive(Debug)]
pub struct FileRead {
    pub path: String,
    pub size: u64,
    pub modified_ms: Option<u64>,
    pub mime: Option<&'static str>,
    pub content: Content,
}

/// 把相对路径解析到根下面；出了根就报错。
pub fn confine(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("工作区目录不可用：{e}"))?;
    let rel = rel.trim().trim_start_matches("./");
    let mut joined = root.clone();
    for part in Path::new(rel).components() {
        match part {
            Component::Normal(p) => joined.push(p),
            Component::CurDir => {}
            _ => return Err(format!("路径不在工作区里：{rel}")),
        }
    }
    let real = joined
        .canonicalize()
        .map_err(|_| format!("没有这个路径：{rel}"))?;
    if !real.starts_with(&root) {
        return Err(format!("路径不在工作区里：{rel}"));
    }
    Ok(joined)
}

/// 还不存在的路径（新建、改名的目标）解析到根下面：每一段都得是普通名字，最近一个已经
/// 存在的祖先解析后必须还在根里（挡住经符号链接目录写到外面）。已经存在就报错。
pub fn confine_new(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("工作区目录不可用：{e}"))?;
    let rel = rel.trim().trim_start_matches("./").trim_end_matches('/');
    if rel.is_empty() {
        return Err("名字不能为空".into());
    }
    let mut joined = root.clone();
    for part in Path::new(rel).components() {
        match part {
            Component::Normal(p) => joined.push(p),
            _ => return Err(format!("路径不在工作区里：{rel}")),
        }
    }
    if joined.symlink_metadata().is_ok() {
        return Err(format!("已经有 {rel} 了"));
    }
    let mut existing = joined.as_path();
    while existing.symlink_metadata().is_err() {
        existing = existing.parent().unwrap_or(&root);
    }
    let real = existing
        .canonicalize()
        .map_err(|e| format!("读不了 {}：{e}", existing.display()))?;
    if !real.starts_with(&root) {
        return Err(format!("路径不在工作区里：{rel}"));
    }
    if !real.is_dir() {
        return Err(format!("{} 不是文件夹", relative(&root, &real)));
    }
    Ok(joined)
}

/// 改文件时出的错：分清是参数 / 路径不对、文件在打开后被别人改过，还是读写本身失败。
#[derive(Debug, PartialEq, Eq)]
pub enum EditError {
    /// 路径出了工作区、不存在、已经存在、类型不对。
    Invalid(String),
    /// 文件在打开编辑之后被改过（多半是 agent）：带上现在的修改时间。
    Conflict { modified_ms: Option<u64> },
    /// 读写失败（权限、磁盘满、跨磁盘移动、没有回收站）。
    Io(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::Invalid(m) | EditError::Io(m) => f.write_str(m),
            EditError::Conflict { .. } => f.write_str("文件在你打开之后被改过了"),
        }
    }
}

/// 写完之后的文件：相对路径、大小、修改时间（下次保存拿它比对）。
#[derive(Debug)]
pub struct Saved {
    pub path: String,
    pub size: u64,
    pub modified_ms: Option<u64>,
}

fn saved(root: &Path, file: &Path) -> Result<Saved, EditError> {
    let root = root
        .canonicalize()
        .map_err(|e| EditError::Io(e.to_string()))?;
    let meta = std::fs::metadata(file).map_err(|e| EditError::Io(e.to_string()))?;
    let real = file
        .canonicalize()
        .map_err(|e| EditError::Io(e.to_string()))?;
    Ok(Saved {
        path: relative(&root, &real),
        size: meta.len(),
        modified_ms: modified_ms(&meta),
    })
}

/// 保存一个已有的文本文件。`base_modified_ms` 是打开时的修改时间：对不上说明期间被
/// 改过，不写，回 [`EditError::Conflict`]（不给就不比对，等于覆盖）。
///
/// 先写同目录的临时文件再改名换上：写到一半崩了不会留下半个文件。照原文件的权限；
/// 符号链接写到它指向的文件，链接本身不动。换的是文件本身：硬链接会断开、属主和扩展属性
/// 不保留；只读的文件不换（回 [`EditError::Io`]）。
///
/// 比对和换上之间有一点空档，修改时间在有的文件系统上也只精确到秒：这是防手滑覆盖
/// agent 的改动，不是锁（见模块头：这里不是安全边界）。
pub fn save_text(
    root: &Path,
    rel: &str,
    text: &str,
    base_modified_ms: Option<u64>,
) -> Result<Saved, EditError> {
    save_bytes(root, rel, text.as_bytes(), base_modified_ms)
}

/// 同 [`save_text`]，内容是任意字节（表格之类的二进制文件）。
pub fn save_bytes(
    root: &Path,
    rel: &str,
    bytes: &[u8],
    base_modified_ms: Option<u64>,
) -> Result<Saved, EditError> {
    use std::io::Write;
    save_with(root, rel, base_modified_ms, |out| out.write_all(bytes))
}

/// 同 [`save_text`]，内容是分块传上来暂存着的那份（[`upload_chunk`]）。保存成功就删掉暂存；
/// 被拦下（期间被改过）时留着，「覆盖保存」不用再传一遍。
///
/// `size` 是客户端说它一共传了多少：对不上（中间丢了块）就不写，免得把半份文件换上去。
pub fn save_upload(
    root: &Path,
    rel: &str,
    upload_id: &str,
    size: Option<u64>,
    base_modified_ms: Option<u64>,
) -> Result<Saved, EditError> {
    let staged = upload_path(upload_id)?;
    let mut src = std::fs::File::open(&staged)
        .map_err(|e| EditError::Invalid(format!("没有这份上传（{upload_id}），要重新传：{e}")))?;
    if let Some(want) = size {
        let have = src
            .metadata()
            .map_err(|e| EditError::Io(e.to_string()))?
            .len();
        if have != want {
            return Err(EditError::Invalid(format!(
                "上传不完整：收到 {have} 字节，应该是 {want}"
            )));
        }
    }
    let saved = save_with(root, rel, base_modified_ms, |out| {
        std::io::copy(&mut src, out).map(|_| ())
    })?;
    drop(src);
    let _ = std::fs::remove_file(&staged);
    Ok(saved)
}

fn save_with(
    root: &Path,
    rel: &str,
    base_modified_ms: Option<u64>,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<Saved, EditError> {
    let file = confine(root, rel).map_err(EditError::Invalid)?;
    let real = file
        .canonicalize()
        .map_err(|e| EditError::Io(e.to_string()))?;
    let meta = std::fs::metadata(&real).map_err(|e| EditError::Io(format!("读不了 {rel}：{e}")))?;
    if !meta.is_file() {
        return Err(EditError::Invalid(format!("不是文件：{rel}")));
    }
    if meta.permissions().readonly() {
        return Err(EditError::Io(format!("{rel} 是只读的")));
    }
    if let Some(base) = base_modified_ms {
        let now = modified_ms(&meta);
        if now != Some(base) {
            return Err(EditError::Conflict { modified_ms: now });
        }
    }
    let dir = real.parent().unwrap_or(Path::new("."));
    let io = |e: std::io::Error| EditError::Io(format!("写不了 {rel}：{e}"));
    let mut tmp = tempfile::Builder::new()
        .prefix(".dock-save-")
        .tempfile_in(dir)
        .map_err(io)?;
    write(tmp.as_file_mut()).map_err(io)?;
    tmp.as_file().sync_all().map_err(io)?;
    std::fs::set_permissions(tmp.path(), meta.permissions()).map_err(io)?;
    tmp.persist(&real).map_err(|e| io(e.error))?;
    saved(root, &real)
}

/// 分块上传一块最多这么大（base64 后约 5.3 MB 一帧，和读的一样）。
pub const UPLOAD_CHUNK_MAX: usize = 4 * 1024 * 1024;
/// 一份上传最多这么大（GUI 能打开改的表格远小于它）：写坏的客户端不至于把盘写满。
pub const UPLOAD_MAX: u64 = 512 * 1024 * 1024;
/// 暂存超过这么久没提交的上传，下次有新上传时清掉。
const UPLOAD_STALE: std::time::Duration = std::time::Duration::from_secs(3600);

/// 分块上传的暂存目录：`$DOCK_HOME/uploads/`。不放在工作区里：传到一半断了也不会在
/// 项目里留下垃圾文件。
pub fn uploads_dir() -> PathBuf {
    let dir = crate::config::dock_home().join("uploads");
    std::path::absolute(&dir).unwrap_or(dir)
}

fn upload_path(id: &str) -> Result<PathBuf, EditError> {
    // 只收小写：不区分大小写的文件系统上 `ABC` 和 `abc` 是同一个暂存文件。
    let ok = (8..=80).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !ok {
        return Err(EditError::Invalid(format!("上传 id 不对：{id}")));
    }
    Ok(uploads_dir().join(id))
}

/// 往上传 `id` 里追加一块。块要按顺序来：`offset` 得等于已经传了多少（0 = 从头开始，
/// 会清掉同名的旧暂存）。回现在一共多少字节。
pub fn upload_chunk(id: &str, offset: u64, bytes: &[u8]) -> Result<u64, EditError> {
    use std::io::Write;
    let path = upload_path(id)?;
    let io = |e: std::io::Error| EditError::Io(format!("暂存上传失败：{e}"));
    if offset + bytes.len() as u64 > UPLOAD_MAX {
        return Err(EditError::Invalid("上传太大了（最多 512 MiB）".into()));
    }
    let dir = uploads_dir();
    std::fs::create_dir_all(&dir).map_err(io)?;
    let mut file = if offset == 0 {
        gc_uploads(&dir);
        // 只给自己读写：暂存的是用户的表格内容。
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        opts.open(&path).map_err(io)?
    } else {
        let f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(|_| EditError::Invalid(format!("没有这份上传（{id}），要从 0 开始传")))?;
        let have = f.metadata().map_err(io)?.len();
        if have != offset {
            return Err(EditError::Invalid(format!(
                "分块顺序不对：已经有 {have} 字节，这块从 {offset} 开始"
            )));
        }
        f
    };
    file.write_all(bytes).map_err(io)?;
    Ok(offset + bytes.len() as u64)
}

fn gc_uploads(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > UPLOAD_STALE);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 新建一个文件（中间缺的文件夹一并建）。已经存在就报错，不覆盖。
pub fn create_file(root: &Path, rel: &str, text: &str) -> Result<Saved, EditError> {
    use std::io::Write;
    let file = confine_new(root, rel).map_err(EditError::Invalid)?;
    let io = |e: std::io::Error| EditError::Io(format!("建不了 {rel}：{e}"));
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .map_err(io)?;
    f.write_all(text.as_bytes()).map_err(io)?;
    saved(root, &file)
}

/// 新建文件夹（中间缺的一并建）。已经存在就报错。回相对路径。
pub fn make_dir(root: &Path, rel: &str) -> Result<String, EditError> {
    let dir = confine_new(root, rel).map_err(EditError::Invalid)?;
    std::fs::create_dir_all(&dir).map_err(|e| EditError::Io(format!("建不了 {rel}：{e}")))?;
    Ok(saved(root, &dir)?.path)
}

/// 改名或移动（文件、文件夹都行）。目标已经存在就报错，不覆盖；不能把文件夹移进它自己里面。
/// 符号链接移的是链接本身。回新的相对路径。
pub fn rename(root: &Path, from: &str, to: &str) -> Result<String, EditError> {
    let src = confine(root, from).map_err(EditError::Invalid)?;
    let base = root
        .canonicalize()
        .map_err(|e| EditError::Io(e.to_string()))?;
    if src == base {
        return Err(EditError::Invalid("不能移动工作区根目录".into()));
    }
    let dst = match confine_new(root, to) {
        Ok(dst) => dst,
        // 只改大小写（`a.md` → `A.md`）：不区分大小写的文件系统上目标「已经存在」，
        // 其实就是它自己。区分大小写的系统上那可能是另一个文件——不是同一个就照常拒绝。
        Err(e) if from.to_lowercase() == to.to_lowercase() && from != to => {
            let dst = confine(root, to).map_err(EditError::Invalid)?;
            if !same_entry(&src, &dst) {
                return Err(EditError::Invalid(e));
            }
            dst
        }
        Err(e) => return Err(EditError::Invalid(e)),
    };
    if dst != src && dst.starts_with(&src) {
        return Err(EditError::Invalid("不能把文件夹移进它自己里面".into()));
    }
    std::fs::rename(&src, &dst).map_err(|e| {
        if e.kind() == std::io::ErrorKind::CrossesDevices {
            EditError::Io(format!("不能跨磁盘移动 {from}"))
        } else {
            EditError::Io(format!("移动不了 {from}：{e}"))
        }
    })?;
    Ok(relative(&base, &dst))
}

/// 两个路径是不是磁盘上同一个条目（不跟符号链接）。
fn same_entry(a: &Path, b: &Path) -> bool {
    match (a.symlink_metadata(), b.symlink_metadata()) {
        #[cfg(unix)]
        (Ok(x), Ok(y)) => {
            use std::os::unix::fs::MetadataExt;
            (x.dev(), x.ino()) == (y.dev(), y.ino())
        }
        #[cfg(not(unix))]
        (Ok(_), Ok(_)) => a.canonicalize().ok() == b.canonicalize().ok(),
        _ => false,
    }
}

/// 移到系统废纸篓（macOS 废纸篓 / Linux freedesktop 回收站 / Windows 回收站）。
/// 没有回收站可用就报错，不硬删。符号链接扔的是链接本身。
pub fn move_to_trash(root: &Path, rel: &str) -> Result<(), EditError> {
    let path = confine(root, rel).map_err(EditError::Invalid)?;
    let base = root
        .canonicalize()
        .map_err(|e| EditError::Io(e.to_string()))?;
    if path == base {
        return Err(EditError::Invalid("不能删除工作区根目录".into()));
    }
    #[allow(unused_mut)]
    let mut trash = trash::TrashContext::default();
    // 默认的 Finder 方式走 AppleScript，会弹「想控制访达」的授权框。
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        trash.set_delete_method(DeleteMethod::NsFileManager);
    }
    trash
        .delete(&path)
        .map_err(|e| EditError::Io(format!("没能移到废纸篓：{e}")))
}

fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(p) => Some(p.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn modified_ms(meta: &std::fs::Metadata) -> Option<u64> {
    let t = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(t.as_millis() as u64)
}

fn walker(dir: &Path, depth: Option<usize>, hidden: bool) -> ignore::Walk {
    ignore::WalkBuilder::new(dir)
        .max_depth(depth)
        .hidden(!hidden)
        .require_git(false)
        .build()
}

/// 列一层：目录在前，名字不分大小写排序。
pub fn list(root: &Path, rel: &str, hidden: bool) -> Result<Listing, String> {
    let dir = confine(root, rel)?;
    if !dir.is_dir() {
        return Err(format!("不是目录：{rel}"));
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let base = dir.canonicalize().map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in walker(&dir, Some(1), hidden).flatten() {
        if entry.depth() == 0 {
            continue;
        }
        if entries.len() >= LIST_LIMIT {
            truncated = true;
            break;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let link = entry.path_is_symlink();
        let meta = std::fs::metadata(entry.path()).ok();
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
        let kind = match (link, is_dir) {
            (true, _) => EntryKind::Symlink,
            (false, true) => EntryKind::Dir,
            (false, false) => EntryKind::File,
        };
        let path = format!(
            "{}{}",
            match relative(&root, &base) {
                p if p.is_empty() => String::new(),
                p => format!("{p}/"),
            },
            name
        );
        entries.push(Entry {
            name,
            path,
            kind,
            size: meta.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
            modified_ms: meta.as_ref().and_then(modified_ms),
        });
    }
    entries.sort_by(|a, b| {
        let dir_first = (b.kind == EntryKind::Dir).cmp(&(a.kind == EntryKind::Dir));
        dir_first.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(Listing {
        path: relative(&root, &base),
        entries,
        truncated,
    })
}

/// 读一个文件：文本（UTF-8、不含 NUL）按 `max_text` 截断；常见图片给字节；其它只给大小。
pub fn read(root: &Path, rel: &str, max_text: usize) -> Result<FileRead, String> {
    let file = confine(root, rel)?;
    let meta = std::fs::metadata(&file).map_err(|e| format!("读不了 {rel}：{e}"))?;
    if !meta.is_file() {
        return Err(format!("不是文件：{rel}"));
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = relative(&root, &file.canonicalize().map_err(|e| e.to_string())?);
    let size = meta.len();
    let mime = image_mime(&file);
    let content = match mime {
        Some(m) if m != "image/svg+xml" => {
            if size > IMAGE_LIMIT {
                Content::Binary
            } else {
                let bytes = std::fs::read(&file).map_err(|e| format!("读不了 {rel}：{e}"))?;
                Content::Image { mime: m, bytes }
            }
        }
        _ => read_text(&file, max_text)?,
    };
    Ok(FileRead {
        path,
        size,
        modified_ms: modified_ms(&meta),
        mime,
        content,
    })
}

/// 原样读一段字节（不认类型、不转码）：大文件分块拉、浏览器里自己解析（表格、文档）。
/// `offset` 超过文件末尾回空；`len` 由调用方限住上限。
#[derive(Debug)]
pub struct RangeRead {
    pub path: String,
    pub size: u64,
    pub modified_ms: Option<u64>,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

pub fn read_range(root: &Path, rel: &str, offset: u64, len: usize) -> Result<RangeRead, String> {
    use std::io::{Read, Seek, SeekFrom};
    let file = confine(root, rel)?;
    let mut f = std::fs::File::open(&file).map_err(|e| format!("读不了 {rel}：{e}"))?;
    // 用打开后的句柄取元数据：和读到的字节是同一个文件。
    let meta = f.metadata().map_err(|e| format!("读不了 {rel}：{e}"))?;
    if !meta.is_file() {
        return Err(format!("不是文件：{rel}"));
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = relative(&root, &file.canonicalize().map_err(|e| e.to_string())?);
    let size = meta.len();
    let mut bytes = Vec::with_capacity(len.min(size.saturating_sub(offset) as usize));
    if offset < size {
        f.seek(SeekFrom::Start(offset))
            .map_err(|e| format!("读不了 {rel}：{e}"))?;
        f.take(len as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("读不了 {rel}：{e}"))?;
    }
    Ok(RangeRead {
        path,
        size,
        modified_ms: modified_ms(&meta),
        offset,
        bytes,
    })
}

fn read_text(file: &Path, max: usize) -> Result<Content, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(file).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    f.by_ref()
        .take(max as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    let truncated = buf.len() > max;
    buf.truncate(max);
    if buf.iter().take(8192).any(|b| *b == 0) {
        return Ok(Content::Binary);
    }
    // 截断可能切在一个多字节字符中间：退到最后一个完整字符。
    let text = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(e) if truncated && e.utf8_error().error_len().is_none() => {
            let valid = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).unwrap_or_default()
        }
        Err(_) => return Ok(Content::Binary),
    };
    Ok(Content::Text { text, truncated })
}

fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// 按名字找文件（快速打开）：查询的字符按顺序出现在相对路径里就算中，文件名里连着命中
/// 的排前面。空查询不找。
pub fn find(root: &Path, query: &str, limit: usize) -> Result<Vec<String>, String> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let base = root
        .canonicalize()
        .map_err(|e| format!("工作区目录不可用：{e}"))?;
    let mut hits: Vec<(i64, String)> = Vec::new();
    for entry in walker(&base, None, false).flatten().take(FIND_SCAN_LIMIT) {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = relative(&base, entry.path());
        if let Some(score) = fuzzy_score(&rel, &query) {
            hits.push((score, rel));
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.len().cmp(&b.1.len())));
    Ok(hits.into_iter().take(limit).map(|(_, p)| p).collect())
}

/// 子序列匹配打分：连着命中、命中在文件名里、命中在词首加分。不中回 `None`。
fn fuzzy_score(path: &str, query: &[char]) -> Option<i64> {
    let lower: Vec<char> = path.chars().flat_map(char::to_lowercase).collect();
    let name_start = lower.iter().rposition(|c| *c == '/').map_or(0, |i| i + 1);
    let mut score = 0i64;
    let mut qi = 0;
    let mut prev: Option<usize> = None;
    for (i, c) in lower.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if *c != query[qi] {
            continue;
        }
        score += 1;
        if prev == Some(i.wrapping_sub(1)) {
            score += 5;
        }
        if i >= name_start {
            score += 3;
        }
        if i == name_start || matches!(lower.get(i.wrapping_sub(1)), Some('/' | '_' | '-' | '.')) {
            score += 4;
        }
        prev = Some(i);
        qi += 1;
    }
    (qi == query.len()).then_some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/tools")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("README.md"), "# 你好\n").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("src/tools/list_dir.rs"), "// list\n").unwrap();
        std::fs::write(root.join("target/debug/out.bin"), [0u8, 1, 2]).unwrap();
        std::fs::write(root.join("logo.png"), [0x89, b'P', b'N', b'G']).unwrap();
        std::fs::write(root.join("blob.dat"), [0u8, 159, 146, 150]).unwrap();
        dir
    }

    #[test]
    fn list_sorts_dirs_first_and_respects_gitignore_and_dotfiles() {
        let dir = tree();
        let got = list(dir.path(), "", false).unwrap();
        let names: Vec<_> = got.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["src", "blob.dat", "logo.png", "README.md"]);
        assert_eq!(got.path, "");
        assert_eq!(got.entries[0].kind, EntryKind::Dir);
        assert_eq!(got.entries[3].size, Some("# 你好\n".len() as u64));

        let hidden = list(dir.path(), "", true).unwrap();
        assert!(hidden.entries.iter().any(|e| e.name == ".env"));

        let sub = list(dir.path(), "src", false).unwrap();
        let paths: Vec<_> = sub.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["src/tools", "src/main.rs"]);
        assert_eq!(sub.path, "src");
    }

    #[test]
    fn paths_outside_the_workspace_are_refused() {
        let dir = tree();
        assert!(list(dir.path(), "..", false).is_err());
        assert!(read(dir.path(), "../x", TEXT_LIMIT).is_err());
        assert!(read(dir.path(), "/etc/hosts", TEXT_LIMIT).is_err());
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "no").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
            let err = read(dir.path(), "escape/secret.txt", TEXT_LIMIT).unwrap_err();
            assert!(err.contains("不在工作区"), "{err}");
        }
    }

    #[test]
    fn read_distinguishes_text_image_and_binary_and_truncates_on_char_boundary() {
        let dir = tree();
        let text = read(dir.path(), "README.md", TEXT_LIMIT).unwrap();
        assert!(
            matches!(text.content, Content::Text { ref text, truncated: false } if text == "# 你好\n")
        );
        // "# 你" 是 5 字节；截在 4 字节会切进「你」中间，要退回 "# "。
        let cut = read(dir.path(), "README.md", 4).unwrap();
        assert!(matches!(cut.content, Content::Text { ref text, truncated: true } if text == "# "));
        let img = read(dir.path(), "logo.png", TEXT_LIMIT).unwrap();
        assert!(matches!(
            img.content,
            Content::Image {
                mime: "image/png",
                ..
            }
        ));
        let bin = read(dir.path(), "blob.dat", TEXT_LIMIT).unwrap();
        assert!(matches!(bin.content, Content::Binary));
        assert_eq!(bin.size, 4);
        assert!(read(dir.path(), "src", TEXT_LIMIT).is_err());
    }

    #[test]
    fn read_range_returns_raw_bytes_in_slices() {
        let dir = tree();
        // 二进制也照读：不认类型。
        let head = read_range(dir.path(), "blob.dat", 0, 2).unwrap();
        assert_eq!(head.bytes, [0u8, 159]);
        assert_eq!((head.size, head.offset), (4, 0));
        let tail = read_range(dir.path(), "blob.dat", 2, 100).unwrap();
        assert_eq!(tail.bytes, [146u8, 150]);
        let past = read_range(dir.path(), "blob.dat", 9, 100).unwrap();
        assert!(past.bytes.is_empty());
        assert_eq!(past.size, 4);
        assert!(read_range(dir.path(), "../x", 0, 1).is_err());
        assert!(read_range(dir.path(), "src", 0, 1).is_err());
    }

    #[test]
    fn save_text_writes_atomically_and_refuses_stale_edits() {
        let dir = tree();
        let before = read(dir.path(), "README.md", TEXT_LIMIT).unwrap();
        let saved = save_text(dir.path(), "README.md", "# 新\n", before.modified_ms).unwrap();
        assert_eq!(saved.path, "README.md");
        assert_eq!(saved.size, "# 新\n".len() as u64);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "# 新\n"
        );
        // 打开时的修改时间对不上（期间被改过）：不写。
        let stale = save_text(dir.path(), "README.md", "旧的覆盖", Some(1)).unwrap_err();
        assert!(
            matches!(
                stale,
                EditError::Conflict {
                    modified_ms: Some(_)
                }
            ),
            "{stale:?}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "# 新\n"
        );
        // 没留下临时文件。
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".dock-save-"))
            .collect();
        assert!(leftovers.is_empty());
        assert!(matches!(
            save_text(dir.path(), "src", "x", None),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            save_text(dir.path(), "../x", "x", None),
            Err(EditError::Invalid(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn save_text_refuses_read_only_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tree();
        let locked = dir.path().join("locked.md");
        std::fs::write(&locked, "原样").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();
        let err = save_text(dir.path(), "locked.md", "改了", None).unwrap_err();
        assert!(
            matches!(err, EditError::Io(ref m) if m.contains("只读")),
            "{err:?}"
        );
        assert_eq!(std::fs::read_to_string(&locked).unwrap(), "原样");
    }

    #[cfg(unix)]
    #[test]
    fn save_text_keeps_permissions_and_writes_through_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tree();
        let script = dir.path().join("run.sh");
        std::fs::write(&script, "echo 1\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("run.sh", dir.path().join("link.sh")).unwrap();
        save_text(dir.path(), "link.sh", "echo 2\n", None).unwrap();
        assert!(dir
            .path()
            .join("link.sh")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&script).unwrap(), "echo 2\n");
        let mode = std::fs::metadata(&script).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn save_bytes_and_chunked_uploads_replace_the_file_atomically() {
        let dir = tree();
        let _env = crate::test_env::scoped().home();
        save_bytes(dir.path(), "blob.dat", &[9, 8, 7], None).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("blob.dat")).unwrap(),
            [9, 8, 7]
        );

        let base = read(dir.path(), "README.md", TEXT_LIMIT)
            .unwrap()
            .modified_ms;
        assert_eq!(upload_chunk("up-12345678", 0, b"ab").unwrap(), 2);
        // 顺序不对：拒绝，已有的不动。
        assert!(matches!(
            upload_chunk("up-12345678", 5, b"x"),
            Err(EditError::Invalid(_))
        ));
        assert_eq!(upload_chunk("up-12345678", 2, b"cd").unwrap(), 4);
        // 期间被改过：不写，暂存留着（覆盖保存不用重传）。
        let stale =
            save_upload(dir.path(), "README.md", "up-12345678", Some(4), Some(1)).unwrap_err();
        assert!(matches!(stale, EditError::Conflict { .. }), "{stale:?}");
        assert!(uploads_dir().join("up-12345678").exists());
        // 总大小对不上：不写。
        assert!(matches!(
            save_upload(dir.path(), "README.md", "up-12345678", Some(9), base),
            Err(EditError::Invalid(_))
        ));
        let saved = save_upload(dir.path(), "README.md", "up-12345678", Some(4), base).unwrap();
        assert_eq!(saved.size, 4);
        assert_eq!(
            std::fs::read(dir.path().join("README.md")).unwrap(),
            b"abcd"
        );
        assert!(
            !uploads_dir().join("up-12345678").exists(),
            "提交后删掉暂存"
        );
        // 暂存不在工作区里。
        assert!(!uploads_dir().starts_with(dir.path().canonicalize().unwrap()));
        assert!(matches!(
            upload_chunk("../evil", 0, b"x"),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            upload_chunk("UP-12345678", 0, b"x"),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            upload_chunk("up-87654321", UPLOAD_MAX, b"x"),
            Err(EditError::Invalid(_))
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            upload_chunk("up-perm-0001", 0, b"x").unwrap();
            let mode = std::fs::metadata(uploads_dir().join("up-perm-0001"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(matches!(
            save_upload(dir.path(), "README.md", "no-such-upload", None, None),
            Err(EditError::Invalid(_))
        ));
    }

    #[test]
    fn create_mkdir_and_rename_stay_inside_and_never_overwrite() {
        let dir = tree();
        let made = create_file(dir.path(), "notes/today.md", "hi").unwrap();
        assert_eq!(made.path, "notes/today.md");
        assert!(matches!(
            create_file(dir.path(), "notes/today.md", "x"),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            create_file(dir.path(), "../out.md", "x"),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            create_file(dir.path(), "", "x"),
            Err(EditError::Invalid(_))
        ));
        // 文件不能当文件夹往下建。
        assert!(matches!(
            create_file(dir.path(), "README.md/x", "x"),
            Err(EditError::Invalid(_))
        ));
        assert_eq!(make_dir(dir.path(), "a/b").unwrap(), "a/b");
        assert!(matches!(
            make_dir(dir.path(), "a/b"),
            Err(EditError::Invalid(_))
        ));

        assert_eq!(
            rename(dir.path(), "notes/today.md", "a/b/today.md").unwrap(),
            "a/b/today.md"
        );
        assert!(dir.path().join("a/b/today.md").exists());
        assert!(matches!(
            rename(dir.path(), "a/b/today.md", "README.md"),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            rename(dir.path(), "a", "a/b/c"),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            rename(dir.path(), "", "x"),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            rename(dir.path(), "a", "../a"),
            Err(EditError::Invalid(_))
        ));
        // 只改大小写。
        assert_eq!(
            rename(dir.path(), "README.md", "Readme.md").unwrap(),
            "Readme.md"
        );
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"Readme.md".to_string()), "{names:?}");

        #[cfg(unix)]
        {
            // 经指到外面的符号链接目录往外写：挡住。
            let outside = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
            assert!(matches!(
                create_file(dir.path(), "escape/x.md", "x"),
                Err(EditError::Invalid(_))
            ));
            assert!(matches!(
                make_dir(dir.path(), "escape/new"),
                Err(EditError::Invalid(_))
            ));
            assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
        }
    }

    /// 区分大小写的文件系统上 `README.md` 和 `Readme.md` 是两个文件：「只改大小写」不能
    /// 把另一个覆盖掉。不区分大小写的系统上建不出两个，这条不测（跑法：TMPDIR 指到
    /// 区分大小写的卷上）。
    #[test]
    fn case_only_rename_never_overwrites_a_different_file() {
        let dir = tree();
        std::fs::write(dir.path().join("Readme.md"), "other").unwrap();
        let count = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .eq_ignore_ascii_case("readme.md")
            })
            .count();
        if count < 2 {
            return; // 不区分大小写：上面那一写就是同一个文件
        }
        let err = rename(dir.path(), "README.md", "Readme.md").unwrap_err();
        assert!(matches!(err, EditError::Invalid(_)), "{err:?}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("Readme.md")).unwrap(),
            "other"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
            "# 你好\n"
        );
    }

    #[test]
    fn trash_refuses_root_and_outside_paths() {
        let dir = tree();
        assert!(matches!(
            move_to_trash(dir.path(), ""),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            move_to_trash(dir.path(), "../x"),
            Err(EditError::Invalid(_))
        ));
        assert!(matches!(
            move_to_trash(dir.path(), "missing.md"),
            Err(EditError::Invalid(_))
        ));
    }

    #[test]
    fn find_ranks_file_name_hits_first_and_skips_ignored() {
        let dir = tree();
        let got = find(dir.path(), "listdir", 10).unwrap();
        assert_eq!(got, ["src/tools/list_dir.rs"]);
        let got = find(dir.path(), "main", 10).unwrap();
        assert_eq!(got.first().map(String::as_str), Some("src/main.rs"));
        assert!(
            find(dir.path(), "out.bin", 10).unwrap().is_empty(),
            "target/ 被忽略"
        );
        assert!(find(dir.path(), "  ", 10).unwrap().is_empty());
    }
}
