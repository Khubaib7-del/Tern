//! The bridge's continuity jobs: message IDs, range actions, sub-sessions
//! and background branches, handoff packets, the side chat, highlight
//! actions, and the Library.

use std::sync::{Arc, Mutex};

use tern_agent::library::{self, LibraryItem, LibraryKind, LibraryQuery};
use tern_agent::range::{self, MoveTarget};
use tern_agent::side::SideChat;
use tern_agent::{CancelFlag, DenyAll, Preferences, Store, handoff, highlight, sub};
use tern_core::SessionId;

use super::{Bridge, Msg, RECENT};

/// Continuity results for the window.
pub enum ContMsg {
    /// The open chat's IDs, trash, and waiting move.
    Ids {
        session: SessionId,
        anchor: Option<u32>,
        /// `(turn number, label)` for live turns with a message.
        labels: Vec<(u32, String)>,
        next: u32,
        removed: Vec<(u32, String)>,
        pending: Option<(Vec<u32>, SessionId)>,
    },
    /// A range action finished: reopen the chat to show it.
    Ranged(SessionId, Result<String, String>),
    /// A sub-session was made; open it.
    SubMade(SessionId),
    /// A background branch started (the sub-session) or reported back.
    Background(SessionId),
    Reported {
        parent: SessionId,
        sub: SessionId,
        note: String,
    },
    /// The side chat is ready (or could not open).
    SideOpened(Result<usize, String>),
    SideAnswer(Result<String, String>),
    /// What a highlight action did.
    Highlighted(Result<String, String>),
    /// A handoff packet was written and a new session started from it.
    HandedOff(Result<(String, SessionId), String>),
    /// One Library page.
    Library {
        request: u64,
        items: Vec<LibraryItem>,
        total: usize,
        counts: Option<Counts>,
        note: Option<String>,
    },
}

impl std::fmt::Debug for ContMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ids { labels, .. } => write!(f, "Ids({})", labels.len()),
            Self::Ranged(_, r) => write!(f, "Ranged(ok: {})", r.is_ok()),
            Self::SubMade(s) => write!(f, "SubMade({s})"),
            Self::Background(s) => write!(f, "Background({s})"),
            Self::Reported { sub, .. } => write!(f, "Reported({sub})"),
            Self::SideOpened(r) => write!(f, "SideOpened(ok: {})", r.is_ok()),
            Self::SideAnswer(r) => write!(f, "SideAnswer(ok: {})", r.is_ok()),
            Self::Highlighted(r) => write!(f, "Highlighted(ok: {})", r.is_ok()),
            Self::HandedOff(r) => write!(f, "HandedOff(ok: {})", r.is_ok()),
            Self::Library { items, total, .. } => {
                write!(f, "Library({} of {total})", items.len())
            }
        }
    }
}

