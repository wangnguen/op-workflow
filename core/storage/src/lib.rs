//! Local SQLite store. Only this crate knows the schema.
//!
//! Migrations are an append-only list; `PRAGMA user_version` records how many ran.
//! Never edit a released migration, add a new one instead.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema version {found} is newer than this app ({supported})")]
    TooNew { found: u32, supported: u32 },
}

pub type StorageResult<T> = Result<T, StorageError>;

const MIGRATIONS: &[&str] = &[
    // 1: tasks the user started, step runs, findings, app event log
    "CREATE TABLE task (
        wp_id       INTEGER PRIMARY KEY,
        display_id  TEXT NOT NULL,
        title       TEXT NOT NULL,
        project     TEXT NOT NULL,
        repo        TEXT,
        branch      TEXT,
        base        TEXT,
        column_name TEXT NOT NULL DEFAULT 'inbox',
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL
     );
     CREATE TABLE step_run (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        wp_id       INTEGER NOT NULL REFERENCES task(wp_id),
        step        TEXT NOT NULL,
        agent       TEXT,
        session_id  TEXT,
        state       TEXT NOT NULL,
        round       INTEGER,
        cost_usd    REAL,
        tokens_in   INTEGER,
        tokens_out  INTEGER,
        started_at  TEXT NOT NULL,
        ended_at    TEXT
     );
     CREATE TABLE finding (
        wp_id       INTEGER NOT NULL REFERENCES task(wp_id),
        fingerprint TEXT NOT NULL,
        severity    TEXT NOT NULL,
        title       TEXT NOT NULL,
        file        TEXT NOT NULL,
        line        INTEGER,
        source      TEXT NOT NULL,
        state       TEXT NOT NULL,
        reason      TEXT,
        PRIMARY KEY (wp_id, fingerprint)
     );
     CREATE TABLE event_log (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        at          TEXT NOT NULL,
        wp_id       INTEGER,
        kind        TEXT NOT NULL,
        detail      TEXT
     );",
];

/// Schema version this build understands.
pub fn schema_version() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub wp_id: u64,
    pub display_id: String,
    pub title: String,
    pub project: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub column: String,
    pub updated_at: String,
}

const TASK_COLUMNS: &str = "wp_id, display_id, title, project, repo, branch, base, column_name, updated_at";

fn task_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        wp_id: u64::try_from(r.get::<_, i64>(0)?).unwrap_or_default(),
        display_id: r.get(1)?,
        title: r.get(2)?,
        project: r.get(3)?,
        repo: r.get(4)?,
        branch: r.get(5)?,
        base: r.get(6)?,
        column: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

