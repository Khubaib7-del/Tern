//! Flashcards: basic, cloze, and "explain why" cards, each with the
//! passage it came from, in one deck with an SM-2 schedule ([`crate::srs`]).
//!
//! The model writes cards from numbered passages ([`cards_prompt`]);
//! [`parse_cards`] reads its reply leniently and drops any card it cannot
//! use. With no model, or nothing usable back, [`fallback_cards`] writes
//! them straight from the passages: definitions become basic cards, key
//! sentences become cloze cards, and sentences that give a reason (or a
//! section heading) become "explain why" cards. Deterministic for the
//! same input.
//!
//! The deck is plain data: `tern-agent` saves it as
//! `<data>/study/cards.json`; [`crate::cards_io`] reads and writes CSV
//! and TSV, and `tern-store` writes Anki's `.apkg`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::material::{Anchor, Source, sources_block};
use crate::srs::{CardState, DAY, Rating, Sched};
use crate::text::{clip, field, json_in, normalise, one_line, overlap, sentences, words};
use crate::topics::definition;

pub const DECK_VERSION: u32 = 1;
/// Most cards written in one go.
pub const MAX_GEN: usize = 40;
/// New cards introduced per day, Anki's default.
pub const NEW_PER_DAY: usize = 20;
/// When nothing else is due, learning cards due this soon are shown.
pub const LEARN_AHEAD: i64 = 20 * 60;
/// Reviews kept in the log.
pub const MAX_LOG: usize = 20_000;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// A question on the front, the answer on the back.
    #[default]
    Basic,
    /// A sentence with `{{c1::term}}` hidden; the back is extra context.
    Cloze,
    /// "Explain why …" on the front, the reason on the back.
    Why,
}

impl CardKind {
    pub const ALL: [CardKind; 3] = [CardKind::Basic, CardKind::Cloze, CardKind::Why];

    pub fn label(self) -> &'static str {
        match self {
            Self::Basic => "Basic",
            Self::Cloze => "Cloze",
            Self::Why => "Explain why",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Cloze => "cloze",
            Self::Why => "why",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase().replace([' ', '-', '/'], "_");
        Some(match s.as_str() {
            "basic" | "qa" | "q_a" | "question" | "front_back" | "definition" => Self::Basic,
            "cloze" | "blank" | "fill_blank" | "fill_in_the_blank" | "fill" => Self::Cloze,
            "why" | "explain_why" | "explain" | "how" | "reason" | "explain_how" => Self::Why,
            _ => return None,
        })
    }
}

/// One flashcard.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Card {
    /// `k1`, `k2`, … (empty until the deck takes the card).
    pub id: String,
    pub kind: CardKind,
    /// The question, or for cloze cards the text with `{{c1::…}}`.
    pub front: String,
    /// The answer, or for cloze cards optional extra context.
    #[serde(default)]
    pub back: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    /// The chunk it was written from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub created_at: i64,
    /// `model`, `fallback`, `you` (edited or written by hand), or `import`.
    pub written_by: String,
    #[serde(default)]
    pub suspended: bool,
    #[serde(default)]
    pub sched: Sched,
}

impl Card {
    pub fn new(kind: CardKind, front: &str, back: &str, written_by: &str, now: i64) -> Self {
        Self {
            id: String::new(),
            kind,
            front: front.trim().to_owned(),
            back: back.trim().to_owned(),
            topic: None,
            anchor: None,
            chunk: None,
            tags: Vec::new(),
            created_at: now,
            written_by: written_by.to_owned(),
            suspended: false,
            sched: Sched::default(),
        }
    }

    /// What the review shows first: the question, or the cloze with its
    /// gaps as `[…]` (or `[hint]`).
    pub fn question(&self) -> String {
        match self.kind {
            CardKind::Cloze => cloze_question(&self.front),
            _ => self.front.clone(),
        }
    }

    /// What the review shows after the flip.
    pub fn answer(&self) -> String {
        match self.kind {
            CardKind::Cloze => {
                let mut s = cloze_plain(&self.front);
                if !self.back.trim().is_empty() {
                    s.push_str("\n\n");
                    s.push_str(self.back.trim());
                }
                s
            }
            _ => self.back.clone(),
        }
    }

    /// The front as plain text, for comparing and sorting.
    pub fn plain_front(&self) -> String {
        match self.kind {
            CardKind::Cloze => cloze_plain(&self.front),
            _ => self.front.clone(),
        }
    }

    /// Whether a later card says the same thing.
    pub fn same_as(&self, other: &Card) -> bool {
        let (a, b) = (self.plain_front(), other.plain_front());
        let (na, nb) = (normalise(&a), normalise(&b));
        if na == nb {
            return true;
        }
        self.kind == other.kind && overlap(&a, &b) >= 0.9 && overlap(&b, &a) >= 0.9
    }
}

