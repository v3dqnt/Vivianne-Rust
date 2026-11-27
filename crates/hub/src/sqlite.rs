//! The project store on SQLite. A commit is one `BEGIN IMMEDIATE` transaction: guards, writes and audit events
//! land together or not at all. A database from a newer build is refused, not rewritten (R3.5).

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rusqlite::{
    Connection, OptionalExtension as _, Params, Transaction, TransactionBehavior, params,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use vivianne_projects::audit;
use vivianne_projects::join::{JoinRequest, State};
use vivianne_projects::membership::{Membership, Status, Transfer};
use vivianne_projects::model::{Lifecycle, Project, ProjectId, UserId, Visibility};
use vivianne_projects::store::{Guard, MAX_PAGE, Store, StoreError, Stored, Write};

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE projects (id TEXT PRIMARY KEY, slug TEXT NOT NULL UNIQUE, listed INTEGER NOT NULL,
    revision INTEGER NOT NULL, data TEXT NOT NULL);
CREATE TABLE members (project TEXT NOT NULL, user TEXT NOT NULL, removed INTEGER NOT NULL,
    revision INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY (project, user));
CREATE TABLE joins (id TEXT PRIMARY KEY, project TEXT NOT NULL, user TEXT NOT NULL, pending INTEGER NOT NULL,
    created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, revision INTEGER NOT NULL, data TEXT NOT NULL);
CREATE INDEX joins_of ON joins (project, user);
CREATE INDEX joins_pending ON joins (project, pending, created_at, id);
CREATE TABLE transfers (project TEXT PRIMARY KEY, data TEXT NOT NULL);
CREATE TABLE blocks (project TEXT NOT NULL, user TEXT NOT NULL, PRIMARY KEY (project, user));
CREATE TABLE keys (user TEXT NOT NULL, key TEXT NOT NULL, resource TEXT NOT NULL, PRIMARY KEY (user, key));
CREATE TABLE audit (seq INTEGER PRIMARY KEY AUTOINCREMENT, project TEXT NOT NULL, data TEXT NOT NULL);
CREATE INDEX audit_of ON audit (project, seq);
CREATE TRIGGER audit_no_update BEFORE UPDATE ON audit BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;
CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;
CREATE TABLE sessions (hash BLOB PRIMARY KEY, user TEXT NOT NULL, expires_at INTEGER NOT NULL);
CREATE INDEX sessions_of ON sessions (user);
";

#[derive(Debug)]
pub struct Database(Mutex<Connection>);

#[derive(Debug)]
pub enum OpenError {
    Newer(i64),
    Sql(rusqlite::Error),
}

impl std::error::Error for OpenError {}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Newer(v) => write!(
                f,
                "the database is from a newer viv-hub (format {v}, this one reads {SCHEMA_VERSION}); it was not touched"
            ),
            Self::Sql(e) => write!(f, "the database could not be opened: {e}"),
        }
    }
}

impl From<rusqlite::Error> for OpenError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}

/// A database failure, said on stderr without any record's contents.
pub(crate) fn unavailable(e: impl std::fmt::Display) -> StoreError {
    eprintln!("viv-hub: storage: {e}");
    StoreError::Unavailable
}

fn number(value: u64) -> Stored<i64> {
    i64::try_from(value).map_err(unavailable)
}

fn encode(value: &impl Serialize) -> Stored<String> {
    serde_json::to_string(value).map_err(unavailable)
}

fn rows<T: DeserializeOwned>(c: &Connection, sql: &str, args: impl Params) -> Stored<Vec<T>> {
    let mut statement = c.prepare(sql).map_err(unavailable)?;
    let found = statement
        .query_map(args, |row| row.get::<_, String>(0))
        .map_err(unavailable)?;
    found
        .map(|data| serde_json::from_str(&data.map_err(unavailable)?).map_err(unavailable))
        .collect()
}

