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
    Ok(plain_path(
        &r.canonicalize().unwrap_or_else(|_| r.to_path_buf()),
    ))
}

/// `p` without Windows' `\\?\` verbatim prefix (what `canonicalize`
/// returns there), so it reads and opens like the path the user picked.
pub fn plain_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p.to_path_buf(),
    }
}

/// The key two spellings of one folder share: no verbatim prefix, one
/// kind of separator, no trailing separator, and on Windows (or a
/// drive-letter path anywhere) case folded, since its file system ignores
/// case.
pub fn folder_key(p: &Path) -> String {
    let plain = plain_path(p).to_string_lossy().into_owned();
    let windows = cfg!(windows) || plain.as_bytes().get(1) == Some(&b':') || plain.contains('\\');
    let mut k = if windows {
        plain.replace('/', "\\").to_lowercase()
    } else {
        plain
    };
    while k.len() > 1 && (k.ends_with('/') || k.ends_with('\\')) && !k.ends_with(":\\") {
        k.pop();
    }
    k
}

/// The name a folder's project gets: the folder's own name (`D:\` gives
/// `D:`), made unique among the projects with " 2", " 3", ….
fn folder_project_name(store: &Store, dir: &Path) -> Result<String, String> {
    let base = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| {
            dir.to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_owned()
        });
    let base = if base.trim().is_empty() || base == INBOX {
        format!("{base} folder").trim().to_owned()
    } else {
        base
    };
    let mut name = base.clone();
    let mut n = 2;
    while store
        .project_named(&name)
        .map_err(|e| e.to_string())?
        .is_some()
    {
        name = format!("{base} {n}");
        n += 1;
    }
    Ok(name)
}

/// The project linked to folder `dir`, made (named after the folder) when
/// none is. Folders match by [`folder_key`], so `D:\Codex` and
/// `d:/codex/` are one project. Returns its id and name.
pub fn for_folder(store: &Store, dir: &Path) -> Result<(ProjectId, String), String> {
    let root = checked_root(dir)?;
    let key = folder_key(&root);
    let all = store.projects().map_err(|e| e.to_string())?;
    if let Some(p) = all
        .iter()
        .find(|p| !p.inbox && p.root.as_deref().is_some_and(|r| folder_key(r) == key))
    {
        return Ok((p.id, p.name.clone()));
    }
    let name = folder_project_name(store, &root)?;
    let id = store
        .new_project(&name, Some(&root))
        .map_err(|e| e.to_string())?;
    Ok((id, name))
}

/// Files each Inbox chat that has a folder open as its workspace under
/// that folder's project (made when missing). Chats the user filed in a
/// project of their own stay put. Returns how many moved.
pub fn file_chats_by_folder(store: &mut Store) -> Result<usize, String> {
    let inbox = store.ensure_project(INBOX).map_err(|e| e.to_string())?;
    let mut moved = 0;
    for (session, path) in store.last_workspaces().map_err(|e| e.to_string())? {
        let Some(path) = path.map(PathBuf::from).filter(|p| p.is_dir()) else {
            continue;
        };
        if store.session_project(session).ok().flatten() != Some(inbox) {
            continue;
        }
        let (project, _) = for_folder(store, &path)?;
        moved += usize::from(move_session(store, session, project)? > 0);
    }
    Ok(moved)
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
    fn one_folder_is_one_project_however_it_is_spelled() {
        assert_eq!(
            folder_key(Path::new(r"\\?\D:\Tern testing\")),
            folder_key(Path::new("d:/tern testing"))
        );
        assert_eq!(folder_key(Path::new("D:\\")), "d:\\");
        assert_eq!(
            plain_path(Path::new(r"\\?\D:\Tern testing")),
            PathBuf::from(r"D:\Tern testing")
        );
        assert_eq!(
            plain_path(Path::new(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\server\share")
        );
        if !cfg!(windows) {
            assert_ne!(
                folder_key(Path::new("/tmp/A")),
                folder_key(Path::new("/tmp/a"))
            );
            assert_eq!(folder_key(Path::new("/tmp/a/")), "/tmp/a");
        }

        let mut s = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("Codex");
        std::fs::create_dir(&codex).unwrap();
        let (id, name) = for_folder(&s, &codex).unwrap();
        assert_eq!(name, "Codex");
        let again = format!("{}/", codex.display());
        assert_eq!(for_folder(&s, Path::new(&again)).unwrap().0, id);
        // Another folder with the same name gets a name of its own.
        let other = dir.path().join("b").join("Codex");
        std::fs::create_dir_all(&other).unwrap();
        let (id2, name2) = for_folder(&s, &other).unwrap();
        assert_ne!(id2, id);
        assert_eq!(name2, "Codex 2");
        assert!(for_folder(&s, &dir.path().join("missing")).is_err());

        // Inbox chats with a folder open are filed under its project.
        let inbox = s.ensure_project(INBOX).unwrap();
        let a = s.create_session(inbox, Harness::Research, "a").unwrap();
        let mine = s.new_project("Mine", None).unwrap();
        let b = s.create_session(mine, Harness::Research, "b").unwrap();
        for ses in [a, b] {
            s.append(&tern_core::EventEnvelope::new(
                ses,
                "session.workspace",
                tern_core::EffectClass::Analysis,
                "w",
                serde_json::json!({"path": codex.display().to_string()}),
            ))
            .unwrap();
        }
        assert_eq!(file_chats_by_folder(&mut s).unwrap(), 1);
        assert_eq!(s.session_project(a).unwrap(), Some(id));
        assert_eq!(s.session_project(b).unwrap(), Some(mine));
        assert_eq!(file_chats_by_folder(&mut s).unwrap(), 0);
    }

    #[test]
    fn find_by_name_or_id_and_link_a_folder() {
        let s = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("TERN.md"), "rules").unwrap();
        let id = create(&s, "Atlas", Some(dir.path())).unwrap();
        assert!(create(&s, "Other", Some(&dir.path().join("missing"))).is_err());
        let all = list(&s).unwrap();
        assert_eq!(all[0].row.name, INBOX);
        let atlas = all.iter().find(|p| p.row.id == id).unwrap();
        assert!(atlas.tern_md.is_some());
        assert_eq!(find(&s, "atlas").unwrap().id, id);
        assert_eq!(find(&s, &id.to_string()).unwrap().id, id);
        assert!(find(&s, "nope").is_err());
        let ses = s.create_session(id, Harness::Research, "x").unwrap();
        assert!(root_of(&s, ses).is_some());
        set_root(&s, id, None).unwrap();
        assert!(root_of(&s, ses).is_none());
    }
}
