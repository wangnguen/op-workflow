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
    // 2: workflow state per task (steps 2-8) and what reviewers said about each finding
    "CREATE TABLE flow (
        wp_id       INTEGER PRIMARY KEY REFERENCES task(wp_id),
        state       TEXT NOT NULL,
        updated_at  TEXT NOT NULL
     );
     ALTER TABLE finding ADD COLUMN detail TEXT;
     ALTER TABLE finding ADD COLUMN suggestion TEXT;
     ALTER TABLE finding ADD COLUMN step TEXT;
     ALTER TABLE finding ADD COLUMN round INTEGER;
     ALTER TABLE finding ADD COLUMN chosen INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE finding ADD COLUMN seen_at TEXT;
     CREATE INDEX step_run_started ON step_run(started_at);",
    // 3: small per-task values: what the user has seen (comments, diff files), what the app has
    // already announced, the last issue read. Inbox tasks have no `task` row, so no foreign key.
    "CREATE TABLE task_kv (
        wp_id       INTEGER NOT NULL,
        kind        TEXT NOT NULL,
        key         TEXT NOT NULL DEFAULT '',
        value       TEXT NOT NULL,
        updated_at  TEXT NOT NULL,
        PRIMARY KEY (wp_id, kind, key)
     );",
    // 4: model and effort of each step run (Settings → Các bước AI; NULL = the CLI's default)
    "ALTER TABLE step_run ADD COLUMN model TEXT;
     ALTER TABLE step_run ADD COLUMN effort TEXT;",
    // 5: what each run produced in one line, its raw log (the Chat tab) and the tool calls the CLI
    // refused (JSON), for the task window's Tiến trình tab
    "ALTER TABLE step_run ADD COLUMN note TEXT;
     ALTER TABLE step_run ADD COLUMN log_file TEXT;
     ALTER TABLE step_run ADD COLUMN denials TEXT;",
    // 6: the 12-step flow. One "fix" step used to answer every review; a fix that came after a
    // cross review of the same task is step 9 now (`fixCross`). The workflow JSON in `flow` is
    // upgraded from 8 to 12 steps when it is read (opw_domain::workflow).
    "UPDATE step_run SET step = 'fixCross'
      WHERE step = 'fix'
        AND EXISTS (SELECT 1 FROM step_run c
                     WHERE c.wp_id = step_run.wp_id
                       AND c.step = 'crossReview'
                       AND c.started_at < step_run.started_at);",
    // 7: Workspace sessions (PLAN.md §12 B): the Claude Code sessions the app runs. What was said
    // lives in the CLI's own transcript; this only knows which session belongs where.
    "CREATE TABLE session (
        id          TEXT PRIMARY KEY,
        wp_id       INTEGER,
        repo        TEXT NOT NULL,
        kind        TEXT NOT NULL,
        title       TEXT NOT NULL DEFAULT '',
        mode        TEXT,
        model       TEXT,
        cost_usd    REAL NOT NULL DEFAULT 0,
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL,
        hidden_at   TEXT
     );
     CREATE INDEX session_wp ON session(wp_id);",
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

/// One Workspace session (PLAN.md §12 B). `kind`: `main` (a task's) or `free`; `mode`: the
/// permission mode chosen for its next start (NULL = the CLI's default).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub wp_id: Option<u64>,
    pub repo: String,
    pub kind: String,
    pub title: String,
    pub mode: Option<String>,
    pub model: Option<String>,
    pub cost_usd: f64,
    pub created_at: String,
    pub updated_at: String,
}

const SESSION_COLUMNS: &str = "id, wp_id, repo, kind, title, mode, model, cost_usd, created_at, updated_at";

fn session_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: r.get(0)?,
        wp_id: r.get::<_, Option<i64>>(1)?.and_then(|n| u64::try_from(n).ok()),
        repo: r.get(2)?,
        kind: r.get(3)?,
        title: r.get(4)?,
        mode: r.get(5)?,
        model: r.get(6)?,
        cost_usd: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

