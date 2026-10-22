//! Live steering: a note the user sends while a run is going, read into
//! what it asks for.
//!
//! | Note | Action |
//! | --- | --- |
//! | `drop sub-question 3`, `skip #3`, `stop worker 2` | [`SteerAction::Drop`] |
//! | `add: cost comparison`, `also cover pricing` | [`SteerAction::Add`] |
//! | `redirect 2: open-source options`, `change 2 to …`, `2: …` | [`SteerAction::Redirect`] |
//! | anything else (`focus on 2024 sources`) | [`SteerAction::Guide`] |
//!
//! The engine in `tern-agent` applies the action at the run's next safe
//! point; nothing here runs anything.

use serde::{Deserialize, Serialize};

/// What a steering note asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SteerAction {
    /// Guidance for everything still to come.
    Guide { text: String },
    /// Stop sub-question `n` (or never start it).
    Drop { n: usize },
    /// Research one more sub-question.
    Add { question: String },
    /// Point sub-question `n` at something else.
    Redirect { n: usize, to: String },
}

impl SteerAction {
    /// The action in a few plain words, for a log line.
    pub fn describe(&self) -> String {
        match self {
            Self::Guide { text } => format!("Guidance: {text}"),
            Self::Drop { n } => format!("Drop sub-question {n}"),
            Self::Add { question } => format!("Add a sub-question: {question}"),
            Self::Redirect { n, to } => format!("Redirect sub-question {n}: {to}"),
        }
    }
}

/// Words that may sit between a verb and a sub-question's number.
const LABELS: [&str; 9] = [
    "sub-question",
    "subquestion",
    "sub question",
    "question",
    "part",
    "worker",
    "step",
    "number",
    "no.",
];

/// Reads a steering note. Blank notes are guidance with no text.
pub fn parse_steer(note: &str) -> SteerAction {
    let text = note.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = text.to_lowercase();

    for verb in ["drop", "remove", "skip", "cut", "stop", "cancel"] {
        if let Some(rest) = strip_word(&lower, verb)
            && let Some((n, tail)) = number_after_label(rest)
            && tail
                .trim_matches(|c: char| c.is_ascii_punctuation() || c == ' ')
                .is_empty()
        {
            return SteerAction::Drop { n };
        }
    }

    for verb in ["redirect", "change", "refocus", "point", "rewrite", "make"] {
        if let Some(rest) = strip_word(&lower, verb)
            && let Some((n, tail)) = number_after_label(rest)
            && let Some(to) = redirect_tail(tail)
        {
            return SteerAction::Redirect {
                n,
                to: original_tail(&text, to),
            };
        }
    }
    // `2: open-source options` or `#2 - open-source options`.
    if let Some((n, tail)) = number_after_label(&lower)
        && let Some(to) = tail
            .trim_start()
            .strip_prefix([':', '-', '→', ')'])
            .map(str::trim)
            .filter(|t| !t.is_empty())
    {
        return SteerAction::Redirect {
            n,
            to: original_tail(&text, to),
        };
    }

    for prefix in [
        "add:",
        "add ",
        "also cover ",
        "also research ",
        "also look at ",
        "also look into ",
        "also compare ",
        "new sub-question:",
        "new question:",
    ] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let rest = rest.trim().trim_start_matches(':').trim();
            if !rest.is_empty() {
                let mut question = original_tail(&text, rest);
                if prefix == "also compare " {
                    question = format!("Compare {question}");
                }
                return SteerAction::Add { question };
            }
        }
    }

    SteerAction::Guide { text }
}

/// `rest` after `word` at the start of `s`, when `word` stands alone.
fn strip_word<'a>(s: &'a str, word: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(word)?;
    (rest.is_empty() || rest.starts_with([' ', ':', '#'])).then(|| rest.trim_start())
}

