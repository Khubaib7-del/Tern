//! Claim verdicts: does the cited source say what the answer says it does?
//!
//! | Verdict | Meaning |
//! | --- | --- |
//! | Supported | A passage of a cited source states the claim |
//! | Partly supported | A passage covers some of it, or its numbers differ |
//! | Not supported | No passage of the cited sources states it |
//! | No source | The claim cites nothing Tern read |
//!
//! Two paths reach a verdict. The lexical path is deterministic and
//! offline: it finds the passage of the cited sources that shares the most
//! of the claim's content words, and checks that every number in the claim
//! appears there. The model path (see [`judge_prompt`] and
//! [`parse_judgments`]) shows a model the best passages and reads its
//! verdict back; the front end runs it through its provider and falls back
//! to the lexical verdict for any claim the model does not answer.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::claims::{Claim, sentences};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Supported,
    Partly,
    NotSupported,
    NoSource,
}

impl Verdict {
    pub const ALL: [Verdict; 4] = [
        Self::Supported,
        Self::Partly,
        Self::NotSupported,
        Self::NoSource,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Supported => "Supported",
            Self::Partly => "Partly supported",
            Self::NotSupported => "Not supported",
            Self::NoSource => "No source",
        }
    }

    /// One character for a badge.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Supported => "✓",
            Self::Partly => "◐",
            Self::NotSupported => "✕",
            Self::NoSource => "○",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase().replace([' ', '-'], "_");
        Some(match s.as_str() {
            "supported" => Self::Supported,
            "partly" | "partly_supported" | "partial" | "partially_supported" => Self::Partly,
            "not_supported" | "unsupported" | "contradicted" => Self::NotSupported,
            "no_source" => Self::NoSource,
            _ => return None,
        })
    }
}

/// How a verdict was reached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    #[default]
    Lexical,
    Model,
}

/// The text of one source the answer could cite.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceText {
    /// The number the answer cites it by.
    pub n: usize,
    pub url: String,
    pub title: String,
    pub site: String,
    /// The text Tern read; empty when it could not be had.
    pub text: String,
}

/// The passage a verdict rests on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub source: usize,
    pub url: String,
    pub title: String,
    pub site: String,
    /// The passage, as the source has it, cut to about 300 characters.
    pub quote: String,
    /// A link straight to the passage: the page with a text fragment, or a
    /// PDF's page.
    pub anchor: String,
    /// The PDF page the passage is on.
    #[serde(default)]
    pub page: Option<u32>,
}

/// One claim and its verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimCheck {
    pub claim: usize,
    pub text: String,
    pub citations: Vec<usize>,
    pub verdict: Verdict,
    #[serde(default)]
    pub evidence: Option<Evidence>,
    /// Share of the claim's content words found in the passage, 0 to 100.
    pub overlap: u8,
    #[serde(default)]
    pub method: Method,
    /// Why, in a few words, when there is more to say than the verdict.
    #[serde(default)]
    pub note: String,
}

/// Counts per verdict.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    pub supported: usize,
    pub partly: usize,
    pub not_supported: usize,
    pub no_source: usize,
}

impl Tally {
    pub fn of(checks: &[ClaimCheck]) -> Self {
        let mut t = Self::default();
        for c in checks {
            *t.slot(c.verdict) += 1;
        }
        t
    }

    fn slot(&mut self, v: Verdict) -> &mut usize {
        match v {
            Verdict::Supported => &mut self.supported,
            Verdict::Partly => &mut self.partly,
            Verdict::NotSupported => &mut self.not_supported,
            Verdict::NoSource => &mut self.no_source,
        }
    }

    pub fn get(&self, v: Verdict) -> usize {
        match v {
            Verdict::Supported => self.supported,
            Verdict::Partly => self.partly,
            Verdict::NotSupported => self.not_supported,
            Verdict::NoSource => self.no_source,
        }
    }

    pub fn total(&self) -> usize {
        self.supported + self.partly + self.not_supported + self.no_source
    }

