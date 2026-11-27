//! Ranked full-text search over saved conversations (the original `session-query-sqlite`), on SQLite FTS5.
//!
//! The index is a database of its own, derived from the saved threads and never the other way round: it can be
//! deleted at any time and rebuilt, and it is rebuilt when it is damaged or written by another version. Threads are
//! re-indexed when their text changes and dropped when they are gone. Results match words and phrases, not arbitrary
//! pieces of words. One process owns an index file at a time (SQLite's own lock enforces it).

use std::path::Path;

use rusqlite::types::ToSql;
use rusqlite::{Connection, TransactionBehavior, params_from_iter};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use super::filters::{
    EventFilter, Kind, Surface, ThreadFilter, check_event_filters, check_thread_filters,
};
use super::{EventDoc, EventRecord, Query, QueryError};

const VERSION: i64 = 1;
pub const DEFAULT_LIMIT: usize = 20;
pub const MAX_LIMIT: usize = 100;
const SNIPPET_WORDS: u32 = 14;

const SCHEMA: &str = "
CREATE TABLE threads (thread TEXT PRIMARY KEY, workspace TEXT NOT NULL, updated_at INTEGER NOT NULL,
    parent TEXT, fingerprint TEXT NOT NULL);
CREATE TABLE docs (id INTEGER PRIMARY KEY, thread TEXT NOT NULL, generation INTEGER NOT NULL, seq INTEGER NOT NULL,
    kind TEXT NOT NULL, time INTEGER, surface TEXT NOT NULL, text TEXT NOT NULL);
CREATE INDEX docs_thread ON docs (thread, generation, seq);
CREATE VIRTUAL TABLE docs_fts USING fts5(text, content='docs', content_rowid='id', tokenize='unicode61 remove_diacritics 2');
";

fn storage(e: impl std::fmt::Display) -> QueryError {
    QueryError::Storage(e.to_string())
}

#[derive(Debug)]
pub struct Index {
    db: Connection,
}

