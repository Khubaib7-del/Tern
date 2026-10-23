//! The Library of everything (`docs/02-ui-definition.md` §2.1): reports,
//! sources, notes, decks, files, Explain-in-3D scenes, and sessions in
//! one index ([`tern_store::library`]) with search, filter, sort, paging,
//! and bulk delete to a trash that restores.
//!
//! [`sync`] rebuilds the index from where things live: `runs/` for
//! reports, the event log for sources and sessions, the vault for notes,
//! the study folder for decks, files, and scenes. Trashing a file-backed
//! item moves its file (and same-named siblings, such as a report's JSON
//! or a scene's SVG) into `<data>/trash/`; restore moves it back; emptying
//! the trash deletes those files for good. A session in the trash is
//! archived (its log is append-only, so it is never deleted). Sources,
//! decks, and study files leave the index only; the cards and materials
//! themselves stay where Study keeps them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tern_platform::AppPaths;
use tern_store::Store;
pub use tern_store::library::{
    ItemState, LibraryItem, LibraryKind, LibraryPage, LibraryQuery, LibrarySort,
};

use crate::study::StudyStore;
use crate::vault::Vault;

/// Items indexed per kind by the last [`sync`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Synced {
    pub counts: Vec<(LibraryKind, usize)>,
}

impl Synced {
    pub fn total(&self) -> usize {
        self.counts.iter().map(|c| c.1).sum()
    }
}

fn err(e: tern_store::StoreError) -> String {
    e.to_string()
}

fn modified(path: &Path) -> (i64, u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (0, 0);
    };
    let at = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (at, meta.len())
}

fn unix(rfc3339: &str) -> i64 {
    time::OffsetDateTime::parse(rfc3339, &time::format_description::well_known::Rfc3339)
        .map(|t| t.unix_timestamp())
        .unwrap_or(0)
}

