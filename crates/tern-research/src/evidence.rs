//! The evidence chain: quotes Tern minted from text it read, citations
//! bound to them, a stance per source for every claim, the agreement
//! across sources, and counter-search for key claims.
//!
//! A citation is only rendered when Tern can back it. A quote is minted
//! only from the text of a page Tern read ([`QuoteBook::mint`]) and its id
//! is a hash of the page and the quote's words ([`quote_id`]), so a model
//! cannot guess one. An answer cites `[n]` (a source Tern read) or a
//! quote token `[[q:ab12cd]]` (a passage a tool showed it with that id).
//! [`render`] turns tokens into `[n]` and strips any token Tern never
//! minted; [`build`] ties each `[n]` of a claim to the minted quote that
//! bears on it, or flags it ([`CiteStatus`]).
//!
//! | Stance | Meaning |
//! | --- | --- |
//! | Supports | A passage of the source states the claim |
//! | Refutes | A passage about the same thing says otherwise |
//! | Mixed | The source (or the sources together) has passages both ways |
//! | Not enough info | No passage bears on it |
//!
//! Stances come from the same cheap checks as [`crate::verdict`] (word
//! overlap, numbers, polarity, antonyms, swapped sides), so the chain works
//! offline and with small local models. A model judge may refine the
//! stances of a few cards within a budget ([`stance_prompt`],
//! [`apply_stances`]); the agreement is always counted here, per
//! independent site, never by the model.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::claims::{extract, sentences};
use crate::scholar::Locator;
use crate::verdict::{
    ClaimCheck, Conflict, Method, PARTLY_AT, QUOTE_CHARS, SUPPORTED_AT, SourceText, Verdict,
    anchor, clip, conflict, content_words, lexical_verdict, negated, numbers, page_at, score_span,
    squash, without_page_marks,
};

/// Hex digits in a quote id.
pub const ID_HEX: usize = 6;

/// A quote's id: six hex digits, hashed from the page and the quote.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuoteId(pub String);

impl QuoteId {
    /// The token a model writes to cite it: `[[q:ab12cd]]`.
    pub fn token(&self) -> String {
        format!("[[q:{}]]", self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for QuoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "q:{}", self.0)
    }
}

/// The id Tern mints for `quote` read on `url`: FNV-1a over the URL
/// (without its fragment) and the quote's words, lower case, spacing
/// squashed, so the same passage gets the same id however it was cut.
pub fn quote_id(url: &str, quote: &str) -> QuoteId {
    let base = url.split('#').next().unwrap_or(url);
    let words = squash(&without_page_marks(quote));
    let words = words.trim_matches(|c: char| !c.is_alphanumeric());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in base.bytes().chain([0u8]).chain(words.bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    QuoteId(format!("{:06x}", h & 0x00ff_ffff))
}

/// One passage Tern read, with where it sits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    pub id: QuoteId,
    /// The citation number of its source; 0 for a page read only by a
    /// counter-search (not citable).
    pub source: usize,
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub site: String,
    /// The passage as the source has it, cut to about 300 characters.
    pub text: String,
    /// The PDF page it is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    /// The section heading it falls under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// A link straight to the passage.
    #[serde(default)]
    pub anchor: String,
    /// Read by a counter-search.
    #[serde(default)]
    pub counter: bool,
}

impl Quote {
    /// "p. 4 · 3 Method", "§ Results", or empty.
    pub fn locator(&self) -> String {
        Locator {
            page: self.page,
            section: self.section.clone(),
            offset: None,
        }
        .label()
    }
}

/// The quotes Tern minted, in the order it minted them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteBook {
    pub quotes: Vec<Quote>,
}

impl QuoteBook {
    /// Mints the quote at `start..end` of `s`, or returns the id it
    /// already has.
    pub fn mint(&mut self, s: &SourceText, start: usize, end: usize, counter: bool) -> QuoteId {
        let raw = &s.text[start..end];
        let text = clip(&without_page_marks(raw), QUOTE_CHARS);
        let id = quote_id(&s.url, &text);
        if self.get(id.as_str()).is_some() {
            return id;
        }
        let page = page_at(&s.text, start);
        let section = section_at(&s.text, start, &text);
        self.quotes.push(Quote {
            id: id.clone(),
            source: s.n,
            url: s.url.clone(),
            title: s.title.clone(),
            site: s.site.clone(),
            anchor: anchor(&s.url, &text, page),
            text,
            page,
            section,
            counter,
        });
        id
    }

    /// Adds a quote minted elsewhere (a tool that showed the model the
    /// passage with its id). Its id is recomputed, so a forged one never
    /// gets in.
    pub fn add(&mut self, mut q: Quote) -> QuoteId {
        q.id = quote_id(&q.url, &q.text);
        if self.get(q.id.as_str()).is_none() {
            if q.anchor.is_empty() {
                q.anchor = anchor(&q.url, &q.text, q.page);
            }
            self.quotes.push(q.clone());
        }
        q.id
    }

    /// The quote with id `id` (`ab12cd` or `q:ab12cd`).
    pub fn get(&self, id: &str) -> Option<&Quote> {
        let id = id.trim().trim_start_matches("q:");
        self.quotes.iter().find(|q| q.id.0 == id)
    }

    pub fn len(&self) -> usize {
        self.quotes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.quotes.is_empty()
    }
}

/// The section a passage falls under: the PDF section heading when the
/// text has `[page N]` markers, else the last Markdown heading above it.
fn section_at(text: &str, start: usize, quote: &str) -> Option<String> {
    if text.contains("[page ") {
        let pages = crate::scholar::pdf::pages_of(text);
        if let Some(l) = crate::scholar::pdf::locate(&pages, quote)
            && l.section.is_some()
        {
            return l.section;
        }
    }
    let before = &text[..start];
    let line_start = before
        .rfind("\n#")
        .map(|i| i + 1)
        .or_else(|| before.starts_with('#').then_some(0))?;
    let line = before[line_start..].lines().next()?;
    let title = line.trim_start_matches('#').trim();
    (!title.is_empty() && title.chars().count() <= 80).then(|| title.to_owned())
}

/// What a source says about a claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stance {
    Supports,
    Refutes,
    Mixed,
    NotEnoughInfo,
}