// ---- Cloze -----------------------------------------------------------

/// The `{{cN::answer::hint}}` parts of a cloze text: (n, answer, hint).
pub fn cloze_parts(text: &str) -> Vec<(u32, String, Option<String>)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("{{c") {
        let after = &rest[at + 3..];
        let Some(end) = after.find("}}") else {
            break;
        };
        let inner = &after[..end];
        if let Some((n, body)) = inner.split_once("::")
            && let Ok(n) = n.trim().parse::<u32>()
        {
            let (ans, hint) = match body.split_once("::") {
                Some((a, h)) => (a.to_owned(), Some(h.to_owned()).filter(|h| !h.is_empty())),
                None => (body.to_owned(), None),
            };
            out.push((n, ans, hint));
        }
        rest = &after[end + 2..];
    }
    out
}

/// The cloze numbers used, in order (`[1, 2]`).
pub fn cloze_numbers(text: &str) -> Vec<u32> {
    let mut v: Vec<u32> = cloze_parts(text).into_iter().map(|p| p.0).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn cloze_map(text: &str, f: impl Fn(&str, Option<&str>) -> String) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("{{c") {
        let after = &rest[at + 3..];
        let Some(end) = after.find("}}") else {
            break;
        };
        let inner = &after[..end];
        match inner.split_once("::") {
            Some((n, body)) if n.trim().parse::<u32>().is_ok() => {
                out.push_str(&rest[..at]);
                let (ans, hint) = match body.split_once("::") {
                    Some((a, h)) => (a, Some(h).filter(|h| !h.is_empty())),
                    None => (body, None),
                };
                out.push_str(&f(ans, hint));
            }
            _ => out.push_str(&rest[..at + 3 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// The cloze text with every gap shown as `[…]` or `[hint]`.
pub fn cloze_question(text: &str) -> String {
    cloze_map(text, |_, hint| match hint {
        Some(h) => format!("[{h}]"),
        None => "[…]".into(),
    })
}

/// The cloze text with every gap filled in.
pub fn cloze_plain(text: &str) -> String {
    cloze_map(text, |ans, _| ans.to_owned())
}

/// `sentence` with the first `term` (any case) made gap 1, or `None` when
/// the term is not in it.
pub fn make_cloze(sentence: &str, term: &str) -> Option<String> {
    let r = crate::text::find_ci(sentence, term)?;
    let mut s = sentence.to_owned();
    let hit = s[r.clone()].to_owned();
    s.replace_range(r, &format!("{{{{c1::{hit}}}}}"));
    Some(s)
}

// ---- The deck --------------------------------------------------------

/// One answer in the review log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewLog {
    pub card: String,
    pub at: i64,
    pub rating: Rating,
    pub state_before: CardState,
    pub interval_before: f32,
    pub interval_after: f32,
    pub ease_after: f32,
}

/// Counts for the deck screen and `tern cards`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeckStats {
    pub total: usize,
    pub new: usize,
    pub learning: usize,
    /// In review with an interval under 21 days.
    pub young: usize,
    /// In review with an interval of 21 days or more.
    pub mature: usize,
    pub suspended: usize,
    /// Due now (learning) or by the end of today (review), plus the new
    /// cards today still allows.
    pub due_today: usize,
    /// New cards still allowed today.
    pub new_left: usize,
    pub reviewed_today: usize,
    /// Share of review answers in the last 30 days that were not Again.
    pub retention: Option<f32>,
}

impl DeckStats {
    /// `12 due · 40 cards (8 new, 3 learning, 25 young, 4 mature) · 91% kept`.
    pub fn line(&self) -> String {
        let mut s = format!(
            "{} due · {} card{} ({} new, {} learning, {} young, {} mature",
            self.due_today,
            self.total,
            if self.total == 1 { "" } else { "s" },
            self.new,
            self.learning,
            self.young,
            self.mature
        );
        if self.suspended > 0 {
            s.push_str(&format!(", {} suspended", self.suspended));
        }
        s.push(')');
        if let Some(r) = self.retention {
            s.push_str(&format!(" · {:.0}% recalled", r * 100.0));
        }
        s
    }
}

/// The day number of `t` for a clock `offset` seconds from UTC.
pub fn day_index(t: i64, offset: i64) -> i64 {
    (t + offset).div_euclid(DAY)
}

/// When today ends on that clock, Unix seconds.
pub fn end_of_day(now: i64, offset: i64) -> i64 {
    (day_index(now, offset) + 1) * DAY - offset
}

/// Every card, the review log, and the deck's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Deck {
    pub version: u32,
    /// The deck name exported to Anki.
    pub name: String,
    pub cards: Vec<Card>,
    #[serde(default)]
    pub log: Vec<ReviewLog>,
    #[serde(default)]
    pub next_card: u32,
    #[serde(default = "default_new_per_day")]
    pub new_per_day: usize,
}

fn default_new_per_day() -> usize {
    NEW_PER_DAY
}

impl Default for Deck {
    fn default() -> Self {
        Self {
            version: DECK_VERSION,
            name: "Tern".into(),
            cards: Vec::new(),
            log: Vec::new(),
            next_card: 0,
            new_per_day: NEW_PER_DAY,
        }
    }
}

impl Deck {
    /// Adds cards, skipping any that repeat a card already in the deck (or
    /// earlier in `cards`). Returns the ones added, with their ids.
    pub fn add(&mut self, cards: Vec<Card>) -> Vec<Card> {
        let mut added = Vec::new();
        for mut c in cards {
            if c.front.trim().is_empty() || self.cards.iter().any(|x| x.same_as(&c)) {
                continue;
            }
            self.next_card = self.next_card.max(self.cards.len() as u32) + 1;
            c.id = format!("k{}", self.next_card);
            self.cards.push(c.clone());
            added.push(c);
        }
        added
    }

    /// How many of `cards` the deck would skip as repeats.
    pub fn repeats(&self, cards: &[Card]) -> usize {
        cards
            .iter()
            .filter(|c| self.cards.iter().any(|x| x.same_as(c)))
            .count()
    }

    /// A card by id, or by its number in the deck (`3`).
    pub fn index(&self, which: &str) -> Result<usize, String> {
        let w = which.trim();
        if let Some(i) = self.cards.iter().position(|c| c.id == w) {
            return Ok(i);
        }
        if let Ok(n) = w.parse::<usize>()
            && (1..=self.cards.len()).contains(&n)
        {
            return Ok(n - 1);
        }
        Err(format!(
            "No card {w}. `tern cards list` shows the deck with numbers."
        ))
    }

    pub fn get(&self, id: &str) -> Option<&Card> {
        self.cards.iter().find(|c| c.id == id)
    }

    /// Rewrites a card's sides. The schedule is kept; the card now counts
    /// as written by you.
    pub fn edit(&mut self, which: &str, front: &str, back: &str) -> Result<&Card, String> {
        let i = self.index(which)?;
        let front = front.trim();
        if front.is_empty() {
            return Err("A card needs a front.".into());
        }
        let c = &mut self.cards[i];
        if c.kind == CardKind::Cloze && cloze_numbers(front).is_empty() {
            return Err("A cloze card needs a gap: wrap the hidden words in {{c1::…}}.".into());
        }
        c.front = front.to_owned();
        c.back = back.trim().to_owned();
        c.written_by = "you".into();
        Ok(&self.cards[i])
    }

    pub fn suspend(&mut self, which: &str, on: bool) -> Result<&Card, String> {
        let i = self.index(which)?;
        self.cards[i].suspended = on;
        Ok(&self.cards[i])
    }

    pub fn delete(&mut self, which: &str) -> Result<Card, String> {
        let i = self.index(which)?;
        let c = self.cards.remove(i);
        self.log.retain(|l| l.card != c.id);
        Ok(c)
    }

    /// Records an answer. Returns the log line and the card's topic.
    pub fn review(
        &mut self,
        id: &str,
        rating: Rating,
        now: i64,
    ) -> Result<(ReviewLog, Option<String>), String> {
        let i = self.index(id)?;
        let c = &mut self.cards[i];
        let before = c.sched.clone();
        c.sched = before.next(rating, now);
        let line = ReviewLog {
            card: c.id.clone(),
            at: now,
            rating,
            state_before: before.state,
            interval_before: before.interval,
            interval_after: c.sched.interval,
            ease_after: c.sched.ease,
        };
        let topic = c.topic.clone();
        self.log.push(line.clone());
        if self.log.len() > MAX_LOG {
            let cut = self.log.len() - MAX_LOG;
            self.log.drain(..cut);
        }
        Ok((line, topic))
    }

    fn new_seen_today(&self, now: i64, offset: i64) -> usize {
        let today = day_index(now, offset);
        self.log
            .iter()
            .filter(|l| l.state_before == CardState::New && day_index(l.at, offset) == today)
            .count()
    }

    /// Today's queue, as card ids: learning cards due now (soonest
    /// first), then review cards due by the end of today, then new cards
    /// up to the daily limit (oldest first). Suspended cards never show;
    /// `topic` narrows it to one topic.
    pub fn queue(&self, now: i64, offset: i64, topic: Option<&str>) -> Vec<String> {
        let live = |c: &&Card| !c.suspended && topic.is_none_or(|t| c.topic.as_deref() == Some(t));
        let mut learning: Vec<&Card> = self
            .cards
            .iter()
            .filter(live)
            .filter(|c| {
                matches!(c.sched.state, CardState::Learning | CardState::Relearning)
                    && c.sched.due <= now
            })
            .collect();
        learning.sort_by_key(|c| c.sched.due);
        let end = end_of_day(now, offset);
        let mut review: Vec<&Card> = self
            .cards
            .iter()
            .filter(live)
            .filter(|c| c.sched.state == CardState::Review && c.sched.due < end)
            .collect();
        review.sort_by_key(|c| c.sched.due);
        let left = self
            .new_per_day
            .saturating_sub(self.new_seen_today(now, offset));
        let new = self
            .cards
            .iter()
            .filter(live)
            .filter(|c| c.sched.state == CardState::New)
            .take(left);
        learning
            .into_iter()
            .chain(review)
            .chain(new)
            .map(|c| c.id.clone())
            .collect()
    }

    /// The card to show next: the head of the queue, or, when the queue is
    /// empty, a learning card coming due within [`LEARN_AHEAD`].
    pub fn next_card(&self, now: i64, offset: i64, topic: Option<&str>) -> Option<&Card> {
        if let Some(id) = self.queue(now, offset, topic).first() {
            return self.get(id);
        }
        self.cards
            .iter()
            .filter(|c| !c.suspended && topic.is_none_or(|t| c.topic.as_deref() == Some(t)))
            .filter(|c| {
                matches!(c.sched.state, CardState::Learning | CardState::Relearning)
                    && c.sched.due <= now + LEARN_AHEAD
            })
            .min_by_key(|c| c.sched.due)
    }

    /// Cards due today (the queue's length).
    pub fn due_count(&self, now: i64, offset: i64, topic: Option<&str>) -> usize {
        self.queue(now, offset, topic).len()
    }

    pub fn stats(&self, now: i64, offset: i64) -> DeckStats {
        let mut s = DeckStats {
            total: self.cards.len(),
            ..DeckStats::default()
        };
        for c in &self.cards {
            if c.suspended {
                s.suspended += 1;
                continue;
            }
            match c.sched.state {
                CardState::New => s.new += 1,
                CardState::Learning | CardState::Relearning => s.learning += 1,
                CardState::Review if c.sched.is_mature() => s.mature += 1,
                CardState::Review => s.young += 1,
            }
        }
        s.new_left = self
            .new_per_day
            .saturating_sub(self.new_seen_today(now, offset))
            .min(s.new);
        s.due_today = self.due_count(now, offset, None);
        let today = day_index(now, offset);
        s.reviewed_today = self
            .log
            .iter()
            .filter(|l| day_index(l.at, offset) == today)
            .count();
        let recent: Vec<&ReviewLog> = self
            .log
            .iter()
            .filter(|l| l.state_before == CardState::Review && now - l.at <= 30 * DAY)
            .collect();
        if !recent.is_empty() {
            let kept = recent.iter().filter(|l| l.rating != Rating::Again).count();
            s.retention = Some(kept as f32 / recent.len() as f32);
        }
        s
    }

    /// Cards on a topic.
    pub fn for_topic<'a>(&'a self, topic: &'a str) -> impl Iterator<Item = &'a Card> + 'a {
        self.cards
            .iter()
            .filter(move |c| c.topic.as_deref() == Some(topic))
    }
}

// ---- Writing cards ---------------------------------------------------

/// What to write cards on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardSpec {
    /// A topic id, or `None` for all the materials.
    pub topic: Option<String>,
    pub count: usize,
    /// The kinds to mix; empty means all three.
    pub kinds: Vec<CardKind>,
}

