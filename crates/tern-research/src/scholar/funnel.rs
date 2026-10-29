//! The rerank funnel: candidates from every service, merged, scored on
//! relevance (BM25 over title and abstract or passage text, and how many
//! of the question's key terms they hold) blended 80/20 with quality
//! (citations, recency, an open-access copy), then a relevance gate.
//! When nothing passes the gate the answer is "not enough evidence",
//! never a guess.
//!
//! The numbers are starting points from published scholarly pipelines
//! (an 80/20 relevance/quality blend, a gate before writing), not tuned
//! for Tern.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::Paper;

/// Words that say nothing about a topic.
const STOP: &[&str] = &[
    "a", "about", "above", "after", "again", "all", "also", "am", "an", "and", "any", "are", "as",
    "at", "be", "been", "being", "between", "both", "but", "by", "can", "could", "did", "do",
    "does", "doing", "during", "each", "effect", "effects", "for", "from", "further", "had", "has",
    "have", "having", "how", "however", "i", "if", "in", "into", "is", "it", "its", "itself",
    "just", "may", "me", "might", "more", "most", "much", "must", "my", "no", "nor", "not", "of",
    "on", "once", "only", "or", "other", "our", "out", "over", "own", "paper", "papers",
    "research", "same", "shall", "should", "show", "shows", "so", "some", "studies", "study",
    "such", "than", "that", "the", "their", "them", "then", "there", "these", "they", "this",
    "those", "through", "to", "too", "under", "until", "up", "use", "used", "using", "very", "was",
    "we", "were", "what", "when", "where", "which", "while", "who", "whom", "why", "will", "with",
    "would", "you", "your", "does", "evidence", "find", "found", "known", "new",
];

/// A word reduced for matching: lower case, a plural `s` dropped.
fn stem(w: &str) -> String {
    let w = w.to_lowercase();
    let n = w.chars().count();
    if n > 4 && w.ends_with("ies") {
        return format!("{}y", &w[..w.len() - 3]);
    }
    if n > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") && !w.ends_with("is") {
        return w[..w.len() - 1].to_owned();
    }
    w
}

/// The words of `text`, stemmed, in order.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(stem)
        .collect()
}

/// The words of a question that carry its topic: no stop words, at least
/// three letters (or two with a digit, like `5g`), each once.
pub fn key_terms(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| {
            let digit = w.chars().any(|c| c.is_ascii_digit());
            (w.chars().count() >= 3 || (digit && w.chars().count() >= 2))
                && !STOP.contains(&w.as_str())
        })
        .map(|w| stem(&w))
        .filter(|w| !STOP.contains(&w.as_str()))
        .filter(|w| seen.insert(w.clone()))
        .collect()
}

/// Share of `terms` that appear in `text` (0 when there are no terms).
pub fn coverage(terms: &[String], text: &str) -> f32 {
    if terms.is_empty() {
        return 0.0;
    }
    let have: BTreeSet<String> = words(text).into_iter().collect();
    terms.iter().filter(|t| have.contains(*t)).count() as f32 / terms.len() as f32
}

/// BM25 scores of `docs` for `terms` (k1 1.2, b 0.75), over the docs as
/// their own corpus.
pub fn bm25(terms: &[String], docs: &[String]) -> Vec<f32> {
    let toks: Vec<Vec<String>> = docs.iter().map(|d| words(d)).collect();
    let n = toks.len().max(1) as f32;
    let avg = (toks.iter().map(Vec::len).sum::<usize>() as f32 / n).max(1.0);
    let mut df: BTreeMap<&str, f32> = BTreeMap::new();
    for t in terms {
        let k = toks.iter().filter(|d| d.contains(t)).count() as f32;
        df.insert(t.as_str(), k);
    }
    toks.iter()
        .map(|d| {
            let len = d.len() as f32;
            terms
                .iter()
                .map(|t| {
                    let f = d.iter().filter(|w| *w == t).count() as f32;
                    if f == 0.0 {
                        return 0.0;
                    }
                    let k = df.get(t.as_str()).copied().unwrap_or(0.0);
                    let idf = ((n - k + 0.5) / (k + 0.5) + 1.0).ln();
                    idf * f * 2.2 / (f + 1.2 * (0.25 + 0.75 * len / avg))
                })
                .sum()
        })
        .collect()
}

