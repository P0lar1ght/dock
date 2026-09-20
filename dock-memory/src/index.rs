//! SQLite-backed memory index with FTS5 + optional sqlite-vec KNN.
//!
//! Call [`init_sqlite_vec`] once before opening indexes. Missing vec → FTS-only.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Once;

use rusqlite::{params, Connection, OptionalExtension};

use crate::chunker::{chunk_hash, chunk_markdown, ChunkConfig};
use crate::query_expansion::extract_keywords;
use crate::schema;

static SQLITE_VEC_INIT: Once = Once::new();

/// Register sqlite-vec globally (safe to call repeatedly).
pub fn init_sqlite_vec() {
    SQLITE_VEC_INIT.call_once(|| {
        // SAFETY: sqlite_vec::sqlite3_vec_init matches sqlite3_auto_extension ABI.
        // Pin sqlite-vec =0.1.7-alpha.2; bump must re-verify.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *mut std::ffi::c_char,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> i32,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

#[derive(Debug, Clone)]
pub struct ChunkRecord {
    pub id: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
    pub source: String,
    pub access_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct FtsHit {
    pub chunk_id: String,
    pub rank: f64,
}

#[derive(Debug, Clone)]
pub struct FtsResult {
    pub chunk_id: String,
    pub rowid: i64,
    pub rank: f64,
}

/// Tool-facing hit (keeps Stage-3 API).
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
    vec_available: bool,
    embedding_dimensions: usize,
}

impl MemoryIndex {
    /// Open or create; default embedding dimensions 1024. Vec table created only
    /// when sqlite-vec loaded.
    pub fn open_or_create(db_path: &Path) -> Result<Self, rusqlite::Error> {
        Self::open_or_create_with_dimensions(db_path, 1024)
    }

    pub fn open_or_create_with_dimensions(
        db_path: &Path,
        dimensions: usize,
    ) -> Result<Self, rusqlite::Error> {
        init_sqlite_vec();
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
        let want = schema::SCHEMA_VERSION.to_string();
        if stored.as_deref().is_some_and(|v| v != want) {
            let _ = db.execute_batch(
                "
                DROP TABLE IF EXISTS chunks_vec;
                DROP TABLE IF EXISTS chunks_fts;
                DROP TABLE IF EXISTS chunks;
                DROP TABLE IF EXISTS meta;
                ",
            );
        }

        let vec_available =
            match db.query_row("SELECT vec_version()", [], |r| r.get::<_, String>(0)) {
                Ok(v) => {
                    tracing::info!(version = %v, "sqlite-vec loaded");
                    true
                }
                Err(_) => {
                    static WARNED: Once = Once::new();
                    WARNED.call_once(|| {
                        tracing::warn!("sqlite-vec not available, FTS-only memory search");
                    });
                    false
                }
            };

        db.execute_batch(&schema::schema_sql(dimensions, vec_available))?;
        db.execute(schema::UPSERT_META_SQL, params!["schema_version", want])?;

        let stored_dims: Option<String> = db
            .query_row(schema::GET_META_SQL, params!["embedding_dimensions"], |r| {
                r.get(0)
            })
            .optional()?;
        match stored_dims {
            Some(ref s) if s.parse::<usize>().ok() == Some(dimensions) => {}
            Some(ref s) => {
                tracing::warn!(
                    stored = %s,
                    requested = dimensions,
                    "embedding dimension mismatch, recreating chunks_vec"
                );
                if vec_available {
                    let _ = db.execute("DROP TABLE IF EXISTS chunks_vec", []);
                    db.execute_batch(&format!(
                        "CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec USING vec0(\n    \
                         chunk_id TEXT PRIMARY KEY,\n    \
                         embedding FLOAT[{dimensions}]\n);"
                    ))?;
                }
                db.execute(
                    schema::UPSERT_META_SQL,
                    params!["embedding_dimensions", dimensions.to_string()],
                )?;
            }
            None => {
                db.execute(
                    schema::UPSERT_META_SQL,
                    params!["embedding_dimensions", dimensions.to_string()],
                )?;
            }
        }

        // Ensure access_count column on older DBs created before this field.
        let _ = db.execute(
            "ALTER TABLE chunks ADD COLUMN access_count INTEGER DEFAULT 0",
            [],
        );
        let _ = db.execute("ALTER TABLE chunks ADD COLUMN last_accessed INTEGER", []);

        Ok(Self {
            db,
            chunk_config: ChunkConfig::default(),
            vec_available,
            embedding_dimensions: dimensions,
        })
    }

    pub fn vec_available(&self) -> bool {
        self.vec_available
    }

    pub fn embedding_dimensions(&self) -> usize {
        self.embedding_dimensions
    }

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
                    if self.vec_available {
                        let _ = tx.execute(
                            "DELETE FROM chunks_vec WHERE chunk_id = ?1",
                            params![chunk_id],
                        );
                    }
                    result.updated += 1;
                }
                None => {
                    tx.execute(
                        "INSERT INTO chunks (id, path, start_line, end_line, text, hash, source, \
                         created_at, updated_at, access_count) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)",
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
                if self.vec_available {
                    let _ = tx.execute(
                        "DELETE FROM chunks_vec WHERE chunk_id = ?1",
                        params![old_id],
                    );
                }
                tx.execute("DELETE FROM chunks WHERE id = ?1", params![old_id])?;
                result.removed += 1;
            }
        }

        tx.commit()?;
        Ok(result)
    }

    fn get_chunks_for_path(
        db: &Connection,
        path: &str,
    ) -> Result<HashMap<String, (String, i64, String)>, rusqlite::Error> {
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
        let mut map = HashMap::new();
        for r in rows {
            let (id, v) = r?;
            map.insert(id, v);
        }
        Ok(map)
    }

    pub fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<FtsResult>, rusqlite::Error> {
        let keywords = extract_keywords(query);
        let fts_query = keywords.join(" OR ");
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.db.prepare(
            "SELECT c.id, c.rowid, f.rank FROM chunks_fts f \
             JOIN chunks c ON f.rowid = c.rowid \
             WHERE chunks_fts MATCH ?1 \
             ORDER BY f.rank LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
            Ok(FtsResult {
                chunk_id: row.get(0)?,
                rowid: row.get(1)?,
                rank: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// FTS restricted to given source labels (e.g. evergreen boost).
    pub fn search_fts_by_sources(
        &self,
        query: &str,
        limit: usize,
        sources: &[&str],
    ) -> Result<Vec<FtsResult>, rusqlite::Error> {
        if sources.is_empty() {
            return self.search_fts(query, limit);
        }
        let keywords = extract_keywords(query);
        let fts_query = keywords.join(" OR ");
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let placeholders = sources
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 3))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT c.id, c.rowid, f.rank FROM chunks_fts f \
             JOIN chunks c ON f.rowid = c.rowid \
             WHERE chunks_fts MATCH ?1 AND c.source IN ({placeholders}) \
             ORDER BY f.rank LIMIT ?2"
        );
        let mut stmt = self.db.prepare(&sql)?;
        let mut vals: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        vals.push(Box::new(fts_query));
        vals.push(Box::new(limit as i64));
        for s in sources {
            vals.push(Box::new(s.to_string()));
        }
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            vals.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            Ok(FtsResult {
                chunk_id: row.get(0)?,
                rowid: row.get(1)?,
                rank: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_chunk(&self, id: &str) -> Result<Option<ChunkRecord>, rusqlite::Error> {
        let mut stmt = self.db.prepare(
            "SELECT id, path, start_line, end_line, text, source, \
             COALESCE(access_count, 0), created_at FROM chunks WHERE id = ?1",
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
                    access_count: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })
            .optional()?;
        Ok(result)
    }

    pub fn chunks_without_embeddings(&self) -> Result<Vec<(String, String)>, rusqlite::Error> {
        if !self.vec_available {
            return Ok(vec![]);
        }
        // vec0 may not expose a simple LEFT JOIN; try chunks_vec_info-style.
        // Grok uses chunks_vec_rowids; fall back to all chunks if join fails.
        let sql = "SELECT c.id, c.text FROM chunks c \
             WHERE c.id NOT IN (SELECT chunk_id FROM chunks_vec)";
        match self.db.prepare(sql) {
            Ok(mut stmt) => {
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect()
            }
            Err(_) => Ok(vec![]),
        }
    }

    pub fn upsert_embedding(
        &self,
        chunk_id: &str,
        embedding: &[f32],
    ) -> Result<(), rusqlite::Error> {
        if !self.vec_available {
            return Ok(());
        }
        let blob = embedding_to_bytes(embedding);
        self.db.execute(
            "INSERT OR REPLACE INTO chunks_vec(chunk_id, embedding) VALUES (?1, ?2)",
            params![chunk_id, blob],
        )?;
        Ok(())
    }

    pub fn vector_search(
        &self,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(String, f64)>, rusqlite::Error> {
        if !self.vec_available {
            return Ok(vec![]);
        }
        let blob = embedding_to_bytes(embedding);
        let mut stmt = self.db.prepare(
            "SELECT chunk_id, distance FROM chunks_vec \
             WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance",
        )?;
        let rows = stmt.query_map(params![blob, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, f64::from(row.get::<_, f32>(1)?)))
        })?;
        rows.collect()
    }

    /// Remove all chunks for `path` from chunks / FTS / vec.
    pub fn delete_path(&mut self, path: &Path) -> Result<usize, rusqlite::Error> {
        let path_str = path.to_string_lossy().to_string();
        let existing = Self::get_chunks_for_path(&self.db, &path_str)?;
        if existing.is_empty() {
            return Ok(0);
        }
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut removed = 0usize;
        for (id, old) in &existing {
            tx.execute(
                "INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES('delete', ?1, ?2)",
                params![old.1, old.2],
            )?;
            if self.vec_available {
                let _ = tx.execute("DELETE FROM chunks_vec WHERE chunk_id = ?1", params![id]);
            }
            tx.execute("DELETE FROM chunks WHERE id = ?1", params![id])?;
            removed += 1;
        }
        tx.commit()?;
        Ok(removed)
    }

    /// Legacy helper: FTS → SearchHit (no MMR). Prefer [`crate::search::search_memory`].
    pub fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchHit>, rusqlite::Error> {
        let fts = self.search_fts(query, max_results.max(1))?;
        let mut out = Vec::with_capacity(fts.len());
        for hit in fts {
            if let Some(chunk) = self.get_chunk(&hit.chunk_id)? {
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

fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(embedding.len() * 4);
    for v in embedding {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
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

    #[test]
    fn delete_path_removes_chunks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = MemoryRoot::open(tmp.path(), Path::new("/tmp/del-proj"));
        root.ensure_layout().unwrap();
        let topic = root.global.topics.join("gone.md");
        std::fs::write(&topic, "## Gone\n\ndelete me soon\n").unwrap();
        let mut idx = MemoryIndex::open_or_create(&root.search_db()).unwrap();
        idx.reindex_file(&topic, "global").unwrap();
        assert!(!idx.search("delete me", 5).unwrap().is_empty());
        let n = idx.delete_path(&topic).unwrap();
        assert!(n >= 1);
        assert!(idx.search("delete me", 5).unwrap().is_empty());
        assert_eq!(idx.delete_path(&topic).unwrap(), 0);
    }
}