    /// "3 supported · 1 partly · 1 not supported", empty parts left out.
    pub fn line(&self) -> String {
        let parts: Vec<String> = [
            (self.supported, "supported"),
            (self.partly, "partly"),
            (self.not_supported, "not supported"),
            (self.no_source, "no source"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, w)| format!("{n} {w}"))
        .collect();
        parts.join(" · ")
    }
}

/// Overlap at or above this is Supported (all numbers present).
pub const SUPPORTED_AT: u8 = 75;
/// Overlap at or above this is Partly supported.
pub const PARTLY_AT: u8 = 45;
/// Longest quote kept, in characters.
pub const QUOTE_CHARS: usize = 300;

const STOP: &[&str] = &[
    "the", "and", "for", "are", "was", "were", "has", "have", "had", "that", "this", "these",
    "those", "with", "from", "into", "onto", "its", "it's", "their", "they", "them", "than",
    "then", "which", "who", "whom", "what", "when", "where", "while", "also", "can", "could",
    "may", "might", "will", "would", "should", "been", "being", "but", "not", "all", "any", "each",
    "such", "more", "most", "other", "some", "about", "over", "under", "between", "both", "only",
    "very", "via", "per", "our", "you", "your", "his", "her", "she", "him", "one", "there", "here",
    "does", "did", "did", "how", "why", "because", "using", "used", "use", "many", "much",
];

/// A word reduced for matching: lower case, common endings taken off.
fn stem(w: &str) -> String {
    let w = w.to_lowercase();
    if w.chars().any(|c| c.is_ascii_digit()) {
        return w;
    }
    for suffix in ["ing", "ed", "es", "ly", "s"] {
        if w.len() > suffix.len() + 3
            && let Some(base) = w.strip_suffix(suffix)
        {
            return base.to_owned();
        }
    }
    w
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '.' || c == '%'))
        .map(|w| w.trim_matches('.'))
        .filter(|w| !w.is_empty())
}

/// The claim's content words: stems of words that carry meaning.
pub fn content_words(text: &str) -> BTreeSet<String> {
    words(text)
        .filter(|w| {
            let lower = w.to_lowercase();
            w.chars().any(|c| c.is_ascii_digit())
                || (w.chars().count() >= 3 && !STOP.contains(&lower.as_str()))
        })
        .map(stem)
        .collect()
}

/// Whether a text says "not", "never", "no"...
fn negated(text: &str) -> bool {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .any(|w| {
            let w = w.to_lowercase();
            matches!(
                w.as_str(),
                "not" | "no" | "never" | "neither" | "nor" | "none" | "cannot" | "without"
            ) || w.ends_with("n't")
        })
}

/// Numbers in a text, normalised (`1,200` and `1200` match).
fn numbers(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .map(|w| w.trim_matches(['.', ',']).replace(',', ""))
        .filter(|w| !w.is_empty() && w.chars().any(|c| c.is_ascii_digit()))
        .collect()
}

/// One stretch of a source: one or two neighbouring sentences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Passage {
    pub source: usize,
    pub start: usize,
    pub end: usize,
    pub overlap: u8,
    /// Every number in the claim is in the passage.
    pub numbers_match: bool,
    /// The claim and the passage are both negated or both not.
    pub polarity_match: bool,
}

/// Passages of `sources` scored against `claim`, best first.
pub fn passages(claim: &str, sources: &[&SourceText]) -> Vec<Passage> {
    let want = content_words(claim);
    let want_numbers = numbers(claim);
    let want_negated = negated(claim);
    let mut out = Vec::new();
    for s in sources {
        let spans = sentences(&s.text);
        for (k, &(a, b)) in spans.iter().enumerate() {
            let mut windows = vec![(a, b)];
            if let Some(&(_, c)) = spans.get(k + 1) {
                windows.push((a, c));
            }
            for (a, b) in windows {
                let text = &s.text[a..b];
                if text.len() > 1600 {
                    continue;
                }
                let have = content_words(text);
                let hit = want.intersection(&have).count();
                let overlap = if want.is_empty() {
                    0
                } else {
                    (hit * 100 / want.len()) as u8
                };
                let have_numbers = numbers(text);
                out.push(Passage {
                    source: s.n,
                    start: a,
                    end: b,
                    overlap,
                    numbers_match: want_numbers.is_subset(&have_numbers),
                    polarity_match: negated(text) == want_negated,
                });
            }
        }
    }
    // Best overlap first; among equals the shorter passage, then the earlier.
    out.sort_by(|x, y| {
        y.overlap
            .cmp(&x.overlap)
            .then(y.numbers_match.cmp(&x.numbers_match))
            .then((x.end - x.start).cmp(&(y.end - y.start)))
            .then(x.source.cmp(&y.source))
            .then(x.start.cmp(&y.start))
    });
    out
}

