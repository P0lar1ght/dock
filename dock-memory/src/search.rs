//! High-level FTS search helpers for tools.

use crate::index::{MemoryIndex, SearchHit};
use crate::layout::MemoryRoot;

/// Open the index and query. Does **not** reindex the tree on every call —
/// writes already call [`MemoryIndex::reindex_file`]. Reindexes once when the
/// DB is missing or still empty (cold start / legacy files).
pub fn search_memory(
    root: &MemoryRoot,
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchHit>, String> {
    let db = root.search_db();
    let missing = !db.exists();
    let mut index = MemoryIndex::open_or_create(&db).map_err(|e| e.to_string())?;
    if missing || index.is_empty() {
        let _ = index.reindex_tree(root);
    }
    index.search(query, max_results).map_err(|e| e.to_string())
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
