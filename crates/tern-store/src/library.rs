//! The Library's unified index: one row per report, source, note, deck,
//! file, Explain-in-3D scene, and session, whatever keeps the thing
//! itself. Search, filter, sort, and paging happen here in SQLite so the
//! Library stays quick with thousands of items; bulk delete moves rows to
//! the trash, restore brings them back, and emptying the trash marks them
//! purged so a later sync does not add them again.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, params, params_from_iter};
use tern_core::MessageRef;

use crate::{Store, StoreResult};

/// What a Library item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryKind {
    Report,
    Source,
    Note,
    Deck,
    File,
    Scene,
    Session,
}

impl LibraryKind {
    pub const ALL: [Self; 7] = [
        Self::Report,
        Self::Source,
        Self::Note,
        Self::Deck,
        Self::File,
        Self::Scene,
        Self::Session,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::Source => "source",
            Self::Note => "note",
            Self::Deck => "deck",
            Self::File => "file",
            Self::Scene => "scene",
            Self::Session => "session",
        }
    }

    /// The tab name: Reports, Sources, ….
    pub fn plural(self) -> &'static str {
        match self {
            Self::Report => "Reports",
            Self::Source => "Sources",
            Self::Note => "Notes",
            Self::Deck => "Decks",
            Self::File => "Files",
            Self::Scene => "Explain in 3D",
            Self::Session => "Sessions",
        }
    }

    /// Reads `report`, `reports`, `Notes`, `3d`, ….
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        let s = s.strip_suffix('s').unwrap_or(&s);
        Some(match s {
            "report" => Self::Report,
            "source" => Self::Source,
            "note" => Self::Note,
            "deck" | "card" => Self::Deck,
            "file" | "material" => Self::File,
            "scene" | "3d" | "explain" | "explain3d" => Self::Scene,
            "session" | "chat" => Self::Session,
            _ => return None,
        })
    }
}

/// Where a Library row stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    #[default]
    Live,
    Trash,
    Purged,
}

impl ItemState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Trash => "trash",
            Self::Purged => "purged",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "trash" => Self::Trash,
            "purged" => Self::Purged,
            _ => Self::Live,
        }
    }
}

/// One Library row.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct LibraryItem {
    /// Stable across syncs: `report:<chat>/turn-3`, `note:Notes/RAG.md`, ….
    pub id: String,
    pub kind: Option<LibraryKind>,
    pub title: String,
    /// The file path, URL, or id that opens the thing.
    pub location: String,
    /// One line under the title: the site, the chat, the card count.
    pub detail: String,
    /// More words search should find (message ids for sessions).
    #[serde(skip)]
    pub search: String,
    pub size: u64,
    /// Unix seconds.
    pub created_at: i64,
    pub updated_at: i64,
    pub state: ItemState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trashed_at: Option<i64>,
}

impl LibraryItem {
    pub fn new(kind: LibraryKind, id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: Some(kind),
            title: title.into(),
            ..Self::default()
        }
    }

    pub fn kind(&self) -> LibraryKind {
        self.kind.unwrap_or(LibraryKind::File)
    }
}

/// How the Library is ordered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LibrarySort {
    #[default]
    Newest,
    Oldest,
    Title,
    Kind,
    Largest,
}

impl LibrarySort {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "newest" | "new" | "recent" => Self::Newest,
            "oldest" | "old" => Self::Oldest,
            "title" | "name" | "az" => Self::Title,
            "kind" | "type" => Self::Kind,
            "largest" | "size" => Self::Largest,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Newest => "Newest",
            Self::Oldest => "Oldest",
            Self::Title => "Title",
            Self::Kind => "Kind",
            Self::Largest => "Largest",
        }
    }

    fn order(self) -> &'static str {
        match self {
            Self::Newest => "updated_at DESC, id",
            Self::Oldest => "updated_at ASC, id",
            Self::Title => "title COLLATE NOCASE ASC, id",
            Self::Kind => "kind ASC, updated_at DESC, id",
            Self::Largest => "size DESC, id",
        }
    }
}