/// A sub-question number, after an optional label and `#`, and the text
/// after it.
fn number_after_label(s: &str) -> Option<(usize, &str)> {
    let mut s = s.trim_start().trim_start_matches(':').trim_start();
    if let Some(rest) = s.strip_prefix("the ") {
        s = rest;
    }
    for label in LABELS {
        if let Some(rest) = s.strip_prefix(label) {
            s = rest.trim_start();
            break;
        }
    }
    let s = s.trim_start_matches('#').trim_start();
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    let n: usize = digits.parse().ok()?;
    (n > 0).then(|| (n, &s[digits.len()..]))
}

/// The new direction after `2`: `: text`, `to text`, `about text`.
fn redirect_tail(tail: &str) -> Option<&str> {
    let t = tail.trim_start();
    let t = t
        .strip_prefix(':')
        .or_else(|| t.strip_prefix("->"))
        .or_else(|| t.strip_prefix('→'))
        .or_else(|| t.strip_prefix('-'))
        .or_else(|| t.strip_prefix("to "))
        .or_else(|| t.strip_prefix("about "))
        .or_else(|| t.strip_prefix("toward "))
        .or_else(|| t.strip_prefix("towards "))
        .or_else(|| t.strip_prefix("at "))?;
    let t = t.trim();
    (!t.is_empty()).then_some(t)
}

/// The same tail in the note's own casing: `lower_tail` is a suffix of
/// the lowercased note, and lowercasing keeps ASCII lengths.
fn original_tail(original: &str, lower_tail: &str) -> String {
    let lower = original.to_lowercase();
    if lower.len() == original.len()
        && let Some(at) = lower.rfind(lower_tail)
        && original.is_char_boundary(at)
    {
        return original[at..].trim().to_owned();
    }
    lower_tail.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drop(n: usize) -> SteerAction {
        SteerAction::Drop { n }
    }

    #[test]
    fn drops() {
        assert_eq!(parse_steer("drop sub-question 3"), drop(3));
        assert_eq!(parse_steer("Drop #2."), drop(2));
        assert_eq!(parse_steer("skip part 1"), drop(1));
        assert_eq!(parse_steer("stop worker 4"), drop(4));
        assert_eq!(parse_steer("remove the question 2"), drop(2));
        // A number that is part of a sentence is guidance.
        assert!(matches!(
            parse_steer("drop sources older than 2020"),
            SteerAction::Guide { .. }
        ));
    }

    #[test]
    fn adds() {
        assert_eq!(
            parse_steer("add: cost comparison"),
            SteerAction::Add {
                question: "cost comparison".into()
            }
        );
        assert_eq!(
            parse_steer("Also cover EU Pricing"),
            SteerAction::Add {
                question: "EU Pricing".into()
            }
        );
        assert_eq!(
            parse_steer("also compare Rust and Zig"),
            SteerAction::Add {
                question: "Compare Rust and Zig".into()
            }
        );
    }

    #[test]
    fn redirects() {
        let r = |n: usize, to: &str| SteerAction::Redirect { n, to: to.into() };
        assert_eq!(
            parse_steer("redirect 2: Open-source options"),
            r(2, "Open-source options")
        );
        assert_eq!(
            parse_steer("change sub-question 1 to costs in 2024"),
            r(1, "costs in 2024")
        );
        assert_eq!(parse_steer("3: benchmarks only"), r(3, "benchmarks only"));
        assert_eq!(parse_steer("#2 - EU rules"), r(2, "EU rules"));
    }

    #[test]
    fn the_rest_is_guidance() {
        assert_eq!(
            parse_steer("  focus on 2024   sources "),
            SteerAction::Guide {
                text: "focus on 2024 sources".into()
            }
        );
        assert!(matches!(parse_steer("add"), SteerAction::Guide { .. }));
        assert_eq!(
            parse_steer("drop 3").describe(),
            "Drop sub-question 3".to_owned()
        );
    }

    #[test]
    fn actions_round_trip() {
        let a = SteerAction::Redirect {
            n: 2,
            to: "x".into(),
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["action"], "redirect");
        assert_eq!(serde_json::from_value::<SteerAction>(v).unwrap(), a);
    }
}
