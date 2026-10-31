//! Project folders: a name, an optional root folder on disk, and the
//! sessions filed under it. Chats with no project live in the Inbox, which
//! is itself a project row that cannot be renamed or deleted.

use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use tern_core::{ProjectId, SessionId};

use crate::{Store, StoreError, StoreResult};

/// The project unassigned chats are filed under.
pub const INBOX: &str = "Inbox";

/// One project folder for the sidebar and `tern project list`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ProjectSummary {
    pub id: ProjectId,
    pub name: String,
    /// The folder the project works in (`TERN.md` and `.tern/skills`
    /// are read from here), when one was linked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    pub created_at: String,
    /// Top-level sessions filed here that are not archived.
    pub sessions: u64,
    /// This is the Inbox.
    pub inbox: bool,
}

impl Store {
    /// Every project, the Inbox first, then by sidebar position and age.
    pub fn projects(&self) -> StoreResult<Vec<ProjectSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.name, p.root_path, p.created_at,
                    (SELECT COUNT(*) FROM sessions s
                      WHERE s.project_id = p.id AND s.parent_id IS NULL AND s.archived_at IS NULL)
             FROM projects p
             ORDER BY (p.name = ?1) DESC, COALESCE(p.position, 1 << 30), p.created_at, p.id",
        )?;
        let rows = stmt.query_map([INBOX], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        let mut seen_inbox = false;
        for row in rows {
            let (id, name, root, created_at, n) = row?;
            let Ok(id) = id.parse() else { continue };
            // Older databases can hold two rows named Inbox; the oldest wins.
            let inbox = name == INBOX && !seen_inbox;
            seen_inbox |= inbox;
            out.push(ProjectSummary {
                id,
                name,
                root: root.map(PathBuf::from),
                created_at,
                sessions: n as u64,
                inbox,
            });
        }
        Ok(out)
    }

    /// One project's row.
    pub fn project(&self, id: ProjectId) -> StoreResult<Option<ProjectSummary>> {
        Ok(self.projects()?.into_iter().find(|p| p.id == id))
    }

    /// Makes a project folder. Names are trimmed; a blank name or a second
    /// project with the same name (ignoring case) is refused.
    pub fn new_project(&self, name: &str, root: Option<&Path>) -> StoreResult<ProjectId> {
        let name = clean(name)?;
        if self.project_named(&name)?.is_some() {
            return Err(StoreError::Invalid(format!(
                "There is already a project called “{name}”."
            )));
        }
        let id = self.create_project(&name, None)?;
        if let Some(r) = root {
            self.set_project_root(id, Some(r))?;
        }
        Ok(id)
    }

    /// The project with this name, ignoring case.
    pub fn project_named(&self, name: &str) -> StoreResult<Option<ProjectId>> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM projects WHERE name = ?1 COLLATE NOCASE ORDER BY created_at, id LIMIT 1",
                [name.trim()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id.and_then(|s| s.parse().ok()))
    }

    /// Renames a project. The Inbox keeps its name.
    pub fn rename_project(&self, id: ProjectId, name: &str) -> StoreResult<()> {
        let name = clean(name)?;
        let row = self.project(id)?.ok_or(StoreError::UnknownProject(id))?;
        if row.inbox {
            return Err(StoreError::Invalid("The Inbox cannot be renamed.".into()));
        }
        if let Some(other) = self.project_named(&name)?
            && other != id
        {
            return Err(StoreError::Invalid(format!(
                "There is already a project called “{name}”."
            )));
        }
        self.conn.execute(
            "UPDATE projects SET name = ?1 WHERE id = ?2",
            params![name, id.to_string()],
        )?;
        Ok(())
    }

    /// Links (or unlinks) the project's root folder.
    pub fn set_project_root(&self, id: ProjectId, root: Option<&Path>) -> StoreResult<()> {
        let n = self.conn.execute(
            "UPDATE projects SET root_path = ?1 WHERE id = ?2",
            params![
                root.map(|p| p.to_string_lossy().into_owned()),
                id.to_string()
            ],
        )?;
        if n == 0 {
            return Err(StoreError::UnknownProject(id));
        }
        Ok(())
    }

    /// Deletes a project folder. Its sessions are not deleted: they move to
    /// the Inbox. Returns how many moved.
    pub fn delete_project(&mut self, id: ProjectId) -> StoreResult<usize> {
        let row = self.project(id)?.ok_or(StoreError::UnknownProject(id))?;
        if row.inbox {
            return Err(StoreError::Invalid("The Inbox cannot be deleted.".into()));
        }
        let inbox = self.ensure_project(INBOX)?;
        let tx = self.conn.transaction()?;
        let moved = tx.execute(
            "UPDATE sessions SET project_id = ?1 WHERE project_id = ?2",
            params![inbox.to_string(), id.to_string()],
        )?;
        tx.execute("DELETE FROM projects WHERE id = ?1", [id.to_string()])?;
        tx.commit()?;
        Ok(moved)
    }

    /// Files a session (with its sub-sessions, at any depth) under a project.
    /// Returns how many sessions moved.
    pub fn move_session(&mut self, session: SessionId, project: ProjectId) -> StoreResult<usize> {
        if self.project(project)?.is_none() {
            return Err(StoreError::UnknownProject(project));
        }
        if self.session(session)?.is_none() {
            return Err(StoreError::UnknownSession(session));
        }
        let tx = self.conn.transaction()?;
        let n = tx.execute(
            "WITH RECURSIVE tree(id) AS (
                 SELECT ?1
                 UNION ALL
                 SELECT s.id FROM sessions s JOIN tree t ON s.parent_id = t.id
             )
             UPDATE sessions SET project_id = ?2 WHERE id IN (SELECT id FROM tree)",
            params![session.to_string(), project.to_string()],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// The project a session is filed under.
    pub fn session_project(&self, session: SessionId) -> StoreResult<Option<ProjectId>> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT project_id FROM sessions WHERE id = ?1",
                [session.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(id.and_then(|s| s.parse().ok()))
    }

    /// The folder each chat last had open as its workspace (the path of its
    /// last `session.workspace` event; `None` when that event closed it),
    /// for top-level chats that are not archived or deleted.
    pub fn last_workspaces(&self) -> StoreResult<Vec<(SessionId, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.session_id, e.body FROM events e
               JOIN sessions s ON s.id = e.session_id
              WHERE e.type = 'session.workspace'
                AND s.parent_id IS NULL AND s.archived_at IS NULL AND s.deleted_at IS NULL
                AND e.seq = (SELECT MAX(seq) FROM events x
                              WHERE x.session_id = e.session_id AND x.type = 'session.workspace')",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, body) = row?;
            let Ok(id) = id.parse() else { continue };
            let path = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["payload"]["path"].as_str().map(str::to_owned));
            out.push((id, path));
        }
        Ok(out)
    }

    /// Sets the sidebar order: projects in `order` get positions 0, 1, ….
    pub fn reorder_projects(&self, order: &[ProjectId]) -> StoreResult<()> {
        for (i, id) in order.iter().enumerate() {
            self.conn.execute(
                "UPDATE projects SET position = ?1 WHERE id = ?2",
                params![i as i64, id.to_string()],
            )?;
        }
        Ok(())
    }
}