impl Stance {
    pub const ALL: [Stance; 4] = [
        Self::Supports,
        Self::Refutes,
        Self::Mixed,
        Self::NotEnoughInfo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Supports => "Supports",
            Self::Refutes => "Refutes",
            Self::Mixed => "Mixed",
            Self::NotEnoughInfo => "Not enough info",
        }
    }

    /// One character for a chip.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Supports => "✓",
            Self::Refutes => "✕",
            Self::Mixed => "±",
            Self::NotEnoughInfo => "○",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase().replace([' ', '-'], "_");
        Some(match s.as_str() {
            "supports" | "support" | "supported" | "supporting" => Self::Supports,
            "refutes" | "refute" | "refuted" | "contradicts" | "contradicted" | "contrasting" => {
                Self::Refutes
            }
            "mixed" | "both" => Self::Mixed,
            "not_enough_info"
            | "not_enough_information"
            | "nei"
            | "noinfo"
            | "no_info"
            | "not_enough"
            | "insufficient"
            | "neutral" => Self::NotEnoughInfo,
            _ => return None,
        })
    }

    /// Whether the source takes a side.
    pub fn has_evidence(self) -> bool {
        self != Self::NotEnoughInfo
    }
}

/// What one source says about one claim, and the quote it rests on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StanceCard {
    /// The quote; none when no passage of the source bears on the claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<QuoteId>,
    /// The source's citation number (0 for a counter-search page).
    pub source: usize,
    pub url: String,
    #[serde(default)]
    pub site: String,
    pub stance: Stance,
    /// Share of the claim's content words in the passage, 0 to 100.
    pub overlap: u8,
    /// The answer cites this source for the claim.
    #[serde(default)]
    pub cited: bool,
    /// Found by a counter-search.
    #[serde(default)]
    pub counter: bool,
    #[serde(default)]
    pub method: Method,
    /// Why, in a few words.
    #[serde(default)]
    pub note: String,
}

/// How many independent sources take each side of a claim. Sources on
/// one site count once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agreement {
    pub supports: usize,
    pub refutes: usize,
    pub mixed: usize,
    /// Cited sources with nothing on the claim.
    pub nei: usize,
}

/// Fewer sources than this taking a side earns a "few sources" note.
pub const FEW_SOURCES: usize = 3;

impl Agreement {
    /// Counts `cards` per independent site.
    pub fn of(cards: &[StanceCard]) -> Self {
        let mut sites: BTreeMap<String, BTreeSet<Stance>> = BTreeMap::new();
        for c in cards {
            sites.entry(site_key(c)).or_default().insert(c.stance);
        }
        let mut a = Self::default();
        for stances in sites.values() {
            let s = stances.contains(&Stance::Supports);
            let r = stances.contains(&Stance::Refutes);
            let m = stances.contains(&Stance::Mixed);
            if m || (s && r) {
                a.mixed += 1;
            } else if s {
                a.supports += 1;
            } else if r {
                a.refutes += 1;
            } else {
                a.nei += 1;
            }
        }
        a
    }

    /// Sources counted.
    pub fn sources(&self) -> usize {
        self.supports + self.refutes + self.mixed + self.nei
    }

    /// Sources that take a side.
    pub fn with_evidence(&self) -> usize {
        self.supports + self.refutes + self.mixed
    }

    /// The claim's stance across its sources.
    pub fn stance(&self) -> Stance {
        match (self.supports > 0, self.refutes > 0, self.mixed > 0) {
            (false, false, false) => Stance::NotEnoughInfo,
            (true, false, false) => Stance::Supports,
            (false, true, false) => Stance::Refutes,
            _ => Stance::Mixed,
        }
    }

    /// Fewer than [`FEW_SOURCES`] sources take a side.
    pub fn few(&self) -> bool {
        self.with_evidence() < FEW_SOURCES
    }

    /// "2 support · 1 refutes · 3 sources", empty parts left out.
    pub fn line(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.supports > 0 {
            parts.push(format!("{} support", self.supports));
        }
        if self.refutes > 0 {
            parts.push(format!(
                "{} {}",
                self.refutes,
                if self.refutes == 1 {
                    "refutes"
                } else {
                    "refute"
                }
            ));
        }
        if self.mixed > 0 {
            parts.push(format!("{} mixed", self.mixed));
        }
        if self.nei > 0 {
            parts.push(format!("{} no info", self.nei));
        }
        let n = self.sources();
        parts.push(format!("{n} source{}", if n == 1 { "" } else { "s" }));
        parts.join(" · ")
    }
}

fn site_key(c: &StanceCard) -> String {
    let s = if c.site.trim().is_empty() {
        c.url
            .split("://")
            .nth(1)
            .unwrap_or(&c.url)
            .split('/')
            .next()
            .unwrap_or_default()
            .to_owned()
    } else {
        c.site.clone()
    };
    s.trim().trim_start_matches("www.").to_lowercase()
}

/// Whether a citation of a claim is backed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiteStatus {
    /// Bound to a quote Tern minted from the source's text.
    Bound,
    /// The source was read, but no passage of it bears on the claim.
    Unbound,
    /// No page Tern read has this number.
    Unread,
    /// A quote token whose id Tern never minted; stripped from the answer.
    Forged,
}

impl CiteStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bound => "Backed by a quote Tern read",
            Self::Unbound => "Tern read this source, but no passage of it bears on the claim",
            Self::Unread => "Cites a source Tern never read",
            Self::Forged => "Cites a quote id Tern never minted",
        }
    }

    /// Whether the citation may be rendered as a link to its source.
    pub fn renders(self) -> bool {
        matches!(self, Self::Bound | Self::Unbound)
    }
}

/// One citation of a claim and what backs it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiteBinding {
    /// The number as written (0 for a forged quote token).
    pub n: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<QuoteId>,
    /// The forged token as written.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
    pub status: CiteStatus,
}

/// Why a claim gets a counter-search.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// The answer's first claim.
    Headline,
    /// It states a number.
    Number,
    /// It is absolute or causal ("always", "the first", "causes").
    Strong,
}

impl Trigger {
    pub fn label(self) -> &'static str {
        match self {
            Self::Headline => "the answer's headline claim",
            Self::Number => "states a number",
            Self::Strong => "a strong claim",
        }
    }
}

