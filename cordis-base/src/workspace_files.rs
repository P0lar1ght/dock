//! 只读看工作区文件：列目录、读一个文件、按名字找文件。给网关的 `fs/*` 用（GUI 的文件面板）。
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