/// One review finding as stored. `state` and `severity` are the lowercase names used in JSON
/// (`open`, `fixing`, `fixed`, `accepted`, `deferred`; `critical` … `low`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingRow {
    pub wp_id: u64,
    pub fingerprint: String,
    pub severity: String,
    pub title: String,
    pub file: String,
    pub line: Option<u32>,
    pub source: String,
    pub state: String,
    pub reason: Option<String>,
    pub detail: Option<String>,
    pub suggestion: Option<String>,
    /// Step that reported it last (`reviewLocal`, `crossReview`, `finalReview`).
    pub step: Option<String>,
    pub round: Option<u8>,
    /// The user asked for it to be fixed.
    pub chosen: bool,
    pub seen_at: Option<String>,
}

const FINDING_COLUMNS: &str = "wp_id, fingerprint, severity, title, file, line, source, state, reason, detail, suggestion, step, round, chosen, seen_at";

fn finding_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FindingRow> {
    Ok(FindingRow {
        wp_id: u64::try_from(r.get::<_, i64>(0)?).unwrap_or_default(),
        fingerprint: r.get(1)?,
        severity: r.get(2)?,
        title: r.get(3)?,
        file: r.get(4)?,
        line: r.get::<_, Option<i64>>(5)?.and_then(|l| u32::try_from(l).ok()),
        source: r.get(6)?,
        state: r.get(7)?,
        reason: r.get(8)?,
        detail: r.get(9)?,
        suggestion: r.get(10)?,
        step: r.get(11)?,
        round: r.get::<_, Option<i64>>(12)?.and_then(|l| u8::try_from(l).ok()),
        chosen: r.get::<_, i64>(13)? != 0,
        seen_at: r.get(14)?,
    })
}

/// One run of a workflow step by an AI process.
#[derive(Debug, Clone, PartialEq)]
pub struct StepRunRow {
    pub id: i64,
    pub wp_id: u64,
    pub step: String,
    pub agent: Option<String>,
    pub session_id: Option<String>,
    /// `running`, `done`, `failed`
    pub state: String,
    pub round: Option<u8>,
    pub cost_usd: Option<f64>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub started_at: String,
    pub ended_at: Option<String>,
    /// Model the run used (the CLI's answer when known, else the one passed).
    pub model: Option<String>,
    /// Effort passed to the CLI; `None` = its default.
    pub effort: Option<String>,
    /// What the run produced, in one line ("báo 3 lỗi (1 high, 2 medium)"), or why it failed.
    pub note: Option<String>,
    /// Raw output of the run, relative to `<data dir>/runs` (`WP-1/<stamp>-implement.jsonl`).
    pub log_file: Option<String>,
    /// JSON array of the refused tool calls: `[{"tool":"Bash","input":"npx jest"}]`.
    pub denials: Option<String>,
}

/// How a step run ended.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StepRunEnd<'a> {
    pub state: &'a str,
    pub session_id: Option<&'a str>,
    pub cost_usd: Option<f64>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub ended_at: &'a str,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    /// JSON array of refused tool calls (see [`StepRunRow::denials`]).
    pub denials: Option<&'a str>,
}

const STEP_RUN_COLUMNS: &str = "id, wp_id, step, agent, session_id, state, round, cost_usd, tokens_in, tokens_out, \
                                started_at, ended_at, model, effort, note, log_file, denials";

