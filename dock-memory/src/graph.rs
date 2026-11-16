//! 记忆图谱：笔记是点，`[[链接]]` 是边，内容相近的是「建议」边。
//!
//! 只读、不建目录、不碰索引库：打开图谱页不该在磁盘上留下任何东西
//! （和 `/memory` 浏览器同一条规矩）。

use std::collections::HashSet;

use crate::layout::MemoryScope;
use crate::links::parse_wikilinks;
use crate::similarity::{similar_pairs, SimilarConfig};
use crate::vault::{LinkResolver, LoadedNote, MemoryVault, GLOBAL_SCOPE_ID};

/// 图上放哪些 scope。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ScopeSelection {
    /// 全局 + 当前工作区（一个会话看得到的那些）。
    #[default]
    Current,
    /// 磁盘上所有 scope。
    All,
    /// 点名的几个（不存在的忽略）。
    Only(Vec<String>),
}

#[derive(Debug, Clone, Default)]
pub struct GraphQuery {
    /// 当前工作区的 scope id（`workspace-<slug>`）。没有会话时是 `None`。
    pub current_workspace: Option<String>,
    pub selection: ScopeSelection,
    pub similar: SimilarConfig,
}

/// 一个 scope 的概况。没选进图里的也列出来（界面的范围切换要用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeSummary {
    pub id: String,
    pub scope: MemoryScope,
    /// 是不是当前工作区。当前工作区还没有任何记忆时也会列出来（两个数都是 0）。
    pub current: bool,
    /// 这个 scope 的笔记在不在图里。
    pub selected: bool,
    pub topics: usize,
    pub observations: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    /// 正文里写了 `[[目标]]`。有方向：`source` 引用 `target`。
    Link,
    /// 内容相近，还没有链接。没有方向（`source` 的 id 排在前）。
    Similar,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Link => "link",
            Self::Similar => "similar",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub kind: EdgeKind,
    /// 相似度（0–1）；链接恒为 1。
    pub score: f32,
}

/// 指向不存在的笔记的链接（Obsidian 里灰色的那种点）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedLink {
    pub source: String,
    /// 链接里写的目标，原样。
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct MemoryGraph {
    pub scopes: Vec<ScopeSummary>,
    pub notes: Vec<LoadedNote>,
    pub edges: Vec<GraphEdge>,
    pub unresolved: Vec<UnresolvedLink>,
    /// 笔记太多，没算相似建议。
    pub similar_skipped: bool,
}

pub fn build_graph(vault: &MemoryVault, query: &GraphQuery) -> MemoryGraph {
    let on_disk = vault.scope_ids();
    let current = query.current_workspace.as_deref();

    // 当前工作区就算磁盘上还没有，也列出来：界面要能往里新建第一篇。
    let mut scope_ids = on_disk.clone();
    if let Some(current) = current {
        if !scope_ids.iter().any(|id| id == current) && vault.scope_paths(current).is_ok() {
            scope_ids.push(current.to_string());
        }
    }
    if !scope_ids.iter().any(|id| id == GLOBAL_SCOPE_ID) {
        scope_ids.insert(0, GLOBAL_SCOPE_ID.to_string());
    }

    let selected: Vec<String> = scope_ids
        .iter()
        .filter(|id| match &query.selection {
            ScopeSelection::All => true,
            ScopeSelection::Current => *id == GLOBAL_SCOPE_ID || Some(id.as_str()) == current,
            ScopeSelection::Only(ids) => ids.contains(id),
        })
        .cloned()
        .collect();

    let scopes = scope_ids
        .iter()
        .filter_map(|id| {
            let (scope, _) = vault.scope_paths(id).ok()?;
            let entries = vault.list_scope(id);
            let topics = entries
                .iter()
                .filter(|e| e.kind == crate::vault::NoteKind::Topic)
                .count();
            Some(ScopeSummary {
                id: id.clone(),
                scope,
                current: Some(id.as_str()) == current,
                selected: selected.contains(id),
                topics,
                observations: entries.len() - topics,
            })
        })
        .collect();

    let notes = vault.load(&selected);
    let (edges, unresolved, similar_skipped) = connect(&notes, &query.similar);
    MemoryGraph {
        scopes,
        notes,
        edges,
        unresolved,
        similar_skipped,
    }
}

