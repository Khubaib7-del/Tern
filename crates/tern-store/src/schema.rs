use rusqlite::Connection;

use crate::{StoreError, StoreResult};

/// Bumped with every migration. Stored in `PRAGMA user_version`.
pub const SCHEMA_VERSION: i64 = 3;

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

/// Chats can be archived (hidden from the list). The event log itself stays
/// append-only, so archiving never deletes events.
const V2: &str = "ALTER TABLE sessions ADD COLUMN archived_at TEXT;";

/// Sub-sessions (a parent chat and the message they branch from, plus a
/// planned question) and the Library's unified index. Library rows are an
/// index over things kept elsewhere (files, the event log, study data), so
/// a row's `state` (`live`, `trash`, `purged`) is the only thing bulk
/// delete changes here.
const V3: &str = "
ALTER TABLE sessions ADD COLUMN parent_id TEXT;
ALTER TABLE sessions ADD COLUMN anchor INTEGER;
ALTER TABLE sessions ADD COLUMN plan TEXT;
CREATE INDEX sessions_by_parent ON sessions(parent_id, anchor);
CREATE TABLE library (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    title       TEXT NOT NULL,
    location    TEXT NOT NULL,
    detail      TEXT NOT NULL DEFAULT '',
    search      TEXT NOT NULL DEFAULT '',
    size        INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    state       TEXT NOT NULL DEFAULT 'live' CHECK (state IN ('live', 'trash', 'purged')),
    trashed_at  INTEGER
);
CREATE INDEX library_by_state_kind_time ON library(state, kind, updated_at DESC);
CREATE INDEX library_by_state_time ON library(state, updated_at DESC);
CREATE INDEX library_by_state_title ON library(state, title COLLATE NOCASE);
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
    if found < 2 {
        conn.execute_batch(&format!("BEGIN; {V2} PRAGMA user_version = 2; COMMIT;"))?;
    }
    if found < 3 {
        conn.execute_batch(&format!("BEGIN; {V3} PRAGMA user_version = 3; COMMIT;"))?;
    }
    Ok(())
}