fn row<T: DeserializeOwned>(c: &Connection, sql: &str, args: impl Params) -> Stored<Option<T>> {
    Ok(rows(c, sql, args)?.into_iter().next())
}

fn scalar(c: &Connection, sql: &str, args: impl Params) -> Stored<Option<i64>> {
    c.query_row(sql, args, |r| r.get(0))
        .optional()
        .map_err(unavailable)
}

fn page(limit: usize) -> Stored<i64> {
    i64::try_from(limit.min(MAX_PAGE)).map_err(unavailable)
}

fn member_count(c: &Connection, project: &ProjectId) -> Stored<usize> {
    let n = scalar(
        c,
        "SELECT COUNT(*) FROM members WHERE project = ?1 AND removed = 0",
        [&project.0],
    )?;
    usize::try_from(n.unwrap_or(0)).map_err(unavailable)
}

fn holds(tx: &Transaction<'_>, guard: &Guard) -> Stored<bool> {
    Ok(match guard {
        Guard::Project(id, revision) => {
            let found = scalar(tx, "SELECT revision FROM projects WHERE id = ?1", [&id.0])?;
            found == Some(number(*revision)?)
        }
        Guard::Member(project, user, revision) => {
            let found = scalar(
                tx,
                "SELECT revision FROM members WHERE project = ?1 AND user = ?2",
                [&project.0, &user.0],
            )?;
            found == Some(number(*revision)?)
        }
        Guard::Members(project, count) => member_count(tx, project)? == *count,
    })
}

/// A new record has revision 1; a change follows the stored one by exactly one.
fn follows(stored: Option<i64>, written: u64) -> Stored<()> {
    if stored.map_or(1, |r| r + 1) == number(written)? {
        Ok(())
    } else {
        Err(StoreError::Conflict)
    }
}

fn run(c: &Connection, sql: &str, args: impl Params) -> Stored<()> {
    c.execute(sql, args).map(|_| ()).map_err(unavailable)
}

fn apply(tx: &Transaction<'_>, write: Write) -> Stored<()> {
    match write {
        Write::Project(p) => {
            let stored = scalar(tx, "SELECT revision FROM projects WHERE id = ?1", [&p.id.0])?;
            follows(stored, p.revision)?;
            let taken = scalar(
                tx,
                "SELECT 1 FROM projects WHERE slug = ?1 AND id <> ?2",
                [&p.slug, &p.id.0],
            )?;
            if taken.is_some() {
                return Err(StoreError::SlugTaken);
            }
            let listed = p.visibility == Visibility::Public && p.lifecycle == Lifecycle::Active;
            run(
                tx,
                "INSERT INTO projects (id, slug, listed, revision, data) VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(id) DO UPDATE SET slug = ?2, listed = ?3, revision = ?4, data = ?5",
                params![p.id.0, p.slug, listed, number(p.revision)?, encode(&p)?],
            )
        }
        Write::Member(m) => {
            let stored = scalar(
                tx,
                "SELECT revision FROM members WHERE project = ?1 AND user = ?2",
                [&m.project.0, &m.user.0],
            )?;
            follows(stored, m.revision)?;
            let removed = m.status == Status::Removed;
            run(
                tx,
                "INSERT OR REPLACE INTO members (project, user, removed, revision, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    m.project.0,
                    m.user.0,
                    removed,
                    number(m.revision)?,
                    encode(&m)?
                ],
            )
        }
        Write::Join(j) => apply_join(tx, &j),
        Write::Transfer(project, Some(transfer)) => run(
            tx,
            "INSERT OR REPLACE INTO transfers (project, data) VALUES (?1, ?2)",
            [project.0, encode(&transfer)?],
        ),
        Write::Transfer(project, None) => {
            run(tx, "DELETE FROM transfers WHERE project = ?1", [project.0])
        }
        Write::Block(project, user) => run(
            tx,
            "INSERT OR IGNORE INTO blocks (project, user) VALUES (?1, ?2)",
            [project.0, user.0],
        ),
        Write::Key {
            user,
            key,
            resource,
        } => {
            let used = scalar(
                tx,
                "SELECT 1 FROM keys WHERE user = ?1 AND key = ?2",
                [&user.0, &key],
            )?;
            if used.is_some() {
                return Err(StoreError::Conflict);
            }
            run(
                tx,
                "INSERT INTO keys (user, key, resource) VALUES (?1, ?2, ?3)",
                [user.0, key, resource],
            )
        }
    }
}