fn store() -> Result<Store, String> {
    tern_agent::open_store()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Live counts per kind, and the trash.
type Counts = (Vec<(LibraryKind, usize)>, usize);
/// A Library page: rows, total, counts, and what a change did.
type Loaded = (Vec<LibraryItem>, usize, Option<Counts>, Option<String>);

/// The open side chat, shared with the runtime.
pub type SideSlot = Arc<Mutex<Option<SideChat>>>;

impl Bridge {
    /// Loads the open chat's message IDs, trash, and waiting move.
    pub fn load_ids(&self, session: SessionId) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let Ok(store) = store() else { return };
            let Ok(Some(row)) = store.session(session) else {
                return;
            };
            let Ok(events) = store.replay(session, 0) else {
                return;
            };
            let all = tern_agent::past_turns(&events);
            let refs = tern_agent::refs::refs_of(&store, session).unwrap_or_default();
            let labels = all
                .iter()
                .filter(|t| !t.removed && t.message.is_some())
                .map(|t| {
                    let label = refs
                        .get(t.number as usize - 1)
                        .map(|r| r.to_string())
                        .unwrap_or_else(|| format!("Q{}", t.number));
                    (t.number, label)
                })
                .collect();
            let removed = all
                .iter()
                .filter(|t| t.removed)
                .map(|t| {
                    let first = t
                        .message
                        .as_deref()
                        .map(|m| tern_agent::attach::split(m).0.to_owned())
                        .unwrap_or_default();
                    (t.number, range::clip(&first, 80))
                })
                .collect();
            let pending = range::pending(&store, session)
                .ok()
                .flatten()
                .map(|p| (p.numbers, p.to));
            let _ = tx.send(Msg::Cont(Box::new(ContMsg::Ids {
                session,
                anchor: row.anchor,
                labels,
                next: all.len() as u32 + 1,
                removed,
                pending,
            })));
        });
    }

    fn ranged(
        &self,
        session: SessionId,
        job: impl FnOnce(&mut Store) -> Result<String, String> + Send + 'static,
    ) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let result = store().and_then(|mut s| job(&mut s));
            let _ = tx.send(Msg::Cont(Box::new(ContMsg::Ranged(session, result))));
            let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Sessions(recent));
        });
    }

    /// Removes turns to the chat's trash.
    pub fn range_remove(&self, session: SessionId, numbers: Vec<u32>) {
        self.ranged(session, move |s| {
            let done = range::remove(s, session, &numbers)?;
            Ok(format!(
                "Removed {} to this chat's trash.",
                range::label(&done)
            ))
        });
    }

    /// Restores removed turns (all of them with `None`).
    pub fn range_restore(&self, session: SessionId, numbers: Option<Vec<u32>>) {
        self.ranged(session, move |s| {
            let back = range::restore(s, session, numbers.as_deref())?;
            Ok(format!("Restored {}.", range::label(&back)))
        });
    }

    /// Copies turns with their context to a new session or a sub-session.
    pub fn range_move(&self, session: SessionId, numbers: Vec<u32>, to_sub: bool) {
        self.ranged(session, move |s| {
            let target = if to_sub {
                MoveTarget::SubSession {
                    anchor: None,
                    title: None,
                }
            } else {
                MoveTarget::NewSession { title: None }
            };
            let p = range::move_to(s, session, &numbers, target)?;
            Ok(format!(
                "Copied {} with its context. The originals stay until you confirm.",
                range::label(&p.numbers)
            ))
        });
    }

    pub fn range_confirm(&self, session: SessionId) {
        self.ranged(session, move |s| {
            let p = range::confirm(s, session)?;
            Ok(format!(
                "Moved {}. The originals are in this chat's trash.",
                range::label(&p.numbers)
            ))
        });
    }

    pub fn range_undo(&self, session: SessionId) {
        self.ranged(session, move |s| {
            let p = range::undo(s, session)?;
            Ok(format!("Undid the move of {}.", range::label(&p.numbers)))
        });
    }

    /// Makes an empty sub-session under message `anchor` and opens it.
    pub fn sub_new(&self, parent: SessionId, anchor: u32) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            match store().and_then(|mut s| sub::create(&mut s, parent, anchor, None, None)) {
                Ok(row) => {
                    let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
                    let _ = tx.send(Msg::Sessions(recent));
                    let _ = tx.send(Msg::Cont(Box::new(ContMsg::SubMade(row.id))));
                }
                Err(e) => {
                    let _ = tx.send(Msg::Notice(format!(
                        "Tern could not make the sub-session: {e}"
                    )));
                }
            }
        });
    }

    /// A background branch: a planned sub-session under `anchor` that runs
    /// `question` on its own and posts a summary back to the parent. It
    /// cannot ask for approvals, so tools that need one are refused.
    pub fn sub_background(&self, parent: SessionId, anchor: u32, question: String) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let made = tokio::task::spawn_blocking({
                let question = question.clone();
                move || {
                    store().and_then(|mut s| {
                        sub::create(&mut s, parent, anchor, None, Some(&question))
                    })
                }
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            let row = match made {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Msg::Notice(format!(
                        "Tern could not start the background branch: {e}"
                    )));
                    return;
                }
            };
            let _ = tx.send(Msg::Cont(Box::new(ContMsg::Background(row.id))));
            let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Sessions(recent));
            let prefs = tokio::task::spawn_blocking(Preferences::load)
                .await
                .unwrap_or_default();
            let note = match sub::run(
                prefs.chat_options(),
                row.id,
                None,
                &DenyAll,
                CancelFlag::default(),
                &|_| {},
            )
            .await
            {
                Ok(run) => run
                    .summary
                    .unwrap_or_else(|| "The branch ended without an answer.".into()),
                Err(e) => format!("The background branch failed: {e}"),
            };
            let _ = tx.send(Msg::Cont(Box::new(ContMsg::Reported {
                parent,
                sub: row.id,
                note,
            })));
            let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Sessions(recent));
        });
    }

    /// Runs a planned sub-session in the background.
    pub fn sub_run(&self, id: SessionId) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let parent = tokio::task::spawn_blocking(move || {
                store()
                    .ok()
                    .and_then(|s| s.session(id).ok().flatten())
                    .and_then(|r| r.parent)
            })
            .await
            .ok()
            .flatten();
            let _ = tx.send(Msg::Cont(Box::new(ContMsg::Background(id))));
            let prefs = tokio::task::spawn_blocking(Preferences::load)
                .await
                .unwrap_or_default();
            let note = match sub::run(
                prefs.chat_options(),
                id,
                None,
                &DenyAll,
                CancelFlag::default(),
                &|_| {},
            )
            .await
            {
                Ok(run) => run.summary.unwrap_or_default(),
                Err(e) => format!("The sub-session could not run: {e}"),
            };
            if let Some(parent) = parent {
                let _ = tx.send(Msg::Cont(Box::new(ContMsg::Reported {
                    parent,
                    sub: id,
                    note,
                })));
            }
            let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Sessions(recent));
        });
    }

    /// Writes a handoff packet for `session` to the vault's `Handoffs/`
    /// folder (and the data folder as JSON), then starts a new session
    /// from the packet alone.
    pub fn handoff(&self, session: SessionId) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let result = (|| -> Result<(String, SessionId), String> {
                let mut s = store()?;
                let packet = handoff::for_session(&s, session, now())?;
                let md = handoff::to_markdown(&packet);
                let saved = match tern_agent::vault::Vault::open_default() {
                    Ok(v) => {
                        let name = tern_agent::vault::clean_name(&packet.title);
                        let note = v.free_name(&format!("Handoffs/{name}"));
                        v.write(&note, &md)?;
                        note
                    }
                    Err(_) => String::new(),
                };
                if let Ok(paths) = tern_agent::library::Sources::discover().map(|x| x.paths) {
                    let dir = paths.data_dir.join("handoffs");
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(
                        dir.join(format!("{session}.json")),
                        handoff::to_json(&packet),
                    );
                    let _ = std::fs::write(dir.join(format!("{session}.md")), &md);
                }
                let id = handoff::start_from(&mut s, &packet)?;
                Ok((saved, id))
            })();
            let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Sessions(recent));
            let _ = tx.send(Msg::Cont(Box::new(ContMsg::HandedOff(result))));
        });
    }

    /// Opens a side chat over `session`: reads the thread once.
    pub fn side_open(&self, session: SessionId) {
        let tx = self.tx.clone();
        let slot = self.side.clone();
        self.rt.spawn(async move {
            let prefs = tokio::task::spawn_blocking(Preferences::load)
                .await
                .unwrap_or_default();
            let result = SideChat::open_default(prefs.chat_options(), session).await;
            let msg = match result {
                Ok(chat) => {
                    let n = chat.thread_len();
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(chat);
                    }
                    Ok(n)
                }
                Err(e) => Err(e),
            };
            let _ = tx.send(Msg::Cont(Box::new(ContMsg::SideOpened(msg))));
        });
    }

    /// Asks the side chat. Nothing reaches the chat's log.
    pub fn side_ask(&self, question: String, focus: Option<String>) {
        let tx = self.tx.clone();
        let slot = self.side.clone();
        self.rt.spawn(async move {
            let taken = slot.lock().ok().and_then(|mut g| g.take());
            let Some(mut chat) = taken else {
                let _ = tx.send(Msg::Cont(Box::new(ContMsg::SideAnswer(Err(
                    "The side chat is not open yet.".into(),
                )))));
                return;
            };
            let answer = chat.ask(&question, focus.as_deref()).await;
            if let Ok(mut g) = slot.lock() {
                *g = Some(chat);
            }
            let _ = tx.send(Msg::Cont(Box::new(ContMsg::SideAnswer(answer))));
        });
    }

    /// Adds highlighted text to the vault's Highlights note.
    pub fn highlight_note(&self, text: String, from: String) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = tern_agent::vault::Vault::open_default()
                .and_then(|v| highlight::add_to_notes(&v, &text, &from))
                .map(|n| format!("Added to {n} in your vault."));
            let _ = tx.send(Msg::Cont(Box::new(ContMsg::Highlighted(r))));
        });
    }

    /// Makes a flashcard from highlighted text.
    pub fn highlight_card(&self, text: String, from: String) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = tern_agent::study::StudyStore::discover()
                .and_then(|s| highlight::add_flashcard(&s, &text, &from, now()))
                .map(|id| format!("Made flashcard {id}; it is due in today's review."));
            let _ = tx.send(Msg::Cont(Box::new(ContMsg::Highlighted(r))));
        });
    }

    /// Checks highlighted text against the pages its answer read.
    pub fn highlight_verify(&self, text: String, answer: String, pages: Vec<tern_agent::PageRef>) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let r = highlight::verify(&text, &answer, &pages).await.map(|c| {
                let mut line = format!("{} {}", c.verdict.glyph(), c.verdict.label());
                if let Some(e) = &c.evidence {
                    line.push_str(&format!(" · “{}”", range::clip(&e.quote, 140)));
                }
                line
            });
            let _ = tx.send(Msg::Cont(Box::new(ContMsg::Highlighted(r))));
        });
    }

    /// One Library page; `sync` rebuilds the index first (on open and after
    /// changes), and `change` trashes, restores, or empties first.
    pub fn library(&self, request: u64, query: LibraryQuery, sync: bool, change: LibraryChange) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let result = (|| -> Result<Loaded, String> {
                let src = library::Sources::discover()?;
                src.paths.ensure().map_err(|e| e.to_string())?;
                let mut s = Store::open(&src.paths.database()).map_err(|e| e.to_string())?;
                if sync {
                    library::sync(&mut s, &src)?;
                }
                let note = match change {
                    LibraryChange::None => None,
                    LibraryChange::Trash(ids) => Some(format!(
                        "Moved {} to the trash.",
                        library::trash(&mut s, &src, &ids)?
                    )),
                    LibraryChange::Restore(ids) => Some(format!(
                        "Restored {}.",
                        library::restore(&mut s, &src, &ids)?
                    )),
                    LibraryChange::Empty => Some(format!(
                        "Deleted {} for good. Sessions stay archived.",
                        library::empty_trash(&mut s, &src)?
                    )),
                };
                let page = s.library_query(&query).map_err(|e| e.to_string())?;
                let counts = s.library_counts().ok();
                Ok((page.items, page.total, counts, note))
            })();
            let msg = match result {
                Ok((items, total, counts, note)) => ContMsg::Library {
                    request,
                    items,
                    total,
                    counts,
                    note,
                },
                Err(e) => ContMsg::Library {
                    request,
                    items: Vec::new(),
                    total: 0,
                    counts: None,
                    note: Some(format!("The Library could not load: {e}")),
                },
            };
            let _ = tx.send(Msg::Cont(Box::new(msg)));
        });
    }
}

/// What a Library request changes before it lists.
#[derive(Clone, Debug, Default)]
pub enum LibraryChange {
    #[default]
    None,
    Trash(Vec<String>),
    Restore(Vec<String>),
    Empty,
}