/// One page of the Library.
#[derive(Clone, Debug, Default)]
pub struct LibraryQuery {
    /// Empty means every kind (the All tab).
    pub kinds: Vec<LibraryKind>,
    /// Words that must all appear in the title, detail, or search text. A
    /// message id (`Q12.3`) matches that message exactly.
    pub text: String,
    /// `Live` for the Library, `Trash` for the trash.
    pub state: ItemState,
    pub sort: LibrarySort,
    pub offset: usize,
    /// 0 means 100.
    pub limit: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryPage {
    pub items: Vec<LibraryItem>,
    /// Rows matching the query across all pages.
    pub total: usize,
    pub offset: usize,
}

const COLUMNS: &str =
    "id, kind, title, location, detail, search, size, created_at, updated_at, state, trashed_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<LibraryItem> {
    let kind: String = r.get(1)?;
    let state: String = r.get(9)?;
    Ok(LibraryItem {
        id: r.get(0)?,
        kind: LibraryKind::parse(&kind),
        title: r.get(2)?,
        location: r.get(3)?,
        detail: r.get(4)?,
        search: r.get(5)?,
        size: r.get::<_, i64>(6)?.max(0) as u64,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
        state: ItemState::parse(&state),
        trashed_at: r.get(10)?,
    })
}

/// The text a query matches against: lower case, message ids padded with
/// spaces so `q12.3` never matches `q12.30`.
fn haystack(item: &LibraryItem) -> String {
    format!(
        " {} {} {} ",
        item.title.to_lowercase(),
        item.detail.to_lowercase(),
        item.search.to_lowercase()
    )
}

