use rusqlite::Connection;

use crate::{StoreError, StoreResult};

/// Bumped with every migration. Stored in `PRAGMA user_version`.
pub const SCHEMA_VERSION: i64 = 1;

const V1: &str = "
CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    vault_path  TEXT,
    created_at  TEXT NOT NULL
);
CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects(id),
    harness     TEXT NOT NULL CHECK (harness IN ('research', 'study')),
    title       TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
CREATE INDEX sessions_by_project ON sessions(project_id);
CREATE TABLE events (
    session_id    TEXT NOT NULL REFERENCES sessions(id),
    seq           INTEGER NOT NULL CHECK (seq > 0),
    event_id      TEXT NOT NULL UNIQUE,
    turn_id       TEXT,
    ts            TEXT NOT NULL,
    type          TEXT NOT NULL,
    effect_class  TEXT NOT NULL,
    user_summary  TEXT NOT NULL,
    body          TEXT NOT NULL,
    PRIMARY KEY (session_id, seq)
) WITHOUT ROWID;
CREATE INDEX events_by_turn ON events(turn_id);
CREATE TRIGGER events_no_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(ABORT, 'the event log is append-only'); END;
CREATE TRIGGER events_no_delete BEFORE DELETE ON events
BEGIN SELECT RAISE(ABORT, 'the event log is append-only'); END;
";

pub(crate) fn migrate(conn: &Connection) -> StoreResult<()> {
    let found: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if found > SCHEMA_VERSION {
        return Err(StoreError::TooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    if found < 1 {
        conn.execute_batch(&format!("BEGIN; {V1} PRAGMA user_version = 1; COMMIT;"))?;
    }
    Ok(())
}