/// The verdict the lexical path gives a passage.
pub fn lexical_verdict(p: &Passage) -> Verdict {
    if p.overlap >= SUPPORTED_AT && p.numbers_match && p.polarity_match {
        Verdict::Supported
    } else if p.overlap >= PARTLY_AT {
        Verdict::Partly
    } else {
        Verdict::NotSupported
    }
}

/// The sources a claim cites, in its citation order.
fn cited<'a>(claim: &Claim, sources: &'a [SourceText]) -> Vec<&'a SourceText> {
    claim
        .citations
        .iter()
        .filter_map(|n| sources.iter().find(|s| s.n == *n))
        .collect()
}

/// Checks one claim against its cited sources, offline.
pub fn check_lexical(claim: &Claim, sources: &[SourceText]) -> ClaimCheck {
    let cited = cited(claim, sources);
    let readable: Vec<&SourceText> = cited
        .iter()
        .copied()
        .filter(|s| !s.text.trim().is_empty())
        .collect();
    let mut check = ClaimCheck {
        claim: claim.n,
        text: claim.text.clone(),
        citations: claim.citations.clone(),
        verdict: Verdict::NoSource,
        evidence: None,
        overlap: 0,
        method: Method::Lexical,
        note: String::new(),
    };
    if readable.is_empty() {
        check.note = if cited.is_empty() {
            format!(
                "Cites {} but Tern read no such source",
                cite_marks(&claim.citations)
            )
        } else {
            "The cited page's text could not be read again".into()
        };
        return check;
    }
    let Some(best) = passages(&claim.text, &readable).into_iter().next() else {
        check.verdict = Verdict::NotSupported;
        check.note = "The cited source has no text to compare".into();
        return check;
    };
    check.verdict = lexical_verdict(&best);
    check.overlap = best.overlap;
    if check.verdict == Verdict::Partly && best.overlap >= SUPPORTED_AT {
        check.note = if !best.numbers_match {
            "A number in the claim is not in the passage".into()
        } else {
            "The passage may say the opposite".into()
        };
    }
    if check.verdict != Verdict::NotSupported {
        check.evidence = Some(evidence_of(&best, &readable));
    } else {
        check.note = "No passage of the cited source states this".into();
    }
    check
}

fn cite_marks(ns: &[usize]) -> String {
    ns.iter().map(|n| format!("[{n}]")).collect()
}

/// The evidence for a passage.
pub fn evidence_of(p: &Passage, sources: &[&SourceText]) -> Evidence {
    let s = sources
        .iter()
        .find(|s| s.n == p.source)
        .expect("passage from a given source");
    let quote = clip(&without_page_marks(&s.text[p.start..p.end]), QUOTE_CHARS);
    let page = page_at(&s.text, p.start);
    Evidence {
        source: s.n,
        url: s.url.clone(),
        title: s.title.clone(),
        site: s.site.clone(),
        anchor: anchor(&s.url, &quote, page),
        quote,
        page,
    }
}

/// Text cut to `max` characters at a word, with an ellipsis.
fn clip(text: &str, max: usize) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= max {
        return t;
    }
    let cut: String = t.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map(|(a, _)| a).unwrap_or(&cut);
    format!("{}…", cut.trim_end_matches([',', ';', ':']))
}

