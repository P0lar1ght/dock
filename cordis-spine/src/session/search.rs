//! Rebuildable SQLite FTS5 index over session titles + user prompts.
//!
//! Layout mirrors grok-build `xai-grok-session-search` (title + content FTS,
//! cwd as filter only). Kept small: open/upsert/delete/query/rebuild; no
//! bootstrap lease machinery. Index file: `$DOCK_HOME/sessions/session_search.sqlite`.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use cordis_base::config::dock_home;
use cordis_base::types::LogEvent;

use super::persist::{self, RosterEntry};

const SCHEMA_VERSION: &str = "1";
const DB_NAME: &str = "session_search.sqlite";

#[derive(Debug, Clone)]
pub struct SessionDoc {
    pub session_id: String,
    pub cwd: String,
    pub updated_at_unix: i64,
    pub title: String,
    pub content: String,
    pub content_hash: String,
}

#[derive(Debug, Clone)]
pub struct SessionHit {
    pub session_id: String,
    pub cwd: String,
    pub title: String,
    pub updated_at_unix: i64,
    pub snippet: Option<String>,
}

pub fn index_path() -> PathBuf {
    dock_home().join("sessions").join(DB_NAME)
}

fn content_hash(content: &str) -> String {
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn user_prompts(events: &[LogEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            LogEvent::User(t) if !t.trim().is_empty() => Some(t.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl SessionDoc {
    pub fn from_session(
        id: &str,
        cwd: &Path,
        title: &str,
        updated_unix: u64,
        events: &[LogEvent],
    ) -> Self {
        let content = user_prompts(events);
        Self {
            session_id: id.to_string(),
            cwd: cwd.to_string_lossy().into_owned(),
            updated_at_unix: updated_unix as i64,
            title: title.to_string(),
            content_hash: content_hash(&content),
            content,
        }
    }

    pub fn from_roster_and_events(entry: &RosterEntry, events: &[LogEvent]) -> Self {
        let updated = entry
            .updated
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self::from_session(&entry.id, &entry.cwd, &entry.title, updated, events)
    }
}

pub struct SessionSearchIndex {
    db: Connection,
}

impl SessionSearchIndex {
    pub fn open_default() -> Result<Self, rusqlite::Error> {
        Self::open_or_create(&index_path())
    }

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
                DROP TRIGGER IF EXISTS session_docs_ai;
                DROP TRIGGER IF EXISTS session_docs_ad;
                DROP TRIGGER IF EXISTS session_docs_au;
                DROP TABLE IF EXISTS session_docs_fts;
                DROP TABLE IF EXISTS session_docs;
                ",
            )?;
        }

        db.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS session_docs (
                session_id TEXT PRIMARY KEY,
                cwd TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                title TEXT NOT NULL,
                content TEXT NOT NULL,
                content_hash TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS session_docs_fts USING fts5(
                title,
                content,
                content='session_docs',
                content_rowid='rowid'
            );
            CREATE TRIGGER IF NOT EXISTS session_docs_ai AFTER INSERT ON session_docs BEGIN
                INSERT INTO session_docs_fts(rowid, title, content)
                VALUES (new.rowid, new.title, new.content);
            END;
            CREATE TRIGGER IF NOT EXISTS session_docs_ad AFTER DELETE ON session_docs BEGIN
                INSERT INTO session_docs_fts(session_docs_fts, rowid, title, content)
                VALUES ('delete', old.rowid, old.title, old.content);
            END;
            CREATE TRIGGER IF NOT EXISTS session_docs_au AFTER UPDATE ON session_docs BEGIN
                INSERT INTO session_docs_fts(session_docs_fts, rowid, title, content)
                VALUES ('delete', old.rowid, old.title, old.content);
                INSERT INTO session_docs_fts(rowid, title, content)
                VALUES (new.rowid, new.title, new.content);
            END;
            ",
        )?;
        db.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
            params![SCHEMA_VERSION],
        )?;
        Ok(Self { db })
    }

    pub fn upsert_doc(&self, doc: &SessionDoc) -> Result<(), rusqlite::Error> {
        if let Ok(Some(hash)) = self.get_content_hash(&doc.session_id) {
            if hash == doc.content_hash {
                // Still refresh title/cwd/updated even when body unchanged.
                self.db.execute(
                    "UPDATE session_docs SET cwd=?2, updated_at=?3, title=?4
                     WHERE session_id=?1",
                    params![doc.session_id, doc.cwd, doc.updated_at_unix, doc.title],
                )?;
                return Ok(());
            }
        }
        self.db.execute(
            "INSERT INTO session_docs(session_id, cwd, updated_at, title, content, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(session_id) DO UPDATE SET
                 cwd = excluded.cwd,
                 updated_at = excluded.updated_at,
                 title = excluded.title,
                 content = excluded.content,
                 content_hash = excluded.content_hash",
            params![
                doc.session_id,
                doc.cwd,
                doc.updated_at_unix,
                doc.title,
                doc.content,
                doc.content_hash
            ],
        )?;
        Ok(())
    }

    pub fn delete_doc(&self, session_id: &str) -> Result<(), rusqlite::Error> {
        self.db.execute(
            "DELETE FROM session_docs WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    pub fn get_content_hash(&self, session_id: &str) -> Result<Option<String>, rusqlite::Error> {
        self.db
            .query_row(
                "SELECT content_hash FROM session_docs WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .optional()
    }

    /// FTS query over title + user prompts. `cwd` filters when `Some`.
    pub fn query(
        &self,
        query: &str,
        cwd: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SessionHit>, rusqlite::Error> {
        let Some(match_q) = build_match_query(query) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.db.prepare(
            "SELECT d.session_id, d.cwd, d.title, d.updated_at,
                    snippet(session_docs_fts, 1, '[', ']', ' … ', 12) AS snip
             FROM session_docs_fts
             JOIN session_docs d ON d.rowid = session_docs_fts.rowid
             WHERE session_docs_fts MATCH ?1
               AND (?2 IS NULL OR d.cwd = ?2)
             ORDER BY bm25(session_docs_fts, 10.0, 1.0) ASC, d.updated_at DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![match_q, cwd, limit as i64], |row| {
            Ok(SessionHit {
                session_id: row.get(0)?,
                cwd: row.get(1)?,
                title: row.get(2)?,
                updated_at_unix: row.get(3)?,
                snippet: row.get(4)?,
            })
        })?;
        rows.collect()
    }
}

fn build_match_query(query: &str) -> Option<String> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .flat_map(|t| {
            t.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .filter(|p| p.chars().any(|c| c.is_ascii_alphanumeric()))
        })
        .map(|t| format!("\"{t}\" *"))
        .collect();
    if tokens.is_empty() {
        return None;
    }
    Some(tokens.join(" AND "))
}

/// Upsert one session into the default index (fail-open).
pub fn index_session(doc: &SessionDoc) {
    let Ok(index) = SessionSearchIndex::open_default() else {
        return;
    };
    let _ = index.upsert_doc(doc);
}

pub fn evict_session(session_id: &str) {
    let Ok(index) = SessionSearchIndex::open_default() else {
        return;
    };
    let _ = index.delete_doc(session_id);
}

/// Rebuild the whole index from `$DOCK_HOME/sessions/*/*/`.
pub fn rebuild_all() -> Result<usize, String> {
    let index = SessionSearchIndex::open_default().map_err(|e| e.to_string())?;
    let roster = persist::load_roster();
    let mut n = 0;
    let mut keep = std::collections::HashSet::new();
    for entry in &roster {
        keep.insert(entry.id.clone());
        let Some(session) = persist::load_session(&entry.id, &entry.cwd) else {
            continue;
        };
        let doc = SessionDoc::from_roster_and_events(entry, &session.events);
        index.upsert_doc(&doc).map_err(|e| e.to_string())?;
        n += 1;
    }
    if let Ok(ids) = index.all_ids() {
        for id in ids {
            if !keep.contains(&id) {
                let _ = index.delete_doc(&id);
            }
        }
    }
    Ok(n)
}

impl SessionSearchIndex {
    fn all_ids(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.db.prepare("SELECT session_id FROM session_docs")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
    }
}

/// Rebuild from disk only when the index has no rows (first run / wiped DB).
pub fn ensure_index() {
    let Ok(index) = SessionSearchIndex::open_default() else {
        return;
    };
    let empty = index
        .db
        .query_row("SELECT COUNT(*) FROM session_docs", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap_or(0)
        == 0;
    drop(index);
    if empty {
        let _ = rebuild_all();
    }
}

/// Search helper for TUI (fail-open). Index fills on `persist::save`; call
/// [`rebuild_all`] once if an older tree has no index yet.
pub fn search(query: &str, cwd: Option<&str>, limit: usize) -> Vec<SessionHit> {
    let Ok(index) = SessionSearchIndex::open_default() else {
        return Vec::new();
    };
    index.query(query, cwd, limit).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn doc(id: &str, title: &str, content: &str) -> SessionDoc {
        SessionDoc {
            session_id: id.into(),
            cwd: "/ws".into(),
            updated_at_unix: 1,
            title: title.into(),
            content: content.into(),
            content_hash: content_hash(content),
        }
    }

    #[test]
    fn upsert_and_query_title_and_content() {
        let tmp = TempDir::new().unwrap();
        let index = SessionSearchIndex::open_or_create(&tmp.path().join("t.sqlite")).unwrap();
        index
            .upsert_doc(&doc("s1", "Rust debugging", "fix the borrow checker"))
            .unwrap();
        let hits = index.query("borrow", None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "s1");
        let by_title = index.query("Rust", None, 10).unwrap();
        assert_eq!(by_title.len(), 1);
    }

    #[test]
    fn cwd_filter_and_delete() {
        let tmp = TempDir::new().unwrap();
        let index = SessionSearchIndex::open_or_create(&tmp.path().join("t.sqlite")).unwrap();
        let mut a = doc("a", "alpha", "cargo build");
        a.cwd = "/a".into();
        let mut b = doc("b", "beta", "cargo test");
        b.cwd = "/b".into();
        index.upsert_doc(&a).unwrap();
        index.upsert_doc(&b).unwrap();
        assert_eq!(index.query("cargo", None, 10).unwrap().len(), 2);
        assert_eq!(index.query("cargo", Some("/a"), 10).unwrap().len(), 1);
        index.delete_doc("a").unwrap();
        assert_eq!(index.query("cargo", None, 10).unwrap().len(), 1);
    }
}