/// The funnel's settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunnelConfig {
    /// Weight of relevance in the blend (quality gets the rest).
    pub relevance_weight: f32,
    /// The least share of the question's key terms a candidate must hold
    /// to pass the gate.
    pub min_coverage: f32,
    /// How many candidates are kept after the gate.
    pub keep: usize,
    /// The year recency is measured from (the caller owns the clock).
    pub this_year: i32,
}

impl Default for FunnelConfig {
    fn default() -> Self {
        Self {
            relevance_weight: 0.8,
            min_coverage: 0.6,
            keep: 10,
            this_year: 2026,
        }
    }
}

/// One candidate's scores.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scored {
    /// Index into the candidates given.
    pub index: usize,
    /// 0..1: BM25 (scaled to the best candidate) and key-term coverage.
    pub relevance: f32,
    /// 0..1: citations, recency, open access.
    pub quality: f32,
    pub score: f32,
    pub coverage: f32,
    /// Passed the relevance gate.
    pub passed: bool,
}

/// Whether what was found can answer the question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", content = "reason", rename_all = "snake_case")]
pub enum Sufficiency {
    Enough,
    /// Not enough evidence, and why, in words the model passes on.
    NotEnough(String),
}

impl Sufficiency {
    pub fn is_enough(&self) -> bool {
        matches!(self, Self::Enough)
    }
}

/// The funnel's result: every candidate scored, best first, and the
/// verdict.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ranked {
    pub scored: Vec<Scored>,
    pub sufficiency: Sufficiency,
    pub terms: Vec<String>,
}

impl Ranked {
    /// The indices that passed the gate, best first, up to `keep`.
    pub fn kept(&self) -> Vec<usize> {
        self.scored
            .iter()
            .filter(|s| s.passed)
            .map(|s| s.index)
            .collect()
    }
}