fn apply_join(tx: &Transaction<'_>, j: &JoinRequest) -> Stored<()> {
    let stored = scalar(tx, "SELECT revision FROM joins WHERE id = ?1", [&j.id])?;
    follows(stored, j.revision)?;
    let pending = j.state == State::Pending;
    // One live request per person and project; the transaction holds the write lock while it looks.
    if pending {
        let twin = scalar(
            tx,
            "SELECT 1 FROM joins WHERE project = ?1 AND user = ?2 AND pending = 1 AND id <> ?3 AND expires_at > ?4",
            params![j.project.0, j.user.0, j.id, number(j.created_at)?],
        )?;
        if twin.is_some() {
            return Err(StoreError::Conflict);
        }
    }
    run(
        tx,
        "INSERT OR REPLACE INTO joins (id, project, user, pending, created_at, expires_at, revision, data) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            j.id,
            j.project.0,
            j.user.0,
            pending,
            number(j.created_at)?,
            // An expiry past what a column holds means it does not expire in practice.
            i64::try_from(j.expires_at).unwrap_or(i64::MAX),
            number(j.revision)?,
            encode(j)?
        ],
    )
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        Self::prepare(Connection::open(path)?)
    }

    /// A database that lives only as long as this value.
    pub fn in_memory() -> Result<Self, OpenError> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(mut connection: Connection) -> Result<Self, OpenError> {
        connection.busy_timeout(Duration::from_secs(5))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(OpenError::Newer(version));
        }
        // `journal_mode` answers with a row, so it is queried and not executed.
        connection.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        if version == 0 {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(SCHEMA)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            tx.commit()?;
        }
        Ok(Self(Mutex::new(connection)))
    }

    pub(crate) fn connection(&self) -> MutexGuard<'_, Connection> {
        // A panic mid-transaction rolls it back, so the connection is still sound.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Store for Database {
    fn project(&self, id: &ProjectId) -> Stored<Option<Project>> {
        row(
            &self.connection(),
            "SELECT data FROM projects WHERE id = ?1",
            [&id.0],
        )
    }

    fn public_projects(&self, after: Option<&str>, limit: usize) -> Stored<Vec<Project>> {
        rows(
            &self.connection(),
            "SELECT data FROM projects WHERE listed = 1 AND slug > ?1 ORDER BY slug LIMIT ?2",
            params![after.unwrap_or(""), page(limit)?],
        )
    }

    fn member(&self, project: &ProjectId, user: &UserId) -> Stored<Option<Membership>> {
        row(
            &self.connection(),
            "SELECT data FROM members WHERE project = ?1 AND user = ?2",
            [&project.0, &user.0],
        )
    }

    fn member_count(&self, project: &ProjectId) -> Stored<usize> {
        member_count(&self.connection(), project)
    }

    fn members(
        &self,
        project: &ProjectId,
        after: Option<&UserId>,
        limit: usize,
    ) -> Stored<Vec<Membership>> {
        rows(
            &self.connection(),
            "SELECT data FROM members WHERE project = ?1 AND user > ?2 ORDER BY user LIMIT ?3",
            params![project.0, after.map_or("", |u| u.0.as_str()), page(limit)?],
        )
    }

    fn join(&self, id: &str) -> Stored<Option<JoinRequest>> {
        row(
            &self.connection(),
            "SELECT data FROM joins WHERE id = ?1",
            [id],
        )
    }

    fn joins_of(&self, project: &ProjectId, user: &UserId) -> Stored<Vec<JoinRequest>> {
        rows(
            &self.connection(),
            "SELECT data FROM joins WHERE project = ?1 AND user = ?2",
            [&project.0, &user.0],
        )
    }

    fn pending_joins(
        &self,
        project: &ProjectId,
        now: u64,
        limit: usize,
    ) -> Stored<Vec<JoinRequest>> {
        rows(
            &self.connection(),
            "SELECT data FROM joins WHERE project = ?1 AND pending = 1 AND expires_at > ?2 \
             ORDER BY created_at, id LIMIT ?3",
            params![
                project.0,
                number(now)?,
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
        )
    }

    fn transfer(&self, project: &ProjectId) -> Stored<Option<Transfer>> {
        row(
            &self.connection(),
            "SELECT data FROM transfers WHERE project = ?1",
            [&project.0],
        )
    }

    fn is_blocked(&self, project: &ProjectId, user: &UserId) -> Stored<bool> {
        let found = scalar(
            &self.connection(),
            "SELECT 1 FROM blocks WHERE project = ?1 AND user = ?2",
            [&project.0, &user.0],
        )?;
        Ok(found.is_some())
    }

    fn audit(&self, project: &ProjectId, limit: usize) -> Stored<Vec<audit::Event>> {
        let mut found: Vec<audit::Event> = rows(
            &self.connection(),
            "SELECT data FROM audit WHERE project = ?1 ORDER BY seq DESC LIMIT ?2",
            params![project.0, page(limit)?],
        )?;
        found.reverse();
        Ok(found)
    }

    fn key(&self, user: &UserId, key: &str) -> Stored<Option<String>> {
        self.connection()
            .query_row(
                "SELECT resource FROM keys WHERE user = ?1 AND key = ?2",
                [user.0.as_str(), key],
                |r| r.get(0),
            )
            .optional()
            .map_err(unavailable)
    }

    fn commit(
        &self,
        guards: Vec<Guard>,
        writes: Vec<Write>,
        events: Vec<audit::Event>,
    ) -> Stored<()> {
        let mut connection = self.connection();
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        for guard in &guards {
            if !holds(&tx, guard)? {
                return Err(StoreError::Conflict);
            }
        }
        for write in writes {
            apply(&tx, write)?;
        }
        for event in events {
            run(
                &tx,
                "INSERT INTO audit (project, data) VALUES (?1, ?2)",
                [event.project.0.clone(), encode(&event)?],
            )?;
        }
        // Dropping the transaction on any early return above rolls it back.
        tx.commit().map_err(unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Database {
        Database::in_memory().expect("opens")
    }

    #[test]
    fn sqlite_passes_the_same_scenarios_as_the_memory_store() {
        vivianne_projects::scenarios::all(fresh);
    }

    #[test]
    fn records_survive_reopening_and_a_newer_database_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("viv-hub-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("hub.db");
        let write = |db: &Database| {
            db.commit(
                Vec::new(),
                vec![Write::Block("p".into(), "u".into())],
                Vec::new(),
            )
        };
        write(&Database::open(&path).expect("created")).expect("written");
        let again = Database::open(&path).expect("reopened");
        assert_eq!(again.is_blocked(&"p".into(), &"u".into()), Ok(true));
        drop(again);
        let raw = Connection::open(&path).expect("raw");
        raw.pragma_update(None, "user_version", 99).expect("set");
        drop(raw);
        assert!(matches!(Database::open(&path), Err(OpenError::Newer(99))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audit_rows_cannot_be_changed_or_removed() {
        let db = fresh();
        db.connection()
            .execute("INSERT INTO audit (project, data) VALUES ('p', '{}')", [])
            .expect("appended");
        for sql in ["UPDATE audit SET data = 'x'", "DELETE FROM audit"] {
            assert!(db.connection().execute(sql, []).is_err(), "{sql}");
        }
    }
}
