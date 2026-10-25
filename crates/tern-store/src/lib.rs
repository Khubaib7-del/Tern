//! Persistence: SQLite for projects, sessions, and the append-only event log,
//! and Anki packages ([`anki`]) for flashcard export.
//!
//! Full-text and vector indexes, artifacts, and checkpoints are planned for
//! later phases. The event log is the source of truth: every UI view is a
//! replay of it.

#![forbid(unsafe_code)]

pub mod anki;
pub mod library;
mod projects;
mod schema;

pub use projects::{INBOX, ProjectSummary};

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use tern_core::{EventEnvelope, ProjectId, Redactor, SessionId};
use thiserror::Error;

pub use schema::SCHEMA_VERSION;

/// SQL: session `s` has an event other than being opened or reopened.
const HAS_CONTENT: &str = "EXISTS (SELECT 1 FROM events c WHERE c.session_id = s.id \
     AND c.type NOT IN ('session.opened', 'session.resumed'))";

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
    /// A request the store refuses, worded for the user.
    #[error("{0}")]
    Invalid(String),
    #[error("database schema version {found} is newer than this build supports ({supported})")]
    TooNew { found: i64, supported: i64 },
}

pub type StoreResult<T> = Result<T, StoreError>;

/// Which harness a session belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Harness {
    #[default]
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
    /// For a sub-session: the chat it hangs under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
    /// For a sub-session: the parent's message number it branches from
    /// (`12` for `Q12`), so its messages read `Q12.1`, `Q12.2`, ….
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<u32>,
    /// A planned sub-session's question, until it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The project folder the session is filed under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectId>,
}

/// The Tern database.
#[derive(Debug)]
pub struct Store {
    pub(crate) conn: Connection,
    redactor: Redactor,
}

/// Whether SQLite said the database is busy or locked.
fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

