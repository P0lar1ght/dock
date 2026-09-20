//! FTS5 memory index at `$DOCK_HOME/memory/search.sqlite` (vec added in a later commit).

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::chunker::{chunk_hash, chunk_markdown, ChunkConfig};
use crate::keywords::extract_keywords;

const SCHEMA_VERSION: &str = "1";

#[derive(Debug, Clone)]
pub struct ChunkRecord {
    pub id: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct FtsHit {
    pub chunk_id: String,
    pub rank: f64,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
    pub source: String,
    pub score: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReindexResult {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

pub struct MemoryIndex {
    db: Connection,
    chunk_config: ChunkConfig,
}

impl MemoryIndex {
    pub fn open_or_create(db_path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let db = Connection::open(db_path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let _ = db.execute_batch("PRAGMA journal_mode=WAL;");

        let stored: Option<String> = db
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None);
        if stored.as_deref().is_some_and(|v| v != SCHEMA_VERSION) {
            db.execute_batch(
                "
                DROP TABLE IF EXISTS chunks_fts;
                DROP TABLE IF EXISTS chunks;
                DROP TABLE IF EXISTS meta;
                ",
            )?;
        }

        db.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chunks (
                rowid INTEGER PRIMARY KEY AUTOINCREMENT,
                id TEXT UNIQUE NOT NULL,
                path TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                text TEXT NOT NULL,
                hash TEXT NOT NULL,
                source TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_chunks_path ON chunks(path);
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(text, content='');
            "#,
        )?;
        db.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
            params![SCHEMA_VERSION],
        )?;
        Ok(Self {
            db,
            chunk_config: ChunkConfig::default(),
        })
    }

    /// True when the chunks table has no rows (fresh / wiped DB).
    pub fn is_empty(&self) -> bool {
        self.db
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get::<_, i64>(0))
            .map(|n| n == 0)
            .unwrap_or(true)
    }

    pub fn reindex_file(
        &mut self,
        path: &Path,
        source: &str,
    ) -> Result<ReindexResult, rusqlite::Error> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Ok(ReindexResult::default()),
        };
        self.reindex_content(path, source, &content)
    }

    pub fn reindex_content(
        &mut self,
        path: &Path,
        source: &str,
        content: &str,
    ) -> Result<ReindexResult, rusqlite::Error> {
        let new_chunks = chunk_markdown(content, &self.chunk_config);
        let path_str = path.to_string_lossy().to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let mut result = ReindexResult::default();
        let mut seen_ids = std::collections::HashSet::new();

        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing = Self::get_chunks_for_path(&tx, &path_str)?;

        for (i, chunk) in new_chunks.iter().enumerate() {
            let chunk_id = format!("{path_str}:{i}");
            let hash = chunk_hash(&chunk.text);
            seen_ids.insert(chunk_id.clone());

            match existing.get(&chunk_id) {
                Some(old) if old.0 == hash => {}
                Some(old) => {
                    tx.execute(
                        "UPDATE chunks SET text = ?1, hash = ?2, start_line = ?3, \
                         end_line = ?4, updated_at = ?5 WHERE id = ?6",
                        params![
                            chunk.text,
                            hash,
                            chunk.start_line as i64,
                            chunk.end_line as i64,
                            now,
                            chunk_id
                        ],
                    )?;
                    tx.execute(
                        "INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES('delete', ?1, ?2)",
                        params![old.1, old.2],
                    )?;
                    let rid: i64 = tx.query_row(
                        "SELECT rowid FROM chunks WHERE id = ?1",
                        params![chunk_id],
                        |r| r.get(0),
                    )?;
                    tx.execute(
                        "INSERT INTO chunks_fts(rowid, text) VALUES (?1, ?2)",
                        params![rid, chunk.text],
                    )?;
                    result.updated += 1;
                }
                None => {
                    tx.execute(
                        "INSERT INTO chunks (id, path, start_line, end_line, text, hash, source, \
                         created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![
                            chunk_id,
                            path_str,
                            chunk.start_line as i64,
                            chunk.end_line as i64,
                            chunk.text,
                            hash,
                            source,
                            now,
                            now,
                        ],
                    )?;
                    let rowid = tx.last_insert_rowid();
                    tx.execute(
                        "INSERT INTO chunks_fts(rowid, text) VALUES (?1, ?2)",
                        params![rowid, chunk.text],
                    )?;
                    result.added += 1;
                }
            }
        }

        for (old_id, old) in &existing {
            if !seen_ids.contains(old_id) {
                tx.execute(
                    "INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES('delete', ?1, ?2)",
                    params![old.1, old.2],
                )?;
                tx.execute("DELETE FROM chunks WHERE id = ?1", params![old_id])?;
                result.removed += 1;
            }
        }

        tx.commit()?;
        Ok(result)
    }

    /// Existing map: chunk_id → (hash, rowid, text)
    fn get_chunks_for_path(
        db: &Connection,
        path: &str,
    ) -> Result<std::collections::HashMap<String, (String, i64, String)>, rusqlite::Error> {
        let mut stmt = db.prepare("SELECT id, hash, rowid, text FROM chunks WHERE path = ?1")?;
        let rows = stmt.query_map(params![path], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ),
            ))
        })?;
        let mut map = std::collections::HashMap::new();
        for r in rows {
            let (id, v) = r?;
            map.insert(id, v);
        }
        Ok(map)
    }

    pub fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<FtsHit>, rusqlite::Error> {
        let keywords = extract_keywords(query);
        let fts_query = keywords.join(" OR ");
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.db.prepare(
            "SELECT c.id, f.rank FROM chunks_fts f \
             JOIN chunks c ON f.rowid = c.rowid \
             WHERE chunks_fts MATCH ?1 \
             ORDER BY f.rank LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
            Ok(FtsHit {
                chunk_id: row.get(0)?,
                rank: row.get(1)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_chunk(&self, id: &str) -> Result<Option<ChunkRecord>, rusqlite::Error> {
        let mut stmt = self.db.prepare(
            "SELECT id, path, start_line, end_line, text, source FROM chunks WHERE id = ?1",
        )?;
        let result = stmt
            .query_row(params![id], |row| {
                Ok(ChunkRecord {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    start_line: row.get::<_, i64>(2)? as usize,
                    end_line: row.get::<_, i64>(3)? as usize,
                    text: row.get(4)?,
                    source: row.get(5)?,
                })
            })
            .optional()?;
        Ok(result)
    }

    pub fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, rusqlite::Error> {
        let fts = self.search_fts(query, max_results.max(1))?;
        let mut out = Vec::with_capacity(fts.len());
        for hit in fts {
            if let Some(chunk) = self.get_chunk(&hit.chunk_id)? {
                // FTS rank is negative BM25; invert for display score in [0,1]-ish.
                let score = 1.0 / (1.0 + (-hit.rank).max(0.0));
                out.push(SearchHit {
                    path: chunk.path,
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    text: chunk.text,
                    source: chunk.source,
                    score,
                });
            }
        }
        Ok(out)
    }

    pub fn reindex_tree(
        &mut self,
        root: &crate::layout::MemoryRoot,
    ) -> Result<usize, rusqlite::Error> {
        let mut n = 0;
        for (scope_paths, source) in [(&root.global, "global"), (&root.workspace, "workspace")] {
            n += self.reindex_dir(&scope_paths.topics, source, true)?;
            // Inbox (recursive not needed — flat .md only) + legacy flat observations.
            n += self.reindex_dir(&scope_paths.inbox, source, false)?;
            n += self.reindex_flat_observations(&scope_paths.observations, source)?;
        }
        Ok(n)
    }

    fn reindex_flat_observations(
        &mut self,
        observations: &Path,
        source: &str,
    ) -> Result<usize, rusqlite::Error> {
        let Ok(entries) = std::fs::read_dir(observations) else {
            return Ok(0);
        };
        let mut n = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let r = self.reindex_file(&path, source)?;
            n += r.added + r.updated;
        }
        Ok(n)
    }

    fn reindex_dir(
        &mut self,
        dir: &Path,
        source: &str,
        recurse: bool,
    ) -> Result<usize, rusqlite::Error> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(0);
        };
        let mut n = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if recurse {
                    n += self.reindex_dir(&path, source, true)?;
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let r = self.reindex_file(&path, source)?;
            n += r.added + r.updated;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::MemoryRoot;

    #[test]
    fn fts_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/fts-proj"));
        root.ensure_layout().unwrap();
        let topic = root.workspace.topics.join("rust.md");
        std::fs::write(
            &topic,
            "## Ownership\n\nrust ownership and borrowing rules\n",
        )
        .unwrap();

        let mut idx = MemoryIndex::open_or_create(&root.search_db()).unwrap();
        let r = idx.reindex_file(&topic, "workspace").unwrap();
        assert!(r.added >= 1);

        let hits = idx.search("rust ownership", 5).unwrap();
        assert!(!hits.is_empty());
        assert!(hits
            .first()
            .unwrap()
            .text
            .to_lowercase()
            .contains("ownership"));
    }
}