pub struct Store {
    conn: Connection,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    pub fn open(path: &Path) -> StorageResult<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> StorageResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> StorageResult<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn version(&self) -> StorageResult<u32> {
        Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    fn migrate(&mut self) -> StorageResult<()> {
        let current = self.version()?;
        let supported = schema_version();
        if current > supported {
            return Err(StorageError::TooNew {
                found: current,
                supported,
            });
        }
        let tx = self.conn.transaction()?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", u32::try_from(i + 1).unwrap_or(u32::MAX))?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_task(&self, t: &TaskRow) -> StorageResult<()> {
        self.conn.execute(
            "INSERT INTO task (wp_id, display_id, title, project, repo, branch, base, column_name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT(wp_id) DO UPDATE SET
               display_id = excluded.display_id, title = excluded.title, project = excluded.project,
               repo = excluded.repo, branch = excluded.branch, base = excluded.base,
               column_name = excluded.column_name, updated_at = excluded.updated_at",
            params![
                i64::try_from(t.wp_id).unwrap_or(i64::MAX),
                t.display_id,
                t.title,
                t.project,
                t.repo,
                t.branch,
                t.base,
                t.column,
                t.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn task(&self, wp_id: u64) -> StorageResult<Option<TaskRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {TASK_COLUMNS} FROM task WHERE wp_id = ?1"),
                [i64::try_from(wp_id).unwrap_or(i64::MAX)],
                task_row,
            )
            .optional()?)
    }

    /// Tasks in the "Đang làm" column, most recently started or resumed first. Several can run
    /// at once.
    pub fn active_tasks(&self) -> StorageResult<Vec<TaskRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM task WHERE column_name = 'active'
             ORDER BY updated_at DESC, wp_id"
        ))?;
        let rows = stmt.query_map([], task_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Move a task to another board column. Returns `false` when the task is unknown.
    pub fn set_column(&self, wp_id: u64, column: &str, at: &str) -> StorageResult<bool> {
        let n = self.conn.execute(
            "UPDATE task SET column_name = ?2, updated_at = ?3 WHERE wp_id = ?1",
            params![i64::try_from(wp_id).unwrap_or(i64::MAX), column, at],
        )?;
        Ok(n > 0)
    }

    pub fn log_event(
        &self,
        at: &str,
        wp_id: Option<u64>,
        kind: &str,
        detail: Option<&str>,
    ) -> StorageResult<()> {
        self.conn.execute(
            "INSERT INTO event_log (at, wp_id, kind, detail) VALUES (?1, ?2, ?3, ?4)",
            params![at, wp_id.and_then(|id| i64::try_from(id).ok()), kind, detail],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(column: &str) -> TaskRow {
        TaskRow {
            wp_id: 187716,
            display_id: "WP-187716".into(),
            title: "t".into(),
            project: "Web Platform".into(),
            repo: Some("~/code/acme-web".into()),
            branch: Some("feature/187716".into()),
            base: Some("origin/development".into()),
            column: column.into(),
            updated_at: "2026-11-13T08:00:00Z".into(),
        }
    }

    #[test]
    fn migrates_to_latest_and_upserts() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.version().unwrap(), schema_version());
        store.upsert_task(&row("active")).unwrap();
        store.upsert_task(&row("awaitingPush")).unwrap();
        assert_eq!(store.task(187716).unwrap().unwrap().column, "awaitingPush");
        assert!(store.task(1).unwrap().is_none());
        assert!(store.active_tasks().unwrap().is_empty());
        assert!(
            store
                .set_column(187716, "active", "2026-11-13T09:00:00Z")
                .unwrap()
        );
        let mut other = row("active");
        other.wp_id = 187900;
        other.display_id = "WP-187900".into();
        other.updated_at = "2026-11-13T09:30:00Z".into();
        store.upsert_task(&other).unwrap();
        // several tasks can be in progress; newest first
        let active = store.active_tasks().unwrap();
        assert_eq!(
            active.iter().map(|r| r.wp_id).collect::<Vec<_>>(),
            [187900, 187716]
        );
        assert_eq!(active[1].updated_at, "2026-11-13T09:00:00Z");
        assert!(store.set_column(187716, "inbox", "2026-11-13T10:00:00Z").unwrap());
        assert_eq!(store.active_tasks().unwrap().len(), 1);
        assert!(!store.set_column(1, "active", "2026-11-13T10:00:00Z").unwrap());
        store
            .log_event(
                "2026-11-13T08:00:00Z",
                Some(187716),
                "branch.created",
                Some("feature/187716 from development"),
            )
            .unwrap();
    }

    #[test]
    fn reopening_a_file_keeps_data_and_does_not_rerun_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opw.db");
        Store::open(&path).unwrap().upsert_task(&row("active")).unwrap();
        let again = Store::open(&path).unwrap();
        assert_eq!(
            again.task(187716).unwrap().unwrap().branch.as_deref(),
            Some("feature/187716")
        );
    }

    #[test]
    fn refuses_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.db");
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        drop(conn);
        assert!(matches!(
            Store::open(&path),
            Err(StorageError::TooNew { found: 99, .. })
        ));
    }
}
