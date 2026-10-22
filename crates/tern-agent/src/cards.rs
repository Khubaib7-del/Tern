//! Flashcards for front ends: the deck on disk, writing cards with the
//! model (or without it), reviews that nudge topic mastery, and export to
//! Anki (`.apkg`), CSV, and TSV, and import back.
//!
//! ```text
//! <data>/study/cards.json   the deck: cards, schedules, review log
//! ```
//!
//! The pure work (card types, SM-2, parsing, fallbacks, CSV and TSV) is in
//! `tern-study`; the `.apkg` writer is in `tern-store`.

use std::path::{Path, PathBuf};

use tern_providers::ChatMessage;

pub use tern_study::cards::{
    Card, CardKind, CardSpec, Deck, DeckStats, ReviewLog, cloze_plain, cloze_question, end_of_day,
};
pub use tern_study::cards_io::Delim;
pub use tern_study::srs::{CardState, Rating, Sched, wait_label};

use crate::study::{
    StudyEngine, StudyLibrary, StudyStore, Topic, ensure_topics, now, read_json, select_sources,
    write_atomic,
};

/// Characters of passages cards are written from.
pub const CARD_CHARS: usize = 14_000;

fn deck_path(store: &StudyStore) -> PathBuf {
    store.root().join("cards.json")
}

/// The deck; a missing file is an empty deck.
pub fn load_deck(store: &StudyStore) -> Result<Deck, String> {
    let d: Deck = read_json(&deck_path(store))?.unwrap_or_default();
    if d.version > tern_study::cards::DECK_VERSION {
        return Err("The flashcard deck was saved by a newer Tern.".into());
    }
    Ok(d)
}

pub fn save_deck(store: &StudyStore, deck: &Deck) -> Result<(), String> {
    write_atomic(
        &deck_path(store),
        &serde_json::to_string_pretty(deck).unwrap_or_default(),
    )
}

/// The local clock's offset from UTC in seconds, so "today" ends at
/// local midnight. Falls back to UTC where the platform cannot say.
pub fn local_offset() -> i64 {
    time::UtcOffset::current_local_offset()
        .map(|o| i64::from(o.whole_seconds()))
        .unwrap_or(0)
}

/// What came of writing cards.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    /// The cards added to the deck, with ids.
    pub added: Vec<Card>,
    /// Cards skipped because the deck already had them.
    pub repeats: usize,
    /// `model` or `fallback`.
    pub written_by: String,
    /// Why the model was not used, when it was tried.
    pub note: Option<String>,
}

/// Writes cards on a topic (or all materials) and adds the new ones to
/// the deck.
pub async fn generate(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    engine: Option<&StudyEngine>,
    spec: &CardSpec,
) -> Result<Generated, String> {
    use tern_study::cards::{cards_prompt, fallback_cards, parse_cards};
    ensure_topics(store, lib)?;
    let topic: Option<Topic> =
        match &spec.topic {
            Some(t) => Some(lib.topic(t).cloned().ok_or_else(|| {
                format!("No topic matches \"{t}\". `tern study topics` lists them.")
            })?),
            None => None,
        };
    let title = topic
        .as_ref()
        .map(|t| t.title.clone())
        .unwrap_or_else(|| "All materials".into());
    let chunks = store.chunks(lib);
    if chunks.is_empty() {
        return Err("Add materials first: `tern study add <files>`.".into());
    }
    let sources = select_sources(&chunks, topic.as_ref(), &title, CARD_CHARS);
    let tid = topic.as_ref().map(|t| t.id.as_str());
    let at = now();
    let mut note = None;
    let mut cards = Vec::new();
    let mut by = "fallback";
    if let Some(e) = engine {
        let (system, user) = cards_prompt(spec, &title, &sources);
        let max = (spec.count() as u32 * 140).clamp(800, 6000);
        match e
            .ask(system, vec![ChatMessage::user(user)], max, 0.4, None)
            .await
        {
            Ok(text) => {
                cards = parse_cards(&text, &sources, tid, at);
                if cards.is_empty() {
                    note = Some("The model's cards could not be read; Tern wrote them from your materials instead.".into());
                } else {
                    by = "model";
                }
            }
            Err(err) => {
                note = Some(format!(
                    "The model could not be reached ({err}); Tern wrote cards from your materials instead."
                ))
            }
        }
    }
    if cards.is_empty() {
        cards = fallback_cards(spec, &sources, tid, at);
    }
    if cards.is_empty() {
        return Err("Your materials are too short to write cards from yet.".into());
    }
    cards.truncate(spec.count());
    // Whole-material cards go to the topic their passage is in.
    for c in cards.iter_mut().filter(|c| c.topic.is_none()) {
        if let Some(chunk) = &c.chunk
            && let Some(t) = lib.topics.iter().find(|t| t.chunks.contains(chunk))
        {
            c.topic = Some(t.id.clone());
            c.tags = vec![t.id.clone()];
        }
    }
    let mut deck = load_deck(store)?;
    let total = cards.len();
    let added = deck.add(cards);
    save_deck(store, &deck)?;
    Ok(Generated {
        repeats: total - added.len(),
        added,
        written_by: by.into(),
        note,
    })
}

