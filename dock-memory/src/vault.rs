//! 整个记忆目录（`$DOCK_HOME/memory/`）：所有 scope 的笔记清单、笔记 id、链接对到哪一篇。
//!
//! [`MemoryRoot`](crate::layout::MemoryRoot) 只看「全局 + 当前 cwd 的工作区」，是给一个会话用的；
//! 图谱和管理界面要跨项目看，所以这里按 scope 目录名（`global`、`workspace-<slug>`）寻址。
//!
//! 笔记 id 是 `<scope 目录>/<相对路径>`，如 `global/topics/prefs.md`、
//! `workspace-dock-b4be1135/observations/_inbox/flush-20261115-131605.md`。id 只在这里
//! 换成磁盘路径（[`MemoryVault::entry`]），换之前逐段校验，界面给的 id 走不出记忆目录。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::access::classify_relative;
use crate::layout::{MemoryScope, ScopePaths};
use crate::storage::MAX_MEMORY_READ_BYTES;

/// 全局 scope 的目录名，也是它的 scope id。
pub const GLOBAL_SCOPE_ID: &str = "global";
/// 工作区 scope 目录名的前缀。
pub const WORKSPACE_SCOPE_PREFIX: &str = "workspace-";

/// 工作区 slug（[`crate::workspace_slug`]）对应的 scope id。
pub fn workspace_scope_id(slug: &str) -> String {
    format!("{WORKSPACE_SCOPE_PREFIX}{slug}")
}

/// scope id 给人看的短名：`workspace-dock-b4be1135` → `dock`，`global` → `global`。
/// slug 末尾那 8 位是身份哈希（见 [`crate::workspace_slug`]），不是名字的一部分。
pub fn scope_label(scope_id: &str) -> String {
    let Some(slug) = scope_id.strip_prefix(WORKSPACE_SCOPE_PREFIX) else {
        return scope_id.to_string();
    };
    match slug.rsplit_once('-') {
        Some((name, hash))
            if !name.is_empty()
                && hash.len() == 8
                && hash.chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            name.to_string()
        }
        _ => slug.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NoteKind {
    /// `topics/`：维护中的稳定笔记。
    Topic,
    /// `observations/`：还没整理的观察。
    Observation,
}

impl NoteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Topic => "topic",
            Self::Observation => "observation",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "topic" => Some(Self::Topic),
            "observation" => Some(Self::Observation),
            _ => None,
        }
    }
}

/// 笔记是怎么来的，按文件名认（`flush-<时间>` / `remember-<时间>` 是 Dock 自己起的名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteOrigin {
    /// `/flush` 或压缩前自动整理出来的会话摘要。
    Flush,
    /// `/remember` 记下的一句话。
    Remember,
    /// agent 或用户自己起名写的。
    Authored,
}

impl NoteOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flush => "flush",
            Self::Remember => "remember",
            Self::Authored => "authored",
        }
    }

    fn of(stem: &str) -> Self {
        let stamped = |prefix: &str| {
            stem.strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        };
        if stamped("flush-") {
            Self::Flush
        } else if stamped("remember-") {
            Self::Remember
        } else {
            Self::Authored
        }
    }
}

/// 磁盘上的一篇笔记（没读正文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteEntry {
    /// `<scope 目录>/<相对路径>`。
    pub id: String,
    pub scope_id: String,
    pub scope: MemoryScope,
    pub kind: NoteKind,
    pub origin: NoteOrigin,
    /// 相对 scope 目录的路径，`/` 分隔：`topics/prefs.md`。
    pub rel: String,
    /// 文件名去掉 `.md`：`[[链接]]` 认的就是它。
    pub stem: String,
    pub path: PathBuf,
    /// 直接在 `topics/` 或 `observations/_inbox/` 下：能改、能遗忘。嵌套目录和旧版平铺的
    /// `observations/*.md` 只读（与 [`crate::MemoryAccessPolicy`] 一致）。
    pub writable: bool,
    pub bytes: u64,
    pub modified_ms: Option<u64>,
}

/// 读了正文的笔记。
#[derive(Debug, Clone)]
pub struct LoadedNote {
    pub entry: NoteEntry,
    /// 给人看的标题：一级标题；没有就取第一行正文；再没有用文件名。
    pub title: String,
    /// 第一行正文（一句话说明）。
    pub summary: String,
    pub text: String,
    /// 正文超过读取上限，只读了前面一段。
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("笔记 id 不合法：{0}")]
    InvalidId(String),
    #[error("记忆范围不合法：{0}")]
    InvalidScope(String),
    #[error("没有这篇笔记：{0}")]
    NotFound(String),
    #[error("笔记过大（{size} 字节；上限 {limit} 字节）：{id}")]
    TooLarge { id: String, size: u64, limit: u64 },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// `$DOCK_HOME/memory/`。
