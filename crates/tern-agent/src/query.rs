//! Search queries built from what the user typed (#90).
//!
//! A scholarly service matches keywords, not requests: "FLARE active
//! retrieval retrieving sentence better Cite" and "main critiques RLHF
//! Cite sources" (the whole sentence, instructions and all) found nothing
//! on topic in the #85 Windows test, and the turn answered "not enough
//! evidence". [`scholar_keywords`] keeps the topic's key terms and drops
//! the words that only tell Tern how to answer ("cite sources", "in five
//! lines", "summarise"). [`broader`] gives the one or two wider wordings a
//! weak search is retried with before Tern ever says there is not enough
//! evidence.

use std::sync::LazyLock;

use regex::Regex;

/// Phrases that say how to answer, not what about: "cite sources", "in 5
/// lines", "with references", "answer in 3 bullet points".
static HOW_TO_ANSWER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b(?:in|within|under|using|with|max(?:imum)?|at\s+most|answer\s+in)?\s*
          (?:\d+|one|two|three|four|five|six|seven|eight|nine|ten)\s*
          (?:lines?|sentences?|words?|bullets?|bullet\s+points?|points?|paragraphs?)\b
        | \bcite\s+(?:your\s+|the\s+|its\s+|all\s+)?(?:sources?|papers?|references?|evidence)\b
        | \b(?:with|and|plus)\s+(?:citations?|sources?|references?)\b
        | \bsources?\s+please\b
        ",
    )
    .expect("valid pattern")
});

/// Words that only tell Tern how to answer.
const INSTRUCTION_WORDS: &[&str] = &[
    "cite",
    "sources",
    "source",
    "references",
    "reference",
    "citation",
    "citations",
    "answer",
    "answers",
    "reply",
    "respond",
    "briefly",
    "brief",
    "short",
    "shortly",
    "concise",
    "concisely",
    "better",
    "best",
    "main",
    "list",
    "give",
    "write",
    "using",
    "include",
    "including",
    "bullet",
    "bullets",
    "markdown",
    "format",
    "describe",
    "discuss",
    "overview",
    "detailed",
    "simple",
    "simply",
    "quick",
    "quickly",
    "lines",
    "summary",
    "summarize",
    "summarise",
    "explain",
    "please",
    "papers",
    "paper",
    "say",
    "says",
    "said",
];

/// The most terms a scholarly query keeps.
const MAX_TERMS: usize = 8;

/// The key terms of `text` as a scholarly keyword query: no filler, no
/// instructions, at most eight terms, in the order typed. Empty when
/// nothing is left.
pub fn scholar_keywords(text: &str) -> String {
    let plain = HOW_TO_ANSWER.replace_all(text, " ");
    crate::webintent::keywords(&plain)
        .into_iter()
        .filter(|w| {
            let l = w.to_lowercase();
            !INSTRUCTION_WORDS.contains(&l.as_str())
                && !crate::intent::is_scholar_filler(&l)
                && !l.chars().all(|c| c.is_ascii_digit())
        })
        .take(MAX_TERMS)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The query a scholarly search runs for `asked`: its keywords, or the
/// text as typed when no keyword is left.
pub fn clean(asked: &str) -> String {
    let k = scholar_keywords(asked);
    if k.is_empty() {
        asked.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        k
    }
}

/// Acronyms the literature also spells out, so a search for one finds
/// papers that only use the long form. Only for building queries: an
/// answer never takes an expansion from here.
const EXPANSIONS: &[(&str, &str)] = &[
    ("rag", "retrieval augmented generation"),
    ("flare", "active retrieval augmented generation"),
    ("rlhf", "reinforcement learning from human feedback"),
    ("rlaif", "reinforcement learning from AI feedback"),
    ("dpo", "direct preference optimization"),
    ("llm", "large language models"),
    ("llms", "large language models"),
    ("lora", "low-rank adaptation"),
    ("peft", "parameter-efficient fine-tuning"),
    ("sft", "supervised fine-tuning"),
    ("cot", "chain of thought prompting"),
    ("icl", "in-context learning"),
    ("moe", "mixture of experts"),
    ("nlp", "natural language processing"),
    ("gan", "generative adversarial networks"),
    ("gans", "generative adversarial networks"),
    ("cnn", "convolutional neural networks"),
    ("rnn", "recurrent neural networks"),
    ("lstm", "long short-term memory"),
    ("vae", "variational autoencoder"),
    ("vit", "vision transformer"),
    ("kd", "knowledge distillation"),
];

/// The expansion Tern searches with for an acronym, if it knows one.
pub fn expansion(word: &str) -> Option<&'static str> {
    let l = word.to_lowercase();
    EXPANSIONS.iter().find(|(a, _)| *a == l).map(|(_, e)| *e)
}