fn first_heading(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

/// Where Library sync reads from.
#[derive(Clone, Debug)]
pub struct Sources {
    pub paths: AppPaths,
    pub vault: Option<Vault>,
    pub study: StudyStore,
}

impl Sources {
    /// The user's data folder, vault, and study folder.
    pub fn discover() -> Result<Self, String> {
        let paths = AppPaths::discover().map_err(|e| e.to_string())?;
        let prefs = crate::Preferences::load();
        let vault = crate::vault::default_root(&prefs).ok().map(Vault::at);
        let study = StudyStore::at(crate::study::study_dir(&paths));
        Ok(Self {
            paths,
            vault,
            study,
        })
    }

    fn trash_dir(&self) -> PathBuf {
        self.paths.data_dir.join("trash")
    }
}

/// Rebuilds the index from every place things live. Rows in the trash or
/// purged keep their state.
pub fn sync(store: &mut Store, src: &Sources) -> Result<Synced, String> {
    let mut out = Synced::default();
    let sessions = store.all_sessions(100_000).map_err(err)?;
    let titles: std::collections::HashMap<String, String> = sessions
        .iter()
        .map(|s| (s.id.to_string(), s.title.clone()))
        .collect();

    // Sessions and the sources their turns read.
    let mut session_items = Vec::new();
    let mut source_items: Vec<LibraryItem> = Vec::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    for s in &sessions {
        let events = store.replay(s.id, 0).map_err(err)?;
        let refs: Vec<String> = match s.parent {
            Some(_) => crate::refs::refs_of(store, s.id)?
                .iter()
                .map(|r| r.to_string())
                .collect(),
            None => (1..=events.iter().filter(|e| e.kind == "turn.started").count())
                .map(|n| format!("Q{n}"))
                .collect(),
        };
        let at = unix(&s.created_at);
        let last = events.last().map(|e| e.ts.unix_timestamp()).unwrap_or(at);
        let mut it = LibraryItem::new(
            LibraryKind::Session,
            format!("session:{}", s.id),
            s.title.clone(),
        );
        it.location = s.id.to_string();
        it.detail = match (s.parent, s.anchor) {
            (Some(p), Some(a)) => format!(
                "↳ from Q{a} of {} · {}",
                titles.get(&p.to_string()).cloned().unwrap_or_default(),
                count(refs.len(), "message")
            ),
            _ => format!(
                "{} · {}",
                if s.harness == tern_store::Harness::Study {
                    "Study"
                } else {
                    "Research"
                },
                count(refs.len(), "message")
            ),
        };
        it.search = format!("{} {}", s.id, refs.join(" "));
        it.size = s.events;
        it.created_at = at;
        it.updated_at = last;
        session_items.push(it);
        for e in &events {
            let Some(p) = crate::sources::page_of(&e.kind, &e.payload) else {
                continue;
            };
            if p.url.is_empty() || !seen_urls.insert(p.url.clone()) {
                if let Some(x) = source_items.iter_mut().find(|x| x.location == p.url) {
                    x.size += 1;
                    x.updated_at = x.updated_at.max(e.ts.unix_timestamp());
                }
                continue;
            }
            let mut it =
                LibraryItem::new(LibraryKind::Source, format!("source:{}", p.url), p.label());
            it.location = p.url.clone();
            it.detail = format!("{} · {}", p.site, s.title);
            it.size = 1;
            it.created_at = e.ts.unix_timestamp();
            it.updated_at = it.created_at;
            source_items.push(it);
        }
    }
    put(store, LibraryKind::Session, session_items, &mut out)?;
    put(store, LibraryKind::Source, source_items, &mut out)?;

    // Reports: runs/<chat>/turn-<n>.md.
    let mut reports = Vec::new();
    let runs = src.paths.data_dir.join("runs");
    for chat in std::fs::read_dir(&runs).into_iter().flatten().flatten() {
        let chat_id = chat.file_name().to_string_lossy().into_owned();
        for f in std::fs::read_dir(chat.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = f.path();
            if path.extension().is_none_or(|x| x != "md") {
                continue;
            }
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (at, size) = modified(&path);
            let title = first_heading(&path).unwrap_or_else(|| format!("Report · {stem}"));
            let mut it = LibraryItem::new(
                LibraryKind::Report,
                format!("report:{chat_id}/{stem}"),
                title,
            );
            it.location = path.to_string_lossy().into_owned();
            it.detail = format!(
                "{} · {}",
                titles.get(&chat_id).cloned().unwrap_or(chat_id.clone()),
                stem.replace('-', " ")
            );
            it.size = size;
            it.created_at = at;
            it.updated_at = at;
            reports.push(it);
        }
    }
    put(store, LibraryKind::Report, reports, &mut out)?;

    // Notes: every .md in the vault.
    let mut notes = Vec::new();
    if let Some(v) = &src.vault {
        for n in v.list() {
            let path = v.root().join(&n.path);
            let (at, size) = modified(&path);
            let mut it = LibraryItem::new(LibraryKind::Note, format!("note:{}", n.path), n.name());
            it.location = path.to_string_lossy().into_owned();
            it.detail = if n.folder().is_empty() {
                "Vault".to_owned()
            } else {
                format!("Vault · {}", n.folder())
            };
            it.search = n.path.clone();
            it.size = size;
            it.created_at = at;
            it.updated_at = at;
            notes.push(it);
        }
    }
    put(store, LibraryKind::Note, notes, &mut out)?;

    // Decks: the flashcards, one deck per topic.
    let mut decks: Vec<LibraryItem> = Vec::new();
    if let Ok(deck) = crate::cards::load_deck(&src.study) {
        let cards_file = src.study.root().join("cards.json");
        let (at, _) = modified(&cards_file);
        for c in &deck.cards {
            let topic = c.topic.clone().unwrap_or_else(|| "General".into());
            let id = format!("deck:{topic}");
            match decks.iter_mut().find(|d| d.id == id) {
                Some(d) => {
                    d.size += 1;
                    d.created_at = d.created_at.min(c.created_at);
                }
                None => {
                    let mut it = LibraryItem::new(LibraryKind::Deck, id, format!("{topic} cards"));
                    it.location = cards_file.to_string_lossy().into_owned();
                    it.search = topic.clone();
                    it.size = 1;
                    it.created_at = c.created_at;
                    it.updated_at = at.max(c.created_at);
                    decks.push(it);
                }
            }
        }
        for d in &mut decks {
            d.detail = format!("{} cards", d.size);
        }
    }
    put(store, LibraryKind::Deck, decks, &mut out)?;

    // Files: study materials.
    let mut files = Vec::new();
    if let Ok(lib) = src.study.load() {
        for m in &lib.materials {
            let mut it = LibraryItem::new(LibraryKind::File, format!("file:{}", m.id), &m.name);
            it.location = m.origin.clone();
            it.detail = format!("{:?} · {} chunks", m.kind, m.chunks);
            it.size = m.chars as u64;
            it.created_at = m.added_at;
            it.updated_at = m.added_at;
            files.push(it);
        }
    }
    put(store, LibraryKind::File, files, &mut out)?;

    // Explain-in-3D scenes.
    let mut scenes = Vec::new();
    for (id, concept, at) in crate::explain::list(&src.study) {
        let path = src.study.root().join("explain").join(format!("{id}.json"));
        let (_, size) = modified(&path);
        let mut it = LibraryItem::new(LibraryKind::Scene, format!("scene:{id}"), concept);
        it.location = path.to_string_lossy().into_owned();
        it.detail = "Explain in 3D".into();
        it.size = size;
        it.created_at = at;
        it.updated_at = at;
        scenes.push(it);
    }
    put(store, LibraryKind::Scene, scenes, &mut out)?;
    Ok(out)
}

fn put(
    store: &mut Store,
    kind: LibraryKind,
    items: Vec<LibraryItem>,
    out: &mut Synced,
) -> Result<(), String> {
    let seen: HashSet<String> = items.iter().map(|i| i.id.clone()).collect();
    store.library_upsert(&items).map_err(err)?;
    store.library_forget_missing(kind, &seen).map_err(err)?;
    out.counts.push((kind, items.len()));
    Ok(())
}

/// Whether a kind keeps a file Tern moves to the trash.
fn file_backed(kind: LibraryKind) -> bool {
    matches!(
        kind,
        LibraryKind::Report | LibraryKind::Note | LibraryKind::Scene
    )
}

fn slot(src: &Sources, id: &str) -> PathBuf {
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    src.trash_dir().join(safe)
}

/// The file and its same-stem siblings (`turn-3.md` and `turn-3.json`).
fn with_siblings(path: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) else {
        return vec![path.to_path_buf()];
    };
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.file_stem() == Some(stem))
        .collect();
    if out.is_empty() {
        out.push(path.to_path_buf());
    }
    out
}

fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|e| format!("{}: {e}", from.display()))?;
    std::fs::remove_file(from).map_err(|e| e.to_string())
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// Moves items to the trash. Returns how many moved.
pub fn trash(store: &mut Store, src: &Sources, ids: &[String]) -> Result<usize, String> {
    let mut moved = Vec::new();
    for id in ids {
        let Some(it) = store.library_get(id).map_err(err)? else {
            continue;
        };
        if it.state != ItemState::Live {
            continue;
        }
        let kind = it.kind();
        if file_backed(kind) {
            let path = PathBuf::from(&it.location);
            if path.exists() {
                let dir = slot(src, id);
                for f in with_siblings(&path) {
                    if let Some(name) = f.file_name() {
                        move_file(&f, &dir.join(name))?;
                    }
                }
            }
        } else if kind == LibraryKind::Session
            && let Ok(s) = it.location.parse()
        {
            store.archive_session(s).map_err(err)?;
        }
        moved.push(id.clone());
    }
    Ok(store
        .library_set_state(&moved, ItemState::Trash, now())
        .map_err(err)?
        .len())
}

/// Brings items back from the trash. Returns how many came back.
pub fn restore(store: &mut Store, src: &Sources, ids: &[String]) -> Result<usize, String> {
    let mut back = Vec::new();
    for id in ids {
        let Some(it) = store.library_get(id).map_err(err)? else {
            continue;
        };
        if it.state != ItemState::Trash {
            continue;
        }
        let kind = it.kind();
        if file_backed(kind) {
            let dir = slot(src, id);
            let target = PathBuf::from(&it.location);
            if let Some(parent) = target.parent() {
                for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    move_file(&f.path(), &parent.join(f.file_name()))?;
                }
            }
            let _ = std::fs::remove_dir(&dir);
        } else if kind == LibraryKind::Session
            && let Ok(s) = it.location.parse()
        {
            store.unarchive_session(s).map_err(err)?;
        }
        back.push(id.clone());
    }
    Ok(store
        .library_set_state(&back, ItemState::Live, 0)
        .map_err(err)?
        .len())
}

