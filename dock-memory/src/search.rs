//! High-level FTS search helpers for tools.

use crate::index::{MemoryIndex, SearchHit};
use crate::layout::MemoryRoot;

pub fn search_memory(
    root: &MemoryRoot,
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchHit>, String> {
    let mut index =
        MemoryIndex::open_or_create(&root.search_db()).map_err(|e| e.to_string())?;
    let _ = index.reindex_tree(root);
    index
        .search(query, max_results)
        .map_err(|e| e.to_string())
}

pub fn format_search_results(hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return "No memory results found for query.".into();
    }
    let mut out = format!("Found {} memory result(s):\n", hits.len());
    for (i, hit) in hits.iter().enumerate() {
        out.push_str(&format!(
            "\n### Result {} (score: {:.2}, source: {})\n**File:** {} (lines {}-{})\n```\n{}\n```\n",
            i + 1,
            hit.score,
            hit.source,
            hit.path,
            hit.start_line + 1,
            hit.end_line,
            hit.text
        ));
    }
    out
}