/// What a reconcile did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Reconciled {
    pub indexed: usize,
    pub unchanged: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventHit {
    #[serde(flatten)]
    pub record: EventRecord,
    /// The words around the match, with the matched words in `[brackets]`.
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreadHit {
    pub thread: String,
    pub workspace: String,
    pub updated_at: u64,
    /// The best match in the thread.
    pub best: EventHit,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Pass back to get the next page; absent on the last.
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Search {
    pub query: String,
    pub threads: Vec<ThreadFilter>,
    /// Metadata filters only: the words are the query's job.
    pub events: Vec<EventFilter>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

fn fingerprint(docs: &[EventDoc]) -> String {
    let mut sha = Sha256::new();
    for d in docs {
        let r = &d.record;
        sha.update(
            format!(
                "{}|{}|{}|{:?}|{:?}|{}\n",
                r.thread, r.generation, r.seq, r.kind, r.time, d.text
            )
            .as_bytes(),
        );
    }
    format!("{:x}", sha.finalize())
}

/// A query a person typed, made into one FTS5 can only read as words and quoted phrases (ANDed). Nothing a person
/// types is an operator: `NEAR`, `OR`, `*`, `-` and a stray quote are plain text.
pub fn match_expression(text: &str) -> Result<String, QueryError> {
    let mut terms: Vec<String> = Vec::new();
    let mut phrase: Option<String> = None;
    let mut word = String::new();
    let mut close = |word: &mut String, terms: &mut Vec<String>| {
        let w: String = std::mem::take(word);
        if w.chars().any(char::is_alphanumeric) {
            terms.push(format!("\"{w}\""));
        }
    };
    for c in text.chars() {
        match (&mut phrase, c) {
            (None, '"') => {
                close(&mut word, &mut terms);
                phrase = Some(String::new());
            }
            (Some(p), '"') => {
                let p = std::mem::take(p);
                phrase = None;
                if p.chars().any(char::is_alphanumeric) {
                    terms.push(format!("\"{}\"", p.trim()));
                }
            }
            (Some(p), c) => p.push(c),
            (None, c) if c.is_whitespace() => close(&mut word, &mut terms),
            (None, c) => word.push(c),
        }
    }
    // An unclosed quote is a phrase up to the end.
    if let Some(p) = phrase
        && p.chars().any(char::is_alphanumeric)
    {
        terms.push(format!("\"{}\"", p.trim()));
    }
    close(&mut word, &mut terms);
    if terms.is_empty() {
        return Err(QueryError::InvalidFilter(
            "a search needs at least one word".to_owned(),
        ));
    }
    Ok(terms.join(" "))
}

fn encode_cursor(rank: f64, id: i64) -> String {
    format!("{:016x}.{id}", rank.to_bits())
}

fn decode_cursor(cursor: &str) -> Result<(f64, i64), QueryError> {
    let (rank, id) = cursor.split_once('.').ok_or(QueryError::InvalidCursor)?;
    let rank = u64::from_str_radix(rank, 16).map_err(|_| QueryError::InvalidCursor)?;
    let id: i64 = id.parse().map_err(|_| QueryError::InvalidCursor)?;
    let rank = f64::from_bits(rank);
    if rank.is_nan() {
        return Err(QueryError::InvalidCursor);
    }
    Ok((rank, id))
}

/// The WHERE clauses for the filters, and what they bind.
fn clauses(
    threads: &[ThreadFilter],
    events: &[EventFilter],
) -> Result<(String, Vec<Box<dyn ToSql>>), QueryError> {
    check_thread_filters(threads)?;
    check_event_filters(events)?;
    let mut sql = String::new();
    let mut args: Vec<Box<dyn ToSql>> = Vec::new();
    let mut list =
        |sql: &mut String, args: &mut Vec<Box<dyn ToSql>>, column: &str, values: Vec<String>| {
            if values.is_empty() {
                sql.push_str(" AND 0");
                return;
            }
            let marks = vec!["?"; values.len()].join(",");
            sql.push_str(&format!(" AND {column} IN ({marks})"));
            args.extend(values.into_iter().map(|v| Box::new(v) as Box<dyn ToSql>));
        };
    let range = |sql: &mut String,
                 args: &mut Vec<Box<dyn ToSql>>,
                 column: &str,
                 r: super::filters::Range| {
        for (op, bound) in [(">=", r.from), ("<=", r.to)] {
            if let Some(b) = bound {
                sql.push_str(&format!(" AND {column} {op} ?"));
                args.push(Box::new(i64::try_from(b).unwrap_or(i64::MAX)));
            }
        }
    };
    for filter in threads {
        match filter {
            ThreadFilter::Id(v) => list(&mut sql, &mut args, "d.thread", v.clone()),
            ThreadFilter::Workspace(v) => list(&mut sql, &mut args, "t.workspace", v.clone()),
            ThreadFilter::UpdatedAt(r) => range(&mut sql, &mut args, "t.updated_at", *r),
            ThreadFilter::Parent(v) => {
                let named: Vec<String> = v.iter().flatten().cloned().collect();
                let none = v.iter().any(Option::is_none);
                let marks = vec!["?"; named.len()].join(",");
                let mut parts = Vec::new();
                if !named.is_empty() {
                    parts.push(format!("t.parent IN ({marks})"));
                }
                if none {
                    parts.push("t.parent IS NULL".to_owned());
                }
                if parts.is_empty() {
                    parts.push("0".to_owned());
                }
                sql.push_str(&format!(" AND ({})", parts.join(" OR ")));
                args.extend(named.into_iter().map(|v| Box::new(v) as Box<dyn ToSql>));
            }
        }
    }
    for filter in events {
        match filter {
            EventFilter::Seq(r) => range(&mut sql, &mut args, "d.seq", *r),
            EventFilter::Time(r) => range(&mut sql, &mut args, "d.time", *r),
            EventFilter::Kind(v) => list(
                &mut sql,
                &mut args,
                "d.kind",
                v.iter().map(|k| k.as_str().to_owned()).collect(),
            ),
            EventFilter::Surface(v) => list(
                &mut sql,
                &mut args,
                "d.surface",
                v.iter().map(|s| s.as_str().to_owned()).collect(),
            ),
            EventFilter::Text(_) => {
                return Err(QueryError::InvalidFilter(
                    "a search takes its words as the query, not as a text filter".to_owned(),
                ));
            }
        }
    }
    Ok((sql, args))
}

fn hit_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, f64, EventHit)> {
    let kind: String = row.get(4)?;
    let surface: String = row.get(6)?;
    Ok((
        row.get(0)?,
        row.get(7)?,
        EventHit {
            record: EventRecord {
                thread: row.get(1)?,
                generation: row.get(2)?,
                seq: usize::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                kind: Kind::parse(&kind).unwrap_or(Kind::Context),
                time: row
                    .get::<_, Option<i64>>(5)?
                    .and_then(|t| u64::try_from(t).ok()),
                surface: if surface == "current" {
                    Surface::Current
                } else {
                    Surface::Shadowed
                },
            },
            snippet: row.get(8)?,
        },
    ))
}

impl Index {
    /// Opens the index at `path`, making it if there is none and making it again if it is damaged or from another
    /// version: it holds nothing that cannot be rebuilt.
    pub fn open(path: &Path) -> Result<Self, QueryError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(storage)?;
        }
        match Self::try_open(path) {
            Ok(index) => Ok(index),
            Err(_) => {
                for suffix in ["", "-wal", "-shm"] {
                    let mut name = path.as_os_str().to_owned();
                    name.push(suffix);
                    let _ = std::fs::remove_file(name);
                }
                Self::try_open(path)
            }
        }
    }

    fn try_open(path: &Path) -> Result<Self, QueryError> {
        let db = Connection::open(path).map_err(storage)?;
        Self::prepare(db)
    }

    pub fn in_memory() -> Result<Self, QueryError> {
        Self::prepare(Connection::open_in_memory().map_err(storage)?)
    }

    fn prepare(mut db: Connection) -> Result<Self, QueryError> {
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage)?;
        let version: i64 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(storage)?;
        if version != 0 && version != VERSION {
            return Err(QueryError::Storage(format!("index format {version}")));
        }
        let _ = db.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()));
        if version == 0 {
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            tx.execute_batch(SCHEMA).map_err(storage)?;
            tx.pragma_update(None, "user_version", VERSION)
                .map_err(storage)?;
            tx.commit().map_err(storage)?;
        }
        // A file that is not a database, or lost a table, fails here and is rebuilt by `open`.
        db.query_row("SELECT count(*) FROM docs_fts", [], |r| r.get::<_, i64>(0))
            .map_err(storage)?;
        Ok(Self { db })
    }

    /// Brings the index in line with the saved threads: new and changed ones indexed, vanished ones dropped.
    pub fn reconcile(&mut self, query: &Query) -> Result<Reconciled, QueryError> {
        let threads = crate::store::all_in(query.dir());
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let mut done = Reconciled::default();
        let mut present = std::collections::BTreeSet::new();
        for thread in &threads {
            present.insert(thread.id.clone());
            let docs = query.documents_of(thread);
            let print = fingerprint(&docs);
            let known: Option<String> = tx
                .query_row(
                    "SELECT fingerprint FROM threads WHERE thread = ?1",
                    [&thread.id],
                    |r| r.get(0),
                )
                .ok();
            let parent = thread.parent.as_ref().map(|p| p.thread.clone());
            let updated = i64::try_from(thread.updated_at).unwrap_or(i64::MAX);
            if known.as_deref() == Some(print.as_str()) {
                tx.execute(
                    "UPDATE threads SET workspace = ?2, updated_at = ?3, parent = ?4 WHERE thread = ?1",
                    rusqlite::params![thread.id, thread.workspace, updated, parent],
                )
                .map_err(storage)?;
                done.unchanged += 1;
                continue;
            }
            drop_docs(&tx, &thread.id)?;
            for d in docs.iter().filter(|d| !d.text.is_empty()) {
                insert_doc(&tx, d)?;
            }
            tx.execute(
                "INSERT INTO threads (thread, workspace, updated_at, parent, fingerprint) VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(thread) DO UPDATE SET workspace = ?2, updated_at = ?3, parent = ?4, fingerprint = ?5",
                rusqlite::params![thread.id, thread.workspace, updated, parent, print],
            )
            .map_err(storage)?;
            done.indexed += 1;
        }
        let stale: Vec<String> = {
            let mut statement = tx.prepare("SELECT thread FROM threads").map_err(storage)?;
            let all = statement
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(storage)?;
            all.filter_map(Result::ok)
                .filter(|t| !present.contains(t))
                .collect()
        };
        for thread in &stale {
            drop_docs(&tx, thread)?;
            tx.execute("DELETE FROM threads WHERE thread = ?1", [thread])
                .map_err(storage)?;
            done.removed += 1;
        }
        tx.commit().map_err(storage)?;
        Ok(done)
    }

    /// Words in any thread, best thread first, each with its best match.
    pub fn search_threads(&self, search: &Search) -> Result<Page<ThreadHit>, QueryError> {
        let (filters, mut args) = clauses(&search.threads, &search.events)?;
        let limit = limit_of(search.limit);
        let expression = match_expression(&search.query)?;
        let cursor = search.cursor.as_deref().map(decode_cursor).transpose()?;
        let mut bound: Vec<Box<dyn ToSql>> = vec![Box::new(expression)];
        bound.append(&mut args);
        let after = cursor_sql(cursor, &mut bound);
        let sql = format!(
            "SELECT * FROM (
               SELECT id, thread, generation, seq, kind, time, surface, rank, snip, workspace, updated_at,
                      row_number() OVER (PARTITION BY thread ORDER BY rank, id) AS n
               FROM (
                 SELECT d.id AS id, d.thread AS thread, d.generation AS generation, d.seq AS seq, d.kind AS kind,
                        d.time AS time, d.surface AS surface, bm25(docs_fts) AS rank,
                        snippet(docs_fts, 0, '[', ']', '…', {SNIPPET_WORDS}) AS snip,
                        t.workspace AS workspace, t.updated_at AS updated_at
                 FROM docs_fts JOIN docs d ON d.id = docs_fts.rowid JOIN threads t ON t.thread = d.thread
                 WHERE docs_fts MATCH ?{filters}
               )
             ) WHERE n = 1{after} ORDER BY rank, id LIMIT {}",
            limit + 1
        );
        let mut statement = self.db.prepare(&sql).map_err(storage)?;
        let mut found = statement
            .query_map(params_from_iter(bound.iter().map(AsRef::as_ref)), |row| {
                let (id, rank, hit) = hit_of(row)?;
                Ok((
                    id,
                    rank,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                    hit,
                ))
            })
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        let next_cursor = overflow(&mut found, limit, |(id, rank, ..)| {
            encode_cursor(*rank, *id)
        });
        Ok(Page {
            items: found
                .into_iter()
                .map(|(_, _, workspace, updated_at, best)| ThreadHit {
                    thread: best.record.thread.clone(),
                    workspace,
                    updated_at: u64::try_from(updated_at).unwrap_or(0),
                    best,
                })
                .collect(),
            next_cursor,
        })
    }

    /// Words in one thread, best match first.
    pub fn search_events(
        &self,
        thread: &str,
        search: &Search,
    ) -> Result<Page<EventHit>, QueryError> {
        let (filters, mut args) = clauses(&search.threads, &search.events)?;
        let limit = limit_of(search.limit);
        let expression = match_expression(&search.query)?;
        let cursor = search.cursor.as_deref().map(decode_cursor).transpose()?;
        let mut bound: Vec<Box<dyn ToSql>> =
            vec![Box::new(expression), Box::new(thread.to_owned())];
        bound.append(&mut args);
        let after = cursor_sql(cursor, &mut bound);
        let sql = format!(
            "SELECT * FROM (
               SELECT d.id AS id, d.thread AS thread, d.generation AS generation, d.seq AS seq, d.kind AS kind,
                      d.time AS time, d.surface AS surface, bm25(docs_fts) AS rank,
                      snippet(docs_fts, 0, '[', ']', '…', {SNIPPET_WORDS}) AS snip
               FROM docs_fts JOIN docs d ON d.id = docs_fts.rowid JOIN threads t ON t.thread = d.thread
               WHERE docs_fts MATCH ? AND d.thread = ?{filters}
             ) WHERE 1 = 1{after} ORDER BY rank, id LIMIT {}",
            limit + 1
        );
        let mut statement = self.db.prepare(&sql).map_err(storage)?;
        let mut found = statement
            .query_map(params_from_iter(bound.iter().map(AsRef::as_ref)), hit_of)
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        let next_cursor = overflow(&mut found, limit, |(id, rank, _)| encode_cursor(*rank, *id));
        Ok(Page {
            items: found.into_iter().map(|(_, _, hit)| hit).collect(),
            next_cursor,
        })
    }
}

