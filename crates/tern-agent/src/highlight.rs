//! Highlight actions on text in answers (`docs/02-ui-definition.md`):
//! Explain, Verify claim, Add to notes, Make flashcard, and Ask in side
//! chat. Explain and Ask go to the side chat ([`crate::side`]); the others
//! are here.

use tern_research::verdict::{ClaimCheck, SourceText};
use tern_study::cards::{Card, CardKind, make_cloze};

use crate::sources::PageRef;
use crate::study::StudyStore;
use crate::vault::Vault;

/// What the popover offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Explain,
    Verify,
    AddToNotes,
    Flashcard,
    AskSide,
}

impl Action {
    pub const ALL: [Self; 5] = [
        Self::Explain,
        Self::Verify,
        Self::AddToNotes,
        Self::Flashcard,
        Self::AskSide,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Explain => "Explain",
            Self::Verify => "Verify claim",
            Self::AddToNotes => "Add to notes",
            Self::Flashcard => "Make flashcard",
            Self::AskSide => "Ask in side chat",
        }
    }

    /// The side chat question Explain asks.
    pub fn explain_prompt(text: &str) -> String {
        format!(
            "Explain this part simply, with an example: \"{}\"",
            text.trim()
        )
    }
}

/// The vault note highlights are added to.
pub const NOTES: &str = "Highlights";

/// Appends a highlight to the vault's `Highlights.md`, with where it came
/// from. Returns the note's name.
pub fn add_to_notes(vault: &Vault, text: &str, from: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Select some text first.".into());
    }
    vault.ensure()?;
    let mut note = vault.read(NOTES).unwrap_or_else(|_| format!("# {NOTES}\n"));
    if !note.ends_with('\n') {
        note.push('\n');
    }
    let quoted: Vec<String> = text.lines().map(|l| format!("> {l}")).collect();
    note.push_str(&format!("\n{}\n\n— {from}\n", quoted.join("\n")));
    vault.write(NOTES, &note)?;
    Ok(NOTES.to_owned())
}

const STOP: [&str; 24] = [
    "because",
    "between",
    "however",
    "therefore",
    "through",
    "without",
    "another",
    "whether",
    "although",
    "something",
    "anything",
    "different",
    "important",
    "example",
    "usually",
    "should",
    "would",
    "could",
    "there",
    "their",
    "which",
    "where",
    "about",
    "these",
];

/// A flashcard from highlighted text: a cloze on its key term (the longest
/// word that is not a common one), or a basic card when there is none.
pub fn flashcard(text: &str, from: &str, now: i64) -> Result<Card, String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err("Select some text first.".into());
    }
    let term = text
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|w| w.chars().count() >= 5 && !STOP.contains(&w.to_lowercase().as_str()))
        .max_by_key(|w| {
            (
                w.chars().count(),
                w.chars().next().is_some_and(char::is_uppercase),
            )
        });
    let mut card = match term.and_then(|t| make_cloze(&text, t)) {
        Some(cloze) => Card::new(CardKind::Cloze, &cloze, "", "you", now),
        None => Card::new(
            CardKind::Basic,
            &format!("What does this say? ({})", crate::range::clip(&text, 60)),
            &text,
            "you",
            now,
        ),
    };
    card.tags.push("highlight".into());
    if !from.is_empty() {
        card.tags
            .push(crate::vault::clean_name(from).replace(' ', "-"));
    }
    Ok(card)
}

/// Adds a highlight's flashcard to the user's deck. Returns its id.
pub fn add_flashcard(
    store: &StudyStore,
    text: &str,
    from: &str,
    now: i64,
) -> Result<String, String> {
    let card = flashcard(text, from, now)?;
    let mut deck = crate::cards::load_deck(store)?;
    let added = deck.add(vec![card]);
    let id = added
        .first()
        .map(|c| c.id.clone())
        .ok_or("That card is already in your deck.")?;
    crate::cards::save_deck(store, &deck)?;
    Ok(id)
}

/// Checks highlighted text against the pages its answer read, by word
/// overlap (no model call). Every page counts as a possible source.
pub fn verify_with(text: &str, sources: &[SourceText]) -> Option<ClaimCheck> {
    let marks: String = sources.iter().map(|s| format!(" [{}]", s.n)).collect();
    let claim = format!("{}{marks}", text.trim().trim_end_matches('.'));
    let claims = tern_research::claims::extract(&claim);
    let c = claims.first()?;
    Some(tern_research::verdict::check_lexical(c, sources))
}

/// [`verify_with`] after reading the answer's pages through Tern Crawl's
/// cache.
pub async fn verify(text: &str, answer: &str, pages: &[PageRef]) -> Result<ClaimCheck, String> {
    if text.trim().is_empty() {
        return Err("Select some text first.".into());
    }
    let mut numbered = crate::verify::numbered(answer, pages);
    if numbered.is_empty() {
        numbered = pages
            .iter()
            .enumerate()
            .map(|(i, p)| (i + 1, p.clone()))
            .collect();
    }
    if numbered.is_empty() {
        return Err("This answer read no pages, so there is nothing to check against.".into());
    }
    let sources = crate::verify::source_texts(&numbered, &crate::web::shared_reader()).await;
    verify_with(text, &sources).ok_or_else(|| "There is no claim in that text to check.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_research::verdict::Verdict;

    #[test]
    fn notes_cards_and_checks_from_a_highlight() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::at(dir.path().join("vault"));
        add_to_notes(&vault, "HNSW builds a layered graph.", "RAG · Q2").unwrap();
        add_to_notes(&vault, "Second.", "RAG · Q3").unwrap();
        let note = vault.read(NOTES).unwrap();
        assert!(note.starts_with("# Highlights"));
        assert!(note.contains("> HNSW builds a layered graph.\n\n— RAG · Q2"));
        assert!(note.contains("> Second."));
        assert!(add_to_notes(&vault, " ", "x").is_err());

        let card = flashcard("Retrieval-augmented generation grounds answers.", "RAG", 5).unwrap();
        assert_eq!(card.kind, CardKind::Cloze);
        assert!(card.front.contains("{{c1::Retrieval-augmented}}"));
        assert!(card.tags.contains(&"highlight".to_owned()));
        let basic = flashcard("It is so.", "", 5).unwrap();
        assert_eq!(basic.kind, CardKind::Basic);

        let store = StudyStore::at(dir.path().join("study"));
        let id = add_flashcard(&store, "Transformers use self-attention layers.", "Q1", 9).unwrap();
        assert!(crate::cards::load_deck(&store).unwrap().get(&id).is_some());

        let sources = vec![SourceText {
            n: 1,
            url: "https://a.org".into(),
            title: "HNSW".into(),
            site: "a.org".into(),
            text: "HNSW builds a layered proximity graph for nearest neighbour search.".into(),
        }];
        let check = verify_with("HNSW builds a layered proximity graph", &sources).unwrap();
        assert_eq!(check.verdict, Verdict::Supported);
        let miss = verify_with("Bananas grow on the moon in winter", &sources).unwrap();
        assert_ne!(miss.verdict, Verdict::Supported);
    }
}