/// Text without the PDF reader's `[page N]` markers.
fn without_page_marks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("[page ") {
        out.push_str(&rest[..i]);
        match rest[i..].find(']') {
            Some(j) => rest = &rest[i + j + 1..],
            None => {
                rest = &rest[i..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The `[page N]` marker the PDF reader put before `at`, if any.
fn page_at(text: &str, at: usize) -> Option<u32> {
    let i = if text[at..].starts_with("[page ") {
        at
    } else {
        text[..at].rfind("[page ")?
    };
    let rest = &text[i + 6..];
    let end = rest.find(']')?;
    rest[..end].trim().parse().ok()
}

/// A link to the passage: `#page=N` for a PDF page, else a text fragment
/// (`#:~:text=start,end`) that browsers scroll to and highlight.
pub fn anchor(url: &str, quote: &str, page: Option<u32>) -> String {
    let base = url.split('#').next().unwrap_or(url);
    if let Some(n) = page {
        return format!("{base}#page={n}");
    }
    let words: Vec<&str> = quote
        .trim_end_matches('…')
        .split_whitespace()
        .filter(|w| !w.contains('['))
        .collect();
    if words.is_empty() {
        return base.to_owned();
    }
    let fragment = if words.len() <= 8 {
        encode(&words.join(" "))
    } else {
        format!(
            "{},{}",
            encode(&words[..4].join(" ")),
            encode(&words[words.len() - 4..].join(" "))
        )
    };
    format!("{base}#:~:text={fragment}")
}

/// Percent-encoding for a text fragment: only letters and digits pass.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Candidate passages per claim shown to the model judge.
pub const JUDGE_PASSAGES: usize = 3;

const JUDGE_SYSTEM: &str = "You check claims against the passages of the sources they cite. \
For each claim, decide one verdict: \"supported\" if a passage states the claim, \"partly\" if a passage states only part of it or with different details or numbers, \"not_supported\" if no passage states it or a passage contradicts it. \
Judge only from the passages given, never from what you know. \
Answer with JSON only: an array of objects {\"claim\": number, \"verdict\": \"supported\" | \"partly\" | \"not_supported\", \"quote\": the exact words of the passage that decide it (empty when none)}.";

/// The system prompt and the message for a model judge over `claims`, each
/// with its best passages. Claims with no readable source are left out:
/// they are No source whatever a model says.
pub fn judge_prompt(claims: &[Claim], sources: &[SourceText]) -> Option<(String, String)> {
    let mut msg = String::new();
    for c in claims {
        let readable: Vec<&SourceText> = cited(c, sources)
            .into_iter()
            .filter(|s| !s.text.trim().is_empty())
            .collect();
        if readable.is_empty() {
            continue;
        }
        msg.push_str(&format!("Claim {}: {}\n", c.n, c.text));
        for (k, p) in passages(&c.text, &readable)
            .into_iter()
            .take(JUDGE_PASSAGES)
            .enumerate()
        {
            let s = readable
                .iter()
                .find(|s| s.n == p.source)
                .expect("passage from a given source");
            msg.push_str(&format!(
                "  Passage {}.{} (source [{}], {}): {}\n",
                c.n,
                k + 1,
                s.n,
                s.site,
                clip(&s.text[p.start..p.end], 600)
            ));
        }
        msg.push('\n');
    }
    (!msg.is_empty()).then(|| (JUDGE_SYSTEM.to_owned(), msg))
}

/// One verdict as a model gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judgment {
    pub claim: usize,
    pub verdict: Verdict,
    pub quote: String,
}

/// Reads a judge's reply: a JSON array (possibly fenced, possibly with
/// prose around it). Entries it cannot read are skipped.
pub fn parse_judgments(reply: &str) -> Vec<Judgment> {
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
            let claim = v.get("claim")?.as_u64()? as usize;
            let verdict = Verdict::parse(v.get("verdict")?.as_str()?)?;
            if verdict == Verdict::NoSource {
                return None;
            }
            let quote = v
                .get("quote")
                .and_then(|q| q.as_str())
                .unwrap_or_default()
                .trim()
                .to_owned();
            Some(Judgment {
                claim,
                verdict,
                quote,
            })
        })
        .collect()
}

/// Folds a model's judgments into lexical checks. The model's verdict
/// wins for the claims it answered; its quote becomes the evidence when it
/// is really in a cited source, otherwise the lexical passage stays.
pub fn apply_judgments(checks: &mut [ClaimCheck], judgments: &[Judgment], sources: &[SourceText]) {
    for j in judgments {
        let Some(check) = checks.iter_mut().find(|c| c.claim == j.claim) else {
            continue;
        };
        if check.verdict == Verdict::NoSource {
            continue;
        }
        check.verdict = j.verdict;
        check.method = Method::Model;
        check.note.clear();
        let cited: Vec<&SourceText> = check
            .citations
            .iter()
            .filter_map(|n| sources.iter().find(|s| s.n == *n))
            .collect();
        if let Some(ev) = find_quote(&j.quote, &cited) {
            check.evidence = Some(ev);
        } else if j.verdict == Verdict::NotSupported {
            check.evidence = None;
            check.note = "No passage of the cited source states this".into();
        } else if check.evidence.is_none()
            && let Some(best) = passages(&check.text, &cited).into_iter().next()
        {
            check.evidence = Some(evidence_of(&best, &cited));
        }
    }
}

/// Where `quote` sits in one of `sources`, compared without case or
/// spacing differences.
fn find_quote(quote: &str, sources: &[&SourceText]) -> Option<Evidence> {
    let q = squash(quote.trim_matches(['"', '…', ' ']));
    if q.chars().count() < 12 {
        return None;
    }
    for s in sources {
        let (flat, map) = squash_with_map(&s.text);
        if let Some(at) = flat.find(&q) {
            let start = map[at];
            let end = map[(at + q.len() - 1).min(map.len() - 1)] + 1;
            let end = (end..=s.text.len())
                .find(|&i| s.text.is_char_boundary(i))
                .unwrap_or(s.text.len());
            let p = Passage {
                source: s.n,
                start,
                end,
                overlap: 100,
                numbers_match: true,
                polarity_match: true,
            };
            return Some(evidence_of(&p, sources));
        }
    }
    None
}

fn squash(s: &str) -> String {
    squash_with_map(s).0
}

/// Lower case with whitespace runs made one space, and for each byte of
/// the result the byte of `s` it came from.
fn squash_with_map(s: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(s.len());
    let mut map = Vec::with_capacity(s.len());
    let mut space = false;
    for (i, c) in s.char_indices() {
        if c.is_whitespace() {
            if !space && !out.is_empty() {
                out.push(' ');
                map.push(i);
            }
            space = true;
            continue;
        }
        space = false;
        for l in c.to_lowercase() {
            let before = out.len();
            out.push(l);
            map.extend(std::iter::repeat_n(i, out.len() - before));
        }
    }
    (out, map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claims::extract;

    fn src(n: usize, text: &str) -> SourceText {
        SourceText {
            n,
            url: format!("https://s{n}.test/page"),
            title: format!("Source {n}"),
            site: format!("s{n}.test"),
            text: text.into(),
        }
    }

    /// Seeded fixtures: supported, misquoted number, contradicted, unread.
    fn fixtures() -> Vec<SourceText> {
        vec![
            src(
                1,
                "Intro text. The Transformer architecture was introduced in 2017 by researchers at Google Brain. It relies entirely on attention mechanisms.",
            ),
            src(
                2,
                "The Eiffel Tower is 330 metres tall and was completed in 1889 for the World's Fair.",
            ),
            src(
                3,
                "[page 1] Cover\n[page 4] Honey bees communicate the location of flowers through a waggle dance.",
            ),
            src(4, ""),
        ]
    }

    fn verdicts(answer: &str) -> Vec<(Verdict, u8)> {
        let s = fixtures();
        extract(answer)
            .iter()
            .map(|c| {
                let k = check_lexical(c, &s);
                (k.verdict, k.overlap)
            })
            .collect()
    }

    #[test]
    fn seeded_verdicts() {
        let v = verdicts(
            "The Transformer architecture was introduced by Google Brain researchers in 2017 [1]. \
The Eiffel Tower is 300 metres tall [2]. \
Penguins migrate across deserts every winter [2]. \
Bees share where flowers are with a waggle dance [3]. \
The moon is made of rock [4]. \
Rust is memory safe [9].",
        );
        let only: Vec<Verdict> = v.iter().map(|x| x.0).collect();
        assert_eq!(
            only,
            [
                Verdict::Supported,
                Verdict::Partly,
                Verdict::NotSupported,
                Verdict::Supported,
                Verdict::NoSource,
                Verdict::NoSource,
            ],
            "{v:?}"
        );
    }

    #[test]
    fn evidence_quotes_and_anchors() {
        let s = fixtures();
        let c =
            &extract("The Transformer was introduced in 2017 by Google Brain researchers [1].")[0];
        let k = check_lexical(c, &s);
        let ev = k.evidence.unwrap();
        assert_eq!(
            ev.quote,
            "The Transformer architecture was introduced in 2017 by researchers at Google Brain."
        );
        assert!(
            ev.anchor.starts_with(
                "https://s1.test/page#:~:text=The%20Transformer%20architecture%20was,"
            )
        );
        let c = &extract("Honey bees communicate flower locations through a waggle dance [3].")[0];
        let ev = check_lexical(c, &s).evidence.unwrap();
        assert_eq!(ev.page, Some(4));
        assert_eq!(ev.anchor, "https://s3.test/page#page=4");
    }

    #[test]
    fn notes_say_why() {
        let s = fixtures();
        let c = &extract("The Eiffel Tower is 300 metres tall and was completed in 1889 [2].")[0];
        let k = check_lexical(c, &s);
        assert_eq!(k.verdict, Verdict::Partly);
        assert_eq!(k.note, "A number in the claim is not in the passage");
        let c = &extract("Rust is memory safe [9].")[0];
        assert!(check_lexical(c, &s).note.contains("[9]"));
        // Contradicted: the source says the opposite.
        let c = &extract("The Eiffel Tower was not completed in 1889 [2].")[0];
        let k = check_lexical(c, &s);
        assert_eq!(k.verdict, Verdict::Partly);
        assert_eq!(k.note, "The passage may say the opposite");
    }

    #[test]
    fn tally_line() {
        let s = fixtures();
        let checks: Vec<_> = extract("Transformers were introduced in 2017 by Google Brain researchers [1]. Cats fly [2]. X is y [7].")
            .iter()
            .map(|c| check_lexical(c, &s))
            .collect();
        let t = Tally::of(&checks);
        assert_eq!(t.total(), 3);
        assert_eq!(t.line(), "1 supported · 1 not supported · 1 no source");
    }

    #[test]
    fn judge_round_trip() {
        let s = fixtures();
        let claims =
            extract("The Eiffel Tower was finished in 1889 [2]. Penguins fly [2]. Moon rock [4].");
        let (_, msg) = judge_prompt(&claims, &s).unwrap();
        assert!(msg.contains("Claim 1: The Eiffel Tower was finished in 1889."));
        assert!(msg.contains("Passage 1.1 (source [2], s2.test)"));
        assert!(
            !msg.contains("Claim 3"),
            "unreadable sources are not judged"
        );
        let reply = "Here you go:\n```json\n[{\"claim\": 1, \"verdict\": \"supported\", \"quote\": \"was completed in 1889 for the World's Fair\"},\n {\"claim\": 2, \"verdict\": \"not_supported\", \"quote\": \"\"}, {\"claim\": 3, \"verdict\": \"supported\"}, {\"claim\": 9, \"verdict\": \"maybe\"}]\n```";
        let j = parse_judgments(reply);
        assert_eq!(j.len(), 3);
        let mut checks: Vec<_> = claims.iter().map(|c| check_lexical(c, &s)).collect();
        apply_judgments(&mut checks, &j, &s);
        assert_eq!(checks[0].verdict, Verdict::Supported);
        assert_eq!(checks[0].method, Method::Model);
        assert_eq!(
            checks[0].evidence.as_ref().unwrap().quote,
            "was completed in 1889 for the World's Fair"
        );
        assert_eq!(checks[1].verdict, Verdict::NotSupported);
        assert!(checks[1].evidence.is_none());
        assert_eq!(checks[2].verdict, Verdict::NoSource, "no source stays");
        assert!(parse_judgments("no json").is_empty());
    }
}