/// A search for evidence against a claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterSearch {
    pub query: String,
    pub trigger: Trigger,
    /// It ran (it may be planned and then skipped for want of a search
    /// service or budget).
    #[serde(default)]
    pub ran: bool,
    /// Pages read for it.
    #[serde(default)]
    pub read: usize,
    /// Of those, pages that refute the claim or are mixed on it.
    #[serde(default)]
    pub disagree: usize,
    #[serde(default)]
    pub note: String,
}

/// One claim's evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimEvidence {
    pub claim: usize,
    pub text: String,
    pub stance: Stance,
    pub agreement: Agreement,
    /// One card per source that bears on the claim, cited ones first.
    pub cards: Vec<StanceCard>,
    /// The claim's citations as written, in order.
    pub cites: Vec<CiteBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter: Option<CounterSearch>,
}

impl ClaimEvidence {
    /// Recounts the agreement and the stance from the cards.
    pub fn recount(&mut self) {
        self.agreement = Agreement::of(&self.cards);
        self.stance = self.agreement.stance();
    }

    /// Citations that may not be rendered.
    pub fn invalid_cites(&self) -> usize {
        self.cites.iter().filter(|c| !c.status.renders()).count()
    }

    /// The card a citation number is bound to.
    pub fn card_for(&self, n: usize) -> Option<&StanceCard> {
        self.cards.iter().find(|c| c.source == n && c.cited)
    }
}

/// Counts per stance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StanceTally {
    pub supports: usize,
    pub refutes: usize,
    pub mixed: usize,
    pub nei: usize,
}

impl StanceTally {
    pub fn get(&self, s: Stance) -> usize {
        match s {
            Stance::Supports => self.supports,
            Stance::Refutes => self.refutes,
            Stance::Mixed => self.mixed,
            Stance::NotEnoughInfo => self.nei,
        }
    }

    /// "3 supports · 1 mixed", empty parts left out.
    pub fn line(&self) -> String {
        Stance::ALL
            .iter()
            .filter(|s| self.get(**s) > 0)
            .map(|s| format!("{} {}", self.get(*s), s.label().to_lowercase()))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// An answer's evidence chain.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    /// Every quote a claim rests on.
    pub quotes: Vec<Quote>,
    pub claims: Vec<ClaimEvidence>,
    /// Quote tokens Tern never minted, stripped from the answer.
    #[serde(default)]
    pub forged: Vec<String>,
    /// The model that refined stances, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<String>,
}

impl Chain {
    pub fn quote(&self, id: &QuoteId) -> Option<&Quote> {
        self.quotes.iter().find(|q| q.id == *id)
    }

    pub fn claim(&self, n: usize) -> Option<&ClaimEvidence> {
        self.claims.iter().find(|c| c.claim == n)
    }

    pub fn tally(&self) -> StanceTally {
        let mut t = StanceTally::default();
        for c in &self.claims {
            match c.stance {
                Stance::Supports => t.supports += 1,
                Stance::Refutes => t.refutes += 1,
                Stance::Mixed => t.mixed += 1,
                Stance::NotEnoughInfo => t.nei += 1,
            }
        }
        t
    }

    /// Citations that may not be rendered, forged tokens included.
    pub fn invalid_cites(&self) -> usize {
        self.claims
            .iter()
            .map(ClaimEvidence::invalid_cites)
            .sum::<usize>()
            + self
                .forged
                .len()
                .saturating_sub(self.claims.iter().map(forged_in).sum::<usize>())
    }

    /// Counter-searches that ran.
    pub fn counter_searches(&self) -> impl Iterator<Item = (&ClaimEvidence, &CounterSearch)> {
        self.claims
            .iter()
            .filter_map(|c| c.counter.as_ref().filter(|s| s.ran).map(|s| (c, s)))
    }

    /// The quotes a citation number is bound to across claims, first use
    /// first: what a `[n]` chip shows on hover.
    pub fn quotes_for(&self, n: usize) -> Vec<(&ClaimEvidence, &StanceCard, &Quote)> {
        let mut out: Vec<(&ClaimEvidence, &StanceCard, &Quote)> = Vec::new();
        for c in &self.claims {
            if let Some(card) = c.card_for(n)
                && let Some(q) = card.quote.as_ref().and_then(|id| self.quote(id))
                && !out.iter().any(|(_, _, seen)| seen.id == q.id)
            {
                out.push((c, card, q));
            }
        }
        out
    }

    /// Every citation number bound to no quote Tern read (unread pages).
    pub fn unread_numbers(&self) -> BTreeSet<usize> {
        self.claims
            .iter()
            .flat_map(|c| c.cites.iter())
            .filter(|b| b.status == CiteStatus::Unread)
            .map(|b| b.n)
            .collect()
    }
}

fn forged_in(c: &ClaimEvidence) -> usize {
    c.cites
        .iter()
        .filter(|b| b.status == CiteStatus::Forged)
        .count()
}

/// An answer with its quote tokens rendered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// Tokens whose id Tern never minted, with the byte in `text` where
    /// each was removed.
    pub forged: Vec<(usize, String)>,
    /// Quotes the answer cited by token, in order.
    pub used: Vec<QuoteId>,
}