#[derive(Debug, Clone)]
pub struct MemoryVault {
    home: PathBuf,
}

impl MemoryVault {
    pub fn new(dock_home: &Path) -> Self {
        Self {
            home: dock_home.join("memory"),
        }
    }

    pub fn open_default() -> Self {
        Self::new(&cordis_base::config::dock_home())
    }

    /// 直接给记忆目录本身（[`crate::MemoryRoot::home`]）。
    pub fn at(memory_home: PathBuf) -> Self {
        Self { home: memory_home }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn search_db(&self) -> PathBuf {
        self.home.join("search.sqlite")
    }

    /// 磁盘上有的 scope：`global` 在前，工作区按名字排。
    pub fn scope_ids(&self) -> Vec<String> {
        let mut workspaces = Vec::new();
        let mut has_global = false;
        let Ok(entries) = std::fs::read_dir(&self.home) else {
            return Vec::new();
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if name == GLOBAL_SCOPE_ID {
                has_global = true;
            } else if scope_of(&name).is_some() {
                workspaces.push(name);
            }
        }
        workspaces.sort();
        let mut out = Vec::with_capacity(workspaces.len() + 1);
        if has_global {
            out.push(GLOBAL_SCOPE_ID.to_string());
        }
        out.extend(workspaces);
        out
    }

    /// 一个 scope 的目录（不要求已经存在）。id 形状不对回错。
    pub fn scope_paths(&self, scope_id: &str) -> Result<(MemoryScope, ScopePaths), VaultError> {
        let scope =
            scope_of(scope_id).ok_or_else(|| VaultError::InvalidScope(scope_id.to_string()))?;
        Ok((scope, ScopePaths::from_root(self.home.join(scope_id))))
    }

    /// 一个 scope 里的笔记，按 id 排。目录不存在回空。
    pub fn list_scope(&self, scope_id: &str) -> Vec<NoteEntry> {
        let Ok((scope, paths)) = self.scope_paths(scope_id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        collect_notes(&paths.root, &paths.topics, scope_id, scope, &mut out);
        collect_notes(&paths.root, &paths.observations, scope_id, scope, &mut out);
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// 几个 scope 的笔记连正文一起读出来。读不了的那篇跳过（别的照常）。
    pub fn load(&self, scope_ids: &[String]) -> Vec<LoadedNote> {
        scope_ids
            .iter()
            .flat_map(|id| self.list_scope(id))
            .filter_map(|entry| {
                let (text, truncated) = read_lossy_capped(&entry.path).ok()?;
                let (title, summary) = describe(&text, &entry.stem);
                Some(LoadedNote {
                    entry,
                    title,
                    summary,
                    text,
                    truncated,
                })
            })
            .collect()
    }

    /// id → 磁盘上的那篇笔记。逐段校验：只认 `topics/`、`observations/` 下的 `.md`，
    /// 不跟符号链接，出不了记忆目录。
    pub fn entry(&self, id: &str) -> Result<NoteEntry, VaultError> {
        let invalid = || VaultError::InvalidId(id.to_string());
        let (scope_id, rel) = id.split_once('/').ok_or_else(invalid)?;
        let (scope, paths) = self.scope_paths(scope_id).map_err(|_| invalid())?;
        let rel_path = Path::new(rel);
        let plain = rel_path
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
        if !plain
            || rel.contains('\\')
            || rel_path.extension().and_then(|e| e.to_str()) != Some("md")
        {
            return Err(invalid());
        }
        let path = paths.root.join(rel_path);
        let kind = kind_of(rel).ok_or_else(invalid)?;
        // 从 scope 目录起，路径上任何一段是符号链接都不认：它可能指到记忆目录外面。
        let mut probe = self.home.clone();
        for part in Path::new(scope_id)
            .components()
            .chain(rel_path.components())
        {
            probe.push(part);
            match std::fs::symlink_metadata(&probe) {
                Ok(meta) if meta.file_type().is_symlink() => return Err(invalid()),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(VaultError::NotFound(id.to_string()))
                }
                Err(e) => return Err(e.into()),
            }
        }
        let meta = std::fs::metadata(&path)?;
        if !meta.is_file() {
            return Err(VaultError::NotFound(id.to_string()));
        }
        Ok(note_entry(scope_id, scope, kind, rel, path, &meta))
    }

    /// agent 给的一个引用对到哪篇笔记：`[[链接]]`、笔记名，或索引里那种相对路径
    /// （`topics/prefs.md`）。在 `scope_ids` 这几个 scope 里找，按 `from_scope` 的视角
    /// 决定同名时取谁（见 [`LinkResolver`]）。
    pub fn resolve_reference(
        &self,
        scope_ids: &[String],
        from_scope: &str,
        reference: &str,
    ) -> Option<NoteEntry> {
        let reference = reference.trim();
        let target = match crate::links::parse_wikilinks(reference).into_iter().next() {
            Some(link) => link.target,
            None => reference.to_string(),
        };
        let notes = self.load(scope_ids);
        let index = LinkResolver::new(&notes).resolve(from_scope, &target)?;
        notes.into_iter().nth(index).map(|note| note.entry)
    }

    /// 读一篇笔记的全文。超过 [`MAX_MEMORY_READ_BYTES`] 的拒绝（和 `memory_get` 同一个上限）。
    pub fn read(&self, entry: &NoteEntry) -> Result<String, VaultError> {
        let meta = std::fs::metadata(&entry.path)?;
        if meta.len() > MAX_MEMORY_READ_BYTES {
            return Err(VaultError::TooLarge {
                id: entry.id.clone(),
                size: meta.len(),
                limit: MAX_MEMORY_READ_BYTES,
            });
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        // 读的时候再限一次：文件在 stat 之后长大也不会整份读进来。
        std::fs::File::open(&entry.path)?
            .take(MAX_MEMORY_READ_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_MEMORY_READ_BYTES {
            return Err(VaultError::TooLarge {
                id: entry.id.clone(),
                size: bytes.len() as u64,
                limit: MAX_MEMORY_READ_BYTES,
            });
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// scope id 的形状：`global`，或 `workspace-` 加一段不含路径分隔符的名字。
fn scope_of(scope_id: &str) -> Option<MemoryScope> {
    if scope_id == GLOBAL_SCOPE_ID {
        return Some(MemoryScope::Global);
    }
    let slug = scope_id.strip_prefix(WORKSPACE_SCOPE_PREFIX)?;
    let plain = !slug.is_empty()
        && !slug.starts_with('.')
        && slug
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'));
    plain.then_some(MemoryScope::Workspace)
}

fn kind_of(rel: &str) -> Option<NoteKind> {
    match rel.split('/').next() {
        Some("topics") => Some(NoteKind::Topic),
        Some("observations") => Some(NoteKind::Observation),
        _ => None,
    }
}

fn note_entry(
    scope_id: &str,
    scope: MemoryScope,
    kind: NoteKind,
    rel: &str,
    path: PathBuf,
    meta: &std::fs::Metadata,
) -> NoteEntry {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    NoteEntry {
        id: format!("{scope_id}/{rel}"),
        scope_id: scope_id.to_string(),
        scope,
        kind,
        origin: NoteOrigin::of(&stem),
        rel: rel.to_string(),
        writable: classify_relative(Path::new(rel), scope).is_writable(),
        stem,
        path,
        bytes: meta.len(),
        modified_ms,
    }
}

/// 递归收 `dir` 下的 `.md`。不跟符号链接、不看隐藏文件（原子写入的临时文件以 `.` 开头）。
fn collect_notes(
    scope_root: &Path,
    dir: &Path,
    scope_id: &str,
    scope: MemoryScope,
    out: &mut Vec<NoteEntry>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') || file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_notes(scope_root, &path, scope_id, scope, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(rel) = path.strip_prefix(scope_root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let (Some(kind), Ok(meta)) = (kind_of(&rel), entry.metadata()) else {
            continue;
        };
        out.push(note_entry(scope_id, scope, kind, &rel, path, &meta));
    }
}

/// 读前 [`MAX_MEMORY_READ_BYTES`] 字节：建图要的是标题、链接和大意，超长的笔记读个开头也能上图。
fn read_lossy_capped(path: &Path) -> std::io::Result<(String, bool)> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_MEMORY_READ_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() as u64 > MAX_MEMORY_READ_BYTES;
    bytes.truncate(MAX_MEMORY_READ_BYTES as usize);
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}

const MAX_TITLE_CHARS: usize = 60;
const MAX_SUMMARY_CHARS: usize = 160;

/// 一篇笔记给人看的 `(标题, 一句话说明)`。
///
/// 有一级标题用一级标题。`/flush` 写的摘要没有一级标题，开头是模板里的
/// `## Decisions & rationale`，每篇都一样、当标题没法认，所以退而取第一行正文。
pub fn describe(text: &str, stem: &str) -> (String, String) {
    let mut first_heading: Option<String> = None;
    let mut h1: Option<String> = None;
    let mut first_body: Option<String> = None;
    let mut in_fence = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("```") || line.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || line.is_empty() || line.starts_with("<!--") {
            continue;
        }
        let level = line.bytes().take_while(|b| *b == b'#').count();
        let heading = (1..=6)
            .contains(&level)
            .then(|| line.get(level..).unwrap_or(""))
            .filter(|rest| rest.starts_with([' ', '\t']))
            .map(|rest| plain_inline(rest.trim().trim_end_matches('#').trim()));
        match heading {
            Some(heading) if heading.is_empty() => {}
            Some(heading) => {
                if level == 1 && h1.is_none() && first_body.is_none() {
                    h1 = Some(heading.clone());
                }
                first_heading.get_or_insert(heading);
            }
            None => {
                let body = plain_inline(line);
                if !body.is_empty() && first_body.is_none() {
                    first_body = Some(body);
                }
            }
        }
        if first_body.is_some() && first_heading.is_some() {
            break;
        }
    }
    let summary = first_body
        .as_deref()
        .map(|s| truncate_chars(s, MAX_SUMMARY_CHARS))
        .unwrap_or_default();
    let title = h1
        .or(first_body)
        .or(first_heading)
        .unwrap_or_else(|| stem.to_string());
    (truncate_chars(&title, MAX_TITLE_CHARS), summary)
}

/// 去掉一行里的 Markdown 记号，留下能当标题读的字：列表符号、引用、加粗、行内代码的
/// 反引号、`[[链接|文字]]` 的方括号。
fn plain_inline(line: &str) -> String {
    let mut rest = line.trim();
    loop {
        let stripped = rest
            .strip_prefix("- ")
            .or_else(|| rest.strip_prefix("* "))
            .or_else(|| rest.strip_prefix("+ "))
            .or_else(|| rest.strip_prefix("> "))
            .or_else(|| rest.strip_prefix("[ ] "))
            .or_else(|| rest.strip_prefix("[x] "));
        match stripped {
            Some(next) => rest = next.trim_start(),
            None => break,
        }
    }
    let mut out = String::with_capacity(rest.len());
    let mut cursor = 0usize;
    for link in crate::links::parse_wikilinks(rest) {
        out.push_str(rest.get(cursor..link.span.start).unwrap_or(""));
        out.push_str(link.display_text());
        cursor = link.span.end;
    }
    out.push_str(rest.get(cursor..).unwrap_or(""));
    out.replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 把 `[[目标]]` 对到具体一篇笔记。
///
/// 规则照 Obsidian：按文件名（不分大小写、不带 `.md`）找；带 `/` 的按路径找。同名的
/// 先取链接所在 scope 里的，再取全局的，最后才是别的项目的；同一档里主题优先于观察。
/// 文件名对不上时再按标题找一次——agent 有时会写成 `[[审计日志改造]]`。
pub struct LinkResolver {
    notes: Vec<ResolverNote>,
    by_path: HashMap<String, usize>,
    by_stem: HashMap<String, Vec<usize>>,
    by_title: HashMap<String, Vec<usize>>,
}

struct ResolverNote {
    id: String,
    scope_id: String,
    kind: NoteKind,
    stem: String,
}

impl LinkResolver {
    /// `notes` 的下标就是 [`Self::resolve`] 回的下标。
    pub fn new(notes: &[LoadedNote]) -> Self {
        let mut by_path = HashMap::new();
        let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_title: HashMap<String, Vec<usize>> = HashMap::new();
        let mut owned = Vec::with_capacity(notes.len());
        for (index, note) in notes.iter().enumerate() {
            let id = note.entry.id.to_lowercase();
            by_path.insert(id.strip_suffix(".md").unwrap_or(&id).to_string(), index);
            by_stem
                .entry(note.entry.stem.to_lowercase())
                .or_default()
                .push(index);
            by_title
                .entry(note.title.trim().to_lowercase())
                .or_default()
                .push(index);
            owned.push(ResolverNote {
                id: note.entry.id.clone(),
                scope_id: note.entry.scope_id.clone(),
                kind: note.entry.kind,
                stem: note.entry.stem.clone(),
            });
        }
        Self {
            notes: owned,
            by_path,
            by_stem,
            by_title,
        }
    }

    /// `from_scope` 里的一篇笔记写了 `[[target]]`：它指的是哪一篇。
    pub fn resolve(&self, from_scope: &str, target: &str) -> Option<usize> {
        let normalized = normalize_target(target);
        if normalized.is_empty() {
            return None;
        }
        let name = if normalized.contains('/') {
            let from = from_scope.to_lowercase();
            let by_path = [
                normalized.clone(),
                format!("{from}/{normalized}"),
                format!("{GLOBAL_SCOPE_ID}/{normalized}"),
            ]
            .iter()
            .find_map(|key| self.by_path.get(key).copied());
            if by_path.is_some() {
                return by_path;
            }
            normalized.rsplit('/').next().unwrap_or("").to_string()
        } else {
            normalized
        };
        self.by_stem
            .get(&name)
            .or_else(|| self.by_title.get(&name))
            .and_then(|candidates| {
                candidates
                    .iter()
                    .copied()
                    .min_by_key(|i| self.notes.get(*i).map(|n| (rank(from_scope, n), &n.id)))
            })
    }

    /// 在 `from_scope` 里要怎么写才能指到第 `to` 篇：文件名不撞车就写文件名，
    /// 撞了（别的 scope 有更优先的同名笔记）写完整路径。
    pub fn target_for(&self, from_scope: &str, to: usize) -> Option<String> {
        let note = self.notes.get(to)?;
        if self.resolve(from_scope, &note.stem) == Some(to) {
            return Some(note.stem.clone());
        }
        Some(note.id.strip_suffix(".md").unwrap_or(&note.id).to_string())
    }
}

fn rank(from_scope: &str, note: &ResolverNote) -> u8 {
    let scope = if note.scope_id == from_scope {
        0
    } else if note.scope_id == GLOBAL_SCOPE_ID {
        2
    } else {
        4
    };
    scope + u8::from(note.kind != NoteKind::Topic)
}

/// 链接目标归一：去空白、去 `.md`、`\` 换 `/`、小写。
fn normalize_target(target: &str) -> String {
    let lowered = target.trim().replace('\\', "/").to_lowercase();
    let trimmed = lowered.trim_start_matches("./").trim_matches('/');
    trimmed
        .strip_suffix(".md")
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn vault_with(files: &[(&str, &str)]) -> (tempfile::TempDir, MemoryVault) {
        let tmp = tempfile::tempdir().unwrap();
        let vault = MemoryVault::new(tmp.path());
        for (rel, body) in files {
            write(vault.home(), rel, body);
        }
        (tmp, vault)
    }

    #[test]
    fn lists_scopes_with_global_first_and_skips_strangers() {
        let (_tmp, vault) = vault_with(&[
            ("workspace-zeta-0a1b2c3d/topics/a.md", "# a\n"),
            ("global/topics/prefs.md", "# prefs\n"),
            ("workspace-alpha-11223344/observations/_inbox/b.md", "# b\n"),
            ("not-a-scope/topics/x.md", "# x\n"),
        ]);
        std::fs::write(vault.home().join("search.sqlite"), b"").unwrap();
        assert_eq!(
            vault.scope_ids(),
            vec![
                "global",
                "workspace-alpha-11223344",
                "workspace-zeta-0a1b2c3d"
            ]
        );
        // 记忆目录还没建：空，不报错。
        assert!(MemoryVault::new(Path::new("/nonexistent/dock-home"))
            .scope_ids()
            .is_empty());
    }

    #[test]
    fn scope_label_drops_the_identity_hash() {
        assert_eq!(scope_label("workspace-dock-b4be1135"), "dock");
        assert_eq!(
            scope_label("workspace-sungods-platform-ca089a7f"),
            "sungods-platform"
        );
        // 末段不是 8 位十六进制：整个 slug 都是名字。
        assert_eq!(scope_label("workspace-my-project"), "my-project");
        assert_eq!(scope_label("global"), "global");
    }

    #[test]
    fn list_classifies_kind_origin_and_writability() {
        let (_tmp, vault) = vault_with(&[
            ("global/topics/prefs.md", "# prefs\n"),
            ("global/topics/nested/deep.md", "# deep\n"),
            (
                "global/observations/_inbox/flush-20261115-131605.md",
                "## x\n",
            ),
            (
                "global/observations/_inbox/remember-20261001-090000.md",
                "## Remember\n",
            ),
            (
                "global/observations/_inbox/flush-notes.md",
                "# hand written\n",
            ),
            ("global/observations/legacy-flat.md", "# legacy\n"),
            ("global/archive/gone.md", "# archived\n"),
            ("global/MEMORY.md", "# Global memory\n"),
            ("global/topics/.note-abc.tmp.md", "half written"),
            ("global/topics/readme.txt", "not markdown"),
        ]);
        let notes = vault.list_scope("global");
        let at = |rel: &str| {
            notes
                .iter()
                .find(|n| n.rel == rel)
                .unwrap_or_else(|| panic!("清单里没有 {rel}"))
        };
        assert_eq!(
            notes.iter().map(|n| n.rel.as_str()).collect::<Vec<_>>(),
            vec![
                "observations/_inbox/flush-20261115-131605.md",
                "observations/_inbox/flush-notes.md",
                "observations/_inbox/remember-20261001-090000.md",
                "observations/legacy-flat.md",
                "topics/nested/deep.md",
                "topics/prefs.md",
            ],
            "归档、MEMORY.md、隐藏文件、非 md 都不是笔记"
        );
        let prefs = at("topics/prefs.md");
        assert_eq!(prefs.id, "global/topics/prefs.md");
        assert_eq!(
            (prefs.kind, prefs.origin),
            (NoteKind::Topic, NoteOrigin::Authored)
        );
        assert!(prefs.writable);
        assert_eq!(prefs.stem, "prefs");
        assert!(prefs.modified_ms.is_some());
        let flush = at("observations/_inbox/flush-20261115-131605.md");
        assert_eq!(
            (flush.kind, flush.origin),
            (NoteKind::Observation, NoteOrigin::Flush)
        );
        assert!(flush.writable);
        assert_eq!(
            at("observations/_inbox/remember-20261001-090000.md").origin,
            NoteOrigin::Remember
        );
        // 名字以 flush- 开头但不是时间戳：是人起的名。
        assert_eq!(
            at("observations/_inbox/flush-notes.md").origin,
            NoteOrigin::Authored
        );
        // 嵌套目录和旧版平铺的观察：看得到，改不了。
        assert!(!at("topics/nested/deep.md").writable);
        assert!(!at("observations/legacy-flat.md").writable);
    }

    #[test]
    fn entry_rejects_ids_that_leave_the_notes() {
        let (tmp, vault) = vault_with(&[
            ("global/topics/prefs.md", "# prefs\n"),
            ("global/archive/gone.md", "# archived\n"),
            ("global/MEMORY.md", "# index\n"),
        ]);
        std::fs::write(tmp.path().join("outside.md"), "secret").unwrap();
        assert_eq!(vault.entry("global/topics/prefs.md").unwrap().stem, "prefs");
        for id in [
            "global/topics/../../outside.md",
            "global/../outside.md",
            "../outside.md",
            "global//etc/passwd.md",
            "global/archive/gone.md",
            "global/MEMORY.md",
            "global/topics/prefs",
            "global/memory_state.sqlite",
            "workspace-/topics/a.md",
            "workspace-a/b/topics/a.md",
            "elsewhere/topics/a.md",
            "global",
            "",
        ] {
            assert!(
                matches!(vault.entry(id), Err(VaultError::InvalidId(_))),
                "{id:?} 应该被拒，得到 {:?}",
                vault.entry(id)
            );
        }
        assert!(matches!(
            vault.entry("global/topics/missing.md"),
            Err(VaultError::NotFound(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_neither_listed_nor_resolved() {
        let (tmp, vault) = vault_with(&[("global/topics/real.md", "# real\n")]);
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.md"), "# secret\n").unwrap();
        let topics = vault.home().join("global/topics");
        std::os::unix::fs::symlink(outside.join("secret.md"), topics.join("link.md")).unwrap();
        std::os::unix::fs::symlink(&outside, topics.join("dir")).unwrap();
        assert_eq!(
            vault
                .list_scope("global")
                .iter()
                .map(|n| n.rel.as_str())
                .collect::<Vec<_>>(),
            vec!["topics/real.md"]
        );
        assert!(matches!(
            vault.entry("global/topics/link.md"),
            Err(VaultError::InvalidId(_))
        ));
        assert!(matches!(
            vault.entry("global/topics/dir/secret.md"),
            Err(VaultError::InvalidId(_))
        ));
        // 整个 scope 目录是个符号链接：清单里没有它，按 id 也够不着里面的文件。
        let elsewhere = tmp.path().join("elsewhere/topics");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("leak.md"), "# leak\n").unwrap();
        std::os::unix::fs::symlink(
            tmp.path().join("elsewhere"),
            vault.home().join("workspace-linked-00000009"),
        )
        .unwrap();
        assert_eq!(vault.scope_ids(), vec!["global"]);
        assert!(matches!(
            vault.entry("workspace-linked-00000009/topics/leak.md"),
            Err(VaultError::InvalidId(_))
        ));
    }

    #[test]
    fn read_refuses_oversize_notes_but_load_keeps_the_head() {
        let big = format!(
            "# 大笔记\n\n{}",
            "字".repeat(MAX_MEMORY_READ_BYTES as usize / 3 + 16)
        );
        let (_tmp, vault) = vault_with(&[("global/topics/big.md", &big)]);
        let entry = vault.entry("global/topics/big.md").unwrap();
        assert!(matches!(
            vault.read(&entry),
            Err(VaultError::TooLarge { .. })
        ));
        let loaded = vault.load(&["global".to_string()]);
        let [note] = loaded.as_slice() else {
            panic!("应该读出一篇");
        };
        assert!(note.truncated);
        assert_eq!(note.title, "大笔记");
        assert!(note.text.len() <= MAX_MEMORY_READ_BYTES as usize);
    }

    #[test]
    fn describe_prefers_h1_then_first_body_line() {
        assert_eq!(
            describe(
                "# 审计日志改造\n\n## 现状\n- `record(...)` 是**唯一**入口\n",
                "audit"
            ),
            (
                "审计日志改造".to_string(),
                "record(...) 是唯一入口".to_string()
            )
        );
        // flush 摘要：模板小标题每篇都一样，取第一行正文。
        let (title, summary) = describe(
            "## Decisions & rationale\n- **Root cause** found for [[cache|缓存]] misses\n",
            "flush-20261115-131605",
        );
        assert_eq!(title, "Root cause found for 缓存 misses");
        assert_eq!(summary, title);
        // 只有标题、没有正文。
        assert_eq!(
            describe("## Remember\n", "remember-1"),
            ("Remember".to_string(), String::new())
        );
        // 空文件：用文件名。
        assert_eq!(
            describe("", "clash-proxy"),
            ("clash-proxy".to_string(), String::new())
        );
        // 正文在前的一级标题不算这篇的标题；代码块里的 `#` 不是标题。
        assert_eq!(
            describe("先有一句话\n\n# 后面的标题\n", "x").0,
            "先有一句话"
        );
        assert_eq!(describe("```sh\n# comment\n```\n正文\n", "x").0, "正文");
        // 太长的截断。
        let long = "长".repeat(100);
        assert_eq!(describe(&long, "x").0.chars().count(), MAX_TITLE_CHARS + 1);
    }

    fn loaded(vault: &MemoryVault) -> Vec<LoadedNote> {
        vault.load(&vault.scope_ids())
    }

    fn resolve_id(notes: &[LoadedNote], from_scope: &str, target: &str) -> Option<String> {
        LinkResolver::new(notes)
            .resolve(from_scope, target)
            .and_then(|i| notes.get(i))
            .map(|n| n.entry.id.clone())
    }

    #[test]
    fn resolver_prefers_own_scope_then_global_then_others() {
        let (_tmp, vault) = vault_with(&[
            ("global/topics/auth.md", "# 全局登录\n"),
            ("workspace-a-00000001/topics/auth.md", "# A 登录\n"),
            (
                "workspace-a-00000001/observations/_inbox/auth.md",
                "# A 观察\n",
            ),
            ("workspace-b-00000002/topics/only-b.md", "# 只在 B\n"),
            (
                "workspace-b-00000002/observations/_inbox/Mixed-Case.md",
                "# 审计日志改造\n",
            ),
        ]);
        let notes = loaded(&vault);
        let at = |scope: &str, target: &str| resolve_id(&notes, scope, target);
        // 自己 scope 里的主题最优先（同名观察排后）。
        assert_eq!(
            at("workspace-a-00000001", "auth").as_deref(),
            Some("workspace-a-00000001/topics/auth.md")
        );
        // 自己没有：取全局的。
        assert_eq!(
            at("workspace-b-00000002", "auth").as_deref(),
            Some("global/topics/auth.md")
        );
        // 全局也没有：别的项目里的也认。
        assert_eq!(
            at("global", "only-b").as_deref(),
            Some("workspace-b-00000002/topics/only-b.md")
        );
        // 不分大小写、`.md` 可带可不带。
        assert_eq!(
            at("global", "mixed-case.MD").as_deref(),
            Some("workspace-b-00000002/observations/_inbox/Mixed-Case.md")
        );
        // 文件名对不上：按标题找。
        assert_eq!(
            at("global", "审计日志改造").as_deref(),
            Some("workspace-b-00000002/observations/_inbox/Mixed-Case.md")
        );
        assert_eq!(at("global", "nowhere"), None);
        assert_eq!(at("global", "  "), None);
    }

    #[test]
    fn resolver_follows_paths_before_falling_back_to_the_name() {
        let (_tmp, vault) = vault_with(&[
            ("global/topics/auth.md", "# 全局登录\n"),
            ("workspace-a-00000001/topics/auth.md", "# A 登录\n"),
            (
                "workspace-a-00000001/observations/_inbox/auth.md",
                "# A 观察\n",
            ),
        ]);
        let notes = loaded(&vault);
        let at = |scope: &str, target: &str| resolve_id(&notes, scope, target);
        // 相对自己 scope 的路径：能点名同名的观察。
        assert_eq!(
            at("workspace-a-00000001", "observations/_inbox/auth").as_deref(),
            Some("workspace-a-00000001/observations/_inbox/auth.md")
        );
        // 完整 id：从哪儿写都指同一篇。
        assert_eq!(
            at("global", "workspace-a-00000001/topics/auth.md").as_deref(),
            Some("workspace-a-00000001/topics/auth.md")
        );
        // 自己 scope 没有这条路径：退到全局的同一路径。
        assert_eq!(
            at("workspace-b-00000002", "topics/auth").as_deref(),
            Some("global/topics/auth.md")
        );
        // 路径对不上任何文件：按最后一段当文件名找。
        assert_eq!(
            at("workspace-a-00000001", "wrong/dir/auth").as_deref(),
            Some("workspace-a-00000001/topics/auth.md")
        );
    }

    #[test]
    fn references_resolve_from_links_names_and_index_paths() {
        let (_tmp, vault) = vault_with(&[
            ("global/topics/prefs.md", "# 偏好\n"),
            ("workspace-a-00000001/topics/auth.md", "# 登录方案\n"),
            ("workspace-b-00000002/topics/secret.md", "# 别的项目\n"),
        ]);
        let scopes = ["global".to_string(), "workspace-a-00000001".to_string()];
        let at = |reference: &str| {
            vault
                .resolve_reference(&scopes, "workspace-a-00000001", reference)
                .map(|e| e.id)
        };
        for reference in [
            "[[auth]]",
            "[[auth#刷新|别名]]",
            "auth",
            " Auth.md ",
            "topics/auth.md",
            "登录方案",
        ] {
            assert_eq!(
                at(reference).as_deref(),
                Some("workspace-a-00000001/topics/auth.md"),
                "{reference}"
            );
        }
        assert_eq!(at("[[prefs]]").as_deref(), Some("global/topics/prefs.md"));
        // 只在给定的 scope 里找：别的项目的笔记够不着。
        assert_eq!(at("secret"), None);
        assert_eq!(at("[[]]"), None);
    }

    #[test]
    fn target_for_uses_the_bare_name_unless_it_is_shadowed() {
        let (_tmp, vault) = vault_with(&[
            ("global/topics/auth.md", "# 全局登录\n"),
            ("global/topics/prefs.md", "# 偏好\n"),
            ("workspace-a-00000001/topics/auth.md", "# A 登录\n"),
        ]);
        let notes = loaded(&vault);
        let resolver = LinkResolver::new(&notes);
        let index = |id: &str| notes.iter().position(|n| n.entry.id == id).unwrap();
        let global_auth = index("global/topics/auth.md");
        assert_eq!(
            resolver
                .target_for("workspace-a-00000001", index("global/topics/prefs.md"))
                .as_deref(),
            Some("prefs")
        );
        // A 里写 `[[auth]]` 会指到 A 自己那篇：指全局的要写全路径。
        let target = resolver
            .target_for("workspace-a-00000001", global_auth)
            .unwrap();
        assert_eq!(target, "global/topics/auth");
        assert_eq!(
            resolver.resolve("workspace-a-00000001", &target),
            Some(global_auth)
        );
        assert_eq!(
            resolver.target_for("global", global_auth).as_deref(),
            Some("auth")
        );
    }
}