/// Escapes `%`, `_`, and `\` for `LIKE … ESCAPE '\'`.
fn like_escape(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    for c in word.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Store {
    /// Adds or updates rows by id. A row in the trash or purged keeps its
    /// state; only what it shows is refreshed.
    pub fn library_upsert(&mut self, items: &[LibraryItem]) -> StoreResult<usize> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO library (id, kind, title, location, detail, search, size, created_at, updated_at, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'live')
                 ON CONFLICT(id) DO UPDATE SET kind = ?2, title = ?3, location = ?4, detail = ?5,
                    search = ?6, size = ?7, created_at = ?8, updated_at = ?9",
            )?;
            for it in items {
                stmt.execute(params![
                    it.id,
                    it.kind().as_str(),
                    it.title,
                    it.location,
                    it.detail,
                    haystack(it),
                    it.size.min(i64::MAX as u64) as i64,
                    it.created_at,
                    it.updated_at,
                ])?;
            }
        }
        tx.commit()?;
        Ok(items.len())
    }

    /// Drops live rows of `kind` whose thing is gone (not in `seen`).
    /// Rows in the trash or purged are kept.
    pub fn library_forget_missing(
        &mut self,
        kind: LibraryKind,
        seen: &HashSet<String>,
    ) -> StoreResult<usize> {
        let ids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM library WHERE kind = ?1 AND state = 'live'")?;
            stmt.query_map([kind.as_str()], |r| r.get(0))?
                .collect::<Result<_, _>>()?
        };
        let gone: Vec<&String> = ids.iter().filter(|id| !seen.contains(*id)).collect();
        let tx = self.conn.transaction()?;
        for id in &gone {
            tx.execute("DELETE FROM library WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(gone.len())
    }

    /// One page of rows matching `q`.
    pub fn library_query(&self, q: &LibraryQuery) -> StoreResult<LibraryPage> {
        let mut clauses = vec!["state = ?".to_owned()];
        let mut args: Vec<String> = vec![q.state.as_str().to_owned()];
        if !q.kinds.is_empty() {
            clauses.push(format!("kind IN ({})", vec!["?"; q.kinds.len()].join(", ")));
            args.extend(q.kinds.iter().map(|k| k.as_str().to_owned()));
        }
        for word in q.text.split_whitespace() {
            let lower = word.to_lowercase();
            let pattern = if lower.parse::<MessageRef>().is_ok() {
                format!("% {} %", like_escape(&lower))
            } else {
                format!("%{}%", like_escape(&lower))
            };
            clauses.push("search LIKE ? ESCAPE '\\'".to_owned());
            args.push(pattern);
        }
        let filter = clauses.join(" AND ");
        let total: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM library WHERE {filter}"),
            params_from_iter(args.iter()),
            |r| r.get(0),
        )?;
        let limit = if q.limit == 0 { 100 } else { q.limit };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM library WHERE {filter} ORDER BY {} LIMIT {} OFFSET {}",
            q.sort.order(),
            limit,
            q.offset
        ))?;
        let items = stmt
            .query_map(params_from_iter(args.iter()), row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(LibraryPage {
            items,
            total: total.max(0) as usize,
            offset: q.offset,
        })
    }

    pub fn library_get(&self, id: &str) -> StoreResult<Option<LibraryItem>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM library WHERE id = ?1"),
                [id],
                row,
            )
            .optional()?)
    }

    /// Moves rows to `state` (trash, back to live, or purged). Returns the
    /// rows that changed, as they were before.
    pub fn library_set_state(
        &mut self,
        ids: &[String],
        state: ItemState,
        at: i64,
    ) -> StoreResult<Vec<LibraryItem>> {
        let mut changed = Vec::new();
        let tx = self.conn.transaction()?;
        for id in ids {
            let before = tx
                .query_row(
                    &format!("SELECT {COLUMNS} FROM library WHERE id = ?1"),
                    [id],
                    row,
                )
                .optional()?;
            let Some(before) = before else { continue };
            if before.state == state {
                continue;
            }
            let trashed = (state == ItemState::Trash).then_some(at);
            tx.execute(
                "UPDATE library SET state = ?1, trashed_at = ?2 WHERE id = ?3",
                params![state.as_str(), trashed, id],
            )?;
            changed.push(before);
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Live rows per kind, and the rows in the trash.
    pub fn library_counts(&self) -> StoreResult<(Vec<(LibraryKind, usize)>, usize)> {
        let mut stmt = self
            .conn
            .prepare("SELECT kind, COUNT(*) FROM library WHERE state = 'live' GROUP BY kind")?;
        let mut counts: Vec<(LibraryKind, usize)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .filter_map(|r| r.ok())
            .filter_map(|(k, n)| Some((LibraryKind::parse(&k)?, n.max(0) as usize)))
            .collect();
        counts.sort();
        let trash: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM library WHERE state = 'trash'",
            [],
            |r| r.get(0),
        )?;
        Ok((counts, trash.max(0) as usize))
    }

    /// Every row in the trash.
    pub fn library_trash(&self) -> StoreResult<Vec<LibraryItem>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM library WHERE state = 'trash' ORDER BY trashed_at DESC, id"
        ))?;
        let rows = stmt.query_map([], row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn item(i: usize) -> LibraryItem {
        let kind = LibraryKind::ALL[i % LibraryKind::ALL.len()];
        let mut it = LibraryItem::new(
            kind,
            format!("{}:{i}", kind.as_str()),
            format!("Item {i} about {}", ["rag", "attention", "graphs"][i % 3]),
        );
        it.location = format!("/data/{i}");
        it.detail = format!("detail {i}");
        it.size = (i * 37 % 1000) as u64;
        it.created_at = i as i64;
        it.updated_at = i as i64;
        if kind == LibraryKind::Session {
            it.search = format!("q1 q2 q{}.3", i % 50);
        }
        it
    }

    #[test]
    fn five_thousand_items_stay_quick() {
        let mut s = Store::open_in_memory().unwrap();
        let items: Vec<_> = (0..5_000).map(item).collect();
        let t = Instant::now();
        s.library_upsert(&items).unwrap();
        let insert = t.elapsed();
        assert!(insert.as_secs() < 10, "indexing took {insert:?}");

        let t = Instant::now();
        let all = s
            .library_query(&LibraryQuery {
                limit: 50,
                ..LibraryQuery::default()
            })
            .unwrap();
        assert_eq!(all.total, 5_000);
        assert_eq!(all.items.len(), 50);
        assert_eq!(all.items[0].id, items[4_999].id, "newest first");

        let notes = s
            .library_query(&LibraryQuery {
                kinds: vec![LibraryKind::Note],
                text: "attention".into(),
                sort: LibrarySort::Title,
                offset: 20,
                limit: 20,
                ..LibraryQuery::default()
            })
            .unwrap();
        assert!(notes.total > 200);
        assert!(
            notes
                .items
                .iter()
                .all(|i| i.kind() == LibraryKind::Note && i.title.contains("attention"))
        );
        let biggest = s
            .library_query(&LibraryQuery {
                sort: LibrarySort::Largest,
                limit: 5,
                ..LibraryQuery::default()
            })
            .unwrap();
        assert!(biggest.items.windows(2).all(|w| w[0].size >= w[1].size));
        let elapsed = t.elapsed();
        assert!(
            elapsed.as_millis() < 2_000,
            "three queries over 5,000 items took {elapsed:?}"
        );

        // A message id matches exactly: q7.3 but not q17.3 or q7.30.
        let q = s
            .library_query(&LibraryQuery {
                text: "Q7.3".into(),
                limit: 500,
                ..LibraryQuery::default()
            })
            .unwrap();
        assert!(q.total > 0);
        assert!(q.items.iter().all(|i| i.search.contains(" q7.3 ")));

        // Bulk delete to the trash and back.
        let ids: Vec<String> = all.items.iter().map(|i| i.id.clone()).collect();
        let t = Instant::now();
        let moved = s.library_set_state(&ids, ItemState::Trash, 99).unwrap();
        assert_eq!(moved.len(), 50);
        assert!(t.elapsed().as_millis() < 2_000);
        let (counts, trash) = s.library_counts().unwrap();
        assert_eq!(trash, 50);
        assert_eq!(counts.iter().map(|c| c.1).sum::<usize>(), 4_950);
        // A sync does not bring trashed rows back to life.
        s.library_upsert(&items[4_990..]).unwrap();
        assert_eq!(s.library_counts().unwrap().1, 50);
        let back = s.library_set_state(&ids[..10], ItemState::Live, 0).unwrap();
        assert_eq!(back.len(), 10);
        assert_eq!(s.library_trash().unwrap().len(), 40);
        let purged = s
            .library_set_state(&ids[10..], ItemState::Purged, 0)
            .unwrap();
        assert_eq!(purged.len(), 40);
        assert!(s.library_trash().unwrap().is_empty());
        assert_eq!(
            s.library_get(&ids[20]).unwrap().unwrap().state,
            ItemState::Purged
        );
    }

    #[test]
    fn missing_things_leave_the_index() {
        let mut s = Store::open_in_memory().unwrap();
        let a = LibraryItem::new(LibraryKind::Note, "note:a", "A");
        let b = LibraryItem::new(LibraryKind::Note, "note:b", "B");
        let c = LibraryItem::new(LibraryKind::Report, "report:c", "C");
        s.library_upsert(&[a, b, c]).unwrap();
        s.library_set_state(&["note:b".into()], ItemState::Trash, 1)
            .unwrap();
        let seen: HashSet<String> = HashSet::new();
        assert_eq!(
            s.library_forget_missing(LibraryKind::Note, &seen).unwrap(),
            1
        );
        assert!(s.library_get("note:a").unwrap().is_none());
        assert!(s.library_get("note:b").unwrap().is_some(), "trash is kept");
        assert!(s.library_get("report:c").unwrap().is_some());
        assert_eq!(LibraryKind::parse("Reports"), Some(LibraryKind::Report));
        assert_eq!(LibraryKind::parse("3d"), Some(LibraryKind::Scene));
        assert_eq!(like_escape("50%_a"), "50\\%\\_a");
    }
}