impl Store {
    pub fn open(path: &Path) -> StoreResult<Self> {
        // A database another connection is creating or migrating at this
        // moment can answer "database is locked" without waiting (switching
        // a new file to WAL, or SQLite's deadlock check); try again for a
        // few seconds before giving up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match Self::init(Connection::open(path)?) {
                Err(StoreError::Sql(e)) if is_busy(&e) && std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                other => return other,
            }
        }
    }

    pub fn open_in_memory() -> StoreResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> StoreResult<Self> {
        // The busy timeout first, so a second connection opening the file
        // at the same moment waits for the first instead of failing.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
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
        self.create_session_with_id(id, project, harness, title)?;
        Ok(id)
    }

    /// [`Store::create_session`] with an id chosen earlier (a chat that
    /// was a draft until its first message).
    pub fn create_session_with_id(
        &self,
        id: SessionId,
        project: ProjectId,
        harness: Harness,
        title: &str,
    ) -> StoreResult<()> {
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
        self.conn.execute(
            "INSERT OR IGNORE INTO sessions (id, project_id, harness, title, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id.to_string(), project.to_string(), harness.as_str(), title, now()],
        )?;
        Ok(())
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

    /// Brings an archived session back to the list.
    pub fn unarchive_session(&self, id: SessionId) -> StoreResult<()> {
        self.conn.execute(
            "UPDATE sessions SET archived_at = NULL WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    /// Deletes a chat to the trash: a tombstone that hides it (and its
    /// sub-sessions) from every list. Its events stay until the trash is
    /// emptied; [`Store::restore_session`] brings it back.
    pub fn delete_session(&self, id: SessionId) -> StoreResult<()> {
        let n = self.conn.execute(
            "UPDATE sessions SET deleted_at = ?1 WHERE (id = ?2 OR parent_id = ?2) AND deleted_at IS NULL",
            params![now(), id.to_string()],
        )?;
        if n == 0 && self.session(id)?.is_none() {
            return Err(StoreError::UnknownSession(id));
        }
        Ok(())
    }

    /// Takes a chat (and its sub-sessions) out of the trash.
    pub fn restore_session(&self, id: SessionId) -> StoreResult<()> {
        self.conn.execute(
            "UPDATE sessions SET deleted_at = NULL WHERE id = ?1 OR parent_id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    /// Whether a chat is in the trash.
    pub fn is_deleted(&self, id: SessionId) -> StoreResult<bool> {
        let v: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT deleted_at FROM sessions WHERE id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(matches!(v, Some(Some(_))))
    }

    /// Archived chats (not deleted), newest first.
    pub fn archived_sessions(&self, limit: usize) -> StoreResult<Vec<SessionSummary>> {
        self.summaries_left(
            "WHERE s.archived_at IS NOT NULL AND s.deleted_at IS NULL AND s.parent_id IS NULL AND ?1 = ?1",
            "",
            limit,
        )
    }

    /// Chats in the trash, newest first (sub-sessions go with their parent).
    pub fn deleted_sessions(&self, limit: usize) -> StoreResult<Vec<SessionSummary>> {
        self.summaries_left(
            "WHERE s.deleted_at IS NOT NULL AND (s.parent_id IS NULL OR s.parent_id NOT IN (SELECT id FROM sessions WHERE deleted_at IS NOT NULL)) AND ?1 = ?1",
            "",
            limit,
        )
    }

    /// Erases a chat in the trash for good: writes its purge record, then
    /// deletes its events, its sub-sessions' events, and the rows. Refuses a
    /// chat that is not in the trash. Returns the ids erased (the chat and
    /// its sub-sessions), so the caller can remove their checkpoints and
    /// run files.
    pub fn purge_session(&mut self, id: SessionId) -> StoreResult<Vec<SessionId>> {
        if !self.is_deleted(id)? {
            return Err(StoreError::Invalid(
                "Only a chat in the trash can be erased.".into(),
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let ids: Vec<(String, String, String)> = {
            let mut stmt = tx.prepare(
                "SELECT id, title, COALESCE(deleted_at, '') FROM sessions WHERE id = ?1 OR parent_id = ?1",
            )?;
            let rows =
                stmt.query_map([id.to_string()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        let at = now();
        for (sid, title, deleted_at) in &ids {
            let n: i64 = tx.query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ?1",
                [sid],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO purges (session_id, title, events, deleted_at, purged_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![sid, title, n, deleted_at, at],
            )?;
            tx.execute("DELETE FROM events WHERE session_id = ?1", [sid])?;
            tx.execute("DELETE FROM sessions WHERE id = ?1", [sid])?;
        }
        tx.commit()?;
        Ok(ids.iter().filter_map(|(s, _, _)| s.parse().ok()).collect())
    }

    /// Erases every chat in the trash. Returns the ids erased.
    pub fn empty_trash(&mut self) -> StoreResult<Vec<SessionId>> {
        let mut out = Vec::new();
        for s in self.deleted_sessions(100_000)? {
            out.extend(self.purge_session(s.id)?);
        }
        Ok(out)
    }

    /// How many chats were ever purged (their records stay).
    pub fn purge_count(&self) -> StoreResult<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM purges", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    /// Whether a session is archived.
    pub fn is_archived(&self, id: SessionId) -> StoreResult<bool> {
        let v: Option<Option<String>> = self
            .conn
            .query_row(
                "SELECT archived_at FROM sessions WHERE id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(matches!(v, Some(Some(_))))
    }

    /// Makes a sub-session of `parent` branching from its message
    /// `anchor` (`Q<anchor>`), in the parent's project. `plan` is the
    /// question a planned sub-session will run.
    pub fn create_sub_session(
        &self,
        parent: SessionId,
        anchor: u32,
        title: &str,
        plan: Option<&str>,
    ) -> StoreResult<SessionId> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT project_id, harness FROM sessions WHERE id = ?1",
                [parent.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((project, harness)) = row else {
            return Err(StoreError::UnknownSession(parent));
        };
        let id = SessionId::new();
        self.conn.execute(
            "INSERT INTO sessions (id, project_id, harness, title, created_at, parent_id, anchor, plan)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id.to_string(),
                project,
                harness,
                title,
                now(),
                parent.to_string(),
                anchor as i64,
                plan
            ],
        )?;
        Ok(id)
    }

    /// Sets or clears a sub-session's planned question.
    pub fn set_session_plan(&self, id: SessionId, plan: Option<&str>) -> StoreResult<()> {
        self.conn.execute(
            "UPDATE sessions SET plan = ?1 WHERE id = ?2",
            params![plan, id.to_string()],
        )?;
        Ok(())
    }

    /// The sub-sessions of `parent`, oldest first, archived ones left out.
    /// Planned sub-sessions with no events yet are included.
    pub fn sub_sessions(&self, parent: SessionId) -> StoreResult<Vec<SessionSummary>> {
        let mut out = self.summaries_left(
            "WHERE s.parent_id = ?1 AND s.archived_at IS NULL AND s.deleted_at IS NULL",
            &parent.to_string(),
            10_000,
        )?;
        out.reverse();
        Ok(out)
    }

    /// One session's row, archived or not.
    pub fn session(&self, id: SessionId) -> StoreResult<Option<SessionSummary>> {
        Ok(self
            .summaries_left("WHERE s.id = ?1", &id.to_string(), 1)?
            .into_iter()
            .next())
    }

    /// The newest sessions first. Sessions with no events, and archived
    /// ones, are left out.
    /// A chat that was only opened (its log holds nothing but
    /// `session.opened` / `session.resumed`) is left out: it has no
    /// message to show.
    pub fn recent_sessions(&self, limit: usize) -> StoreResult<Vec<SessionSummary>> {
        self.summaries(
            &format!(
                "WHERE s.archived_at IS NULL AND s.deleted_at IS NULL AND ?1 = ?1 AND {HAS_CONTENT}"
            ),
            "",
            limit,
        )
    }

    /// Every session, newest first, archived or not, with or without
    /// events (for the Library index).
    /// Chats that were only opened are left out, but a planned
    /// sub-session (no events yet, a parent) is kept.
    pub fn all_sessions(&self, limit: usize) -> StoreResult<Vec<SessionSummary>> {
        self.summaries_left(
            &format!(
                "WHERE ?1 = ?1 AND s.deleted_at IS NULL AND (s.parent_id IS NOT NULL OR {HAS_CONTENT})"
            ),
            "",
            limit,
        )
    }

    fn summaries(&self, filter: &str, arg: &str, limit: usize) -> StoreResult<Vec<SessionSummary>> {
        self.summaries_with("JOIN", filter, arg, limit)
    }

    /// [`Store::summaries`] that keeps sessions with no events.
    fn summaries_left(
        &self,
        filter: &str,
        arg: &str,
        limit: usize,
    ) -> StoreResult<Vec<SessionSummary>> {
        self.summaries_with("LEFT JOIN", filter, arg, limit)
    }

    fn summaries_with(
        &self,
        join: &str,
        filter: &str,
        arg: &str,
        limit: usize,
    ) -> StoreResult<Vec<SessionSummary>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT s.id, s.title, s.harness, s.created_at, COUNT(e.seq) AS n,
                    s.parent_id, s.anchor, s.plan, s.project_id
             FROM sessions s {join} events e ON e.session_id = s.id
             {filter}
             GROUP BY s.id ORDER BY s.created_at DESC, s.id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![arg, limit as i64], |r| {
            Ok((
                (
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                ),
                (
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                ),
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let ((id, title, harness, created_at, n), (parent, anchor, plan, project)) = row?;
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
                parent: parent.and_then(|p| p.parse().ok()),
                anchor: anchor.map(|a| a as u32),
                plan,
                project: project.and_then(|p| p.parse().ok()),
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
    fn a_chat_that_was_only_opened_is_hidden_everywhere() {
        let (mut s, ses) = setup();
        let p = s.ensure_project("Inbox").unwrap();
        let draft = SessionId::new();
        s.create_session_with_id(draft, p, Harness::Research, "New chat")
            .unwrap();
        // Writing the same id again is harmless.
        s.create_session_with_id(draft, p, Harness::Research, "New chat")
            .unwrap();
        s.append(&ev(draft, "session.opened")).unwrap();
        s.append(&ev(draft, "session.resumed")).unwrap();
        assert!(s.recent_sessions(10).unwrap().is_empty());
        assert!(
            s.all_sessions(10).unwrap().iter().all(|r| r.id != draft),
            "no empty row in the Library"
        );
        s.append(&ev(draft, "turn.started")).unwrap();
        let rows = s.recent_sessions(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, draft);
        assert_ne!(rows[0].id, ses);
        assert!(
            s.create_session_with_id(SessionId::new(), ProjectId::new(), Harness::Study, "x")
                .is_err()
        );
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
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn sub_sessions_hang_under_their_parent() {
        let (mut s, ses) = setup();
        s.append(&ev(ses, "turn.started")).unwrap();
        let sub = s
            .create_sub_session(ses, 12, "Costs", Some("What does it cost?"))
            .unwrap();
        let subs = s.sub_sessions(ses).unwrap();
        assert_eq!(subs.len(), 1, "planned sub-sessions show before they run");
        assert_eq!(subs[0].id, sub);
        assert_eq!(subs[0].parent, Some(ses));
        assert_eq!(subs[0].anchor, Some(12));
        assert_eq!(subs[0].plan.as_deref(), Some("What does it cost?"));
        s.set_session_plan(sub, None).unwrap();
        assert_eq!(s.session(sub).unwrap().unwrap().plan, None);
        assert!(
            s.create_sub_session(SessionId::new(), 1, "x", None)
                .is_err()
        );
        s.archive_session(sub).unwrap();
        assert!(s.is_archived(sub).unwrap());
        assert!(s.sub_sessions(ses).unwrap().is_empty());
        s.unarchive_session(sub).unwrap();
        assert_eq!(s.sub_sessions(ses).unwrap().len(), 1);
        assert_eq!(s.all_sessions(10).unwrap().len(), 2);
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

    #[test]
    fn delete_is_a_tombstone_until_the_trash_is_emptied() {
        let (mut s, ses) = setup();
        s.append(&ev(ses, "turn.started")).unwrap();
        let sub = s.create_sub_session(ses, 1, "Costs", None).unwrap();
        s.append(&ev(sub, "turn.started")).unwrap();
        let other = s
            .create_session(
                s.ensure_project("Inbox").unwrap(),
                Harness::Research,
                "Keep",
            )
            .unwrap();
        s.append(&ev(other, "turn.started")).unwrap();

        s.delete_session(ses).unwrap();
        assert!(s.is_deleted(ses).unwrap());
        assert!(
            s.is_deleted(sub).unwrap(),
            "sub-sessions go with their chat"
        );
        let ids: Vec<_> = s
            .recent_sessions(10)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, [other]);
        assert!(s.all_sessions(10).unwrap().iter().all(|r| r.id != ses));
        assert!(s.sub_sessions(ses).unwrap().is_empty());
        let trash: Vec<_> = s
            .deleted_sessions(10)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(
            trash,
            [ses],
            "the trash lists the chat, not its sub-session"
        );
        // Still append-only while in the trash.
        assert!(
            s.conn
                .execute(
                    "DELETE FROM events WHERE session_id = ?1",
                    [ses.to_string()]
                )
                .is_err()
        );

        s.restore_session(ses).unwrap();
        assert!(!s.is_deleted(sub).unwrap());
        assert_eq!(s.recent_sessions(10).unwrap().len(), 3);

        // Erasing needs the tombstone first.
        assert!(s.purge_session(ses).is_err());
        s.delete_session(ses).unwrap();
        let gone = s.purge_session(ses).unwrap();
        assert_eq!(gone.len(), 2);
        assert!(s.session(ses).unwrap().is_none());
        assert!(s.replay(ses, 0).unwrap().is_empty());
        assert!(s.replay(sub, 0).unwrap().is_empty());
        assert_eq!(s.purge_count().unwrap(), 2);
        // Purge records cannot be changed, and other chats stay append-only.
        assert!(s.conn.execute("DELETE FROM purges", []).is_err());
        assert!(s.conn.execute("UPDATE purges SET title = 'x'", []).is_err());
        assert!(
            s.conn
                .execute(
                    "DELETE FROM events WHERE session_id = ?1",
                    [other.to_string()]
                )
                .is_err()
        );
        assert_eq!(s.replay(other, 0).unwrap().len(), 1);
        // A purged chat takes no new events.
        assert!(matches!(
            s.append(&ev(ses, "x")),
            Err(StoreError::UnknownSession(_))
        ));
    }

    #[test]
    fn archived_view_and_empty_trash() {
        let (mut s, ses) = setup();
        s.append(&ev(ses, "turn.started")).unwrap();
        s.archive_session(ses).unwrap();
        assert_eq!(s.archived_sessions(10).unwrap().len(), 1);
        s.delete_session(ses).unwrap();
        assert!(
            s.archived_sessions(10).unwrap().is_empty(),
            "deleted leaves Archived"
        );
        assert_eq!(s.empty_trash().unwrap(), [ses]);
        assert!(s.deleted_sessions(10).unwrap().is_empty());
        assert!(s.integrity_ok().unwrap());
    }

    #[test]
    fn a_new_database_opened_from_two_threads_at_once_opens_in_both() {
        for round in 0..32 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(format!("race-{round}.sqlite3"));
            let go = std::sync::Arc::new(std::sync::Barrier::new(4));
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let path = path.clone();
                    let go = go.clone();
                    std::thread::spawn(move || {
                        go.wait();
                        Store::open(&path).map(|s| s.schema_version().unwrap())
                    })
                })
                .collect();
            for h in handles {
                let v = h.join().unwrap().expect("every connection opens");
                assert_eq!(v, schema::SCHEMA_VERSION);
            }
        }
    }

    #[test]
    fn version_4_databases_migrate_to_5_keeping_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v4.sqlite3");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(&format!(
                "BEGIN; {} {} {} {} PRAGMA user_version = 4; COMMIT;",
                schema::tests_v(1),
                schema::tests_v(2),
                schema::tests_v(3),
                schema::tests_v(4)
            ))
            .unwrap();
            c.execute(
                "INSERT INTO projects (id, name, created_at) VALUES ('p', 'Inbox', 'now')",
                [],
            )
            .unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.schema_version().unwrap(), 5);
        assert!(
            s.conn.execute("DELETE FROM events", []).is_ok(),
            "no rows, nothing refused"
        );
        assert_eq!(s.purge_count().unwrap(), 0);
    }
}