/// A quote token at `at`: `[[q:ab12cd]]` or `[q:ab12cd]`, its id and the
/// byte just past it.
fn token_at(s: &str, at: usize) -> Option<(String, usize)> {
    let rest = s.get(at..)?;
    let (open, close) = if rest.starts_with("[[q:") {
        (4, "]]")
    } else if rest.starts_with("[q:") {
        (3, "]")
    } else {
        return None;
    };
    let id: String = rest[open..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    if id.is_empty() || id.len() > 16 {
        return None;
    }
    let after = open + id.len();
    // `[q:ab12 · p. 4]`, as a tool labels a passage, is not a citation.
    if !rest[after..].starts_with(close) {
        return None;
    }
    Some((id.to_lowercase(), at + after + close.len()))
}

/// Turns quote tokens into `[n]` (the number of the quote's source) and
/// strips tokens whose id is not in `book`. A token right after the same
/// `[n]` is not repeated.
pub fn render(answer: &str, book: &QuoteBook) -> Rendered {
    let mut out = Rendered {
        text: String::with_capacity(answer.len()),
        ..Rendered::default()
    };
    if !answer.contains("[q:") && !answer.contains("[[q:") {
        out.text = answer.to_owned();
        return out;
    }
    let mut i = 0;
    let mut in_code = false;
    while i < answer.len() {
        let c = answer.as_bytes()[i];
        if c == b'`' {
            in_code = !in_code;
        }
        if c == b'['
            && !in_code
            && let Some((id, end)) = token_at(answer, i)
        {
            match book.get(&id).filter(|q| q.source > 0) {
                Some(q) => {
                    let mark = format!("[{}]", q.source);
                    if !out.text.ends_with(&mark) {
                        out.text.push_str(&mark);
                    }
                    if !out.used.contains(&q.id) {
                        out.used.push(q.id.clone());
                    }
                }
                None => {
                    while out.text.ends_with(' ')
                        && answer[end..].starts_with(['.', ',', ';', ':', ' ', '\n'])
                    {
                        out.text.pop();
                    }
                    out.forged.push((out.text.len(), answer[i..end].to_owned()));
                }
            }
            i = end;
            continue;
        }
        let ch = answer[i..].chars().next().unwrap_or(' ');
        out.text.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// What the chain may spend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Most sources each claim is compared against (cited ones always).
    pub sources: usize,
    /// Most claims that get a counter-search.
    pub counter_claims: usize,
    /// Most pages read per counter-search.
    pub counter_reads: usize,
    /// Most cards a model judge is shown.
    pub judge_cards: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            sources: 8,
            counter_claims: 2,
            counter_reads: 2,
            judge_cards: 8,
        }
    }
}

/// A source cut into the windows claims are matched against, with each
/// window's content words worked out once.
struct Indexed<'a> {
    src: &'a SourceText,
    windows: Vec<(usize, usize, BTreeSet<String>)>,
}

impl<'a> Indexed<'a> {
    fn of(src: &'a SourceText) -> Self {
        let spans = sentences(&src.text);
        let mut windows = Vec::new();
        for (k, &(a, b)) in spans.iter().enumerate() {
            let mut ends = vec![b];
            if let Some(&(_, c)) = spans.get(k + 1) {
                ends.push(c);
            }
            for e in ends {
                if e - a > 1600 {
                    continue;
                }
                windows.push((a, e, content_words(&src.text[a..e])));
            }
        }
        Self { src, windows }
    }

    /// The best `k` windows for `want`, by overlap, then shorter, then
    /// earlier.
    fn best(&self, want: &BTreeSet<String>, k: usize) -> Vec<(usize, usize, u8)> {
        if want.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(usize, usize, u8)> = self
            .windows
            .iter()
            .map(|(a, b, have)| {
                let hit = want.intersection(have).count();
                (*a, *b, (hit * 100 / want.len()) as u8)
            })
            .filter(|w| w.2 > 0)
            .collect();
        scored.sort_by(|x, y| {
            y.2.cmp(&x.2)
                .then((x.1 - x.0).cmp(&(y.1 - y.0)))
                .then(x.0.cmp(&y.0))
        });
        scored.truncate(k);
        scored
    }
}

/// Windows looked at per source for one claim.
const WINDOWS: usize = 4;

/// A source's stance on a claim: the stance, the window it rests on, its
/// overlap, and a note.
struct Found {
    stance: Stance,
    span: Option<(usize, usize)>,
    overlap: u8,
    note: String,
}