fn is_acronym(w: &str) -> bool {
    let letters = w.chars().filter(|c| c.is_alphabetic()).count();
    let caps = w.chars().filter(|c| c.is_uppercase()).count();
    letters >= 2 && caps * 2 > letters
}

/// One or two wider wordings of `query` (already keywords) to retry a
/// weak search with: the named technique with its long form, then the
/// topic's two strongest terms. Never `query` itself, never a repeat.
pub fn broader(query: &str) -> Vec<String> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut push = |q: String| {
        let q = q.split_whitespace().collect::<Vec<_>>().join(" ");
        if !q.is_empty()
            && !q.eq_ignore_ascii_case(query)
            && !out.iter().any(|o| o.eq_ignore_ascii_case(&q))
        {
            out.push(q);
        }
    };
    // A named technique: its name with its long form ("FLARE active
    // retrieval augmented generation"), then the long form alone.
    let named: Vec<(&str, Option<&str>)> = terms
        .iter()
        .filter(|t| is_acronym(t) || expansion(t).is_some())
        .map(|t| (*t, expansion(t)))
        .collect();
    if let Some((name, Some(long))) = named.iter().find(|(_, e)| e.is_some()) {
        push(format!("{name} {long}"));
        // The rest of the topic with the long form instead of the name.
        let rest: Vec<&str> = terms
            .iter()
            .filter(|t| {
                !t.eq_ignore_ascii_case(name)
                    && !long.split_whitespace().any(|w| w.eq_ignore_ascii_case(t))
            })
            .take(1)
            .copied()
            .collect();
        push(format!("{long} {}", rest.join(" ")));
    } else if let Some((name, None)) = named.first() {
        // An acronym Tern cannot spell out: it with the strongest other
        // term, and it alone.
        let other = terms
            .iter()
            .filter(|t| !t.eq_ignore_ascii_case(name))
            .max_by_key(|t| t.chars().count())
            .copied()
            .unwrap_or_default();
        push(format!("{name} {other}"));
        push((*name).to_owned());
    } else {
        // Plain words: the first three, then the two longest.
        if terms.len() > 3 {
            push(terms[..3].join(" "));
        }
        let mut longest: Vec<&str> = terms.clone();
        longest.sort_by_key(|t| std::cmp::Reverse(t.chars().count()));
        longest.truncate(2);
        // Kept in the order typed.
        let two: Vec<&str> = terms
            .iter()
            .filter(|t| longest.contains(*t))
            .copied()
            .collect();
        if terms.len() > 2 {
            push(two.join(" "));
        }
    }
    out.truncate(2);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_leave_the_query() {
        assert_eq!(
            scholar_keywords(
                "How does FLARE active retrieval compare with retrieving before every sentence? Which is better? Cite sources."
            ),
            "FLARE active retrieval retrieving sentence"
        );
        assert_eq!(
            scholar_keywords("What are the main critiques of RLHF? Cite sources."),
            "critiques RLHF"
        );
        assert_eq!(
            scholar_keywords("Summarise what research says about spaced repetition in 5 lines"),
            "spaced repetition"
        );
        assert_eq!(clean("Cite sources"), "Cite sources");
    }

    #[test]
    fn a_weak_search_has_wider_wordings() {
        assert_eq!(
            broader("FLARE active retrieval retrieving sentence"),
            vec![
                "FLARE active retrieval augmented generation".to_owned(),
                "active retrieval augmented generation retrieving".to_owned()
            ]
        );
        assert_eq!(
            broader("critiques RLHF"),
            vec![
                "RLHF reinforcement learning from human feedback".to_owned(),
                "reinforcement learning from human feedback critiques".to_owned()
            ]
        );
        let plain = broader("octopus sleep stages memory consolidation");
        assert_eq!(plain[0], "octopus sleep stages");
        assert!(plain.len() <= 2);
        assert!(broader("").is_empty());
    }
}