/// 一组笔记之间的边：`(边, 悬空链接, 是否跳过了相似度)`。
pub fn connect(
    notes: &[LoadedNote],
    similar: &SimilarConfig,
) -> (Vec<GraphEdge>, Vec<UnresolvedLink>, bool) {
    let resolver = LinkResolver::new(notes);
    let mut edges = Vec::new();
    let mut unresolved = Vec::new();
    let mut linked: HashSet<(usize, usize)> = HashSet::new();
    for (index, note) in notes.iter().enumerate() {
        let mut dangling: HashSet<String> = HashSet::new();
        for link in parse_wikilinks(&note.text) {
            match resolver.resolve(&note.entry.scope_id, &link.target) {
                // 指向自己的不画。
                Some(target) if target == index => {}
                Some(target) => {
                    // 同一篇里链了多次只算一条边。
                    if linked.insert((index, target)) {
                        if let Some(to) = notes.get(target) {
                            edges.push(GraphEdge {
                                source: note.entry.id.clone(),
                                target: to.entry.id.clone(),
                                kind: EdgeKind::Link,
                                score: 1.0,
                            });
                        }
                    }
                }
                None => {
                    if dangling.insert(link.target.to_lowercase()) {
                        unresolved.push(UnresolvedLink {
                            source: note.entry.id.clone(),
                            target: link.target,
                        });
                    }
                }
            }
        }
    }

    let texts: Vec<&str> = notes.iter().map(|n| n.text.as_str()).collect();
    let pairs = similar_pairs(&texts, similar);
    let similar_skipped = pairs.is_none();
    for pair in pairs.unwrap_or_default() {
        // 已经链上的（不管哪个方向）不用再建议。
        if linked.contains(&(pair.a, pair.b)) || linked.contains(&(pair.b, pair.a)) {
            continue;
        }
        let (Some(a), Some(b)) = (notes.get(pair.a), notes.get(pair.b)) else {
            continue;
        };
        let (source, target) = if a.entry.id <= b.entry.id {
            (a, b)
        } else {
            (b, a)
        };
        edges.push(GraphEdge {
            source: source.entry.id.clone(),
            target: target.entry.id.clone(),
            kind: EdgeKind::Similar,
            score: pair.score,
        });
    }
    (edges, unresolved, similar_skipped)
}

/// 一篇笔记的链接情况：它引用了谁、谁引用了它（带那一行原文）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoteLinks {
    pub outgoing: Vec<LinkedNote>,
    pub backlinks: Vec<LinkedNote>,
    /// 指向不存在的笔记的目标名。
    pub unresolved: Vec<String>,
    /// 这篇正文里每一种链接写法指向谁（界面把正文里的 `[[…]]` 画成可点的链接用）。
    pub targets: Vec<LinkTargetView>,
}

/// 正文里的一种链接写法（`[[target]]` 的 `target`，原样）和它指向的笔记。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTargetView {
    pub target: String,
    /// 指向的笔记 id；`None` = 悬空，或指向这篇自己以外找不到的笔记。
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedNote {
    pub id: String,
    pub title: String,
    /// 链接所在的那一行（去掉首尾空白）。
    pub context: String,
    /// 行号，从 1 起（在写了链接的那篇里）。
    pub line: usize,
}

/// `id` 这篇在 `notes` 里的出链与反链。`notes` 里没有这篇回空。
pub fn note_links(notes: &[LoadedNote], id: &str) -> NoteLinks {
    let Some(me) = notes.iter().position(|n| n.entry.id == id) else {
        return NoteLinks::default();
    };
    let resolver = LinkResolver::new(notes);
    let mut out = NoteLinks::default();
    let mut seen: HashSet<(bool, &str)> = HashSet::new();
    let mut dangling: HashSet<String> = HashSet::new();
    for (index, note) in notes.iter().enumerate() {
        for link in parse_wikilinks(&note.text) {
            let resolved = resolver.resolve(&note.entry.scope_id, &link.target);
            let context = || {
                note.text
                    .lines()
                    .nth(link.line.saturating_sub(1))
                    .unwrap_or("")
                    .trim()
                    .to_string()
            };
            if index == me && !out.targets.iter().any(|t| t.target == link.target) {
                out.targets.push(LinkTargetView {
                    target: link.target.clone(),
                    id: resolved
                        .and_then(|target| notes.get(target))
                        .map(|n| n.entry.id.clone()),
                });
            }
            let outgoing = resolved.filter(|target| index == me && *target != me);
            let incoming = resolved == Some(me) && index != me;
            if let Some(to) = outgoing.and_then(|target| notes.get(target)) {
                // 同一篇链了多次只列一次（取第一次出现的那一行）。
                if seen.insert((true, to.entry.id.as_str())) {
                    out.outgoing.push(LinkedNote {
                        id: to.entry.id.clone(),
                        title: to.title.clone(),
                        context: context(),
                        line: link.line,
                    });
                }
            } else if incoming && seen.insert((false, note.entry.id.as_str())) {
                out.backlinks.push(LinkedNote {
                    id: note.entry.id.clone(),
                    title: note.title.clone(),
                    context: context(),
                    line: link.line,
                });
            } else if resolved.is_none()
                && index == me
                && dangling.insert(link.target.to_lowercase())
            {
                out.unresolved.push(link.target.clone());
            }
        }
    }
    out
}