/// Records an answer on a card, nudges its topic's mastery, and saves
/// both. Returns the log line.
pub fn review(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    deck: &mut Deck,
    card: &str,
    rating: Rating,
    at: i64,
) -> Result<ReviewLog, String> {
    let (line, topic) = deck.review(card, rating, at)?;
    save_deck(store, deck)?;
    if let Some(t) = topic.filter(|t| lib.topics.iter().any(|x| &x.id == t)) {
        lib.mastery.entry(t).or_default().nudge(rating.credit(), at);
        store.save(lib)?;
    }
    Ok(line)
}

/// When each button would bring the card back: `[1m, 6m, 10m, 4d]`.
pub fn button_hints(card: &Card, at: i64) -> [String; 4] {
    Rating::ALL.map(|r| wait_label(card.sched.next(r, at).due - at))
}

// ---- Export and import -------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Anki's package.
    Apkg,
    /// Tern's plain CSV.
    Csv,
    /// Anki's text import.
    Tsv,
}

impl Format {
    /// The format a file name suggests.
    pub fn for_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        Some(match ext.as_str() {
            "apkg" => Self::Apkg,
            "csv" => Self::Csv,
            "tsv" | "txt" => Self::Tsv,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Apkg => "Anki package",
            Self::Csv => "CSV",
            Self::Tsv => "TSV for Anki",
        }
    }
}

/// The deck's cards as Anki notes.
pub fn to_anki(deck: &Deck, cards: &[&Card], at: i64) -> tern_store::anki::Package {
    use tern_store::anki::{Note, NoteKind, Package, ReviewState};
    use tern_study::cards_io::{anki_tags, html};
    let notes =
        cards
            .iter()
            .map(|c| {
                let kind = match c.kind {
                    CardKind::Cloze => NoteKind::Cloze,
                    _ => NoteKind::Basic,
                };
                let review = matches!(c.sched.state, CardState::Review | CardState::Relearning)
                    .then(|| ReviewState {
                        interval: c.sched.interval.round().max(1.0) as i64,
                        factor: (c.sched.ease * 1000.0).round() as i64,
                        reps: i64::from(c.sched.reps),
                        lapses: i64::from(c.sched.lapses),
                        due: c.sched.due,
                    });
                Note {
                    kind,
                    fields: [
                        html(&c.front),
                        html(&c.back),
                        c.anchor
                            .as_ref()
                            .map(|a| html(&a.label()))
                            .unwrap_or_default(),
                    ],
                    tags: anki_tags(c),
                    guid_seed: format!("tern:{}:{}", c.id, c.created_at),
                    clozes: tern_study::cards::cloze_numbers(&c.front),
                    review,
                    suspended: c.suspended,
                }
            })
            .collect();
    Package {
        deck: deck.name.clone(),
        notes,
        created: at,
    }
}

