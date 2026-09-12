//! `search_tool` 的检索：BM25（Grok `session/tool_index.rs` 同款）。
//!
//! 索引在每次 `search_tool` 调用时重建——隐藏目录只有几十到几百条，重建是
//! 亚毫秒级，免掉一份需要失效的缓存状态。完全同名走快路径，BM25 零命中时回
//! 退到子串扫描（stemmer 处理不好的怪异标识符仍能命中）。

use bm25::{Language, SearchEngineBuilder};
use serde_json::{json, Value};

use crate::types::ToolSpec;

use super::protocol::split_mcp_public_name;

/// 一条可检索的隐藏工具。
pub(super) struct IndexedTool {
    /// 公名（`mcp_{server}__{tool}` 或按需本地工具名）。
    pub name: String,
    /// server / 分组名。
    pub group: String,
    /// 去掉 `mcp_{server}__` 前缀的本名。
    pub raw: String,
    pub description: String,
    /// 入参名，进 BM25 文档（"issue id" 这类查询靠它命中）。
    pub parameters: Vec<String>,
    pub schema: Value,
}

impl IndexedTool {
    fn from_spec(spec: &ToolSpec, group: String) -> Self {
        let raw = split_mcp_public_name(&spec.name)
            .map(|(_, t)| t.to_string())
            .unwrap_or_else(|| spec.name.clone());
        let schema: Value = serde_json::from_str(&spec.parameters_json)
            .unwrap_or_else(|_| json!({"type":"object"}));
        let parameters = parameter_names(&schema);
        Self {
            name: spec.name.clone(),
            group,
            raw,
            description: spec.description.clone(),
            parameters,
            schema,
        }
    }

    /// BM25 文档：`{group} {raw} {description} {params}` + 标识符拆词。
    fn to_document(&self) -> String {
        let params = self.parameters.join(" ");
        let base = format!(
            "{} {} {} {}",
            self.group, self.raw, self.description, params
        );
        let extra: String = [self.group.as_str(), self.raw.as_str()]
            .iter()
            .flat_map(|s| split_identifier(s))
            .chain(self.parameters.iter().flat_map(|p| split_identifier(p)))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{base} {extra}")
    }
}

pub(super) fn index_tool(spec: &ToolSpec, group: String) -> IndexedTool {
    IndexedTool::from_spec(spec, group)
}

/// 检索结果。
pub(super) enum Ranking {
    /// 完全同名：单条命中。调用方对它永远回完整 schema（重复搜索的逃生舱）。
    Exact(usize),
    /// 打分命中，已按分数降序并截到 `limit`。
    Ranked(Vec<(usize, f32)>),
}

pub(super) fn rank(docs: &[IndexedTool], query: &str, limit: usize) -> Ranking {
    let q = query.trim().to_lowercase();
    if let Some(i) = docs
        .iter()
        .position(|d| d.name.to_lowercase() == q || d.raw.to_lowercase() == q)
    {
        return Ranking::Exact(i);
    }
    let limit = limit.max(1);
    let hits = bm25_hits(docs, &q, limit);
    if !hits.is_empty() {
        return Ranking::Ranked(hits);
    }
    Ranking::Ranked(substring_hits(docs, &q, limit))
}

fn bm25_hits(docs: &[IndexedTool], query: &str, limit: usize) -> Vec<(usize, f32)> {
    if docs.is_empty() {
        return Vec::new();
    }
    let corpus: Vec<String> = docs.iter().map(IndexedTool::to_document).collect();
    let engine = SearchEngineBuilder::<u32>::with_corpus(Language::English, corpus).build();
    engine
        .search(&normalize_query(query), limit)
        .into_iter()
        .filter(|r| r.score > 0.0)
        .filter_map(|r| {
            let i = r.document.id as usize;
            docs.get(i).map(|_| (i, r.score))
        })
        .collect()
}

/// BM25 零命中时的回退：名/组/本名/描述的子串扫描。
fn substring_hits(docs: &[IndexedTool], query: &str, limit: usize) -> Vec<(usize, f32)> {
    let mut scored: Vec<(usize, f32)> = docs
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let score = token_score(query, d);
            (score > 0.0).then_some((i, score))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(limit);
    scored
}

