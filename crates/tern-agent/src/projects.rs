//! Project folders for front ends: list, create, rename, link a root
//! folder, delete (its sessions move to the Inbox), and file a session
//! under a project. A project's root folder is where its `TERN.md` and
//! `.tern/skills` are read from.

use std::path::{Path, PathBuf};

use tern_core::{ProjectId, SessionId};
use tern_store::{ProjectSummary, Store};

pub use tern_store::INBOX;

/// A project as the sidebar shows it, with its `TERN.md` when the root
/// folder has one.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Project {
    #[serde(flatten)]
    pub row: ProjectSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tern_md: Option<PathBuf>,
}

fn with_rules(row: ProjectSummary) -> Project {
    let tern_md = row
        .root
        .as_ref()
        .map(|r| r.join("TERN.md"))
        .filter(|p| p.is_file());
    Project { row, tern_md }
}

/// Every project, the Inbox first.
pub fn list(store: &Store) -> Result<Vec<Project>, String> {
    store.ensure_project(INBOX).map_err(|e| e.to_string())?;
    Ok(store
        .projects()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(with_rules)
        .collect())
}

/// Finds a project by name (ignoring case), id, or the start of its id.
pub fn find(store: &Store, which: &str) -> Result<ProjectSummary, String> {
    let which = which.trim();
    let all = store.projects().map_err(|e| e.to_string())?;
    if let Some(p) = all.iter().find(|p| p.name.eq_ignore_ascii_case(which)) {
        return Ok(p.clone());
    }
    let hits: Vec<_> = all
        .iter()
        .filter(|p| which.len() >= 4 && p.id.to_string().starts_with(which))
        .collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!(
            "There is no project called “{which}”. See `tern project list`."
        )),
        _ => Err(format!(
            "“{which}” matches more than one project; give more of its id."
        )),
    }
}

/// Makes a project, optionally linked to a folder (which must exist).
pub fn create(store: &Store, name: &str, root: Option<&Path>) -> Result<ProjectId, String> {
    let root = match root {
        Some(r) => Some(checked_root(r)?),
        None => None,
    };
    store
        .new_project(name, root.as_deref())
        .map_err(|e| e.to_string())
}

fn checked_root(r: &Path) -> Result<PathBuf, String> {
    if !r.is_dir() {
        return Err(format!("{} is not a folder.", r.display()));
    }
    Ok(r.canonicalize().unwrap_or_else(|_| r.to_path_buf()))
}

pub fn rename(store: &Store, id: ProjectId, name: &str) -> Result<(), String> {
    store.rename_project(id, name).map_err(|e| e.to_string())
}

/// Links (or, with `None`, unlinks) a project's root folder.
pub fn set_root(store: &Store, id: ProjectId, root: Option<&Path>) -> Result<(), String> {
    let root = match root {
        Some(r) => Some(checked_root(r)?),
        None => None,
    };
    store
        .set_project_root(id, root.as_deref())
        .map_err(|e| e.to_string())
}

/// Deletes a project; its sessions move to the Inbox. Returns how many.
pub fn delete(store: &mut Store, id: ProjectId) -> Result<usize, String> {
    store.delete_project(id).map_err(|e| e.to_string())
}

/// Files a session and its sub-sessions under a project.
pub fn move_session(store: &mut Store, session: SessionId, to: ProjectId) -> Result<usize, String> {
    store.move_session(session, to).map_err(|e| e.to_string())
}

/// The root folder of the project a session is filed under.
pub fn root_of(store: &Store, session: SessionId) -> Option<PathBuf> {
    let p = store.session_project(session).ok().flatten()?;
    store.project(p).ok().flatten()?.root
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_store::Harness;

    #[test]
    fn find_by_name_or_id_and_link_a_folder() {
        let s = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("TERN.md"), "rules").unwrap();
        let id = create(&s, "FYP", Some(dir.path())).unwrap();
        assert!(create(&s, "Other", Some(&dir.path().join("missing"))).is_err());
        let all = list(&s).unwrap();
        assert_eq!(all[0].row.name, INBOX);
        let fyp = all.iter().find(|p| p.row.id == id).unwrap();
        assert!(fyp.tern_md.is_some());
        assert_eq!(find(&s, "fyp").unwrap().id, id);
        assert_eq!(find(&s, &id.to_string()).unwrap().id, id);
        assert!(find(&s, "nope").is_err());
        let ses = s.create_session(id, Harness::Research, "x").unwrap();
        assert!(root_of(&s, ses).is_some());
        set_root(&s, id, None).unwrap();
        assert!(root_of(&s, ses).is_none());
    }
}