/// Deletes what is in the trash for good. Sessions stay archived (their
/// log is append-only); every row is marked purged so sync skips it.
pub fn empty_trash(store: &mut Store, src: &Sources) -> Result<usize, String> {
    let items = store.library_trash().map_err(err)?;
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    for it in &items {
        if file_backed(it.kind()) {
            let dir = slot(src, &it.id);
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(store
        .library_set_state(&ids, ItemState::Purged, 0)
        .map_err(err)?
        .len())
}

/// Opens the user's database and sources, syncs, and hands both over.
pub fn open() -> Result<(Store, Sources), String> {
    let src = Sources::discover()?;
    src.paths.ensure().map_err(|e| e.to_string())?;
    let mut store = Store::open(&src.paths.database()).map_err(|e| e.to_string())?;
    sync(&mut store, &src)?;
    Ok((store, src))
}

/// "1 message", "3 messages".
pub fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// `1.2 KB`, `3 MB`.
pub fn size_label(item: &LibraryItem) -> String {
    match item.kind() {
        LibraryKind::Deck => format!("{} cards", item.size),
        LibraryKind::Session => count(item.size as usize, "event"),
        LibraryKind::Source => format!("read {}×", item.size),
        LibraryKind::File => format!("{} chars", item.size),
        _ => {
            let b = item.size as f64;
            if b >= 1_048_576.0 {
                format!("{:.1} MB", b / 1_048_576.0)
            } else if b >= 1024.0 {
                format!("{:.1} KB", b / 1024.0)
            } else {
                format!("{} B", item.size)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::INBOX;
    use serde_json::json;
    use tern_core::{EffectClass, EventEnvelope};
    use tern_store::Harness;

    #[test]
    fn sync_trash_restore_and_empty() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::at(dir.path());
        let vault = Vault::at(dir.path().join("vault"));
        vault.write("Notes/RAG.md", "# RAG\nnotes").unwrap();
        let study = StudyStore::at(dir.path().join("study"));
        let mut store = Store::open_in_memory().unwrap();
        let p = store.ensure_project(INBOX).unwrap();
        let s = store
            .create_session(p, Harness::Research, "Vector DBs")
            .unwrap();
        store
            .append(&EventEnvelope::new(
                s,
                "turn.started",
                EffectClass::Analysis,
                "x",
                json!({"message": "q"}),
            ))
            .unwrap();
        store
            .append(&EventEnvelope::new(
                s,
                "tool.executed",
                EffectClass::Network,
                "Read",
                json!({"meta": {"page": {"title": "pgvector", "url": "https://github.com/pgvector/pgvector", "site": "github.com"}}}),
            ))
            .unwrap();
        let runs = crate::report::runs_dir(&paths, s);
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(runs.join("turn-1.md"), "# Which DB?\n").unwrap();
        std::fs::write(runs.join("turn-1.json"), "{}").unwrap();
        let src = Sources {
            paths: paths.clone(),
            vault: Some(vault.clone()),
            study,
        };
        let synced = sync(&mut store, &src).unwrap();
        assert_eq!(synced.total(), 4);
        let all = store.library_query(&LibraryQuery::default()).unwrap();
        let kinds: HashSet<_> = all.items.iter().map(|i| i.kind()).collect();
        assert!(kinds.contains(&LibraryKind::Report) && kinds.contains(&LibraryKind::Source));
        let q1 = store
            .library_query(&LibraryQuery {
                text: "Q1".into(),
                ..LibraryQuery::default()
            })
            .unwrap();
        assert_eq!(
            q1.items[0].id,
            format!("session:{s}"),
            "message ids find their session"
        );

        let report = format!("report:{s}/turn-1");
        let ids = vec![
            report.clone(),
            "note:Notes/RAG.md".to_owned(),
            format!("session:{s}"),
        ];
        assert_eq!(trash(&mut store, &src, &ids).unwrap(), 3);
        assert!(!runs.join("turn-1.md").exists() && !runs.join("turn-1.json").exists());
        assert!(vault.read("Notes/RAG.md").is_err());
        assert!(store.is_archived(s).unwrap());
        // A sync while things are in the trash keeps them there.
        sync(&mut store, &src).unwrap();
        assert_eq!(store.library_trash().unwrap().len(), 3);

        assert_eq!(restore(&mut store, &src, &ids[..2]).unwrap(), 2);
        assert!(runs.join("turn-1.md").exists() && runs.join("turn-1.json").exists());
        assert_eq!(vault.read("Notes/RAG.md").unwrap(), "# RAG\nnotes");
        assert_eq!(restore(&mut store, &src, &ids[2..]).unwrap(), 1);
        assert!(!store.is_archived(s).unwrap());

        trash(&mut store, &src, &ids[..1]).unwrap();
        assert_eq!(empty_trash(&mut store, &src).unwrap(), 1);
        assert!(!runs.join("turn-1.md").exists());
        assert_eq!(
            store.library_get(&report).unwrap().unwrap().state,
            ItemState::Purged
        );
        assert!(store.library_trash().unwrap().is_empty());
    }
}