/// A paper's quality, 0..1: half citations (log-scaled to the most cited
/// candidate), 0.3 recency (ten years to zero), 0.2 an open-access copy.
/// A retracted paper has none.
pub fn quality(p: &Paper, max_cites: u64, this_year: i32) -> f32 {
    if p.retracted {
        return 0.0;
    }
    let cites = match (p.citations, max_cites) {
        (Some(c), m) if m > 0 => ((1.0 + c as f32).ln() / (1.0 + m as f32).ln()).min(1.0),
        _ => 0.0,
    };
    let recent = p
        .year
        .map(|y| (1.0 - (this_year - y).max(0) as f32 / 10.0).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    let oa = if p.oa_pdf.is_some() { 1.0 } else { 0.0 };
    0.5 * cites + 0.3 * recent + 0.2 * oa
}

/// Runs the funnel over `papers` with `texts` as what each is judged on
/// (title and abstract, or a passage), for `question`.
pub fn rank_texts(
    question: &str,
    papers: &[&Paper],
    texts: &[String],
    cfg: &FunnelConfig,
) -> Ranked {
    let terms = key_terms(question);
    let raw = bm25(&terms, texts);
    let top = raw.iter().copied().fold(0.0_f32, f32::max);
    let max_cites = papers.iter().filter_map(|p| p.citations).max().unwrap_or(0);
    let mut scored: Vec<Scored> = papers
        .iter()
        .zip(texts)
        .enumerate()
        .map(|(i, (p, t))| {
            let cov = coverage(&terms, t);
            let b = if top > 0.0 { raw[i] / top } else { 0.0 };
            let relevance = 0.6 * b + 0.4 * cov;
            let q = quality(p, max_cites, cfg.this_year);
            let score = cfg.relevance_weight * relevance + (1.0 - cfg.relevance_weight) * q;
            Scored {
                index: i,
                relevance,
                quality: q,
                score,
                coverage: cov,
                passed: !p.retracted && cov + 1e-6 >= cfg.min_coverage && raw[i] > 0.0,
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.index.cmp(&b.index))
    });
    let mut kept = 0;
    for s in &mut scored {
        if s.passed {
            kept += 1;
            if kept > cfg.keep {
                s.passed = false;
            }
        }
    }
    let sufficiency = if terms.is_empty() {
        Sufficiency::NotEnough("the question has no searchable terms".into())
    } else if papers.is_empty() {
        Sufficiency::NotEnough("the scholarly services returned no papers".into())
    } else if kept == 0 {
        let best = scored.first().map(|s| s.coverage).unwrap_or(0.0);
        Sufficiency::NotEnough(format!(
            "none of the {} papers found is about all of it: the best holds {:.0}% of the key terms ({})",
            papers.len(),
            best * 100.0,
            terms.join(", ")
        ))
    } else {
        Sufficiency::Enough
    };
    Ranked {
        scored,
        sufficiency,
        terms,
    }
}

/// Runs the funnel over papers on their title and abstract.
pub fn rank(question: &str, papers: &[Paper], cfg: &FunnelConfig) -> Ranked {
    let refs: Vec<&Paper> = papers.iter().collect();
    let texts: Vec<String> = papers.iter().map(Paper::text).collect();
    rank_texts(question, &refs, &texts, cfg)
}

/// Whether one claim is supported by any of `passages`: the passage that
/// holds the most of the claim's key terms, and the verdict. A claim is
/// only "enough" when one passage holds at least `min` of its terms.
pub fn support(claim: &str, passages: &[String], min: f32) -> (Option<usize>, Sufficiency) {
    let terms = key_terms(claim);
    if terms.is_empty() {
        return (
            None,
            Sufficiency::NotEnough("the claim has no checkable terms".into()),
        );
    }
    let best = passages
        .iter()
        .enumerate()
        .map(|(i, p)| (i, coverage(&terms, p)))
        .max_by(|a, b| {
            a.1.partial_cmp(&b.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.0.cmp(&a.0))
        });
    match best {
        Some((i, c)) if c + 1e-6 >= min => (Some(i), Sufficiency::Enough),
        Some((i, c)) => (
            Some(i),
            Sufficiency::NotEnough(format!(
                "the closest passage holds {:.0}% of the claim's key terms",
                c * 100.0
            )),
        ),
        None => (
            None,
            Sufficiency::NotEnough("there are no passages to check it against".into()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper(title: &str, abs: &str, cites: u64, year: i32) -> Paper {
        Paper {
            title: title.into(),
            abstract_text: Some(abs.into()),
            citations: Some(cites),
            year: Some(year),
            ..Paper::default()
        }
    }

    #[test]
    fn terms_drop_stop_words_and_plurals() {
        assert_eq!(
            key_terms("What are the effects of transformers on protein folding?"),
            vec!["transformer", "protein", "folding"]
        );
        assert_eq!(key_terms("5G and the studies"), vec!["5g"]);
        assert_eq!(stem("studies"), "study");
        assert_eq!(stem("class"), "class");
    }

    #[test]
    fn relevant_papers_outrank_famous_off_topic_ones() {
        let papers = vec![
            paper(
                "Deep residual learning for image recognition",
                "Residual networks ease training of deep networks.",
                200_000,
                2016,
            ),
            paper(
                "Protein structure prediction with transformers",
                "A transformer predicts protein folding from sequence.",
                300,
                2024,
            ),
            paper(
                "Transformers for language",
                "Attention models for translation.",
                90_000,
                2017,
            ),
        ];
        let r = rank(
            "transformer protein folding",
            &papers,
            &FunnelConfig::default(),
        );
        assert_eq!(r.scored[0].index, 1);
        assert!(r.sufficiency.is_enough());
        assert_eq!(r.kept(), vec![1]);
    }

    #[test]
    fn nothing_on_topic_is_not_enough_evidence() {
        let papers = vec![paper(
            "Deep residual learning",
            "Residual networks for images.",
            10,
            2016,
        )];
        let r = rank("octopus sleep cycles", &papers, &FunnelConfig::default());
        assert!(!r.sufficiency.is_enough());
        assert!(r.kept().is_empty());
        let r = rank("octopus sleep", &[], &FunnelConfig::default());
        assert!(matches!(r.sufficiency, Sufficiency::NotEnough(_)));
    }

    #[test]
    fn retracted_papers_never_pass() {
        let mut p = paper(
            "Octopus sleep cycles",
            "Octopus sleep has two stages.",
            5,
            2023,
        );
        p.retracted = true;
        let r = rank("octopus sleep cycles", &[p], &FunnelConfig::default());
        assert!(!r.sufficiency.is_enough());
    }

    #[test]
    fn claim_support_needs_one_passage_with_most_terms() {
        let passages = vec![
            "Octopuses show two sleep stages, quiet and active.".to_owned(),
            "Cuttlefish change colour.".to_owned(),
        ];
        let (i, s) = support("Octopuses have two sleep stages", &passages, 0.6);
        assert_eq!(i, Some(0));
        assert!(s.is_enough());
        let (_, s) = support("Octopuses dream about crabs", &passages, 0.6);
        assert!(!s.is_enough());
    }
}
