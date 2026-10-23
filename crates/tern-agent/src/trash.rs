//! Archiving and deleting chats (ADR 0009).
//!
//! | Action | What changes |
//! | --- | --- |
//! | Archive | The chat leaves the sidebar; it shows under Archived chats |
//! | Delete | A tombstone: the chat (and its sub-sessions) leave every list and show under Deleted; Restore brings them back |
//! | Empty trash / Erase | A permanent purge record, then the chats' events, rows, checkpoints, and run reports are removed |
//!
//! The event log stays append-only for every chat that was not purged: the
//! database refuses to delete events unless a purge record names the chat.

use std::path::Path;

use tern_core::SessionId;
use tern_platform::AppPaths;
use tern_store::{SessionSummary, Store};

/// Puts a chat in the trash.
pub fn delete(store: &Store, id: SessionId) -> Result<(), String> {
    store.delete_session(id).map_err(|e| e.to_string())
}

/// Takes a chat out of the trash.
pub fn restore(store: &Store, id: SessionId) -> Result<(), String> {
    store.restore_session(id).map_err(|e| e.to_string())
}

/// Brings an archived chat back to the sidebar.
pub fn unarchive(store: &Store, id: SessionId) -> Result<(), String> {
    store.unarchive_session(id).map_err(|e| e.to_string())
}

/// Archived and deleted chats, for the Archived chats view.
pub fn lists(store: &Store) -> Result<(Vec<SessionSummary>, Vec<SessionSummary>), String> {
    let archived = store.archived_sessions(500).map_err(|e| e.to_string())?;
    let deleted = store.deleted_sessions(500).map_err(|e| e.to_string())?;
    Ok((archived, deleted))
}

/// Erases one chat in the trash: its purge record, events, rows, and the
/// files kept beside the database under `data` (checkpoints, run reports).
/// Returns how many sessions were erased (the chat and its sub-sessions).
pub fn erase(store: &mut Store, data: &Path, id: SessionId) -> Result<usize, String> {
    let ids = store.purge_session(id).map_err(|e| e.to_string())?;
    remove_files(data, &ids);
    Ok(ids.len())
}

/// Erases every chat in the trash.
pub fn empty(store: &mut Store, data: &Path) -> Result<usize, String> {
    let ids = store.empty_trash().map_err(|e| e.to_string())?;
    remove_files(data, &ids);
    Ok(ids.len())
}

fn remove_files(data: &Path, ids: &[SessionId]) {
    for id in ids {
        let s = id.to_string();
        let _ = std::fs::remove_file(data.join("checkpoints").join(format!("{s}.json")));
        let _ = std::fs::remove_dir_all(data.join("runs").join(&s));
    }
}

/// Opens the app's database.
pub fn open_store() -> Result<Store, String> {
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    Store::open(&paths.database()).map_err(|e| e.to_string())
}

/// The app's data folder, where checkpoints and run reports live.
pub fn data_dir() -> Result<std::path::PathBuf, String> {
    Ok(AppPaths::discover().map_err(|e| e.to_string())?.data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_core::{EffectClass, EventEnvelope};
    use tern_store::Harness;

    #[test]
    fn erase_removes_checkpoint_and_reports_but_only_from_the_trash() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let p = store.ensure_project("Inbox").unwrap();
        let id = store
            .create_session(p, Harness::Research, "Paging")
            .unwrap();
        store
            .append(&EventEnvelope::new(
                id,
                "turn.started",
                EffectClass::Analysis,
                "x",
                serde_json::json!({}),
            ))
            .unwrap();
        std::fs::create_dir_all(dir.path().join("checkpoints")).unwrap();
        std::fs::create_dir_all(dir.path().join("runs").join(id.to_string())).unwrap();
        std::fs::write(
            dir.path().join("checkpoints").join(format!("{id}.json")),
            "{}",
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join("runs")
                .join(id.to_string())
                .join("turn-1.md"),
            "r",
        )
        .unwrap();

        assert!(
            erase(&mut store, dir.path(), id).is_err(),
            "not in the trash yet"
        );
        delete(&store, id).unwrap();
        let (archived, deleted) = lists(&store).unwrap();
        assert!(archived.is_empty());
        assert_eq!(deleted.len(), 1);
        restore(&store, id).unwrap();
        assert!(lists(&store).unwrap().1.is_empty());
        delete(&store, id).unwrap();
        assert_eq!(empty(&mut store, dir.path()).unwrap(), 1);
        assert!(
            !dir.path()
                .join("checkpoints")
                .join(format!("{id}.json"))
                .exists()
        );
        assert!(!dir.path().join("runs").join(id.to_string()).exists());
        assert!(store.session(id).unwrap().is_none());
    }
}
