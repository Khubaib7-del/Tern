//! Words for the local scorer: lower case, common endings off, stop words out.

use std::collections::{BTreeMap, BTreeSet};

const STOP: &[&str] = &[
    "the", "and", "for", "are", "was", "were", "has", "have", "had", "that", "this", "these",
    "those", "with", "from", "into", "onto", "its", "their", "they", "them", "than", "then",
    "which", "who", "whom", "what", "when", "where", "while", "also", "can", "could", "may",
    "might", "will", "would", "should", "been", "being", "but", "all", "any", "each", "such",
    "more", "most", "other", "some", "about", "over", "under", "between", "both", "only", "very",
    "via", "per", "our", "you", "your", "his", "her", "she", "him", "one", "there", "here", "does",
    "did", "how", "why", "because", "using", "used", "use", "many", "much", "is", "a", "an", "of",
    "to", "in", "on", "at", "by", "or", "as", "be", "it", "do", "me", "my", "we", "i", "so", "if",
];

/// A word reduced for matching.
pub fn stem(w: &str) -> String {
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

fn raw_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '.' || c == '%'))
        .map(|w| w.trim_matches('.'))
        .filter(|w| !w.is_empty())
}

/// Content words of `text`, stemmed, in order, repeats kept.
pub fn terms(text: &str) -> Vec<String> {
    raw_words(text)
        .filter(|w| {
            let lower = w.to_lowercase();
            w.chars().any(|c| c.is_ascii_digit())
                || (w.chars().count() >= 3 && !STOP.contains(&lower.as_str()))
        })
        .map(stem)
        .collect()
}

pub fn term_set(text: &str) -> BTreeSet<String> {
    terms(text).into_iter().collect()
}

/// Numbers in a text, normalised (`1,200` and `1200` match).
pub fn numbers(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .map(|w| w.trim_matches(['.', ',']).replace(',', ""))
        .filter(|w| !w.is_empty() && w.chars().any(|c| c.is_ascii_digit()))
        .collect()
}

/// Whether a text says "not", "never", "no"...
pub fn negated(text: &str) -> bool {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .any(|w| {
            let w = w.to_lowercase();
            matches!(
                w.as_str(),
                "not" | "no" | "never" | "neither" | "nor" | "none" | "cannot" | "without"
            ) || w.ends_with("n't")
        })
}

/// Okapi BM25 of `query` against each of `docs`, the docs being the corpus.
pub fn bm25(query: &[String], docs: &[Vec<String>]) -> Vec<f32> {
    const K1: f32 = 1.2;
    const B: f32 = 0.75;
    let n = docs.len().max(1) as f32;
    let avg = (docs.iter().map(Vec::len).sum::<usize>() as f32 / n).max(1.0);
    let mut df: BTreeMap<&str, usize> = BTreeMap::new();
    for d in docs {
        let set: BTreeSet<&str> = d.iter().map(String::as_str).collect();
        for t in set {
            *df.entry(t).or_default() += 1;
        }
    }
    let q: BTreeSet<&str> = query.iter().map(String::as_str).collect();
    docs.iter()
        .map(|d| {
            let len = d.len() as f32;
            q.iter()
                .map(|t| {
                    let f = d.iter().filter(|w| w.as_str() == *t).count() as f32;
                    if f == 0.0 {
                        return 0.0;
                    }
                    let df = *df.get(t).unwrap_or(&0) as f32;
                    // The "plus" variant: never negative, even for a term
                    // every doc has.
                    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                    idf * f * (K1 + 1.0) / (f + K1 * (1.0 - B + B * len / avg))
                })
                .sum()
        })
        .collect()
}

/// Share of `want` found in `have`, 0 to 1.
pub fn coverage(want: &BTreeSet<String>, have: &BTreeSet<String>) -> f32 {
    if want.is_empty() {
        return 0.0;
    }
    want.intersection(have).count() as f32 / want.len() as f32
}

/// Sentences of `text`, split on `.`, `!`, `?`, and new lines.
pub fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let end_mark = matches!(b, b'!' | b'?' | b'\n')
            || (b == b'.'
                && bytes.get(i + 1).is_none_or(|n| n.is_ascii_whitespace())
                && !bytes
                    .get(i.wrapping_sub(1))
                    .is_some_and(|p| p.is_ascii_digit() && bytes.get(i + 1).is_none()));
        if end_mark {
            let s = text[start..=i].trim();
            if !s.is_empty() {
                out.push(s);
            }
            start = i + 1;
        }
    }
    let rest = text[start..].trim();
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_drop_stop_words_and_stem() {
        assert_eq!(
            terms("The towers were built in 1889"),
            ["tower", "built", "1889"]
        );
        assert!(numbers("1,200 people and 3.5%").contains("1200"));
        assert!(negated("It isn't so"));
        assert!(!negated("It is so"));
    }

    #[test]
    fn bm25_prefers_the_matching_doc() {
        let q = terms("transformer attention paper");
        let docs = vec![
            terms("A recipe for bread"),
            terms("Attention is all you need: the transformer paper"),
        ];
        let s = bm25(&q, &docs);
        assert!(s[1] > s[0]);
        assert_eq!(s[0], 0.0);
    }

    #[test]
    fn sentences_split() {
        assert_eq!(sentences("One. Two! Three"), ["One.", "Two!", "Three"]);
        assert_eq!(sentences("It is 3.5 m. Next"), ["It is 3.5 m.", "Next"]);
    }
}
