//! 记忆笔记的管理操作（图谱界面用）：读、存、新建、改名、挪位置、遗忘、建链 / 断链。
//!
//! 都按笔记 id 寻址（见 [`crate::vault`]），能改的只有直接放在 `topics/` 和
//! `observations/_inbox/` 下的 `.md`——和 agent 的写入规则同一条线。
//!
//! 改完做两件善后：
//! - 检索库里对应的条目跟着改。库还没建过就不建：第一次检索会整棵重建，这里先建一个
//!   只有一篇的库反而会让它以为「已经建好了」。
//! - 这个 scope 的 `MEMORY.md` 重新生成（agent 下一步读到的就是新索引）。
//!
//! 善后失败只记日志：文件已经改了，回一个错误会让界面以为没改成。

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::access::{forget_in_scope, AccessError, ForgetResult};
use crate::index::MemoryIndex;
use crate::layout::{MemoryScope, ScopePaths};
use crate::links::{
    append_related_link, is_linkable_name, parse_wikilinks, remove_wikilinks, retarget_wikilinks,
};
use crate::slug::note_stem;
use crate::storage::MAX_MEMORY_READ_BYTES;
use crate::vault::{
    describe, LinkResolver, LoadedNote, MemoryVault, NoteEntry, NoteKind, VaultError,
};

/// 文件名（不含 `.md`）最多多少个字符。
const MAX_STEM_CHARS: usize = 48;

/// 一篇笔记的全文。
#[derive(Debug, Clone)]
pub struct NoteContent {
    pub entry: NoteEntry,
    pub title: String,
    pub summary: String,
    pub text: String,
    /// 正文的 blake3（十六进制）。保存和遗忘时带回来，用来发现「期间被别人改过」。
    pub hash: String,
}

#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error("这篇笔记是只读的：{0}")]
    ReadOnly(String),
    #[error("笔记在这期间被改过，请重新打开再保存")]
    Conflict { expected: String, actual: String },
    #[error("笔记过大（{size} 字节；上限 {limit} 字节）")]
    TooLarge { size: usize, limit: u64 },
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Access(#[from] AccessError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Index(#[from] rusqlite::Error),
}

pub type NoteResult<T> = Result<T, NoteError>;

/// 正文的指纹：和 [`crate::forget`] 校验的是同一个值。
pub fn content_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

pub fn read_note(vault: &MemoryVault, id: &str) -> NoteResult<NoteContent> {
    let entry = vault.entry(id)?;
    let text = vault.read(&entry)?;
    Ok(content_of(entry, text))
}

fn content_of(entry: NoteEntry, text: String) -> NoteContent {
    let (title, summary) = describe(&text, &entry.stem);
    NoteContent {
        hash: content_hash(&text),
        entry,
        title,
        summary,
        text,
    }
}

/// 保存正文。`base_hash` 是打开时拿到的指纹：对不上说明期间被改过（agent 也在写记忆），
/// 回 [`NoteError::Conflict`]，由界面决定覆盖还是放弃；不给就直接覆盖。
pub fn save_note(
    vault: &MemoryVault,
    id: &str,
    text: &str,
    base_hash: Option<&str>,
) -> NoteResult<NoteContent> {
    let entry = writable_entry(vault, id)?;
    if let Some(expected) = base_hash {
        let actual = content_hash(&vault.read(&entry)?);
        if actual != expected {
            return Err(NoteError::Conflict {
                expected: expected.to_string(),
                actual,
            });
        }
    }
    let text = with_final_newline(text);
    check_size(&text)?;
    write_atomic(&entry.path, &text)?;
    settle(vault, &[], &[&entry]);
    read_note(vault, id)
}