fn limit_of(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// The keyset condition for "after this (rank, id)", binding its values.
fn cursor_sql(cursor: Option<(f64, i64)>, bound: &mut Vec<Box<dyn ToSql>>) -> &'static str {
    match cursor {
        Some((rank, id)) => {
            bound.extend([
                Box::new(rank) as Box<dyn ToSql>,
                Box::new(rank),
                Box::new(id),
            ]);
            " AND (rank > ? OR (rank = ? AND id > ?))"
        }
        None => "",
    }
}

/// Takes the one extra row off the end and, if there was one, makes the cursor from the last row kept.
fn overflow<T>(rows: &mut Vec<T>, limit: usize, cursor: impl Fn(&T) -> String) -> Option<String> {
    if rows.len() <= limit {
        return None;
    }
    rows.truncate(limit);
    rows.last().map(cursor)
}

fn drop_docs(tx: &rusqlite::Transaction<'_>, thread: &str) -> Result<(), QueryError> {
    tx.execute(
        "INSERT INTO docs_fts (docs_fts, rowid, text) SELECT 'delete', id, text FROM docs WHERE thread = ?1",
        [thread],
    )
    .map_err(storage)?;
    tx.execute("DELETE FROM docs WHERE thread = ?1", [thread])
        .map_err(storage)?;
    Ok(())
}

fn insert_doc(tx: &rusqlite::Transaction<'_>, d: &EventDoc) -> Result<(), QueryError> {
    let r = &d.record;
    tx.execute(
        "INSERT INTO docs (thread, generation, seq, kind, time, surface, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            r.thread,
            r.generation,
            i64::try_from(r.seq).unwrap_or(i64::MAX),
            r.kind.as_str(),
            r.time.and_then(|t| i64::try_from(t).ok()),
            r.surface.as_str(),
            d.text
        ],
    )
    .map_err(storage)?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO docs_fts (rowid, text) VALUES (?1, ?2)",
        rusqlite::params![id, d.text],
    )
    .map_err(storage)?;
    Ok(())
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