fn step_run_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<StepRunRow> {
    Ok(StepRunRow {
        id: r.get(0)?,
        wp_id: u64::try_from(r.get::<_, i64>(1)?).unwrap_or_default(),
        step: r.get(2)?,
        agent: r.get(3)?,
        session_id: r.get(4)?,
        state: r.get(5)?,
        round: r.get::<_, Option<i64>>(6)?.and_then(|v| u8::try_from(v).ok()),
        cost_usd: r.get(7)?,
        tokens_in: r.get::<_, Option<i64>>(8)?.and_then(|v| u64::try_from(v).ok()),
        tokens_out: r.get::<_, Option<i64>>(9)?.and_then(|v| u64::try_from(v).ok()),
        started_at: r.get(10)?,
        ended_at: r.get(11)?,
        model: r.get(12)?,
        effort: r.get(13)?,
        note: r.get(14)?,
        log_file: r.get(15)?,
        denials: r.get(16)?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub at: String,
    pub kind: String,
    pub detail: Option<String>,
}

fn id_param(wp_id: u64) -> i64 {
    i64::try_from(wp_id).unwrap_or(i64::MAX)
}

fn opt_u64(v: Option<u64>) -> Option<i64> {
    v.and_then(|n| i64::try_from(n).ok())
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

    /// Keep the selected repo and its verified identity in the same database transaction.
    pub fn upsert_task_with_binding(&self, t: &TaskRow, key: &str, binding: &str) -> StorageResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        self.upsert_task(t)?;
        self.kv_set(t.wp_id, key, "", binding, &t.updated_at)?;
        tx.commit()?;
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

    /// Tasks in one board column, most recently moved first, at most `limit`.
    pub fn tasks_in(&self, column: &str, limit: usize) -> StorageResult<Vec<TaskRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TASK_COLUMNS} FROM task WHERE column_name = ?1
             ORDER BY updated_at DESC, wp_id LIMIT ?2"
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![column, limit], task_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Ids of the tasks in one board column (e.g. `done`).
    pub fn task_ids_in(&self, column: &str) -> StorageResult<Vec<u64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT wp_id FROM task WHERE column_name = ?1")?;
        let rows = stmt.query_map([column], |r| {
            Ok(u64::try_from(r.get::<_, i64>(0)?).unwrap_or_default())
        })?;
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

    /// Newest `limit` events of one task, oldest first.
    pub fn events(&self, wp_id: u64, limit: usize) -> StorageResult<Vec<EventRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT at, kind, detail FROM (
               SELECT id, at, kind, detail FROM event_log WHERE wp_id = ?1 ORDER BY id DESC LIMIT ?2
             ) ORDER BY id",
        )?;
        let rows = stmt.query_map(
            params![id_param(wp_id), i64::try_from(limit).unwrap_or(i64::MAX)],
            |r| {
                Ok(EventRow {
                    at: r.get(0)?,
                    kind: r.get(1)?,
                    detail: r.get(2)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // ---------- workflow ----------

    /// Workflow state of one task (JSON owned by the orchestrator).
    pub fn flow(&self, wp_id: u64) -> StorageResult<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT state FROM flow WHERE wp_id = ?1",
                [id_param(wp_id)],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Every stored workflow, `(wp_id, state)`.
    pub fn flows(&self) -> StorageResult<Vec<(u64, String)>> {
        let mut stmt = self.conn.prepare("SELECT wp_id, state FROM flow")?;
        let rows = stmt.query_map([], |r| {
            Ok((u64::try_from(r.get::<_, i64>(0)?).unwrap_or_default(), r.get(1)?))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn save_flow(&self, wp_id: u64, state: &str, at: &str) -> StorageResult<()> {
        self.conn.execute(
            "INSERT INTO flow (wp_id, state, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(wp_id) DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at",
            params![id_param(wp_id), state, at],
        )?;
        Ok(())
    }

    /// Keep a chosen finding and its fix target in the workflow in the same transaction.
    pub fn save_finding_decision(&self, row: &FindingRow, flow: &str, at: &str) -> StorageResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        self.upsert_finding(row)?;
        self.save_flow(row.wp_id, flow, at)?;
        tx.commit()?;
        Ok(())
    }

    // ---------- per-task values ----------

    /// One value of `kind` (`key` = `""` for single values).
    pub fn kv_get(&self, wp_id: u64, kind: &str, key: &str) -> StorageResult<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM task_kv WHERE wp_id = ?1 AND kind = ?2 AND key = ?3",
                params![id_param(wp_id), kind, key],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Every `(key, value)` of `kind` for one task, by key.
    pub fn kv_all(&self, wp_id: u64, kind: &str) -> StorageResult<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM task_kv WHERE wp_id = ?1 AND kind = ?2 ORDER BY key")?;
        let rows = stmt.query_map(params![id_param(wp_id), kind], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Every `(wp_id, key, value)` of `kind`, all tasks (badges for the whole board at once).
    pub fn kv_kind(&self, kind: &str) -> StorageResult<Vec<(u64, String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT wp_id, key, value FROM task_kv WHERE kind = ?1")?;
        let rows = stmt.query_map([kind], |r| {
            Ok((
                u64::try_from(r.get::<_, i64>(0)?).unwrap_or_default(),
                r.get(1)?,
                r.get(2)?,
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn kv_set(&self, wp_id: u64, kind: &str, key: &str, value: &str, at: &str) -> StorageResult<()> {
        self.conn.execute(
            "INSERT INTO task_kv (wp_id, kind, key, value, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(wp_id, kind, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![id_param(wp_id), kind, key, value, at],
        )?;
        Ok(())
    }

    /// Replace every value of `kind` for one task with `entries`.
    pub fn kv_replace(
        &self,
        wp_id: u64,
        kind: &str,
        entries: &[(String, String)],
        at: &str,
    ) -> StorageResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM task_kv WHERE wp_id = ?1 AND kind = ?2",
            params![id_param(wp_id), kind],
        )?;
        for (key, value) in entries {
            tx.execute(
                "INSERT INTO task_kv (wp_id, kind, key, value, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id_param(wp_id), kind, key, value, at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// A step run starts; returns its id for [`Store::end_step_run`]. `log_file` is where its raw
    /// output goes (relative to `<data dir>/runs`), so the Chat tab can follow it while it runs.
    pub fn begin_step_run(
        &self,
        wp_id: u64,
        step: &str,
        agent: &str,
        round: Option<u8>,
        at: &str,
        log_file: Option<&str>,
    ) -> StorageResult<i64> {
        self.conn.execute(
            "INSERT INTO step_run (wp_id, step, agent, state, round, started_at, log_file)
             VALUES (?1, ?2, ?3, 'running', ?4, ?5, ?6)",
            params![id_param(wp_id), step, agent, round, at, log_file],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// What the run produced, in one line (written after the app read its answer).
    pub fn set_step_run_note(&self, id: i64, note: &str) -> StorageResult<()> {
        self.conn
            .execute("UPDATE step_run SET note = ?2 WHERE id = ?1", params![id, note])?;
        Ok(())
    }

    /// How an ended run turned out once the app read its answer: `waiting` (the AI stopped to
    /// ask the user), or `failed` (the process ended well but its answer could not be used).
    /// A run still going is left alone.
    pub fn set_step_run_state(&self, id: i64, state: &str) -> StorageResult<()> {
        self.conn.execute(
            "UPDATE step_run SET state = ?2 WHERE id = ?1 AND state <> 'running'",
            params![id, state],
        )?;
        Ok(())
    }

    pub fn end_step_run(&self, id: i64, end: &StepRunEnd<'_>) -> StorageResult<()> {
        self.conn.execute(
            "UPDATE step_run SET state = ?2, session_id = COALESCE(?3, session_id), cost_usd = ?4,
               tokens_in = ?5, tokens_out = ?6, ended_at = ?7, model = ?8, effort = ?9, denials = ?10
             WHERE id = ?1",
            params![
                id,
                end.state,
                end.session_id,
                end.cost_usd,
                opt_u64(end.tokens_in),
                opt_u64(end.tokens_out),
                end.ended_at,
                end.model,
                end.effort,
                end.denials
            ],
        )?;
        Ok(())
    }

    /// Runs that never ended (the app closed while they ran) are marked failed.
    pub fn fail_unfinished_runs(&self, at: &str) -> StorageResult<usize> {
        Ok(self.conn.execute(
            "UPDATE step_run SET state = 'failed', ended_at = ?1 WHERE ended_at IS NULL",
            [at],
        )?)
    }

    /// Step runs of one task, oldest first.
    pub fn step_runs(&self, wp_id: u64) -> StorageResult<Vec<StepRunRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {STEP_RUN_COLUMNS} FROM step_run WHERE wp_id = ?1 ORDER BY id"
        ))?;
        let rows = stmt.query_map([id_param(wp_id)], step_run_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Finished step runs of every task, oldest first (Settings shows cost and time per step). A
    /// run that stopped to ask the user ran in full too.
    pub fn done_step_runs(&self) -> StorageResult<Vec<StepRunRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {STEP_RUN_COLUMNS} FROM step_run WHERE state IN ('done', 'waiting') AND ended_at IS NOT NULL \
             ORDER BY id"
        ))?;
        let rows = stmt.query_map([], step_run_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Cost and tokens (in + out) of every step run started at or after `since` (RFC 3339).
    pub fn usage_since(&self, since: &str) -> StorageResult<(f64, u64)> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(cost_usd), 0), COALESCE(SUM(COALESCE(tokens_in, 0) + COALESCE(tokens_out, 0)), 0)
             FROM step_run WHERE started_at >= ?1",
            [since],
            |r| {
                Ok((
                    r.get::<_, f64>(0)?,
                    u64::try_from(r.get::<_, i64>(1)?).unwrap_or_default(),
                ))
            },
        )?)
    }

    // ---------- findings ----------

    /// Findings of one task, most severe first, then by file and line.
    pub fn findings(&self, wp_id: u64) -> StorageResult<Vec<FindingRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {FINDING_COLUMNS} FROM finding WHERE wp_id = ?1
             ORDER BY CASE severity WHEN 'critical' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 ELSE 3 END,
                      file, line"
        ))?;
        let rows = stmt.query_map([id_param(wp_id)], finding_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Insert a session, or update everything but its creation time.
    pub fn save_session(&self, s: &SessionRow) -> StorageResult<()> {
        self.conn.execute(
            &format!(
                "INSERT INTO session ({SESSION_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                   wp_id = excluded.wp_id, repo = excluded.repo, kind = excluded.kind,
                   title = excluded.title, mode = excluded.mode, model = excluded.model,
                   cost_usd = excluded.cost_usd, updated_at = excluded.updated_at"
            ),
            params![
                s.id,
                opt_u64(s.wp_id),
                s.repo,
                s.kind,
                s.title,
                s.mode,
                s.model,
                s.cost_usd,
                s.created_at,
                s.updated_at
            ],
        )?;
        Ok(())
    }

    /// Sessions not hidden, the most recently used first.
    pub fn sessions(&self) -> StorageResult<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SESSION_COLUMNS} FROM session WHERE hidden_at IS NULL
             ORDER BY updated_at DESC, created_at DESC"
        ))?;
        let rows = stmt.query_map([], session_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn session(&self, id: &str) -> StorageResult<Option<SessionRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {SESSION_COLUMNS} FROM session WHERE id = ?1"),
                [id],
                session_row,
            )
            .optional()?)
    }

    /// Take a session off the Workspace list (its transcript stays with the CLI).
    pub fn hide_session(&self, id: &str, at: &str) -> StorageResult<bool> {
        let n = self
            .conn
            .execute("UPDATE session SET hidden_at = ?2 WHERE id = ?1", params![id, at])?;
        Ok(n > 0)
    }

    pub fn upsert_finding(&self, f: &FindingRow) -> StorageResult<()> {
        self.conn.execute(
            &format!(
                "INSERT INTO finding ({FINDING_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT(wp_id, fingerprint) DO UPDATE SET
                   severity = excluded.severity, title = excluded.title, file = excluded.file,
                   line = excluded.line, source = excluded.source, state = excluded.state,
                   reason = excluded.reason, detail = excluded.detail, suggestion = excluded.suggestion,
                   step = excluded.step, round = excluded.round, chosen = excluded.chosen,
                   seen_at = excluded.seen_at"
            ),
            params![
                id_param(f.wp_id),
                f.fingerprint,
                f.severity,
                f.title,
                f.file,
                f.line,
                f.source,
                f.state,
                f.reason,
                f.detail,
                f.suggestion,
                f.step,
                f.round,
                i64::from(f.chosen),
                f.seen_at
            ],
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
    fn sessions_save_update_list_and_hide() {
        let store = Store::open_in_memory().unwrap();
        let mut s = SessionRow {
            id: "a1".into(),
            wp_id: Some(187716),
            repo: "C:/code/web".into(),
            kind: "main".into(),
            title: String::new(),
            mode: None,
            model: None,
            cost_usd: 0.0,
            created_at: "2026-11-20T08:00:00Z".into(),
            updated_at: "2026-11-20T08:00:00Z".into(),
        };
        store.save_session(&s).unwrap();
        let free = SessionRow {
            id: "b2".into(),
            wp_id: None,
            kind: "free".into(),
            updated_at: "2026-11-20T09:00:00Z".into(),
            ..s.clone()
        };
        store.save_session(&free).unwrap();
        s.title = "Sửa lỗi".into();
        s.mode = Some("plan".into());
        s.cost_usd = 0.25;
        s.updated_at = "2026-11-20T10:00:00Z".into();
        store.save_session(&s).unwrap();
        let list = store.sessions().unwrap();
        assert_eq!(
            list.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["a1", "b2"]
        );
        assert_eq!(list[0], s);
        assert_eq!(list[1].wp_id, None);
        assert!(store.hide_session("a1", "2026-11-20T11:00:00Z").unwrap());
        assert_eq!(store.sessions().unwrap().len(), 1);
        assert_eq!(store.session("a1").unwrap().unwrap().title, "Sửa lỗi");
    }

    #[test]
    fn per_task_values_set_replace_and_list() {
        let store = Store::open_in_memory().unwrap();
        let at = "2026-11-14T08:00:00Z";
        assert!(store.kv_get(1, "comments.seen", "").unwrap().is_none());
        // inbox tasks have no task row
        store.kv_set(1, "comments.seen", "", "41", at).unwrap();
        store.kv_set(1, "comments.seen", "", "42", at).unwrap();
        assert_eq!(
            store.kv_get(1, "comments.seen", "").unwrap().as_deref(),
            Some("42")
        );
        store
            .kv_replace(
                2,
                "diff.seen",
                &[("b.ts".into(), "h2".into()), ("a.ts".into(), "h1".into())],
                at,
            )
            .unwrap();
        store
            .kv_replace(2, "diff.seen", &[("a.ts".into(), "h3".into())], at)
            .unwrap();
        assert_eq!(
            store.kv_all(2, "diff.seen").unwrap(),
            vec![("a.ts".into(), "h3".into())]
        );
        assert_eq!(
            store.kv_kind("comments.seen").unwrap(),
            vec![(1, String::new(), "42".to_owned())]
        );
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
    fn lists_a_column_newest_first() {
        let store = Store::open_in_memory().unwrap();
        for (id, at) in [
            (1, "2026-11-13T08:00:00Z"),
            (2, "2026-11-15T08:00:00Z"),
            (3, "2026-11-14T08:00:00Z"),
        ] {
            store
                .upsert_task(&TaskRow {
                    wp_id: id,
                    updated_at: at.into(),
                    ..row("done")
                })
                .unwrap();
        }
        store.set_column(3, "active", "2026-11-16T08:00:00Z").unwrap();
        let done: Vec<u64> = store
            .tasks_in("done", 10)
            .unwrap()
            .iter()
            .map(|r| r.wp_id)
            .collect();
        assert_eq!(done, [2, 1]);
        assert_eq!(store.tasks_in("done", 1).unwrap().len(), 1);
    }

    #[test]
    fn keeps_flow_step_runs_findings_and_events() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_task(&row("active")).unwrap();
        assert_eq!(store.flow(187716).unwrap(), None);
        store
            .save_flow(187716, "{\"a\":1}", "2026-11-14T08:00:00Z")
            .unwrap();
        store
            .save_flow(187716, "{\"a\":2}", "2026-11-14T08:01:00Z")
            .unwrap();
        assert_eq!(store.flow(187716).unwrap().as_deref(), Some("{\"a\":2}"));
        assert_eq!(store.flows().unwrap().len(), 1);

        let id = store
            .begin_step_run(
                187716,
                "analyze",
                "claude",
                None,
                "2026-11-14T08:00:00Z",
                Some("WP-187716/a-analyze.jsonl"),
            )
            .unwrap();
        let open = store
            .begin_step_run(187716, "implement", "claude", None, "2026-11-14T09:00:00Z", None)
            .unwrap();
        store
            .end_step_run(
                id,
                &StepRunEnd {
                    state: "done",
                    session_id: Some("s-1"),
                    cost_usd: Some(0.5),
                    tokens_in: Some(1000),
                    tokens_out: Some(200),
                    ended_at: "2026-11-14T08:05:00Z",
                    model: Some("claude-opus-5-5"),
                    effort: None,
                    denials: Some(r#"[{"tool":"Bash","input":"npx jest"}]"#),
                },
            )
            .unwrap();
        store.set_step_run_note(id, "plan: 2 file sẽ sửa").unwrap();
        // a run still going keeps "running"; an ended one takes what its answer meant
        store.set_step_run_state(open, "waiting").unwrap();
        assert_eq!(store.step_runs(187716).unwrap()[1].state, "running");
        store.set_step_run_state(id, "waiting").unwrap();
        assert_eq!(store.step_runs(187716).unwrap()[0].state, "waiting");
        assert_eq!(store.done_step_runs().unwrap().len(), 1);
        store.set_step_run_state(id, "done").unwrap();
        assert_eq!(store.fail_unfinished_runs("2026-11-14T10:00:00Z").unwrap(), 1);
        let runs = store.step_runs(187716).unwrap();
        assert_eq!(runs[0].session_id.as_deref(), Some("s-1"));
        assert_eq!(runs[0].model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(runs[0].effort, None);
        assert_eq!(runs[0].note.as_deref(), Some("plan: 2 file sẽ sửa"));
        assert_eq!(runs[0].log_file.as_deref(), Some("WP-187716/a-analyze.jsonl"));
        assert!(runs[0].denials.as_deref().is_some_and(|d| d.contains("npx jest")));
        assert_eq!(runs[1].note, None);
        assert_eq!(runs[1].id, open);
        assert_eq!(runs[1].state, "failed");
        let done = store.done_step_runs().unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].step, "analyze");
        let (cost, tokens) = store.usage_since("2026-11-14T00:00:00Z").unwrap();
        assert!((cost - 0.5).abs() < 1e-9);
        assert_eq!(tokens, 1200);
        assert_eq!(store.usage_since("2026-11-15T00:00:00Z").unwrap().1, 0);

        let mut f = FindingRow {
            wp_id: 187716,
            fingerprint: "abc".into(),
            severity: "low".into(),
            title: "t".into(),
            file: "a.ts".into(),
            line: Some(3),
            source: "codex".into(),
            state: "open".into(),
            reason: None,
            detail: Some("d".into()),
            suggestion: None,
            step: Some("crossReview".into()),
            round: Some(1),
            chosen: false,
            seen_at: None,
        };
        store.upsert_finding(&f).unwrap();
        f.fingerprint = "def".into();
        f.severity = "high".into();
        f.chosen = true;
        store.upsert_finding(&f).unwrap();
        f.state = "accepted".into();
        f.reason = Some("ok".into());
        store.upsert_finding(&f).unwrap();
        let all = store.findings(187716).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].severity, "high");
        assert_eq!(all[0].state, "accepted");
        assert!(all[0].chosen);
        assert_eq!(all[1].line, Some(3));

        for kind in ["a", "b", "c"] {
            store
                .log_event("2026-11-14T08:00:00Z", Some(187716), kind, None)
                .unwrap();
        }
        let events = store.events(187716, 2).unwrap();
        assert_eq!(
            events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            ["b", "c"]
        );
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
    fn migration_6_moves_fixes_after_a_cross_review_to_step_9() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v5.db");
        let conn = Connection::open(&path).unwrap();
        for sql in &MIGRATIONS[..5] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 5).unwrap();
        conn.execute_batch(
            "INSERT INTO task (wp_id, display_id, title, project, column_name, created_at, updated_at)
               VALUES (1, 'WP-1', 'T', 'P', 'active', '2026-11-14T00:00:00Z', '2026-11-14T00:00:00Z');
             INSERT INTO step_run (wp_id, step, state, started_at) VALUES
               (1, 'reviewLocal', 'done', '2026-11-14T01:00:00Z'),
               (1, 'fix', 'done', '2026-11-14T02:00:00Z'),
               (1, 'crossReview', 'done', '2026-11-14T03:00:00Z'),
               (1, 'fix', 'done', '2026-11-14T04:00:00Z');",
        )
        .unwrap();
        drop(conn);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.version().unwrap(), schema_version());
        let steps: Vec<String> = store.step_runs(1).unwrap().into_iter().map(|r| r.step).collect();
        assert_eq!(steps, ["reviewLocal", "fix", "crossReview", "fixCross"]);
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