/// 新建一篇。文件名取自标题（中文标题就是中文文件名）；重名自动加 `-2`、`-3`。
/// scope 的目录还没有（当前项目的第一篇记忆）会先建好。
pub fn create_note(
    vault: &MemoryVault,
    scope_id: &str,
    kind: NoteKind,
    title: &str,
    body: &str,
) -> NoteResult<NoteContent> {
    let title = single_line(title);
    if title.is_empty() {
        return Err(NoteError::Invalid("标题不能为空".into()));
    }
    let (_, paths) = vault.scope_paths(scope_id)?;
    std::fs::create_dir_all(vault.home())?;
    paths.ensure()?;
    let stem = Some(note_stem(&title, MAX_STEM_CHARS))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("note-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let path = free_path(kind_dir(&paths, kind), &stem, None);

    let body = body.trim();
    let starts_with_h1 = body.lines().next().is_some_and(|l| l.starts_with("# "));
    let text = if starts_with_h1 {
        with_final_newline(body)
    } else if body.is_empty() {
        format!("# {title}\n")
    } else {
        format!("# {title}\n\n{body}\n")
    };
    check_size(&text)?;
    write_atomic(&path, &text)?;
    let id = id_under(vault, &path)?;
    let entry = vault.entry(&id)?;
    settle(vault, &[], &[&entry]);
    read_note(vault, &id)
}

/// 改名 / 挪位置的结果。
#[derive(Debug, Clone)]
pub struct Relocated {
    pub note: NoteContent,
    /// 原来的 id（没换地方时和现在一样）。
    pub old_id: String,
    /// 链接被跟着改写了的其它笔记。
    pub relinked: Vec<String>,
}

/// 改标题和 / 或文件名。
///
/// - `title`：换掉正文的一级标题（没有就加在最前面）。
/// - `name`：换文件名（不含 `.md`，会规整成小写加 `-`）。指向这篇的 `[[链接]]` 跟着改写，
///   不会断。重名自动加后缀。
pub fn rename_note(
    vault: &MemoryVault,
    id: &str,
    title: Option<&str>,
    name: Option<&str>,
) -> NoteResult<Relocated> {
    let entry = writable_entry(vault, id)?;
    let text = match title {
        Some(title) => {
            let title = single_line(title);
            if title.is_empty() {
                return Err(NoteError::Invalid("标题不能为空".into()));
            }
            Some(set_title(&vault.read(&entry)?, &title))
        }
        None => None,
    };
    let stem = match name {
        Some(name) => {
            let stem = note_stem(name, MAX_STEM_CHARS);
            if stem.is_empty() {
                return Err(NoteError::Invalid(
                    "文件名里至少要有一个字母、数字或汉字".into(),
                ));
            }
            stem
        }
        None => entry.stem.clone(),
    };
    let (scope_id, kind) = (entry.scope_id.clone(), entry.kind);
    relocate(vault, entry, text, &scope_id, kind, &stem)
}

/// 挪到别的 scope 和 / 或换类别（观察升为主题）。文件名不变，目标处重名才加后缀；
/// 指向它的链接、它指向别人的链接都保持指向原来那几篇。
pub fn move_note(
    vault: &MemoryVault,
    id: &str,
    scope_id: Option<&str>,
    kind: Option<NoteKind>,
) -> NoteResult<Relocated> {
    let entry = writable_entry(vault, id)?;
    let scope_id = scope_id.unwrap_or(&entry.scope_id).to_string();
    let kind = kind.unwrap_or(entry.kind);
    let stem = entry.stem.clone();
    relocate(vault, entry, None, &scope_id, kind, &stem)
}

fn relocate(
    vault: &MemoryVault,
    entry: NoteEntry,
    new_text: Option<String>,
    dest_scope_id: &str,
    dest_kind: NoteKind,
    dest_stem: &str,
) -> NoteResult<Relocated> {
    let (dest_scope, dest_paths) = vault.scope_paths(dest_scope_id)?;
    let same_place = dest_scope_id == entry.scope_id && dest_kind == entry.kind;
    if !same_place {
        dest_paths.ensure()?;
    }
    let dest_path = free_path(
        kind_dir(&dest_paths, dest_kind),
        dest_stem,
        Some(&entry.path),
    );
    let moved = dest_path != entry.path;
    let text = match new_text {
        Some(text) => text,
        None if moved => vault.read(&entry)?,
        // 没换地方、也没换内容：什么都不用做。
        None => {
            return Ok(Relocated {
                old_id: entry.id.clone(),
                note: read_note(vault, &entry.id)?,
                relinked: Vec::new(),
            })
        }
    };

    // 链接是按名字找笔记的：动之前先把「谁指向谁」定下来，动完凡是指向变了的都改写回去。
    let before_notes = vault.load(&vault.scope_ids());
    let me = position(&before_notes, &entry.id)?;
    let before = LinkResolver::new(&before_notes);

    let dest_rel = dest_path
        .strip_prefix(&dest_paths.root)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .map_err(|_| NoteError::Invalid("目标路径不在记忆目录里".into()))?;
    let dest_id = format!("{dest_scope_id}/{dest_rel}");
    let mut after_notes = before_notes.clone();
    if let Some(slot) = after_notes.get_mut(me) {
        slot.entry.id = dest_id.clone();
        slot.entry.scope_id = dest_scope_id.to_string();
        slot.entry.scope = dest_scope;
        slot.entry.kind = dest_kind;
        slot.entry.stem = dest_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(dest_stem)
            .to_string();
        slot.entry.rel = dest_rel;
        slot.entry.path = dest_path.clone();
        slot.title = describe(&text, &slot.entry.stem).0;
    }
    let after = LinkResolver::new(&after_notes);
    // 一篇笔记（动之前在 `from`、动之后在 `to` 这个 scope）里的链接：原来指向谁，
    // 现在还得指向谁。原来就悬空的不管。
    let keep_pointing = |text: &str, from: &str, to: &str| {
        retarget_wikilinks(text, |link| {
            let target = before.resolve(from, &link.target)?;
            (after.resolve(to, &link.target) != Some(target))
                .then(|| after.target_for(to, target))
                .flatten()
        })
    };

    let (text, _) = keep_pointing(&text, &entry.scope_id, dest_scope_id);
    let text = with_final_newline(&text);
    check_size(&text)?;
    // 先在原处写好内容，再整个挪过去：只改大小写的改名、跨目录的移动都是一次 rename。
    write_atomic(&entry.path, &text)?;
    if moved {
        std::fs::rename(&entry.path, &dest_path)?;
    }

    let mut relinked: Vec<NoteEntry> = Vec::new();
    for (index, note) in before_notes.iter().enumerate() {
        // 只读的笔记不改（它指过来的链接会悬空）；只读了开头的大文件也不能拿来整篇写回。
        if index == me || !note.entry.writable || note.truncated {
            continue;
        }
        let scope = &note.entry.scope_id;
        let (rewritten, changed) = keep_pointing(&note.text, scope, scope);
        if changed == 0 {
            continue;
        }
        match write_atomic(&note.entry.path, &rewritten) {
            Ok(()) => relinked.push(note.entry.clone()),
            Err(e) => tracing::warn!(
                target: "dock_memory",
                note = %note.entry.id,
                error = %e,
                "failed to rewrite links after a note moved"
            ),
        }
    }

    let new_entry = vault.entry(&dest_id)?;
    let mut written: Vec<&NoteEntry> = relinked.iter().collect();
    written.push(&new_entry);
    let removed: Vec<&NoteEntry> = if moved { vec![&entry] } else { Vec::new() };
    settle(vault, &removed, &written);
    Ok(Relocated {
        note: read_note(vault, &dest_id)?,
        old_id: entry.id.clone(),
        relinked: relinked.into_iter().map(|e| e.id).collect(),
    })
}

/// 遗忘：挪进这个 scope 的 `archive/`、记墓碑、清检索条目（同 `/memory` 里按两次 `x`）。
/// `expected_hash` 是界面看到的那一版的指纹，对不上就不删。
pub fn forget_note(vault: &MemoryVault, id: &str, expected_hash: &str) -> NoteResult<ForgetResult> {
    let entry = writable_entry(vault, id)?;
    let (scope, paths) = vault.scope_paths(&entry.scope_id)?;
    // 手工建的 scope 目录可能没有归档目录和墓碑库：先补齐。
    paths.ensure()?;
    let mut index = MemoryIndex::open_or_create(&vault.search_db())?;
    Ok(forget_in_scope(
        &paths,
        scope,
        &entry.path,
        expected_hash,
        &mut index,
    )?)
}

/// 建链 / 断链的结果。
#[derive(Debug, Clone)]
pub struct LinkChange {
    pub note: NoteContent,
    /// 正文有没有变（已经链着再建一次、本来就没链却去断，都是 `false`）。
    pub changed: bool,
}

/// 在 `from` 里加一条指向 `to` 的链接（写进文末的「相关」一节）。
pub fn link_notes(vault: &MemoryVault, from: &str, to: &str) -> NoteResult<LinkChange> {
    let entry = writable_entry(vault, from)?;
    if from == to {
        return Err(NoteError::Invalid("不能链接到自己".into()));
    }
    let notes = vault.load(&vault.scope_ids());
    let resolver = LinkResolver::new(&notes);
    let target = position(&notes, to)?;
    let text = vault.read(&entry)?;
    let linked = parse_wikilinks(&text)
        .iter()
        .any(|l| resolver.resolve(&entry.scope_id, &l.target) == Some(target));
    if linked {
        return Ok(LinkChange {
            note: content_of(entry, text),
            changed: false,
        });
    }
    let name = resolver
        .target_for(&entry.scope_id, target)
        .filter(|name| is_linkable_name(name))
        .ok_or_else(|| {
            NoteError::Invalid("目标笔记的文件名里有不能写进链接的字符，先给它改个名".into())
        })?;
    let text = append_related_link(&text, &name);
    check_size(&text)?;
    write_atomic(&entry.path, &text)?;
    settle(vault, &[], &[&entry]);
    Ok(LinkChange {
        note: read_note(vault, from)?,
        changed: true,
    })
}

/// 断开的是哪条链接。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkTarget<'a> {
    /// 指向这篇笔记（id）的所有链接。
    Note(&'a str),
    /// 指向不存在的笔记的链接，按链接里写的名字（不分大小写）。
    Dangling(&'a str),
}

/// 去掉 `from` 里指向 `target` 的链接：「相关」里的整条删掉，句子里的只去方括号。
pub fn unlink_notes(
    vault: &MemoryVault,
    from: &str,
    target: LinkTarget<'_>,
) -> NoteResult<LinkChange> {
    let entry = writable_entry(vault, from)?;
    let notes = vault.load(&vault.scope_ids());
    let resolver = LinkResolver::new(&notes);
    let text = vault.read(&entry)?;
    let (text, removed) = match target {
        LinkTarget::Note(to) => {
            let to = position(&notes, to)?;
            remove_wikilinks(&text, |l| {
                resolver.resolve(&entry.scope_id, &l.target) == Some(to)
            })
        }
        LinkTarget::Dangling(name) => {
            let name = name.trim().to_lowercase();
            remove_wikilinks(&text, |l| {
                l.target.to_lowercase() == name
                    && resolver.resolve(&entry.scope_id, &l.target).is_none()
            })
        }
    };
    if removed == 0 {
        return Ok(LinkChange {
            note: content_of(entry, text),
            changed: false,
        });
    }
    write_atomic(&entry.path, &text)?;
    settle(vault, &[], &[&entry]);
    Ok(LinkChange {
        note: read_note(vault, from)?,
        changed: true,
    })
}

fn position(notes: &[LoadedNote], id: &str) -> NoteResult<usize> {
    notes
        .iter()
        .position(|n| n.entry.id == id)
        .ok_or_else(|| VaultError::NotFound(id.to_string()).into())
}

fn writable_entry(vault: &MemoryVault, id: &str) -> NoteResult<NoteEntry> {
    let entry = vault.entry(id)?;
    if !entry.writable {
        return Err(NoteError::ReadOnly(entry.id));
    }
    Ok(entry)
}

fn kind_dir(paths: &ScopePaths, kind: NoteKind) -> &Path {
    match kind {
        NoteKind::Topic => &paths.topics,
        NoteKind::Observation => &paths.inbox,
    }
}

fn id_under(vault: &MemoryVault, path: &Path) -> NoteResult<String> {
    path.strip_prefix(vault.home())
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .map_err(|_| NoteError::Invalid("路径不在记忆目录里".into()))
}

fn check_size(text: &str) -> NoteResult<()> {
    if text.len() as u64 > MAX_MEMORY_READ_BYTES {
        return Err(NoteError::TooLarge {
            size: text.len(),
            limit: MAX_MEMORY_READ_BYTES,
        });
    }
    Ok(())
}

fn with_final_newline(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

/// 标题只占一行：换行和多余的空白压成一个空格。
fn single_line(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 换掉正文开头的一级标题；开头不是一级标题就加一行。
fn set_title(text: &str, title: &str) -> String {
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let body = line.trim();
        if body.is_empty() || body.starts_with("<!--") {
            offset += line.len();
            continue;
        }
        if body.starts_with("# ") {
            let ending = line
                .get(line.trim_end_matches(['\n', '\r']).len()..)
                .unwrap_or("");
            let mut out = String::with_capacity(text.len() + title.len());
            out.push_str(text.get(..offset).unwrap_or(""));
            out.push_str("# ");
            out.push_str(title);
            out.push_str(ending);
            out.push_str(text.get(offset + line.len()..).unwrap_or(""));
            return out;
        }
        break;
    }
    if text.trim().is_empty() {
        return format!("# {title}\n");
    }
    format!("# {title}\n\n{}", text.trim_start_matches(['\n', '\r']))
}

/// `dir` 下一个没被占用的 `<stem>.md`：重名就试 `-2`、`-3`……`own` 是正在改名的那个
/// 文件自己，它占着不算重名（只改大小写时，大小写不敏感的文件系统会说「已存在」）。
fn free_path(dir: &Path, stem: &str, own: Option<&Path>) -> PathBuf {
    let taken = |candidate: &Path| {
        candidate.symlink_metadata().is_ok() && !own.is_some_and(|own| same_file(own, candidate))
    };
    let first = dir.join(format!("{stem}.md"));
    if !taken(&first) {
        return first;
    }
    (2..10_000)
        .map(|n| dir.join(format!("{stem}-{n}.md")))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| dir.join(format!("{stem}-{}.md", std::process::id())))
}

fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        matches!(
            (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)),
            (Ok(x), Ok(y)) if x.dev() == y.dev() && x.ino() == y.ino()
        )
    }
    #[cfg(not(unix))]
    {
        matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
    }
}

