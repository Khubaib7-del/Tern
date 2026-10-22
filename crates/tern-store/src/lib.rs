//! Persistence: SQLite for projects, sessions, and the append-only event log.
//!
//! Full-text and vector indexes, artifacts, and checkpoints are planned for
//! later phases. The event log is the source of truth: every UI view is a
//! replay of it.

#![forbid(unsafe_code)]

mod schema;

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use tern_core::{EventEnvelope, ProjectId, Redactor, SessionId};
use thiserror::Error;

pub use schema::SCHEMA_VERSION;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("event encoding error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unknown session {0}")]
    UnknownSession(SessionId),
    #[error("unknown project {0}")]
    UnknownProject(ProjectId),
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    TooNew { found: i64, supported: i64 },
}

pub type StoreResult<T> = Result<T, StoreError>;

/// Which harness a session belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Harness {
    Research,
    Study,
}

impl Harness {
    fn as_str(self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Study => "study",
        }
    }
}

/// One row of the session list.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SessionSummary {
    pub id: SessionId,
    pub title: String,
    pub harness: Harness,
    pub created_at: String,
    /// Events recorded so far.
    pub events: u64,
}

/// The Tern database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    redactor: Redactor,
}

impl Store {
    pub fn open(path: &Path) -> StoreResult<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> StoreResult<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        schema::migrate(&conn)?;
        Ok(Self {
            conn,
            redactor: Redactor::default(),
        })
    }

    pub fn schema_version(&self) -> StoreResult<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    /// Runs SQLite's own consistency check. `true` means `ok`.
    pub fn integrity_ok(&self) -> StoreResult<bool> {
        let s: String = self
            .conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        Ok(s == "ok")
    }

    pub fn create_project(&self, name: &str, vault_path: Option<&Path>) -> StoreResult<ProjectId> {
        let id = ProjectId::new();
        self.conn.execute(
            "INSERT INTO projects (id, name, vault_path, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                id.to_string(),
                name,
                vault_path.map(|p| p.to_string_lossy().into_owned()),
                now()
            ],
        )?;
        Ok(id)
    }

    /// The oldest project with this name, if any.
    pub fn project_by_name(&self, name: &str) -> StoreResult<Option<ProjectId>> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM projects WHERE name = ?1 ORDER BY created_at, id LIMIT 1",
                [name],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id.and_then(|s| s.parse().ok()))
    }

    /// The named project, created if it does not exist yet.
    pub fn ensure_project(&self, name: &str) -> StoreResult<ProjectId> {
        match self.project_by_name(name)? {
            Some(id) => Ok(id),
            None => self.create_project(name, None),
        }
    }

    pub fn create_session(
        &self,
        project: ProjectId,
        harness: Harness,
        title: &str,
    ) -> StoreResult<SessionId> {
        let exists: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM projects WHERE id = ?1",
                [project.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(StoreError::UnknownProject(project));
        }
        let id = SessionId::new();
        self.conn.execute(
            "INSERT INTO sessions (id, project_id, harness, title, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id.to_string(), project.to_string(), harness.as_str(), title, now()],
        )?;
        Ok(id)
    }

    /// Renames a session (the chat title in the sidebar).
    pub fn rename_session(&self, id: SessionId, title: &str) -> StoreResult<()> {
        self.conn.execute(
            "UPDATE sessions SET title = ?1 WHERE id = ?2",
            params![title, id.to_string()],
        )?;
        Ok(())
    }

    /// Hides a session from the list. Its events stay in the log.
    pub fn archive_session(&self, id: SessionId) -> StoreResult<()> {
        self.conn.execute(
            "UPDATE sessions SET archived_at = ?1 WHERE id = ?2",
            params![now(), id.to_string()],
        )?;
        Ok(())
    }

    /// One session's row, archived or not.
    pub fn session(&self, id: SessionId) -> StoreResult<Option<SessionSummary>> {
        Ok(self
            .summaries("WHERE s.id = ?1", &id.to_string(), 1)?
            .into_iter()
            .next())
    }

    /// The newest sessions first. Sessions with no events, and archived
    /// ones, are left out.
    pub fn recent_sessions(&self, limit: usize) -> StoreResult<Vec<SessionSummary>> {
        self.summaries("WHERE s.archived_at IS NULL AND ?1 = ?1", "", limit)
    }

    fn summaries(&self, filter: &str, arg: &str, limit: usize) -> StoreResult<Vec<SessionSummary>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT s.id, s.title, s.harness, s.created_at, COUNT(e.seq) AS n
             FROM sessions s JOIN events e ON e.session_id = s.id
             {filter}
             GROUP BY s.id ORDER BY s.created_at DESC, s.id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![arg, limit as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, title, harness, created_at, n) = row?;
            let Ok(id) = id.parse() else { continue };
            out.push(SessionSummary {
                id,
                title,
                harness: if harness == "study" {
                    Harness::Study
                } else {
                    Harness::Research
                },
                created_at,
                events: n as u64,
            });
        }
        Ok(out)
    }

    /// Appends an event. The store redacts it, assigns the next `seq` for its
    /// session, and returns the stored copy.
    pub fn append(&mut self, event: &EventEnvelope) -> StoreResult<EventEnvelope> {
        let mut e = event.redacted(&self.redactor);
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let known: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM sessions WHERE id = ?1",
                [e.session_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if known.is_none() {
            return Err(StoreError::UnknownSession(e.session_id));
        }
        let last: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = ?1",
            [e.session_id.to_string()],
            |r| r.get(0),
        )?;
        e.seq = last as u64 + 1;
        tx.execute(
            "INSERT INTO events (session_id, seq, event_id, turn_id, ts, type, effect_class, user_summary, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                e.session_id.to_string(),
                e.seq as i64,
                e.event_id.to_string(),
                e.turn_id.map(|t| t.to_string()),
                e.ts.format(&time::format_description::well_known::Rfc3339).expect("rfc3339"),
                e.kind,
                serde_json::to_value(e.effect_class)?.as_str().unwrap_or_default(),
                e.user_summary,
                serde_json::to_string(&e)?,
            ],
        )?;
        tx.commit()?;
        Ok(e)
    }

    /// Every event in `session` with `seq > after`, in order. `after = 0`
    /// replays the whole session.
    pub fn replay(&self, session: SessionId, after: u64) -> StoreResult<Vec<EventEnvelope>> {
        let mut stmt = self
            .conn
            .prepare("SELECT body FROM events WHERE session_id = ?1 AND seq > ?2 ORDER BY seq")?;
        let rows = stmt.query_map(params![session.to_string(), after as i64], |r| {
            r.get::<_, String>(0)
        })?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }

    pub fn last_seq(&self, session: SessionId) -> StoreResult<u64> {
        let v: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = ?1",
            [session.to_string()],
            |r| r.get(0),
        )?;
        Ok(v as u64)
    }
}

fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("rfc3339")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tern_core::EffectClass;

    fn setup() -> (Store, SessionId) {
        let s = Store::open_in_memory().unwrap();
        let p = s.create_project("FYP", None).unwrap();
        let ses = s
            .create_session(p, Harness::Research, "Transformers survey")
            .unwrap();
        (s, ses)
    }

    fn ev(ses: SessionId, kind: &str) -> EventEnvelope {
        EventEnvelope::new(ses, kind, EffectClass::Analysis, kind, json!({"k": kind}))
    }

    #[test]
    fn recent_sessions_skip_empty_ones_and_rename_works() {
        let (mut s, ses) = setup();
        let p = s.ensure_project("FYP").unwrap();
        let empty = s.create_session(p, Harness::Study, "Nothing yet").unwrap();
        assert!(s.recent_sessions(10).unwrap().is_empty());
        s.append(&ev(ses, "turn.started")).unwrap();
        s.append(&ev(ses, "turn.finished")).unwrap();
        s.rename_session(ses, "Attention paper").unwrap();
        let rows = s.recent_sessions(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, ses);
        assert_eq!(rows[0].title, "Attention paper");
        assert_eq!(rows[0].events, 2);
        assert_eq!(rows[0].harness, Harness::Research);
        assert_ne!(rows[0].id, empty);
    }

    #[test]
    fn archived_sessions_leave_the_list_but_keep_their_events() {
        let (mut s, ses) = setup();
        s.append(&ev(ses, "turn.started")).unwrap();
        assert_eq!(s.recent_sessions(10).unwrap().len(), 1);
        s.archive_session(ses).unwrap();
        assert!(s.recent_sessions(10).unwrap().is_empty());
        assert_eq!(
            s.session(ses).unwrap().unwrap().title,
            "Transformers survey"
        );
        assert_eq!(s.replay(ses, 0).unwrap().len(), 1);
        assert_eq!(s.schema_version().unwrap(), 2);
    }

    #[test]
    fn ensure_project_reuses_by_name() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.project_by_name("Inbox").unwrap(), None);
        let a = s.ensure_project("Inbox").unwrap();
        assert_eq!(s.ensure_project("Inbox").unwrap(), a);
        assert_ne!(s.ensure_project("FYP").unwrap(), a);
    }

    #[test]
    fn append_assigns_monotonic_seq_per_session() {
        let (mut s, a) = setup();
        let p = s.create_project("Other", None).unwrap();
        let b = s.create_session(p, Harness::Study, "Cards").unwrap();
        assert_eq!(s.append(&ev(a, "one")).unwrap().seq, 1);
        assert_eq!(s.append(&ev(a, "two")).unwrap().seq, 2);
        assert_eq!(s.append(&ev(b, "one")).unwrap().seq, 1);
        assert_eq!(s.last_seq(a).unwrap(), 2);
    }

    #[test]
    fn replay_from_a_sequence_number() {
        let (mut s, ses) = setup();
        for k in [
            "plan.created",
            "source.fetched",
            "claim.extracted",
            "claim.verified",
        ] {
            s.append(&ev(ses, k)).unwrap();
        }
        let all = s.replay(ses, 0).unwrap();
        assert_eq!(all.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2, 3, 4]);
        let tail = s.replay(ses, 2).unwrap();
        assert_eq!(
            tail.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
            ["claim.extracted", "claim.verified"]
        );
        assert!(s.replay(ses, 4).unwrap().is_empty());
    }

    #[test]
    fn events_are_redacted_before_persisting() {
        let (mut s, ses) = setup();
        let key = "sk-ant-api03-ZZZZZZZZZZZZZZZZZZZZZZZZZZZZ";
        let e = EventEnvelope::new(
            ses,
            "provider.tested",
            EffectClass::Credentials,
            format!("key {key}"),
            json!({"key": key}),
        );
        s.append(&e).unwrap();
        let raw: String = s
            .conn
            .query_row("SELECT body || user_summary FROM events", [], |r| r.get(0))
            .unwrap();
        assert!(!raw.contains(key));
    }

    #[test]
    fn log_is_append_only() {
        let (mut s, ses) = setup();
        s.append(&ev(ses, "one")).unwrap();
        assert!(s.conn.execute("UPDATE events SET type = 'x'", []).is_err());
        assert!(s.conn.execute("DELETE FROM events", []).is_err());
        assert_eq!(s.replay(ses, 0).unwrap().len(), 1);
    }

    #[test]
    fn unknown_session_is_refused() {
        let (mut s, _) = setup();
        assert!(matches!(
            s.append(&ev(SessionId::new(), "x")),
            Err(StoreError::UnknownSession(_))
        ));
    }

    #[test]
    fn reopening_keeps_events_and_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite3");
        let ses = {
            let mut s = Store::open(&path).unwrap();
            let p = s.create_project("FYP", None).unwrap();
            let ses = s.create_session(p, Harness::Research, "t").unwrap();
            s.append(&ev(ses, "one")).unwrap();
            ses
        };
        let s = Store::open(&path).unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
        assert!(s.integrity_ok().unwrap());
        assert_eq!(s.replay(ses, 0).unwrap().len(), 1);
    }
}