fn token_score(query: &str, doc: &IndexedTool) -> f32 {
    let name = doc.name.to_lowercase();
    let raw = doc.raw.to_lowercase();
    let hay = format!("{} {} {} {}", name, doc.group, raw, doc.description).to_lowercase();
    let mut score = 0.0f32;
    for tok in query.split_whitespace().filter(|t| !t.is_empty()) {
        if name.contains(tok) || raw == tok {
            score += 2.0;
        } else if hay.contains(tok) {
            score += 1.0;
        }
    }
    score
}

/// 入参名（JSON Schema `properties` 的键）。
fn parameter_names(schema: &Value) -> Vec<String> {
    schema
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default()
}

/// 把复合标识符拆成词：`__`、`_`、`-`、camelCase / PascalCase 边界。
fn split_identifier(s: &str) -> Vec<&str> {
    let mut words: Vec<&str> = Vec::new();
    for part in s
        .split("__")
        .flat_map(|p| p.split('_'))
        .flat_map(|p| p.split('-'))
    {
        if part.is_empty() {
            continue;
        }
        let bytes = part.as_bytes();
        let mut start = 0;
        for i in 1..bytes.len() {
            if bytes[i - 1].is_ascii_lowercase() && bytes[i].is_ascii_uppercase() {
                words.push(&part[start..i]);
                start = i;
            }
        }
        words.push(&part[start..]);
    }
    words
}

/// 查询含标识符特征时把拆出来的词追加进去，让 BM25 能命中单个部件。
fn normalize_query(query: &str) -> String {
    let needs_split = query.contains("__")
        || query.contains('_')
        || query.contains('-')
        || query
            .as_bytes()
            .windows(2)
            .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase());
    if !needs_split {
        return query.to_owned();
    }
    let extra: Vec<&str> = query
        .split_whitespace()
        .flat_map(split_identifier)
        .collect();
    if extra.is_empty() {
        return query.to_owned();
    }
    format!("{query} {}", extra.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, group: &str, desc: &str) -> IndexedTool {
        index_tool(
            &ToolSpec {
                name: name.into(),
                description: desc.into(),
                parameters_json: r#"{"type":"object","properties":{"issue_id":{"type":"string"}}}"#
                    .into(),
            },
            group.into(),
        )
    }

    #[test]
    fn splits_identifiers_and_camel_case() {
        assert_eq!(
            split_identifier("mcp_linear__saveIssue"),
            vec!["mcp", "linear", "save", "Issue"]
        );
        assert_eq!(split_identifier("grafana-ai"), vec!["grafana", "ai"]);
        assert_eq!(normalize_query("hello world"), "hello world");
        assert!(normalize_query("save_issue").contains("save issue"));
    }

    #[test]
    fn bm25_matches_morphological_variants() {
        let docs = vec![
            tool("mcp_linear__create_issue", "linear", "Create an issue"),
            tool("mcp_slack__post_message", "slack", "Post a chat message"),
        ];
        let Ranking::Ranked(hits) = rank(&docs, "creating issues", 5) else {
            panic!("exact match not expected");
        };
        assert_eq!(hits.first().map(|(i, _)| *i), Some(0), "{hits:?}");
    }

    #[test]
    fn exact_name_takes_the_fast_path() {
        let docs = vec![
            tool("mcp_linear__create_issue", "linear", "Create an issue"),
            tool("scheduler_create", "scheduler", "Create a scheduled task"),
        ];
        assert!(matches!(
            rank(&docs, "mcp_linear__create_issue", 5),
            Ranking::Exact(0)
        ));
        // 去掉 server 前缀的本名同样算精确命中。
        assert!(matches!(rank(&docs, "create_issue", 5), Ranking::Exact(0)));
    }

    #[test]
    fn parameter_names_feed_the_document() {
        let doc = tool("mcp_linear__create_issue", "linear", "Create an issue");
        assert_eq!(doc.parameters, vec!["issue_id".to_string()]);
        assert!(doc.to_document().contains("issue_id"));
    }

    #[test]
    fn falls_back_to_substring_when_bm25_misses() {
        // 全停用词查询：BM25 侧没有可打分的 token，回退扫描仍能按子串命中。
        let docs = vec![tool("mcp_probe__the_it", "probe", "the it")];
        let Ranking::Ranked(hits) = rank(&docs, "the", 5) else {
            panic!("exact match not expected");
        };
        assert_eq!(hits, vec![(0, 2.0)]);
    }
}