/// 同目录临时文件写好再换上去：读的人要么看到旧的、要么看到新的。临时文件以 `.` 开头、
/// 以 `.tmp` 结尾，笔记清单和文件监听都不认它。
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".note-")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    tmp.write_all(text.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// 善后：检索库 + 各 scope 的 `MEMORY.md`。失败只记日志（见模块说明）。
fn settle(vault: &MemoryVault, removed: &[&NoteEntry], written: &[&NoteEntry]) {
    let db = vault.search_db();
    if db.is_file() {
        let synced = MemoryIndex::open_or_create(&db).and_then(|mut index| {
            for entry in removed {
                index.delete_path(&entry.path)?;
            }
            for entry in written {
                index.reindex_file(&entry.path, source_label(entry.scope))?;
            }
            Ok(())
        });
        if let Err(e) = synced {
            tracing::warn!(target: "dock_memory", error = %e, "failed to sync the search index");
        }
    }
    let mut scopes: Vec<&str> = removed
        .iter()
        .chain(written)
        .map(|e| e.scope_id.as_str())
        .collect();
    scopes.sort_unstable();
    scopes.dedup();
    for scope_id in scopes {
        let Ok((scope, paths)) = vault.scope_paths(scope_id) else {
            continue;
        };
        if let Err(e) = crate::manifest::regenerate_scope(&paths, scope) {
            tracing::warn!(target: "dock_memory", scope = scope_id, error = %e, "failed to refresh MEMORY.md");
        }
    }
}

/// 检索库里的来源标签（和 [`crate::MemoryRoot::classify_source`] 一致）。
fn source_label(scope: MemoryScope) -> &'static str {
    match scope {
        MemoryScope::Global => "global",
        MemoryScope::Workspace => "workspace",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "workspace-a-00000001";
    const B: &str = "workspace-b-00000002";

    fn vault_with(files: &[(&str, &str)]) -> (tempfile::TempDir, MemoryVault) {
        let tmp = tempfile::tempdir().unwrap();
        let vault = MemoryVault::new(tmp.path());
        for (rel, body) in files {
            let path = vault.home().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        (tmp, vault)
    }

    fn text(vault: &MemoryVault, rel: &str) -> String {
        std::fs::read_to_string(vault.home().join(rel)).unwrap()
    }

    fn searchable(vault: &MemoryVault, query: &str) -> bool {
        !MemoryIndex::open_or_create(&vault.search_db())
            .unwrap()
            .search(query, 5)
            .unwrap()
            .is_empty()
    }

    #[test]
    fn save_writes_refreshes_the_manifest_and_detects_conflicts() {
        let (_tmp, vault) = vault_with(&[("global/topics/prefs.md", "# 偏好\n\n旧内容\n")]);
        let id = "global/topics/prefs.md";
        let opened = read_note(&vault, id).unwrap();
        assert_eq!(opened.title, "偏好");
        assert_eq!(opened.hash, content_hash("# 偏好\n\n旧内容\n"));

        let saved = save_note(
            &vault,
            id,
            "# 提交偏好\n\n用 conventional commits",
            Some(&opened.hash),
        )
        .unwrap();
        assert_eq!(
            saved.text, "# 提交偏好\n\n用 conventional commits\n",
            "补上文末换行"
        );
        assert_eq!(text(&vault, "global/topics/prefs.md"), saved.text);
        assert!(text(&vault, "global/MEMORY.md").contains("提交偏好"));
        // 没建过检索库：保存不该顺手建一个（见模块说明）。
        assert!(!vault.search_db().exists());

        // 拿着旧指纹再存：期间被改过。
        let stale = save_note(&vault, id, "覆盖", Some(&opened.hash));
        assert!(
            matches!(stale, Err(NoteError::Conflict { .. })),
            "{stale:?}"
        );
        assert_eq!(text(&vault, "global/topics/prefs.md"), saved.text);
        // 不带指纹：直接覆盖。
        save_note(&vault, id, "# 覆盖\n", None).unwrap();
        assert_eq!(text(&vault, "global/topics/prefs.md"), "# 覆盖\n");
    }

    #[test]
    fn save_keeps_an_existing_search_index_in_sync() {
        let (_tmp, vault) = vault_with(&[("global/topics/prefs.md", "# 偏好\n\nzebra crossing\n")]);
        let path = vault.home().join("global/topics/prefs.md");
        MemoryIndex::open_or_create(&vault.search_db())
            .unwrap()
            .reindex_file(&path, "global")
            .unwrap();
        assert!(searchable(&vault, "zebra"));
        save_note(
            &vault,
            "global/topics/prefs.md",
            "# 偏好\n\ngiraffe enclosure\n",
            None,
        )
        .unwrap();
        assert!(searchable(&vault, "giraffe"));
        assert!(!searchable(&vault, "zebra"));
    }

    #[test]
    fn writes_are_refused_outside_the_writable_notes() {
        let big = "x".repeat(MAX_MEMORY_READ_BYTES as usize + 1);
        let (_tmp, vault) = vault_with(&[
            ("global/topics/ok.md", "# ok\n"),
            ("global/topics/nested/deep.md", "# deep\n"),
            ("global/observations/legacy.md", "# legacy\n"),
            ("global/archive/gone.md", "# gone\n"),
        ]);
        for id in [
            "global/topics/nested/deep.md",
            "global/observations/legacy.md",
        ] {
            assert!(
                matches!(
                    save_note(&vault, id, "x", None),
                    Err(NoteError::ReadOnly(_))
                ),
                "{id}"
            );
            assert!(matches!(
                rename_note(&vault, id, Some("新标题"), None),
                Err(NoteError::ReadOnly(_))
            ));
            assert!(matches!(
                forget_note(&vault, id, "whatever"),
                Err(NoteError::ReadOnly(_))
            ));
        }
        assert!(matches!(
            save_note(&vault, "global/archive/gone.md", "x", None),
            Err(NoteError::Vault(VaultError::InvalidId(_)))
        ));
        assert!(matches!(
            save_note(&vault, "global/topics/../MEMORY.md", "x", None),
            Err(NoteError::Vault(VaultError::InvalidId(_)))
        ));
        assert!(matches!(
            save_note(&vault, "global/topics/ok.md", &big, None),
            Err(NoteError::TooLarge { .. })
        ));
        assert_eq!(text(&vault, "global/topics/ok.md"), "# ok\n");
    }

    #[test]
    fn create_names_the_file_after_the_title() {
        let (_tmp, vault) = vault_with(&[("global/topics/prefs.md", "# 偏好\n")]);
        let note = create_note(
            &vault,
            "global",
            NoteKind::Topic,
            " 审计日志  改造 ",
            "先降噪。",
        )
        .unwrap();
        assert_eq!(note.entry.id, "global/topics/审计日志-改造.md");
        assert_eq!(note.text, "# 审计日志 改造\n\n先降噪。\n");
        assert!(note.entry.writable);
        // 重名加后缀；正文自带一级标题就不再加。
        let again = create_note(
            &vault,
            "global",
            NoteKind::Topic,
            "审计日志 改造",
            "# 另一篇\n\n正文",
        )
        .unwrap();
        assert_eq!(again.entry.id, "global/topics/审计日志-改造-2.md");
        assert_eq!(again.text, "# 另一篇\n\n正文\n");
        // 观察放进 _inbox；空正文只有标题。
        let obs = create_note(&vault, "global", NoteKind::Observation, "Clash 代理", "").unwrap();
        assert_eq!(obs.entry.id, "global/observations/_inbox/clash-代理.md");
        assert_eq!(obs.text, "# Clash 代理\n");
        assert!(text(&vault, "global/MEMORY.md").contains("审计日志 改造"));
        // 标题全是符号：起一个带时间的名字，标题照留。
        let odd = create_note(&vault, "global", NoteKind::Topic, "？！", "").unwrap();
        assert!(odd.entry.stem.starts_with("note-"), "{}", odd.entry.stem);
        assert_eq!(odd.title, "？！");
        assert!(matches!(
            create_note(&vault, "global", NoteKind::Topic, "  \n ", "x"),
            Err(NoteError::Invalid(_))
        ));
    }

    #[test]
    fn create_lays_out_a_scope_that_has_no_memory_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = MemoryVault::new(tmp.path());
        let note = create_note(&vault, A, NoteKind::Topic, "第一篇", "正文").unwrap();
        assert_eq!(note.entry.id, format!("{A}/topics/第一篇.md"));
        let scope = vault.home().join(A);
        assert!(scope.join("observations/_inbox").is_dir());
        assert!(scope.join("archive").is_dir());
        assert!(scope.join("memory_state.sqlite").is_file());
        assert!(text(&vault, &format!("{A}/MEMORY.md")).contains("第一篇"));
        // scope 名字不对：不建任何东西。
        for bad in ["../escape", "workspace-", "elsewhere", "workspace-a/b"] {
            assert!(
                matches!(
                    create_note(&vault, bad, NoteKind::Topic, "x", ""),
                    Err(NoteError::Vault(VaultError::InvalidScope(_)))
                ),
                "{bad}"
            );
        }
        assert!(!tmp.path().join("escape").exists());
    }

    #[test]
    fn rename_title_only_touches_the_heading() {
        let (_tmp, vault) = vault_with(&[
            ("global/topics/auth.md", "\n# 登录\r\n\r\n正文\r\n"),
            (
                "global/observations/_inbox/flush-20261115-131605.md",
                "## Decisions\n- 用 JWT\n",
            ),
            ("global/topics/other.md", "见 [[auth]]\n"),
            (
                "global/topics/by-title.md",
                "按标题链的 [[登录|登录那篇]]\n",
            ),
        ]);
        let renamed =
            rename_note(&vault, "global/topics/auth.md", Some("登录  方案"), None).unwrap();
        assert_eq!(
            renamed.note.entry.id, "global/topics/auth.md",
            "没给文件名就不动文件"
        );
        assert_eq!(
            renamed.note.text, "\n# 登录 方案\r\n\r\n正文\r\n",
            "只换标题那一行，行尾不动"
        );
        assert_eq!(text(&vault, "global/topics/other.md"), "见 [[auth]]\n");
        // 按旧标题写的链接：标题换了就对不上了，改成按文件名链，不让它断。
        assert_eq!(renamed.relinked, vec!["global/topics/by-title.md"]);
        assert_eq!(
            text(&vault, "global/topics/by-title.md"),
            "按标题链的 [[auth|登录那篇]]\n"
        );
        // flush 摘要没有一级标题：加在最前面。
        let flush = "global/observations/_inbox/flush-20261115-131605.md";
        let titled = rename_note(&vault, flush, Some("登录方案的决定"), None).unwrap();
        assert_eq!(
            titled.note.text,
            "# 登录方案的决定\n\n## Decisions\n- 用 JWT\n"
        );
        assert_eq!(titled.note.title, "登录方案的决定");
        assert!(text(&vault, "global/MEMORY.md").contains("登录方案的决定"));
    }

    #[test]
    fn rename_file_rewrites_links_everywhere() {
        let (_tmp, vault) = vault_with(&[
            ("global/topics/auth.md", "# 登录\n\n自己提一下 [[auth]]。\n"),
            (
                "global/topics/other.md",
                "见 [[auth]]、[[Auth#刷新|刷新那节]] 和 [[global/topics/auth]]，代码里的 `[[auth]]` 不动。\n",
            ),
            ("workspace-a-00000001/topics/uses.md", "跨项目引用 [[auth]] 和 [[prefs]]。\n"),
            ("workspace-a-00000001/topics/nested/ro.md", "只读的 [[auth]]\n"),
            ("global/topics/prefs.md", "# 偏好\n"),
        ]);
        MemoryIndex::open_or_create(&vault.search_db())
            .unwrap()
            .reindex_file(&vault.home().join("global/topics/auth.md"), "global")
            .unwrap();

        let renamed =
            rename_note(&vault, "global/topics/auth.md", None, Some("Login Flow")).unwrap();
        assert_eq!(renamed.old_id, "global/topics/auth.md");
        assert_eq!(renamed.note.entry.id, "global/topics/login-flow.md");
        assert!(!vault.home().join("global/topics/auth.md").exists());
        assert_eq!(renamed.note.text, "# 登录\n\n自己提一下 [[login-flow]]。\n");
        let mut relinked = renamed.relinked.clone();
        relinked.sort();
        assert_eq!(
            relinked,
            vec![
                "global/topics/other.md",
                "workspace-a-00000001/topics/uses.md"
            ]
        );
        assert_eq!(
            text(&vault, "global/topics/other.md"),
            "见 [[login-flow]]、[[login-flow#刷新|刷新那节]] 和 [[login-flow]]，代码里的 `[[auth]]` 不动。\n"
        );
        assert_eq!(
            text(&vault, "workspace-a-00000001/topics/uses.md"),
            "跨项目引用 [[login-flow]] 和 [[prefs]]。\n"
        );
        // 只读的笔记不改（它的链接成了悬空的）。
        assert_eq!(
            text(&vault, "workspace-a-00000001/topics/nested/ro.md"),
            "只读的 [[auth]]\n"
        );
        // 检索库跟着走：旧路径的条目没了，新路径的在。
        let index = MemoryIndex::open_or_create(&vault.search_db()).unwrap();
        let paths: Vec<String> = index
            .search("登录", 5)
            .unwrap()
            .into_iter()
            .map(|h| h.path)
            .collect();
        assert!(paths.iter().all(|p| !p.ends_with("auth.md")), "{paths:?}");
        assert!(
            paths.iter().any(|p| p.ends_with("login-flow.md")),
            "{paths:?}"
        );
        let manifest = text(&vault, "global/MEMORY.md");
        assert!(manifest.contains("topics/login-flow.md") && !manifest.contains("topics/auth.md"));
    }

    #[test]
    fn rename_handles_collisions_and_case_only_changes() {
        let (_tmp, vault) = vault_with(&[
            ("global/topics/auth.md", "# 登录\n"),
            ("global/topics/taken.md", "# 占着\n"),
        ]);
        // 目标名被占：加后缀，不覆盖。
        let moved = rename_note(&vault, "global/topics/auth.md", None, Some("taken")).unwrap();
        assert_eq!(moved.note.entry.id, "global/topics/taken-2.md");
        assert_eq!(text(&vault, "global/topics/taken.md"), "# 占着\n");
        // 名字规整后和原来一样：什么都不动。
        let same = rename_note(&vault, "global/topics/taken-2.md", None, Some("Taken 2")).unwrap();
        assert_eq!(same.note.entry.id, "global/topics/taken-2.md");
        assert_eq!(same.old_id, same.note.entry.id);
        assert!(matches!(
            rename_note(&vault, "global/topics/taken.md", None, Some("///")),
            Err(NoteError::Invalid(_))
        ));
    }

    #[test]
    fn promote_moves_an_observation_into_topics() {
        let (_tmp, vault) = vault_with(&[
            (
                "workspace-a-00000001/observations/_inbox/audit.md",
                "# 审计\n\n见 [[auth]]\n",
            ),
            (
                "workspace-a-00000001/topics/auth.md",
                "# 登录\n\n反过来 [[audit]]\n",
            ),
        ]);
        let promoted = move_note(
            &vault,
            "workspace-a-00000001/observations/_inbox/audit.md",
            None,
            Some(NoteKind::Topic),
        )
        .unwrap();
        assert_eq!(
            promoted.note.entry.id,
            "workspace-a-00000001/topics/audit.md"
        );
        assert_eq!(promoted.note.entry.kind, NoteKind::Topic);
        // 文件名没变，指向它的链接本来就还对得上：不用改写。
        assert!(promoted.relinked.is_empty());
        assert_eq!(
            text(&vault, "workspace-a-00000001/topics/auth.md"),
            "# 登录\n\n反过来 [[audit]]\n"
        );
        let manifest = text(&vault, "workspace-a-00000001/MEMORY.md");
        assert!(manifest.contains("topics/audit.md") && !manifest.contains("_inbox/audit.md"));
        // 原地不动：不报错、id 不变。
        let noop = move_note(&vault, &promoted.note.entry.id, None, Some(NoteKind::Topic)).unwrap();
        assert_eq!(noop.note.entry.id, noop.old_id);
    }

    #[test]
    fn moving_across_scopes_keeps_every_link_pointing_at_the_same_note() {
        let (_tmp, vault) = vault_with(&[
            // A 里的笔记链着 A 自己的 auth；全局也有一篇同名的 auth。
            (
                "workspace-a-00000001/topics/plan.md",
                "# 计划\n\n依赖 [[auth]] 和 [[prefs]]。\n",
            ),
            (
                "workspace-a-00000001/topics/auth.md",
                "# A 的登录\n\n回链 [[plan]]\n",
            ),
            ("global/topics/auth.md", "# 全局登录\n"),
            ("global/topics/prefs.md", "# 偏好\n"),
            ("global/topics/plan.md", "# 全局已有一篇 plan\n"),
            (
                "workspace-b-00000002/topics/b.md",
                "B 引用 [[workspace-a-00000001/topics/plan]]\n",
            ),
        ]);
        let moved = move_note(
            &vault,
            "workspace-a-00000001/topics/plan.md",
            Some("global"),
            None,
        )
        .unwrap();
        // 全局已有 plan.md：加后缀。
        assert_eq!(moved.note.entry.id, "global/topics/plan-2.md");
        // 到了全局，`[[auth]]` 会指到全局那篇——改成全路径，仍然指 A 的；prefs 不受影响。
        assert_eq!(
            moved.note.text,
            "# 计划\n\n依赖 [[workspace-a-00000001/topics/auth]] 和 [[prefs]]。\n"
        );
        let mut relinked = moved.relinked.clone();
        relinked.sort();
        assert_eq!(
            relinked,
            vec![
                "workspace-a-00000001/topics/auth.md",
                "workspace-b-00000002/topics/b.md"
            ]
        );
        assert_eq!(
            text(&vault, "workspace-a-00000001/topics/auth.md"),
            "# A 的登录\n\n回链 [[plan-2]]\n"
        );
        assert_eq!(
            text(&vault, "workspace-b-00000002/topics/b.md"),
            "B 引用 [[plan-2]]\n"
        );
        // 图上核对：挪完以后谁指向谁和挪之前一样。
        let notes = vault.load(&vault.scope_ids());
        let (edges, unresolved, _) = crate::graph::connect(
            &notes,
            &crate::similarity::SimilarConfig {
                per_note: 0,
                ..Default::default()
            },
        );
        let mut pairs: Vec<(&str, &str)> = edges
            .iter()
            .map(|e| (e.source.as_str(), e.target.as_str()))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                ("global/topics/plan-2.md", "global/topics/prefs.md"),
                (
                    "global/topics/plan-2.md",
                    "workspace-a-00000001/topics/auth.md"
                ),
                (
                    "workspace-a-00000001/topics/auth.md",
                    "global/topics/plan-2.md"
                ),
                (
                    "workspace-b-00000002/topics/b.md",
                    "global/topics/plan-2.md"
                ),
            ]
        );
        assert!(unresolved.is_empty(), "{unresolved:?}");
    }

    #[test]
    fn forget_archives_the_note_and_checks_the_hash() {
        let (_tmp, vault) = vault_with(&[(
            "workspace-b-00000002/topics/bye.md",
            "# 再见\n\nforget this\n",
        )]);
        // 手工放进去的一篇：这个 scope 还没有归档目录和墓碑库。
        let id = "workspace-b-00000002/topics/bye.md";
        let note = read_note(&vault, id).unwrap();
        MemoryIndex::open_or_create(&vault.search_db())
            .unwrap()
            .reindex_file(&note.entry.path, "workspace")
            .unwrap();

        let wrong = forget_note(&vault, id, "0000");
        assert!(matches!(wrong, Err(NoteError::Access(_))), "{wrong:?}");
        assert!(note.entry.path.exists());

        let forgotten = forget_note(&vault, id, &note.hash).unwrap();
        assert!(!note.entry.path.exists());
        let archived = forgotten.archived_to.unwrap();
        assert!(archived.starts_with(vault.home().join(B).join("archive")));
        assert!(!searchable(&vault, "forget this"));
        assert!(matches!(
            read_note(&vault, id),
            Err(NoteError::Vault(VaultError::NotFound(_)))
        ));
    }

    #[test]
    fn link_appends_once_and_unlink_removes_it() {
        let (_tmp, vault) = vault_with(&[
            ("global/topics/auth.md", "# 登录\n\n正文\n"),
            ("global/topics/db.md", "# 数据库\n"),
        ]);
        let (auth, db) = ("global/topics/auth.md", "global/topics/db.md");
        let linked = link_notes(&vault, auth, db).unwrap();
        assert!(linked.changed);
        assert_eq!(linked.note.text, "# 登录\n\n正文\n\n## 相关\n\n- [[db]]\n");
        // 已经链着：不重复加。
        let again = link_notes(&vault, auth, db).unwrap();
        assert!(!again.changed);
        assert_eq!(again.note.text, linked.note.text);
        assert!(matches!(
            link_notes(&vault, auth, auth),
            Err(NoteError::Invalid(_))
        ));
        assert!(matches!(
            link_notes(&vault, auth, "global/topics/missing.md"),
            Err(NoteError::Vault(VaultError::NotFound(_)))
        ));

        let unlinked = unlink_notes(&vault, auth, LinkTarget::Note(db)).unwrap();
        assert!(unlinked.changed);
        assert_eq!(
            unlinked.note.text, "# 登录\n\n正文\n",
            "「相关」一节空了，连标题一起去掉"
        );
        assert!(
            !unlink_notes(&vault, auth, LinkTarget::Note(db))
                .unwrap()
                .changed
        );
    }

    #[test]
    fn link_spells_out_the_path_when_the_name_is_shadowed() {
        let (_tmp, vault) = vault_with(&[
            ("workspace-a-00000001/topics/plan.md", "# 计划\n"),
            ("workspace-a-00000001/topics/auth.md", "# A 的登录\n"),
            ("global/topics/auth.md", "# 全局登录\n"),
        ]);
        let plan = "workspace-a-00000001/topics/plan.md";
        let linked = link_notes(&vault, plan, "global/topics/auth.md").unwrap();
        assert_eq!(
            linked.note.text,
            "# 计划\n\n## 相关\n\n- [[global/topics/auth]]\n"
        );
        // 再链 A 自己的那篇：是另一篇笔记，照加。
        let both = link_notes(&vault, plan, "workspace-a-00000001/topics/auth.md").unwrap();
        assert!(both.changed);
        assert!(both
            .note
            .text
            .ends_with("- [[global/topics/auth]]\n- [[auth]]\n"));
        // 只断全局那条。
        let one = unlink_notes(&vault, plan, LinkTarget::Note("global/topics/auth.md")).unwrap();
        assert_eq!(one.note.text, "# 计划\n\n## 相关\n\n- [[auth]]\n");
    }

    #[test]
    fn unlink_can_drop_a_dangling_link_by_name() {
        let (_tmp, vault) = vault_with(&[
            (
                "global/topics/auth.md",
                "# 登录\n\n见 [[没写的笔记]] 和 [[db]]。\n\n## 相关\n\n- [[没写的笔记|以后写]]\n",
            ),
            ("global/topics/db.md", "# 数据库\n"),
        ]);
        let auth = "global/topics/auth.md";
        // 按名字断的只能是悬空链接：db 存在，不会被当成悬空的断掉。
        assert!(
            !unlink_notes(&vault, auth, LinkTarget::Dangling("db"))
                .unwrap()
                .changed
        );
        let cleaned = unlink_notes(&vault, auth, LinkTarget::Dangling(" 没写的笔记 ")).unwrap();
        assert!(cleaned.changed);
        assert_eq!(cleaned.note.text, "# 登录\n\n见 没写的笔记 和 [[db]]。\n");
    }
}