/// 按内容找笔记的一条结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteHit {
    pub id: String,
    /// 命中的那一行，从 1 起。
    pub line: usize,
    /// 那一行的原文（去掉首尾空白，过长截断）。
    pub snippet: String,
}

const MAX_SNIPPET_CHARS: usize = 160;

/// 在一组笔记里按内容找：查询按空白切成几个词，每个词都要在标题或正文里出现
/// （不分大小写，和 `/memory` 里按 `/` 过滤同一个规则）。标题命中的排前面，其余按
/// 最近修改排。
///
/// 直接扫正文，不走检索库：图谱页要的是「眼前这些笔记里哪几篇提到了它」，范围就是
/// 已经读出来的这一组，也不该因为搜一下就在磁盘上建库。
pub fn search_notes(notes: &[LoadedNote], query: &str, limit: usize) -> Vec<NoteHit> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut hits: Vec<(bool, u64, NoteHit)> = Vec::new();
    for note in notes {
        let title = note.title.to_lowercase();
        let body = note.text.to_lowercase();
        if !terms.iter().all(|t| title.contains(t) || body.contains(t)) {
            continue;
        }
        // 取第一行含有任意一个词的；都只在标题里就取第一行有字的。
        let (line, snippet) = note
            .text
            .lines()
            .enumerate()
            .find(|(_, l)| {
                let lowered = l.to_lowercase();
                terms.iter().any(|t| lowered.contains(t))
            })
            .or_else(|| {
                note.text
                    .lines()
                    .enumerate()
                    .find(|(_, l)| !l.trim().is_empty())
            })
            .map(|(i, l)| (i + 1, l.trim()))
            .unwrap_or((1, ""));
        let mut snippet: String = snippet.chars().take(MAX_SNIPPET_CHARS).collect();
        if snippet.chars().count() == MAX_SNIPPET_CHARS {
            snippet.push('…');
        }
        hits.push((
            terms.iter().all(|t| title.contains(t)),
            note.entry.modified_ms.unwrap_or(0),
            NoteHit {
                id: note.entry.id.clone(),
                line,
                snippet,
            },
        ));
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.id.cmp(&b.2.id)));
    hits.into_iter()
        .take(limit)
        .map(|(_, _, hit)| hit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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

    fn edge_set(graph: &MemoryGraph, kind: EdgeKind) -> Vec<(&str, &str)> {
        let mut out: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| (e.source.as_str(), e.target.as_str()))
            .collect();
        out.sort();
        out
    }

    const A: &str = "workspace-a-00000001";
    const B: &str = "workspace-b-00000002";

    fn fixture() -> (tempfile::TempDir, MemoryVault) {
        vault_with(&[
            ("global/topics/prefs.md", "# 偏好\n\n提交用 conventional commits。\n"),
            (
                "workspace-a-00000001/topics/auth.md",
                "# 登录方案\n\n用 JWT，刷新见 [[refresh-token]]，偏好见 [[prefs]] 和 [[prefs|再说一次]]。\n自己 [[auth]]。\n",
            ),
            (
                "workspace-a-00000001/observations/_inbox/refresh-token.md",
                "# 刷新令牌\n\n回头看 [[auth#细节]]，还有 [[没写的笔记]] 和 [[没写的笔记]]。\n",
            ),
            (
                "workspace-b-00000002/topics/other.md",
                "# B 的笔记\n\n提到 [[auth]]。\n",
            ),
        ])
    }

    fn query(current: Option<&str>, selection: ScopeSelection) -> GraphQuery {
        GraphQuery {
            current_workspace: current.map(str::to_string),
            selection,
            // 这些用例只看链接。
            similar: SimilarConfig {
                per_note: 0,
                ..SimilarConfig::default()
            },
        }
    }

    #[test]
    fn current_selection_shows_global_plus_the_current_workspace() {
        let (_tmp, vault) = fixture();
        let graph = build_graph(&vault, &query(Some(A), ScopeSelection::Current));
        let mut ids: Vec<&str> = graph.notes.iter().map(|n| n.entry.id.as_str()).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "global/topics/prefs.md",
                "workspace-a-00000001/observations/_inbox/refresh-token.md",
                "workspace-a-00000001/topics/auth.md",
            ]
        );
        assert_eq!(
            edge_set(&graph, EdgeKind::Link),
            vec![
                (
                    "workspace-a-00000001/observations/_inbox/refresh-token.md",
                    "workspace-a-00000001/topics/auth.md"
                ),
                (
                    "workspace-a-00000001/topics/auth.md",
                    "global/topics/prefs.md"
                ),
                (
                    "workspace-a-00000001/topics/auth.md",
                    "workspace-a-00000001/observations/_inbox/refresh-token.md"
                ),
            ],
            "重复的链接并成一条，指向自己的不画"
        );
        assert_eq!(
            graph.unresolved,
            vec![UnresolvedLink {
                source: "workspace-a-00000001/observations/_inbox/refresh-token.md".into(),
                target: "没写的笔记".into(),
            }]
        );
        // 没选进来的 scope 也在概况里（范围切换要用），只是没标 selected。
        let summary: Vec<_> = graph
            .scopes
            .iter()
            .map(|s| {
                (
                    s.id.as_str(),
                    s.current,
                    s.selected,
                    s.topics,
                    s.observations,
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("global", false, true, 1, 0),
                (A, true, true, 1, 1),
                (B, false, false, 1, 0),
            ]
        );
        assert!(!graph.similar_skipped);
    }

    #[test]
    fn all_selection_resolves_links_across_workspaces() {
        let (_tmp, vault) = fixture();
        let graph = build_graph(&vault, &query(Some(A), ScopeSelection::All));
        assert_eq!(graph.notes.len(), 4);
        assert!(edge_set(&graph, EdgeKind::Link).contains(&(
            "workspace-b-00000002/topics/other.md",
            "workspace-a-00000001/topics/auth.md"
        )));
        // 只看 B：它链的 auth 不在图里，成了悬空链接。
        let only_b = build_graph(
            &vault,
            &query(Some(A), ScopeSelection::Only(vec![B.into()])),
        );
        assert!(edge_set(&only_b, EdgeKind::Link).is_empty());
        assert_eq!(
            only_b.unresolved,
            vec![UnresolvedLink {
                source: "workspace-b-00000002/topics/other.md".into(),
                target: "auth".into(),
            }]
        );
    }

    #[test]
    fn a_workspace_without_memory_yet_is_still_listed() {
        let (_tmp, vault) = fixture();
        let fresh = "workspace-fresh-0badc0de";
        let graph = build_graph(&vault, &query(Some(fresh), ScopeSelection::Current));
        let listed = graph.scopes.iter().find(|s| s.id == fresh).unwrap();
        assert!(listed.current && listed.selected);
        assert_eq!((listed.topics, listed.observations), (0, 0));
        // 看图不建目录。
        assert!(!vault.home().join(fresh).exists());
        // 没有会话（没有当前工作区）：只有全局。
        let none = build_graph(&vault, &query(None, ScopeSelection::Current));
        assert_eq!(
            none.notes
                .iter()
                .map(|n| n.entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["global/topics/prefs.md"]
        );
    }

    #[test]
    fn an_empty_memory_directory_gives_an_empty_graph() {
        let graph = build_graph(
            &MemoryVault::new(Path::new("/nonexistent/dock-home")),
            &query(None, ScopeSelection::All),
        );
        assert!(graph.notes.is_empty() && graph.edges.is_empty());
        // 全局总是列着：新建第一篇笔记要有地方放。
        assert_eq!(
            graph
                .scopes
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["global"]
        );
    }

    #[test]
    fn similar_edges_skip_pairs_that_are_already_linked() {
        let body = "审计日志降噪，AuditApplicationService.record 是唯一入口，补全结构化字段。";
        let (_tmp, vault) = vault_with(&[
            (
                "global/topics/audit-a.md",
                &format!("# 审计日志改造\n\n{body}\n"),
            ),
            (
                "global/topics/audit-b.md",
                &format!("# 审计日志形态\n\n{body}\n"),
            ),
            (
                "global/topics/audit-c.md",
                &format!("# 审计日志复盘\n\n{body}\n\n另见 [[audit-a]]。\n"),
            ),
            (
                "global/topics/tomcat.md",
                "# 分组删除\n\nTomcat 拒绝编码斜杠。\n",
            ),
        ]);
        let graph = build_graph(
            &vault,
            &GraphQuery {
                selection: ScopeSelection::All,
                ..GraphQuery::default()
            },
        );
        assert_eq!(
            edge_set(&graph, EdgeKind::Link),
            vec![("global/topics/audit-c.md", "global/topics/audit-a.md")]
        );
        assert_eq!(
            edge_set(&graph, EdgeKind::Similar),
            vec![
                ("global/topics/audit-a.md", "global/topics/audit-b.md"),
                ("global/topics/audit-b.md", "global/topics/audit-c.md"),
            ],
            "a—c 已经有链接，不再建议；tomcat 和谁都不像"
        );
        assert!(graph
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Similar)
            .all(|e| e.score >= SimilarConfig::default().min_score && e.score <= 1.0));
    }

    #[test]
    fn note_links_report_both_directions_with_context() {
        let (_tmp, vault) = fixture();
        let notes = vault.load(&vault.scope_ids());
        let links = note_links(&notes, "workspace-a-00000001/topics/auth.md");
        assert_eq!(
            links
                .outgoing
                .iter()
                .map(|l| (l.id.as_str(), l.title.as_str(), l.line))
                .collect::<Vec<_>>(),
            vec![
                (
                    "workspace-a-00000001/observations/_inbox/refresh-token.md",
                    "刷新令牌",
                    3
                ),
                ("global/topics/prefs.md", "偏好", 3),
            ]
        );
        let mut backlinks: Vec<_> = links
            .backlinks
            .iter()
            .map(|l| (l.id.as_str(), l.context.as_str()))
            .collect();
        backlinks.sort();
        assert_eq!(
            backlinks,
            vec![
                (
                    "workspace-a-00000001/observations/_inbox/refresh-token.md",
                    "回头看 [[auth#细节]]，还有 [[没写的笔记]] 和 [[没写的笔记]]。"
                ),
                ("workspace-b-00000002/topics/other.md", "提到 [[auth]]。"),
            ]
        );
        assert!(links.unresolved.is_empty());
        // 每种写法各指向谁：重复的写法只列一次，指向自己的也给出来。
        assert_eq!(
            links
                .targets
                .iter()
                .map(|t| (t.target.as_str(), t.id.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "refresh-token",
                    Some("workspace-a-00000001/observations/_inbox/refresh-token.md")
                ),
                ("prefs", Some("global/topics/prefs.md")),
                ("auth", Some("workspace-a-00000001/topics/auth.md")),
            ]
        );
        let dangling = note_links(
            &notes,
            "workspace-a-00000001/observations/_inbox/refresh-token.md",
        );
        assert_eq!(dangling.unresolved, vec!["没写的笔记"]);
        assert_eq!(
            dangling.targets.last().map(|t| (t.target.as_str(), &t.id)),
            Some(("没写的笔记", &None))
        );
        assert_eq!(
            note_links(&notes, "global/topics/nope.md"),
            NoteLinks::default()
        );
    }

    #[test]
    fn search_matches_every_term_and_ranks_titles_first() {
        let (_tmp, vault) = vault_with(&[
            (
                "global/topics/audit.md",
                "# 审计日志改造\n\n先降噪。\n入口是 AuditService.record。\n",
            ),
            (
                "global/topics/tomcat.md",
                "# 分组删除 400\n\nTomcat 拒绝编码斜杠，和审计日志无关，但 record 也提了一句。\n",
            ),
            ("global/topics/prefs.md", "# 偏好\n\n短提交。\n"),
        ]);
        let notes = vault.load(&vault.scope_ids());
        let ids = |query: &str| {
            search_notes(&notes, query, 10)
                .into_iter()
                .map(|h| (h.id, h.line))
                .collect::<Vec<_>>()
        };
        // 两个词都要有（不分大小写）；行号指到正文里第一处命中。
        let mut both = ids("审计日志 RECORD");
        both.sort();
        assert_eq!(
            both,
            vec![
                ("global/topics/audit.md".to_string(), 1),
                ("global/topics/tomcat.md".to_string(), 3),
            ]
        );
        // 标题里就有的排在只是正文提到的前面。
        assert_eq!(
            ids("审计日志"),
            vec![
                ("global/topics/audit.md".to_string(), 1),
                ("global/topics/tomcat.md".to_string(), 3),
            ]
        );
        assert_eq!(
            ids("审计日志改造"),
            vec![("global/topics/audit.md".to_string(), 1)]
        );
        assert!(ids("审计日志 没有的词").is_empty());
        assert!(ids("   ").is_empty());
        let hit = search_notes(&notes, "tomcat", 10).pop().unwrap();
        assert!(
            hit.snippet.starts_with("Tomcat 拒绝编码斜杠"),
            "{}",
            hit.snippet
        );
        assert_eq!(search_notes(&notes, "审计日志", 1).len(), 1);
    }
}