impl Default for CardSpec {
    fn default() -> Self {
        Self {
            topic: None,
            count: 12,
            kinds: Vec::new(),
        }
    }
}

impl CardSpec {
    pub fn kinds(&self) -> Vec<CardKind> {
        if self.kinds.is_empty() {
            CardKind::ALL.to_vec()
        } else {
            self.kinds.clone()
        }
    }

    pub fn count(&self) -> usize {
        self.count.clamp(1, MAX_GEN)
    }
}

/// The prompt asking a model for flashcards: (system, user).
pub fn cards_prompt(spec: &CardSpec, title: &str, sources: &[Source]) -> (String, String) {
    let kinds: Vec<&str> = spec.kinds().iter().map(|k| k.key()).collect();
    let system = format!(
        "You write flashcards for a student, using only the numbered passages from their own study materials. Write up to {n} cards, mixing these types: {kinds}. Types: \"basic\" (a question on the front, a short answer on the back), \"cloze\" (one sentence from the passages on the front with its key term marked {{{{c1::term}}}}; the back is optional extra context), \"why\" (the front asks to explain why or how something holds; the back gives the reason in one to three sentences). One idea per card. Fronts under 25 words, backs under 60. Prefer what a student must remember or understand: definitions, causes, contrasts, steps. Every card names the passage it comes from as \"source\": \"S2\". Do not use anything the passages do not say. Reply with JSON only: {{\"cards\": [{{\"type\": \"basic\", \"front\": \"…\", \"back\": \"…\", \"source\": \"S1\"}}]}}",
        n = spec.count(),
        kinds = kinds.join(", "),
    );
    let user = format!(
        "Flashcards on: {title}\n\nPassages:\n\n{}",
        sources_block(sources)
    );
    (system, user)
}