/// Writes the deck (or one topic's cards) to `path` in `format`. Returns a
/// line saying what was written.
pub fn export(
    deck: &Deck,
    topic: Option<&str>,
    path: &Path,
    format: Format,
) -> Result<String, String> {
    let cards: Vec<&Card> = deck
        .cards
        .iter()
        .filter(|c| topic.is_none_or(|t| c.topic.as_deref() == Some(t)))
        .collect();
    if cards.is_empty() {
        return Err("No cards to export yet: `tern cards gen` writes some.".into());
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    match format {
        Format::Apkg => {
            let s = tern_store::anki::write_apkg(&to_anki(deck, &cards, now()), path)?;
            Ok(format!(
                "{} notes, {} cards in deck \"{}\" → {}",
                s.notes,
                s.cards,
                deck.name,
                path.display()
            ))
        }
        Format::Csv | Format::Tsv => {
            let owned: Vec<Card> = cards.into_iter().cloned().collect();
            let d = if format == Format::Csv {
                Delim::Comma
            } else {
                Delim::Tab
            };
            let text = tern_study::cards_io::export_delimited(&owned, d, &deck.name);
            std::fs::write(path, text)
                .map_err(|e| format!("Tern could not write {}: {e}", path.display()))?;
            Ok(format!(
                "{} cards as {} → {}",
                owned.iter().filter(|c| !c.suspended).count(),
                format.label(),
                path.display()
            ))
        }
    }
}

/// Reads cards from a CSV or TSV file into the deck. Returns (added,
/// repeats skipped).
pub fn import(store: &StudyStore, deck: &mut Deck, path: &Path) -> Result<(usize, usize), String> {
    if Format::for_path(path) == Some(Format::Apkg) {
        return Err(
            "Tern imports CSV and TSV; in Anki, export the deck as \"Notes in Plain Text\" first."
                .into(),
        );
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Tern could not read {}: {e}", path.display()))?;
    let cards = tern_study::cards_io::import_delimited(&text, now())?;
    let total = cards.len();
    let added = deck.add(cards).len();
    save_deck(store, deck)?;
    Ok((added, total - added))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tern_providers::{ChatRequest, ChatResponse, ProviderResult};

    struct Script(Mutex<Vec<String>>);

    impl crate::engine::Model for Script {
        fn provider_id(&self) -> &str {
            "test"
        }
        fn complete<'a>(
            &'a self,
            _req: &'a ChatRequest,
        ) -> crate::tools::BoxFuture<'a, ProviderResult<ChatResponse>> {
            Box::pin(async move {
                let v = self.0.lock().unwrap();
                Ok(ChatResponse {
                    text: v[0].clone(),
                    usage: tern_providers::Usage::measured(10, 5),
                    finish_reason: None,
                    model: None,
                })
            })
        }
    }

    const NOTES: &str = "# Graphs\nA graph is a set of vertices joined by edges. Graphs model road networks and social networks.\n\n# Shortest paths\nDijkstra's algorithm fails on negative weights because a settled vertex is never revisited. It keeps a priority queue of tentative distances.\n\n# Spanning trees\nA minimum spanning tree is the cheapest set of edges that connects every vertex. Kruskal's algorithm adds edges in order of weight while avoiding cycles.";

    fn setup() -> (tempfile::TempDir, StudyStore, StudyLibrary) {
        let dir = tempfile::tempdir().unwrap();
        let store = StudyStore::at(dir.path().join("study"));
        let file = dir.path().join("algo.md");
        std::fs::write(&file, NOTES).unwrap();
        let mut lib = store.load().unwrap();
        crate::study::add_path(&store, &mut lib, &file).unwrap();
        store.save(&lib).unwrap();
        (dir, store, lib)
    }

    #[tokio::test]
    async fn offline_cards_land_in_the_deck_once() {
        let (_d, store, mut lib) = setup();
        let g = generate(&store, &mut lib, None, &CardSpec::default())
            .await
            .unwrap();
        assert_eq!(g.written_by, "fallback");
        assert!(g.added.len() >= 5, "{:#?}", g.added);
        assert!(
            g.added
                .iter()
                .all(|c| c.anchor.is_some() && c.topic.is_some())
        );
        let again = generate(&store, &mut lib, None, &CardSpec::default())
            .await
            .unwrap();
        assert!(again.added.is_empty(), "every card a repeat");
        assert_eq!(again.repeats, g.added.len());
        assert_eq!(load_deck(&store).unwrap().cards.len(), g.added.len());
        let spec = CardSpec {
            topic: Some("spanning".into()),
            ..CardSpec::default()
        };
        let topic = generate(&store, &mut lib, None, &spec).await;
        assert!(topic.is_ok());
        assert!(
            generate(
                &store,
                &mut lib,
                None,
                &CardSpec {
                    topic: Some("quantum".into()),
                    ..CardSpec::default()
                }
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn model_cards_are_used_and_bad_replies_fall_back() {
        let (_d, store, mut lib) = setup();
        let good = StudyEngine::with_model(
            Arc::new(Script(Mutex::new(vec![
                r#"{"cards": [{"type": "basic", "front": "What does Kruskal avoid?", "back": "Cycles", "source": "S3"}]}"#.into(),
            ]))),
            "m",
            "Test",
        );
        let g = generate(&store, &mut lib, Some(&good), &CardSpec::default())
            .await
            .unwrap();
        assert_eq!(g.written_by, "model");
        assert_eq!(g.added.len(), 1);
        assert_eq!(g.added[0].topic.as_deref(), Some("spanning-trees"));
        let bad = StudyEngine::with_model(
            Arc::new(Script(Mutex::new(vec!["no cards today".into()]))),
            "m",
            "Test",
        );
        let g = generate(&store, &mut lib, Some(&bad), &CardSpec::default())
            .await
            .unwrap();
        assert_eq!(g.written_by, "fallback");
        assert!(g.note.unwrap().contains("could not be read"));
    }

    #[tokio::test]
    async fn reviews_nudge_mastery_and_exports_round_trip() {
        let (dir, store, mut lib) = setup();
        generate(&store, &mut lib, None, &CardSpec::default())
            .await
            .unwrap();
        let mut deck = load_deck(&store).unwrap();
        let card = deck.cards[0].clone();
        let hints = button_hints(&card, 0);
        assert_eq!(hints, ["1m", "5m", "10m", "4d"].map(String::from));
        review(&store, &mut lib, &mut deck, &card.id, Rating::Easy, 100).unwrap();
        let t = card.topic.clone().unwrap();
        let m = store.load().unwrap().mastery_of(&t);
        assert_eq!(m.card_reviews, 1);
        assert!(m.rating > 0.0);
        assert_eq!(
            load_deck(&store).unwrap().cards[0].sched.state,
            CardState::Review
        );

        let apkg = dir.path().join("out/deck.apkg");
        let line = export(&deck, None, &apkg, Format::Apkg).unwrap();
        assert!(line.contains("deck \"Tern\""), "{line}");
        let s = tern_store::anki::inspect_apkg(&apkg).unwrap();
        assert_eq!(s.notes, deck.cards.len());
        assert!(s.cards >= s.notes);

        let csv = dir.path().join("deck.csv");
        export(&deck, None, &csv, Format::Csv).unwrap();
        let tsv = dir.path().join("deck.tsv");
        export(&deck, Some(&t), &tsv, Format::Tsv).unwrap();
        // Importing our own export adds nothing new.
        let (added, skipped) = import(&store, &mut deck, &csv).unwrap();
        assert_eq!((added, skipped), (0, deck.cards.len()));
        // A fresh deck takes them all.
        let other = StudyStore::at(dir.path().join("other"));
        let mut fresh = Deck::default();
        let (added, _) = import(&other, &mut fresh, &tsv).unwrap();
        assert!(added > 0);
        assert!(import(&store, &mut deck, &apkg).is_err());
        assert_eq!(Format::for_path(Path::new("x.TSV")), Some(Format::Tsv));
        assert!(export(&Deck::default(), None, &csv, Format::Csv).is_err());
    }
}