fn overlaps(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// What `ix` says about `claim`, from its best windows.
fn stance_in(claim: &str, want: &BTreeSet<String>, ix: &Indexed<'_>) -> Found {
    let best = ix.best(want, WINDOWS);
    let Some(&(a0, b0, top)) = best.first() else {
        return Found {
            stance: Stance::NotEnoughInfo,
            span: None,
            overlap: 0,
            note: "No passage of this source is about the claim".into(),
        };
    };
    let mut support: Option<(usize, usize, u8)> = None;
    let mut refute: Option<(usize, usize, u8, Conflict)> = None;
    for &(a, b, ov) in &best {
        if ov < PARTLY_AT {
            continue;
        }
        let text = &ix.src.text[a..b];
        let found = conflict(claim, text);
        let counts = match &found {
            Some(Conflict::Polarity | Conflict::Roles) => ov >= SUPPORTED_AT,
            Some(_) => true,
            None => false,
        };
        if counts {
            if refute.is_none() {
                refute = found.map(|c| (a, b, ov, c));
            }
            continue;
        }
        let p = score_span(claim, ix.src, a, b, ov);
        if lexical_verdict(&p) == Verdict::Supported && support.is_none() {
            support = Some((a, b, ov));
        }
    }
    match (support, refute) {
        (Some(s), Some(r)) if !overlaps((s.0, s.1), (r.0, r.1)) => Found {
            stance: Stance::Mixed,
            span: Some((s.0, s.1)),
            overlap: s.2,
            note: format!("One passage states it; another disagrees: {}", r.3.note()),
        },
        (_, Some(r)) => Found {
            stance: Stance::Refutes,
            span: Some((r.0, r.1)),
            overlap: r.2,
            note: r.3.note(),
        },
        (Some(s), None) => Found {
            stance: Stance::Supports,
            span: Some((s.0, s.1)),
            overlap: s.2,
            note: String::new(),
        },
        (None, None) => Found {
            stance: Stance::NotEnoughInfo,
            span: (top >= PARTLY_AT).then_some((a0, b0)),
            overlap: top,
            note: if top >= PARTLY_AT {
                "A passage covers part of it, not all".into()
            } else {
                "No passage of this source states it".into()
            },
        },
    }
}

/// A card for what `ix` says about `claim`, minting its quote.
fn card(
    book: &mut QuoteBook,
    claim: &str,
    want: &BTreeSet<String>,
    ix: &Indexed<'_>,
    cited: bool,
    counter: bool,
) -> StanceCard {
    let f = stance_in(claim, want, ix);
    let quote = f.span.map(|(a, b)| book.mint(ix.src, a, b, counter));
    StanceCard {
        quote,
        source: ix.src.n,
        url: ix.src.url.clone(),
        site: ix.src.site.clone(),
        stance: f.stance,
        overlap: f.overlap,
        cited,
        counter,
        method: Method::Lexical,
        note: f.note,
    }
}

/// The stance of every source on one claim, offline: the cards (each
/// minting its quote in `book`) for the cited sources, then for the other
/// sources that take a side.
pub fn cards_for(
    book: &mut QuoteBook,
    claim: &str,
    cited: &[usize],
    sources: &[SourceText],
) -> Vec<StanceCard> {
    let want = content_words(claim);
    let mut out = Vec::new();
    for s in sources.iter().filter(|s| !s.text.trim().is_empty()) {
        let ix = Indexed::of(s);
        let is_cited = cited.contains(&s.n);
        let c = card(book, claim, &want, &ix, is_cited, false);
        if is_cited || c.stance.has_evidence() {
            out.push(c);
        }
    }
    out.sort_by_key(|c| (!c.cited, c.source));
    out
}

/// Builds the chain for `answer` over the sources Tern read: tokens
/// rendered against `minted` (quotes tools showed the model), every claim's
/// citations bound, and every source's stance on every claim (within
/// `budget.sources`, cited sources always in). Counter-searches are
/// planned ([`plan_counter`]) but not run: the engine runs them and adds
/// what it read with [`add_counter`].
pub fn build(answer: &str, sources: &[SourceText], minted: &QuoteBook, budget: &Budget) -> Chain {
    let rendered = render(answer, minted);
    let claims = extract(&rendered.text);
    let mut book = QuoteBook::default();
    let readable: Vec<&SourceText> = sources
        .iter()
        .filter(|s| !s.text.trim().is_empty())
        .collect();
    let indexed: Vec<Indexed<'_>> = readable.iter().copied().map(Indexed::of).collect();
    let mut chain = Chain {
        forged: rendered.forged.iter().map(|(_, t)| t.clone()).collect(),
        ..Chain::default()
    };
    for c in &claims {
        let want = content_words(&c.text);
        // Cited sources first, then the others in reading order, within
        // the budget.
        let mut order: Vec<&Indexed<'_>> = indexed
            .iter()
            .filter(|ix| c.citations.contains(&ix.src.n))
            .collect();
        for ix in &indexed {
            if order.len() >= budget.sources.max(c.citations.len()) {
                break;
            }
            if !c.citations.contains(&ix.src.n) {
                order.push(ix);
            }
        }
        let mut cards = Vec::new();
        for ix in order {
            let cited = c.citations.contains(&ix.src.n);
            let k = card(&mut book, &c.text, &want, ix, cited, false);
            if cited || k.stance.has_evidence() {
                cards.push(k);
            }
        }
        // A token the model cited by id binds its quote, whatever the
        // overlap found: the model saw that passage.
        let mut cites: Vec<CiteBinding> = Vec::new();
        for n in &c.citations {
            let card = cards.iter().find(|k| k.cited && k.source == *n);
            let token_quote = rendered
                .used
                .iter()
                .filter_map(|id| minted.get(id.as_str()))
                .find(|q| q.source == *n);
            let binding = match (card, token_quote) {
                (_, Some(q)) => {
                    let id = book.add(q.clone());
                    CiteBinding {
                        n: *n,
                        quote: Some(id),
                        token: String::new(),
                        status: CiteStatus::Bound,
                    }
                }
                (Some(k), None) if k.quote.is_some() && k.overlap >= PARTLY_AT => CiteBinding {
                    n: *n,
                    quote: k.quote.clone(),
                    token: String::new(),
                    status: CiteStatus::Bound,
                },
                (Some(_), None) => CiteBinding {
                    n: *n,
                    quote: None,
                    token: String::new(),
                    status: CiteStatus::Unbound,
                },
                (None, None) if sources.iter().any(|s| s.n == *n) => CiteBinding {
                    n: *n,
                    quote: None,
                    token: String::new(),
                    status: CiteStatus::Unbound,
                },
                (None, None) => CiteBinding {
                    n: *n,
                    quote: None,
                    token: String::new(),
                    status: CiteStatus::Unread,
                },
            };
            cites.push(binding);
        }
        for (at, token) in &rendered.forged {
            if *at >= c.span.0 && *at <= c.span.1 {
                cites.push(CiteBinding {
                    n: 0,
                    quote: None,
                    token: token.clone(),
                    status: CiteStatus::Forged,
                });
            }
        }
        let mut ev = ClaimEvidence {
            claim: c.n,
            text: c.text.clone(),
            stance: Stance::NotEnoughInfo,
            agreement: Agreement::default(),
            cards,
            cites,
            counter: None,
        };
        ev.recount();
        chain.claims.push(ev);
    }
    let used: BTreeSet<QuoteId> = chain
        .claims
        .iter()
        .flat_map(|c| {
            c.cards
                .iter()
                .filter_map(|k| k.quote.clone())
                .chain(c.cites.iter().filter_map(|b| b.quote.clone()))
        })
        .collect();
    chain.quotes = book
        .quotes
        .into_iter()
        .filter(|q| used.contains(&q.id))
        .collect();
    chain
}

/// Brings the chain in line with the claim checks a model or the
/// decision model judged: a cited source the judge found supported
/// supports the claim; one it found not supported cannot.
pub fn align(chain: &mut Chain, checks: &[ClaimCheck]) {
    for ev in &mut chain.claims {
        let Some(check) = checks.iter().find(|k| k.claim == ev.claim) else {
            continue;
        };
        if check.method == Method::Lexical {
            continue;
        }
        let judged = check.evidence.as_ref().map(|e| e.source);
        for card in ev.cards.iter_mut().filter(|k| k.cited) {
            if judged.is_some_and(|n| n != card.source) {
                continue;
            }
            let to = match (check.verdict, card.stance) {
                (Verdict::Supported, Stance::NotEnoughInfo) => Some(Stance::Supports),
                (Verdict::NotSupported, Stance::Supports) => Some(Stance::NotEnoughInfo),
                _ => None,
            };
            if let Some(to) = to {
                card.stance = to;
                card.method = check.method;
                card.note = match check.method {
                    Method::Model => "The model judge's verdict".into(),
                    _ => "The decision model's verdict".into(),
                };
            }
        }
        ev.recount();
    }
}

/// Words that mark a hedged claim, which gets no counter-search.
const HEDGES: &[&str] = &[
    "may",
    "might",
    "could",
    "possibly",
    "perhaps",
    "reportedly",
    "suggests",
    "suggest",
    "likely",
    "unclear",
    "some",
];

/// Words that mark an absolute or causal claim.
const STRONG: &[&str] = &[
    "always",
    "never",
    "all",
    "none",
    "only",
    "best",
    "worst",
    "first",
    "largest",
    "smallest",
    "most",
    "least",
    "proven",
    "proves",
    "causes",
    "cause",
    "caused",
    "prevents",
    "prevent",
    "cures",
    "cure",
    "guarantees",
    "every",
    "fastest",
    "safest",
    "highest",
    "lowest",
];

fn lower_words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Why `claim` should get a counter-search, if it should. `first` is
/// whether it is the answer's first claim. Hedged claims and claims a
/// source already disputes get none.
pub fn trigger(claim: &str, first: bool, stance: Stance) -> Option<Trigger> {
    if matches!(stance, Stance::Refutes | Stance::Mixed) {
        return None;
    }
    let words = lower_words(claim);
    if words.iter().any(|w| HEDGES.contains(&w.as_str())) {
        return None;
    }
    if content_words(claim).len() < 3 {
        return None;
    }
    if first {
        Some(Trigger::Headline)
    } else if !numbers(claim).is_empty() {
        Some(Trigger::Number)
    } else if words.iter().any(|w| STRONG.contains(&w.as_str())) {
        Some(Trigger::Strong)
    } else {
        None
    }
}

/// Words a counter-query keeps from a claim.
const QUERY_WORDS: usize = 10;

/// The search for evidence against `claim`: its key words, then words that
/// find disagreement. A negated claim is searched without its negation
/// (to find sources that say it is so).
pub fn counter_query(claim: &str) -> String {
    let neg = negated(claim);
    let stop = |w: &str| {
        matches!(
            w,
            "the"
                | "and"
                | "for"
                | "are"
                | "was"
                | "were"
                | "has"
                | "have"
                | "had"
                | "that"
                | "this"
                | "with"
                | "from"
                | "into"
                | "its"
                | "their"
                | "they"
                | "than"
                | "which"
                | "who"
                | "what"
                | "when"
                | "where"
                | "also"
                | "been"
                | "being"
                | "but"
                | "about"
                | "over"
                | "under"
                | "such"
                | "there"
                | "here"
                | "does"
                | "did"
                | "not"
                | "never"
                | "cannot"
                | "without"
                | "none"
                | "nor"
                | "neither"
        ) || w.ends_with("n't")
    };
    let words: Vec<&str> = claim
        .split(|c: char| !(c.is_alphanumeric() || c == '.' || c == '%' || c == '\''))
        .map(|w| w.trim_matches(['.', '\'']))
        .filter(|w| {
            let l = w.to_lowercase();
            !w.is_empty()
                && (w.chars().any(|c| c.is_ascii_digit())
                    || (w.chars().count() >= 3 && !stop(l.as_str())))
        })
        .take(QUERY_WORDS)
        .collect();
    let core = words.join(" ");
    if neg {
        format!("{core} evidence")
    } else {
        format!("{core} disputed OR incorrect OR \"no evidence\"")
    }
}

/// The claims to counter-search, most important first, within
/// `budget.counter_claims`: each gets a planned [`CounterSearch`].
pub fn plan_counter(chain: &mut Chain, budget: &Budget) -> Vec<usize> {
    let mut picks: Vec<(Trigger, usize)> = chain
        .claims
        .iter()
        .enumerate()
        .filter_map(|(i, c)| trigger(&c.text, i == 0, c.stance).map(|t| (t, c.claim)))
        .collect();
    picks.sort();
    picks.truncate(budget.counter_claims);
    let mut out = Vec::new();
    for (t, n) in picks {
        if let Some(ev) = chain.claims.iter_mut().find(|c| c.claim == n) {
            ev.counter = Some(CounterSearch {
                query: counter_query(&ev.text),
                trigger: t,
                ran: false,
                read: 0,
                disagree: 0,
                note: String::new(),
            });
            out.push(n);
        }
    }
    out.sort_unstable();
    out
}

/// Adds what a counter-search for claim `n` read: each page's stance on
/// the claim (pages that take a side become cards), then the recount.
/// Pages already among the claim's cards are skipped. Returns how many
/// pages disagree.
pub fn add_counter(chain: &mut Chain, n: usize, pages: &[SourceText]) -> usize {
    let mut book = QuoteBook {
        quotes: std::mem::take(&mut chain.quotes),
    };
    let mut disagree = 0;
    let mut read = 0;
    if let Some(ev) = chain.claims.iter_mut().find(|c| c.claim == n) {
        let want = content_words(&ev.text);
        for p in pages.iter().filter(|p| !p.text.trim().is_empty()) {
            if ev.cards.iter().any(|k| same_page(&k.url, &p.url)) {
                continue;
            }
            read += 1;
            let ix = Indexed::of(p);
            let mut k = card(&mut book, &ev.text, &want, &ix, false, true);
            k.source = 0;
            if let Some(id) = &k.quote
                && let Some(q) = book.quotes.iter_mut().find(|q| q.id == *id)
            {
                q.source = 0;
            }
            if matches!(k.stance, Stance::Refutes | Stance::Mixed) {
                disagree += 1;
            }
            if k.stance.has_evidence() {
                ev.cards.push(k);
            }
        }
        ev.recount();
        if let Some(cs) = ev.counter.as_mut() {
            cs.ran = true;
            cs.read = read;
            cs.disagree = disagree;
            cs.note = match (read, disagree) {
                (0, _) => "Found no page to read".into(),
                (_, 0) => format!(
                    "Read {read} page{}; none disagrees",
                    if read == 1 { "" } else { "s" }
                ),
                (_, d) => format!(
                    "{d} of {read} page{} read disagree{}",
                    if read == 1 { "" } else { "s" },
                    if d == 1 { "s" } else { "" }
                ),
            };
        }
    }
    let used: BTreeSet<QuoteId> = chain
        .claims
        .iter()
        .flat_map(|c| {
            c.cards
                .iter()
                .filter_map(|k| k.quote.clone())
                .chain(c.cites.iter().filter_map(|b| b.quote.clone()))
        })
        .collect();
    chain.quotes = book
        .quotes
        .into_iter()
        .filter(|q| used.contains(&q.id))
        .collect();
    disagree
}

fn same_page(a: &str, b: &str) -> bool {
    let norm = |u: &str| {
        let u = u.split('#').next().unwrap_or(u);
        let u = u
            .strip_prefix("https://")
            .or_else(|| u.strip_prefix("http://"))
            .unwrap_or(u);
        u.trim_start_matches("www.")
            .trim_end_matches('/')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

const STANCE_SYSTEM: &str = "You judge what a passage says about a claim. \
For each numbered pair, answer one stance: \"supports\" if the passage states the claim, \"refutes\" if the passage is about the same thing and says otherwise, \"mixed\" if it says both, \"not_enough_info\" if it does not settle it. \
Judge only from the passage, never from what you know. \
Answer with JSON only: an array of objects {\"pair\": number, \"stance\": \"supports\" | \"refutes\" | \"mixed\" | \"not_enough_info\"}.";

/// A judge prompt: the system prompt, the message, and the numbered
/// (claim, card index) pairs.
pub type StancePrompt = (String, String, Vec<(usize, usize)>);

/// The system prompt and message for a model judge over the cards most
/// worth a second look, within `budget.judge_cards`: counter-search cards
/// first, then other uncited cards that take a side. Returns the pairs as
/// (claim, card index) in the order they were numbered, or `None` when no
/// card needs it.
pub fn stance_prompt(chain: &Chain, budget: &Budget) -> Option<StancePrompt> {
    let mut picks: Vec<(u8, usize, usize)> = Vec::new();
    for ev in &chain.claims {
        for (i, k) in ev.cards.iter().enumerate() {
            if k.quote.is_none() || k.method != Method::Lexical {
                continue;
            }
            let rank = match (k.counter, k.cited) {
                (true, _) => 0,
                (false, false) if k.stance.has_evidence() => 1,
                _ => continue,
            };
            picks.push((rank, ev.claim, i));
        }
    }
    picks.sort();
    picks.truncate(budget.judge_cards);
    if picks.is_empty() {
        return None;
    }
    let mut msg = String::new();
    let mut pairs = Vec::new();
    for (j, (_, claim, i)) in picks.iter().enumerate() {
        let Some(ev) = chain.claim(*claim) else {
            continue;
        };
        let k = &ev.cards[*i];
        let Some(q) = k.quote.as_ref().and_then(|id| chain.quote(id)) else {
            continue;
        };
        msg.push_str(&format!(
            "Pair {}:\n  Claim: {}\n  Passage ({}): {}\n\n",
            j + 1,
            ev.text,
            q.site,
            q.text
        ));
        pairs.push((*claim, *i));
    }
    (!pairs.is_empty()).then(|| (STANCE_SYSTEM.to_owned(), msg, pairs))
}

/// Reads a stance judge's reply: (pair number, stance), entries it cannot
/// read skipped.
pub fn parse_stances(reply: &str) -> Vec<(usize, Stance)> {
    let (Some(a), Some(b)) = (reply.find('['), reply.rfind(']')) else {
        return Vec::new();
    };
    if b <= a {
        return Vec::new();
    }
    let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&reply[a..=b]) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| {
            let pair = v.get("pair")?.as_u64()? as usize;
            let stance = Stance::parse(v.get("stance")?.as_str()?)?;
            Some((pair, stance))
        })
        .collect()
}

/// Folds a stance judge's answers into the chain (`pairs` as
/// [`stance_prompt`] returned them) and recounts. A refutation the judge
/// gives a card whose quote shares less than half the claim's words is
/// kept as not enough info: the passage is not about the same thing.
pub fn apply_stances(
    chain: &mut Chain,
    pairs: &[(usize, usize)],
    judged: &[(usize, Stance)],
    model: &str,
) {
    let mut any = false;
    for (j, stance) in judged {
        let Some((claim, i)) = j.checked_sub(1).and_then(|k| pairs.get(k)).copied() else {
            continue;
        };
        let Some(ev) = chain.claims.iter_mut().find(|c| c.claim == claim) else {
            continue;
        };
        let Some(k) = ev.cards.get_mut(i) else {
            continue;
        };
        let stance = if *stance == Stance::Refutes && k.overlap < PARTLY_AT {
            Stance::NotEnoughInfo
        } else {
            *stance
        };
        if stance != k.stance {
            k.note = format!("The model judge: {}", stance.label().to_lowercase());
        }
        k.stance = stance;
        k.method = Method::Model;
        any = true;
    }
    if any {
        for ev in &mut chain.claims {
            ev.recount();
        }
        chain.judge = Some(model.to_owned());
    }
}

/// The chain as Markdown for the run report: each claim's stance and
/// agreement, its quotes with locators, and its counter-search.
pub fn markdown(chain: &Chain) -> String {
    let mut out = String::new();
    if chain.claims.is_empty() {
        return out;
    }
    out.push_str("## Evidence\n\n");
    let t = chain.tally();
    out.push_str(&format!("{}\n\n", t.line()));
    for ev in &chain.claims {
        out.push_str(&format!(
            "- **Claim {}** — {} ({}): {}\n",
            ev.claim,
            ev.stance.label(),
            ev.agreement.line(),
            ev.text
        ));
        for k in &ev.cards {
            let Some(q) = k.quote.as_ref().and_then(|id| chain.quote(id)) else {
                continue;
            };
            let loc = q.locator();
            out.push_str(&format!(
                "  - {} {}{}{} — “{}” ({})\n",
                k.stance.glyph(),
                if k.source > 0 {
                    format!("[{}] ", k.source)
                } else {
                    String::new()
                },
                q.site,
                if loc.is_empty() {
                    String::new()
                } else {
                    format!(", {loc}")
                },
                q.text,
                q.id
            ));
        }
        for b in ev.cites.iter().filter(|b| !b.status.renders()) {
            let what = if b.status == CiteStatus::Forged {
                b.token.clone()
            } else {
                format!("[{}]", b.n)
            };
            out.push_str(&format!("  - {what}: {}\n", b.status.label()));
        }
        if let Some(cs) = ev.counter.as_ref().filter(|c| c.ran) {
            out.push_str(&format!(
                "  - Counter-search ({}): “{}” — {}\n",
                cs.trigger.label(),
                cs.query,
                cs.note
            ));
        }
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(n: usize, site: &str, text: &str) -> SourceText {
        SourceText {
            n,
            url: format!("https://{site}/page"),
            title: format!("Source {n}"),
            site: site.into(),
            text: text.into(),
        }
    }

    #[test]
    fn ids_are_stable_and_tied_to_the_page() {
        let a = quote_id("https://a.test/x#frag", "The tower is  330 metres tall.");
        let b = quote_id("https://a.test/x", "the tower is 330 metres tall");
        assert_eq!(a, b, "case, spacing and fragment do not change the id");
        assert_eq!(a.0.len(), ID_HEX);
        assert_ne!(
            a,
            quote_id("https://b.test/x", "the tower is 330 metres tall")
        );
        assert_eq!(a.token(), format!("[[q:{}]]", a.0));
    }

    #[test]
    fn tokens_render_and_forged_ones_are_stripped() {
        let s = src(
            2,
            "a.test",
            "The tower is 330 metres tall. It opened in 1889.",
        );
        let mut book = QuoteBook::default();
        let id = book.mint(&s, 0, 29, false);
        let answer = format!(
            "It is 330 metres tall {}. It opened in 1889 [[q:zzzzzz]]. Code `[q:abc]` stays.",
            id.token()
        );
        let r = render(&answer, &book);
        assert_eq!(
            r.text,
            "It is 330 metres tall [2]. It opened in 1889. Code `[q:abc]` stays."
        );
        assert_eq!(r.used, vec![id]);
        assert_eq!(r.forged.len(), 1);
        assert_eq!(r.forged[0].1, "[[q:zzzzzz]]");
        assert_eq!(render("plain [1].", &book).text, "plain [1].");
        assert!(
            token_at("[q:ab12 · p. 4]", 0).is_none(),
            "a label is not a token"
        );
    }

    #[test]
    fn citations_bind_to_minted_quotes_or_are_flagged() {
        let sources = vec![
            src(
                1,
                "a.test",
                "The Eiffel Tower is 330 metres tall. It is in Paris.",
            ),
            src(2, "b.test", "Paris has many bakeries and museums."),
        ];
        let answer = "The Eiffel Tower is 330 metres tall [1][2][5].";
        let chain = build(answer, &sources, &QuoteBook::default(), &Budget::default());
        let ev = &chain.claims[0];
        let st: Vec<CiteStatus> = ev.cites.iter().map(|b| b.status).collect();
        assert_eq!(
            st,
            [CiteStatus::Bound, CiteStatus::Unbound, CiteStatus::Unread]
        );
        let q = chain.quote(ev.cites[0].quote.as_ref().unwrap()).unwrap();
        assert!(q.text.contains("330 metres"));
        assert_eq!(ev.stance, Stance::Supports);
        assert_eq!(ev.invalid_cites(), 1);
        assert_eq!(chain.unread_numbers(), BTreeSet::from([5]));
    }

    #[test]
    fn agreement_counts_sites_and_refutations() {
        let sources = vec![
            src(1, "a.test", "The Eiffel Tower is 330 metres tall."),
            src(
                2,
                "b.test",
                "Measured in 2022, the Eiffel Tower is 330 metres tall.",
            ),
            src(
                3,
                "c.test",
                "The Eiffel Tower is 300 metres tall, its original height.",
            ),
            src(
                4,
                "a.test",
                "Visitors say the Eiffel Tower is 330 metres tall.",
            ),
        ];
        let chain = build(
            "The Eiffel Tower is 330 metres tall [1].",
            &sources,
            &QuoteBook::default(),
            &Budget::default(),
        );
        let ev = &chain.claims[0];
        assert_eq!(ev.agreement.supports, 2, "{:?}", ev.cards);
        assert_eq!(ev.agreement.refutes, 1, "{:?}", ev.cards);
        assert_eq!(ev.stance, Stance::Mixed);
        assert_eq!(ev.agreement.line(), "2 support · 1 refutes · 3 sources");
    }

    #[test]
    fn counter_search_is_planned_for_key_claims_and_can_flip_a_claim() {
        let sources = vec![src(
            1,
            "a.test",
            "Coffee was first brewed in Yemen. Tea is older.",
        )];
        let answer = "Coffee was first brewed in Yemen [1]. Tea may be older [1].";
        let mut chain = build(answer, &sources, &QuoteBook::default(), &Budget::default());
        let planned = plan_counter(&mut chain, &Budget::default());
        assert_eq!(planned, vec![1], "the hedged claim gets none");
        let cs = chain.claims[0].counter.clone().unwrap();
        assert_eq!(cs.trigger, Trigger::Headline);
        assert!(
            cs.query.starts_with("Coffee first brewed Yemen"),
            "{}",
            cs.query
        );
        let against = vec![src(
            0,
            "c.test",
            "Coffee was first brewed in Ethiopia, historians say.",
        )];
        let d = add_counter(&mut chain, 1, &against);
        assert_eq!(d, 1);
        let ev = &chain.claims[0];
        assert_eq!(ev.stance, Stance::Mixed);
        assert!(ev.counter.as_ref().unwrap().ran);
        assert!(ev.cards.iter().any(|k| k.counter && k.source == 0));
        assert_eq!(
            trigger("It costs 5 dollars a month here.", false, Stance::Supports),
            Some(Trigger::Number)
        );
        assert_eq!(
            trigger("It is a nice tool for notes.", false, Stance::Supports),
            None
        );
        assert!(counter_query("Ice does not melt at 0 C.").ends_with(" evidence"));
    }

    #[test]
    fn a_judge_refines_uncited_cards_within_its_budget() {
        let sources = vec![
            src(1, "a.test", "The Eiffel Tower is 330 metres tall."),
            src(2, "b.test", "Today the Eiffel Tower is 330 metres tall."),
        ];
        let mut chain = build(
            "The Eiffel Tower is 330 metres tall [1].",
            &sources,
            &QuoteBook::default(),
            &Budget::default(),
        );
        let (_, msg, pairs) = stance_prompt(&chain, &Budget::default()).unwrap();
        assert_eq!(pairs.len(), 1, "only the uncited card");
        assert!(msg.contains("Pair 1"));
        let judged = parse_stances(r#"[{"pair": 1, "stance": "not_enough_info"}]"#);
        apply_stances(&mut chain, &pairs, &judged, "m");
        assert_eq!(chain.claims[0].agreement.supports, 1);
        assert_eq!(chain.judge.as_deref(), Some("m"));
        assert!(markdown(&chain).contains("## Evidence"));
    }
}