fn source_of<'a>(item: &Value, sources: &'a [Source]) -> Option<&'a Source> {
    let raw = match item
        .get("source")
        .or_else(|| item.get("passage"))
        .or_else(|| item.get("src"))?
    {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => match a.first()? {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => return None,
        },
        _ => return None,
    };
    let n: usize = raw
        .trim()
        .trim_start_matches(['[', 'S', 's'])
        .trim_end_matches(']')
        .trim()
        .parse()
        .ok()?;
    sources.iter().find(|s| s.n == n)
}

/// Reads a model's cards. Cards that cannot be used are dropped; repeats
/// within the reply are dropped too.
pub fn parse_cards(text: &str, sources: &[Source], topic: Option<&str>, now: i64) -> Vec<Card> {
    let Some(v) = json_in(text) else {
        return Vec::new();
    };
    let items = match &v {
        Value::Array(a) => a.clone(),
        _ => v
            .get("cards")
            .or_else(|| v.get("flashcards"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    let mut out: Vec<Card> = Vec::new();
    for item in items.iter().take(MAX_GEN * 2) {
        let Some(mut c) = parse_card(item, now) else {
            continue;
        };
        if let Some(s) = source_of(item, sources) {
            c.anchor = Some(s.anchor.clone());
            c.chunk = Some(s.chunk.clone());
        }
        c.topic = topic.map(str::to_owned);
        if let Some(t) = topic {
            c.tags = vec![t.to_owned()];
        }
        if !out.iter().any(|x| x.same_as(&c)) {
            out.push(c);
        }
        if out.len() >= MAX_GEN {
            break;
        }
    }
    out
}

fn parse_card(item: &Value, now: i64) -> Option<Card> {
    let front = one_line(field(
        item,
        &["front", "question", "q", "text", "prompt", "cloze"],
    )?);
    let back = field(item, &["back", "answer", "a", "extra", "explanation"])
        .map(one_line)
        .unwrap_or_default();
    let mut kind = field(item, &["type", "kind"])
        .and_then(CardKind::parse)
        .unwrap_or(if front.contains("{{c") {
            CardKind::Cloze
        } else {
            CardKind::Basic
        });
    let mut front = clip(&front, 400);
    let mut back = clip(&back, 800);
    if kind == CardKind::Cloze && cloze_numbers(&front).is_empty() {
        // `___` with the answer, or a named term found in the sentence.
        let term = field(item, &["term", "answer", "hidden"])
            .map(str::to_owned)
            .unwrap_or_else(|| back.clone());
        if front.contains("___") && !term.is_empty() {
            front = front.replacen("___", &format!("{{{{c1::{term}}}}}"), 1);
            if back == term {
                back.clear();
            }
        } else if let Some(c) = make_cloze(&front, &term).filter(|_| !term.is_empty()) {
            front = c;
            back.clear();
        } else if !back.is_empty() {
            kind = CardKind::Basic;
        } else {
            return None;
        }
    }
    if kind != CardKind::Cloze && back.trim().is_empty() {
        return None;
    }
    if front.chars().filter(|c| c.is_alphanumeric()).count() < 4 {
        return None;
    }
    Some(Card::new(kind, &front, &back, "model", now))
}

/// A sentence that gives a reason: (what is explained, the sentence).
pub fn reason(sentence: &str) -> Option<String> {
    for cue in [" because ", " since ", " so that ", " in order to "] {
        if let Some((head, _)) = sentence.split_once(cue) {
            let head = head.trim().trim_end_matches(',');
            if head.split_whitespace().count() >= 2 {
                return Some(head.to_owned());
            }
        }
    }
    for cue in [
        ", so ",
        "; therefore ",
        ". Therefore ",
        ", therefore ",
        " which means ",
    ] {
        if let Some((_, tail)) = sentence.split_once(cue) {
            let tail = tail.trim().trim_end_matches(['.', '!', '?']);
            if tail.split_whitespace().count() >= 3 {
                return Some(tail.to_owned());
            }
        }
    }
    None
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match (c.next(), c.clone().next()) {
        // Keep acronyms and names: `TCP`, `Dijkstra`.
        (Some(f), Some(n)) if f.is_uppercase() && n.is_lowercase() => {
            let first_word = s.split_whitespace().next().unwrap_or("");
            if matches!(
                first_word,
                "The" | "A" | "An" | "This" | "These" | "It" | "Each" | "Every"
            ) {
                f.to_lowercase().collect::<String>() + c.as_str()
            } else {
                s.to_owned()
            }
        }
        _ => s.to_owned(),
    }
}

/// The sentence's best term to hide: a long word the passage repeats.
fn key_term(sentence: &str, passage: &str) -> Option<String> {
    let pw = words(passage);
    words(sentence)
        .into_iter()
        .filter(|w| w.chars().count() >= 5)
        .max_by_key(|w| {
            (
                pw.iter().filter(|x| *x == w).count().min(3),
                w.chars().count(),
            )
        })
}

/// Cards without a model, from the passages: definitions as basic cards,
/// key sentences as cloze cards, reasons (or section headings) as
/// "explain why" cards. Kinds take turns; deterministic.
pub fn fallback_cards(
    spec: &CardSpec,
    sources: &[Source],
    topic: Option<&str>,
    now: i64,
) -> Vec<Card> {
    let mut basic: Vec<Card> = Vec::new();
    let mut cloze: Vec<Card> = Vec::new();
    let mut why: Vec<Card> = Vec::new();
    // Heading cards come after every card with a stated reason.
    let mut why_headings: Vec<Card> = Vec::new();
    let mut headings_used: BTreeSet<String> = BTreeSet::new();
    let with = |mut c: Card, s: &Source| {
        c.anchor = Some(s.anchor.clone());
        c.chunk = Some(s.chunk.clone());
        c.topic = topic.map(str::to_owned);
        if let Some(t) = topic {
            c.tags = vec![t.to_owned()];
        }
        c
    };
    for s in sources {
        let body: String = s
            .text
            .lines()
            .filter(|l| crate::material::heading(l).is_none())
            .collect::<Vec<_>>()
            .join("\n");
        let sents: Vec<String> = sentences(&body)
            .into_iter()
            .filter(|x| (5..=45).contains(&x.split_whitespace().count()))
            .collect();
        let mut used_reason = false;
        for sent in &sents {
            if let Some((term, _)) = definition(sent) {
                let verb = if sent.contains(&format!("{term} are ")) {
                    "are"
                } else {
                    "is"
                };
                // Keep the sentence's article: "What is a graph?"
                let lead = sent.trim_start();
                let article = ["A ", "An ", "The "]
                    .iter()
                    .find(|a| lead.starts_with(**a))
                    .map(|a| a.to_lowercase())
                    .unwrap_or_default();
                basic.push(with(
                    Card::new(
                        CardKind::Basic,
                        &format!("What {verb} {article}{}?", lower_first(&term)),
                        sent,
                        "fallback",
                        now,
                    ),
                    s,
                ));
            } else if let Some(r) = reason(sent) {
                used_reason = true;
                why.push(with(
                    Card::new(
                        CardKind::Why,
                        &format!("Explain why {}.", lower_first(&r).trim_end_matches('.')),
                        sent,
                        "fallback",
                        now,
                    ),
                    s,
                ));
            } else if let Some(t) = key_term(sent, &s.text)
                && let Some(text) = make_cloze(sent, &t)
            {
                cloze.push(with(
                    Card::new(CardKind::Cloze, &text, "", "fallback", now),
                    s,
                ));
            }
        }
        // A section with no stated reason: why does it matter?
        if !used_reason
            && let Some(h) = &s.anchor.section
            && headings_used.insert(h.clone())
            && !sents.is_empty()
        {
            let back: Vec<String> = sents.iter().take(2).cloned().collect();
            why_headings.push(with(
                Card::new(
                    CardKind::Why,
                    &format!("Explain the main idea of “{h}” and why it matters."),
                    &clip(&back.join(" "), 400),
                    "fallback",
                    now,
                ),
                s,
            ));
        }
    }
    why.extend(why_headings);
    let kinds = spec.kinds();
    let mut pools: Vec<(CardKind, std::collections::VecDeque<Card>)> = kinds
        .iter()
        .map(|k| {
            let v = match k {
                CardKind::Basic => basic.clone(),
                CardKind::Cloze => cloze.clone(),
                CardKind::Why => why.clone(),
            };
            (*k, v.into())
        })
        .collect();
    let mut out: Vec<Card> = Vec::new();
    while out.len() < spec.count() && pools.iter().any(|(_, p)| !p.is_empty()) {
        for (_, pool) in pools.iter_mut() {
            if out.len() >= spec.count() {
                break;
            }
            while let Some(c) = pool.pop_front() {
                if !out.iter().any(|x| x.same_as(&c)) {
                    out.push(c);
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{Anchor, Source};

    fn src(n: usize, text: &str, section: Option<&str>) -> Source {
        Source {
            n,
            chunk: format!("m1#{n}"),
            anchor: Anchor {
                material: "m1".into(),
                file: "algo.md".into(),
                page: Some(n as u32),
                section: section.map(str::to_owned),
            },
            text: text.into(),
        }
    }

    fn sources() -> Vec<Source> {
        vec![
            src(
                1,
                "# Graphs\nA graph is a set of vertices joined by edges. Graphs model road networks and social networks between people.",
                Some("Graphs"),
            ),
            src(
                2,
                "# Shortest paths\nDijkstra's algorithm fails on negative weights because a settled vertex is never revisited later. It keeps a priority queue of tentative distances for every vertex.",
                Some("Shortest paths"),
            ),
        ]
    }

    #[test]
    fn cloze_helpers() {
        let t = "The {{c1::mitochondria::organelle}} makes {{c2::ATP}}.";
        assert_eq!(cloze_question(t), "The [organelle] makes […].");
        assert_eq!(cloze_plain(t), "The mitochondria makes ATP.");
        assert_eq!(cloze_numbers(t), vec![1, 2]);
        assert_eq!(cloze_parts(t)[0].2.as_deref(), Some("organelle"));
        assert_eq!(
            make_cloze("Cells use Glucose for energy.", "glucose").unwrap(),
            "Cells use {{c1::Glucose}} for energy."
        );
        assert!(make_cloze("x", "y").is_none());
        assert_eq!(cloze_plain("no {{gap}} here"), "no {{gap}} here");
    }

    #[test]
    fn fallback_writes_each_kind_with_anchors() {
        let cards = fallback_cards(&CardSpec::default(), &sources(), Some("graphs"), 5);
        let kinds: BTreeSet<CardKind> = cards.iter().map(|c| c.kind).collect();
        assert_eq!(kinds.len(), 3, "{cards:#?}");
        let basic = cards.iter().find(|c| c.kind == CardKind::Basic).unwrap();
        assert_eq!(basic.front, "What is a graph?");
        assert_eq!(basic.anchor.as_ref().unwrap().page, Some(1));
        let why = cards.iter().find(|c| c.kind == CardKind::Why).unwrap();
        assert_eq!(
            why.front,
            "Explain why Dijkstra's algorithm fails on negative weights."
        );
        assert!(cards.iter().all(|c| c.topic.as_deref() == Some("graphs")));
        assert!(cards.iter().all(|c| c.tags == vec!["graphs".to_string()]));
        let cloze = cards.iter().find(|c| c.kind == CardKind::Cloze).unwrap();
        assert_eq!(cloze_numbers(&cloze.front), vec![1]);
        // Same input, same cards.
        assert_eq!(
            cards,
            fallback_cards(&CardSpec::default(), &sources(), Some("graphs"), 5)
        );
        let only = fallback_cards(
            &CardSpec {
                kinds: vec![CardKind::Cloze],
                count: 2,
                ..CardSpec::default()
            },
            &sources(),
            None,
            5,
        );
        assert!(only.len() <= 2 && only.iter().all(|c| c.kind == CardKind::Cloze));
    }

    #[test]
    fn parses_model_cards_leniently() {
        let reply = r#"Here:
```json
{"cards": [
 {"type": "basic", "question": "What does Dijkstra need?", "answer": "Non-negative weights", "source": "S2"},
 {"type": "cloze", "text": "A graph is a set of ___ joined by edges.", "answer": "vertices", "source": 1},
 {"kind": "explain why", "front": "Explain why Dijkstra fails on negative weights", "back": "Settled vertices are never revisited."},
 {"type": "cloze", "front": "No gap and no answer here at all"},
 {"type": "basic", "front": "What does Dijkstra need?", "back": "dup"},
 {"front": "Graphs model {{c1::networks}}."}
]}
```"#;
        let cards = parse_cards(reply, &sources(), None, 9);
        assert_eq!(cards.len(), 4, "{cards:#?}");
        assert_eq!(cards[0].anchor.as_ref().unwrap().page, Some(2));
        assert_eq!(
            cards[1].front,
            "A graph is a set of {{c1::vertices}} joined by edges."
        );
        assert!(cards[1].back.is_empty());
        assert_eq!(cards[2].kind, CardKind::Why);
        assert_eq!(cards[3].kind, CardKind::Cloze);
        assert!(parse_cards("no", &sources(), None, 0).is_empty());
        let (sys, user) = cards_prompt(&CardSpec::default(), "Graphs", &sources());
        assert!(sys.contains("{{c1::term}}"));
        assert!(user.contains("[S2] algo.md p. 2"));
    }

    fn deck_with(n: usize) -> Deck {
        let mut d = Deck::default();
        let cards = (0..n)
            .map(|i| {
                let mut c = Card::new(CardKind::Basic, &format!("What is item{i}?"), "A", "you", 0);
                c.topic = Some(if i % 2 == 0 { "even" } else { "odd" }.into());
                c
            })
            .collect();
        d.add(cards);
        d
    }

    #[test]
    fn deck_adds_dedupes_edits_suspends_and_deletes() {
        let mut d = deck_with(3);
        assert_eq!(d.cards[2].id, "k3");
        let again = d.add(vec![Card::new(
            CardKind::Basic,
            "what is ITEM1",
            "x",
            "you",
            0,
        )]);
        assert!(again.is_empty(), "repeat skipped");
        assert_eq!(
            d.repeats(&[Card::new(CardKind::Basic, "What is item2?", "", "you", 0)]),
            1
        );
        d.edit("k1", "New front", "New back").unwrap();
        assert_eq!(d.cards[0].written_by, "you");
        assert!(d.edit("k1", " ", "").is_err());
        d.suspend("2", true).unwrap();
        assert!(d.get("k2").unwrap().suspended);
        let gone = d.delete("k3").unwrap();
        assert_eq!(gone.id, "k3");
        assert_eq!(
            d.add(vec![Card::new(CardKind::Basic, "Fresh?", "y", "you", 0)])[0].id,
            "k4"
        );
        assert!(d.index("k99").is_err());
        let mut cl = Deck::default();
        cl.add(vec![Card::new(
            CardKind::Cloze,
            "A {{c1::b}} c",
            "",
            "you",
            0,
        )]);
        assert!(cl.edit("k1", "no gap", "").is_err());
    }

    #[test]
    fn queue_orders_learning_review_then_new_within_limits() {
        let mut d = deck_with(30);
        d.new_per_day = 5;
        let now = 10 * DAY + 3600;
        assert_eq!(d.due_count(now, 0, None), 5, "new cards capped");
        // Answer two new cards: one Good (learning, 10 min), one Easy (review in 4 days).
        d.review("k1", Rating::Good, now).unwrap();
        d.review("k2", Rating::Easy, now).unwrap();
        let q = d.queue(now, 0, None);
        assert_eq!(q.len(), 3, "two of five new used: {q:?}");
        assert!(!q.contains(&"k1".to_string()), "learning card not yet due");
        let later = now + 700;
        assert_eq!(d.queue(later, 0, None)[0], "k1", "learning first once due");
        // Nothing left: learn ahead picks the learning card early.
        let mut small = deck_with(1);
        small.review("k1", Rating::Good, now).unwrap();
        assert!(small.queue(now, 0, None).is_empty());
        assert_eq!(small.next_card(now, 0, None).unwrap().id, "k1");
        // Review due later today counts today; tomorrow's does not.
        let mut r = deck_with(2);
        r.cards[0].sched = Sched {
            state: CardState::Review,
            interval: 3.0,
            due: now + 3600,
            ..Sched::default()
        };
        r.cards[1].sched = Sched {
            state: CardState::Review,
            interval: 3.0,
            due: now + DAY,
            ..Sched::default()
        };
        assert_eq!(r.queue(now, 0, None), vec!["k1".to_string()]);
        // Topic filter and suspension.
        let mut t = deck_with(4);
        assert_eq!(t.queue(now, 0, Some("odd")), vec!["k2", "k4"]);
        t.suspend("k2", true).unwrap();
        assert_eq!(t.queue(now, 0, Some("odd")), vec!["k4"]);
    }

    #[test]
    fn stats_and_retention() {
        let mut d = deck_with(4);
        let now = 100 * DAY;
        d.cards[0].sched = Sched {
            state: CardState::Review,
            interval: 30.0,
            due: now,
            ..Sched::default()
        };
        d.cards[1].sched = Sched {
            state: CardState::Review,
            interval: 5.0,
            due: now,
            ..Sched::default()
        };
        d.review("k1", Rating::Good, now).unwrap();
        d.review("k2", Rating::Again, now).unwrap();
        d.suspend("k4", true).unwrap();
        let s = d.stats(now, 0);
        assert_eq!((s.total, s.new, s.suspended), (4, 1, 1));
        assert_eq!(s.mature, 1);
        assert_eq!(s.learning, 1, "the lapse relearns");
        assert_eq!(s.reviewed_today, 2);
        assert_eq!(s.retention, Some(0.5));
        assert!(s.line().contains("50% recalled"), "{}", s.line());
        assert_eq!(end_of_day(DAY + 5, 0), 2 * DAY);
        assert_eq!(end_of_day(DAY + 5, 5 * 3600), 2 * DAY - 5 * 3600);
        let json = serde_json::to_string(&d).unwrap();
        let back: Deck = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn reasons_are_found() {
        assert_eq!(
            reason("Ice floats because it is less dense than water.").as_deref(),
            Some("Ice floats")
        );
        assert_eq!(
            reason("The cache is small, so misses are common in large loops.").as_deref(),
            Some("misses are common in large loops")
        );
        assert!(reason("Plain fact here.").is_none());
        let c = Card::new(CardKind::Cloze, "A {{c1::b}} c", "extra", "you", 0);
        assert_eq!(c.question(), "A […] c");
        assert_eq!(c.answer(), "A b c\n\nextra");
    }
}