fn clean(name: &str) -> StoreResult<String> {
    let line = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return Err(StoreError::Invalid("A project needs a name.".into()));
    }
    Ok(line.chars().take(80).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Harness;
    use serde_json::json;
    use tern_core::{EffectClass, EventEnvelope};

    fn ev(ses: SessionId) -> EventEnvelope {
        EventEnvelope::new(ses, "turn.started", EffectClass::Analysis, "x", json!({}))
    }

    #[test]
    fn projects_hold_sessions_and_deleting_moves_them_to_the_inbox() {
        let mut s = Store::open_in_memory().unwrap();
        let inbox = s.ensure_project(INBOX).unwrap();
        let a = s.create_session(inbox, Harness::Research, "loose").unwrap();
        s.append(&ev(a)).unwrap();
        let atlas = s.new_project("  Atlas   notes ", None).unwrap();
        assert!(
            s.new_project("atlas NOTES", None).is_err(),
            "names are unique"
        );
        assert!(s.new_project("   ", None).is_err());
        let rows = s.projects().unwrap();
        assert_eq!(rows[0].name, INBOX);
        assert!(rows[0].inbox);
        assert_eq!(rows[1].name, "Atlas notes");
        assert_eq!(rows[0].sessions, 1);

        // Moving a chat moves its sub-sessions too.
        let sub = s.create_sub_session(a, 1, "sub", None).unwrap();
        let subsub = s.create_sub_session(sub, 1, "subsub", None).unwrap();
        assert_eq!(s.move_session(a, atlas).unwrap(), 3);
        assert_eq!(s.session_project(subsub).unwrap(), Some(atlas));
        assert_eq!(s.session(a).unwrap().unwrap().project, Some(atlas));
        let rows = s.projects().unwrap();
        assert_eq!((rows[0].sessions, rows[1].sessions), (0, 1));

        s.rename_project(atlas, "Atlas project").unwrap();
        assert!(s.rename_project(inbox, "Other").is_err());
        s.set_project_root(atlas, Some(Path::new("/tmp/atlas")))
            .unwrap();
        assert_eq!(
            s.project(atlas).unwrap().unwrap().root,
            Some(PathBuf::from("/tmp/atlas"))
        );

        assert_eq!(s.delete_project(atlas).unwrap(), 3);
        assert_eq!(s.session_project(a).unwrap(), Some(inbox));
        assert!(s.delete_project(inbox).is_err());
        assert_eq!(s.projects().unwrap().len(), 1);
    }

    #[test]
    fn order_is_kept() {
        let s = Store::open_in_memory().unwrap();
        s.ensure_project(INBOX).unwrap();
        let a = s.new_project("A", None).unwrap();
        let b = s.new_project("B", None).unwrap();
        s.reorder_projects(&[b, a]).unwrap();
        let names: Vec<_> = s.projects().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, [INBOX, "B", "A"]);
    }

    #[test]
    fn the_last_workspace_of_each_chat() {
        let mut s = Store::open_in_memory().unwrap();
        let inbox = s.ensure_project(INBOX).unwrap();
        let a = s.create_session(inbox, Harness::Research, "a").unwrap();
        let b = s.create_session(inbox, Harness::Research, "b").unwrap();
        let ws = |ses, path: Option<&str>| {
            EventEnvelope::new(
                ses,
                "session.workspace",
                EffectClass::Analysis,
                "w",
                json!({"path": path}),
            )
        };
        s.append(&ws(a, Some("/tmp/one"))).unwrap();
        s.append(&ev(a)).unwrap();
        s.append(&ws(a, Some("/tmp/two"))).unwrap();
        s.append(&ws(b, Some("/tmp/one"))).unwrap();
        s.append(&ws(b, None)).unwrap();
        let mut got = s.last_workspaces().unwrap();
        got.sort_by_key(|(id, _)| *id != a);
        assert_eq!(got, [(a, Some("/tmp/two".into())), (b, None)]);
    }
}
