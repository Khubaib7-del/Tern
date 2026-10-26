//! The relevance guard for web search results.
//!
//! A keyword index (Wikipedia above all) answers a query it does not
//! understand with whatever matches one common word: "SubQ Snap 1.1 AI
//! model definition or meaning" came back with six pages about Japanese
//! words and medicine, and "harness2 by Google AI model or project" would
//! have matched "harness". [`Topic`] keeps the question's main terms and
//! drops every result whose title, snippet and link share none of them.
//!
//! Terms come in three strengths, and a result must carry one of the
//! strongest kind the question has:
//! 1. **Strong**: quoted phrases, words with digits (`harness2`,
//!    `gpt-4o`), a name followed by a version (`Snap 1.1`), and words
//!    with an inner capital (`SubQ`, `LangChain`).
//! 2. **Names**: other capitalised words that do not start the question
//!    (`Google`, `Lahore`).
//! 3. **Keywords**: the remaining words, without filler and without
//!    generic words ("model", "project", "definition", "AI").
//!
//! A generic word alone is never enough, and a version number alone
//! (`1.1`) is never a term.

use crate::webintent::is_stop;

/// Words too common to say what a question is about.
const GENERIC: &[&str] = &[
    "ai",
    "app",
    "apps",
    "article",
    "best",
    "company",
    "created",
    "creator",
    "data",
    "define",
    "definition",
    "describe",
    "description",
    "example",
    "examples",
    "free",
    "guide",
    "history",
    "intro",
    "introduction",
    "latest",
    "llm",
    "mean",
    "meaning",
    "means",
    "model",
    "models",
    "new",
    "news",
    "official",
    "online",
    "overview",
    "product",
    "project",
    "projects",
    "release",
    "released",
    "software",
    "system",
    "thing",
    "tool",
    "tools",
    "top",
    "tutorial",
    "use",
    "used",
    "uses",
    "using",
    "version",
    "web",
    "website",
    "wiki",
    "wikipedia",
    "work",
    "works",
];

/// Whether `word` (any case) is too common to carry a question alone.
pub fn is_generic(word: &str) -> bool {
    let w = word.to_lowercase();
    GENERIC.contains(&w.as_str())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Strength {
    Keyword,
    Name,
    Strong,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Term {
    /// Lower case, words joined by single spaces (`snap 1.1`).
    words: String,
    strength: Strength,
}

/// The main terms of a question, to test search results against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Topic {
    terms: Vec<Term>,
}

/// Lower case, every character that is not a letter, digit or a dot
/// between digits turned into a space, spaces collapsed.
fn normalise(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, c) in chars.iter().enumerate() {
        let keep = c.is_alphanumeric()
            || (*c == '.'
                && i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit()));
        if keep {
            out.extend(c.to_lowercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.trim().to_owned()
}

/// `1.1`, `2512`, `v3`, `3.5b`: a version or size, not a word.
fn is_version(w: &str) -> bool {
    let w = w.strip_prefix(['v', 'V']).unwrap_or(w);
    if !w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    let body: String = w.trim_end_matches(|c: char| c.is_ascii_alphabetic()).into();
    let tail = w.len() - body.len();
    tail <= 1 && body.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn has_digit(w: &str) -> bool {
    w.chars().any(|c| c.is_ascii_digit())
}

fn has_letter(w: &str) -> bool {
    w.chars().any(char::is_alphabetic)
}

/// `SubQ`, `LangChain`, `iPhone`: a capital after the first letter, with
/// lower case too (so not an acronym like `AI`).
fn inner_capital(w: &str) -> bool {
    let rest: Vec<char> = w.chars().skip(1).collect();
    rest.iter().any(|c| c.is_uppercase()) && w.chars().any(char::is_lowercase)
}

impl Topic {
    /// The terms of `question` (a sentence or a short keyword query).
    pub fn of(question: &str) -> Self {
        let mut terms: Vec<Term> = Vec::new();
        let mut push = |words: String, strength: Strength| {
            if words.is_empty() {
                return;
            }
            if let Some(t) = terms.iter_mut().find(|t| t.words == words) {
                t.strength = t.strength.max(strength);
            } else {
                terms.push(Term { words, strength });
            }
        };
        // Quoted phrases first; the rest is read word by word.
        let mut rest = String::new();
        let mut quoted = String::new();
        let mut in_quote = false;
        for c in question.chars() {
            if matches!(c, '"' | '“' | '”' | '`') {
                if in_quote {
                    push(normalise(&quoted), Strength::Strong);
                    quoted.clear();
                }
                in_quote = !in_quote;
                rest.push(' ');
            } else if in_quote {
                quoted.push(c);
            } else {
                rest.push(c);
            }
        }
        rest.push_str(&quoted);
        let raw: Vec<&str> = rest
            .split(|c: char| c.is_whitespace() || ",;:!?()[]{}<>|/".contains(c))
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
            .filter(|w| !w.is_empty())
            .collect();
        let mut prev: Option<&str> = None;
        for (i, w) in raw.iter().enumerate() {
            let lower = w.to_lowercase();
            if is_version(w) {
                // "Snap 1.1": the name before it with the version.
                if let Some(p) = prev {
                    push(normalise(&format!("{p} {w}")), Strength::Strong);
                }
                prev = None;
                continue;
            }
            if is_stop(&lower) || (lower.chars().count() < 2 && !has_digit(w)) {
                prev = None;
                continue;
            }
            let generic = is_generic(w);
            let strength = if (has_digit(w) && has_letter(w)) || inner_capital(w) {
                Strength::Strong
            } else if generic {
                prev = None;
                continue;
            } else if i > 0 && w.chars().next().is_some_and(char::is_uppercase) {
                Strength::Name
            } else {
                Strength::Keyword
            };
            push(normalise(w), strength);
            prev = Some(w);
        }
        Topic { terms }
    }

    /// Whether the question gave anything to test against.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The terms a result must share one of: the strongest kind present.
    fn required(&self) -> Vec<&str> {
        let Some(top) = self.terms.iter().map(|t| t.strength).max() else {
            return Vec::new();
        };
        self.terms
            .iter()
            .filter(|t| t.strength == top)
            .map(|t| t.words.as_str())
            .collect()
    }

    /// The terms results are tested against, for logs and tests.
    pub fn key_terms(&self) -> Vec<String> {
        self.required().into_iter().map(str::to_owned).collect()
    }

    /// Whether `text` (a result's title, snippet and link) carries one of
    /// the question's main terms. Always true for an empty topic.
    pub fn matches(&self, text: &str) -> bool {
        let need = self.required();
        if need.is_empty() {
            return true;
        }
        let hay = format!(" {} ", normalise(text));
        let compact: String = hay.chars().filter(|c| *c != ' ').collect();
        need.iter().any(|t| {
            hay.contains(&format!(" {t} "))
                // A plural or possessive: "transformers", "langchain s".
                || hay.contains(&format!(" {t}s "))
                // "harness 2" / "snap-1.1" / "snap1.1" for terms with digits.
                || (has_digit(t) && t.chars().count() >= 4 && compact.contains(&t.replace(' ', "")))
        })
    }

    /// Keeps the hits that match; returns them and how many were dropped.
    pub fn filter<T>(&self, hits: Vec<T>, text: impl Fn(&T) -> String) -> (Vec<T>, usize) {
        let before = hits.len();
        let kept: Vec<T> = hits
            .into_iter()
            .filter(|h| self.matches(&text(h)))
            .collect();
        let dropped = before - kept.len();
        (kept, dropped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_main_terms_are_names_versions_and_quotes() {
        let t = Topic::of("SubQ Snap 1.1 AI model definition or meaning");
        assert_eq!(t.key_terms(), ["subq", "snap 1.1"]);
        let t = Topic::of("harness2 by Google AI model or project");
        assert_eq!(t.key_terms(), ["harness2"]);
        let t = Topic::of(r#"what is "mixture of experts" in Mistral"#);
        assert_eq!(t.key_terms(), ["mixture of experts"]);
        let t = Topic::of("Who founded Anthropic and when");
        assert_eq!(t.key_terms(), ["anthropic"]);
        let t = Topic::of("langchain creator");
        assert_eq!(t.key_terms(), ["langchain"]);
        let t = Topic::of("ministral-14b-2512 context window");
        assert!(t.key_terms().contains(&"ministral 14b 2512".to_owned()));
        // Only generic words: nothing to hold results to.
        assert!(Topic::of("what is an AI model").is_empty());
        assert!(Topic::of("").matches("anything"));
    }

    #[test]
    fn a_lone_generic_word_or_version_never_counts() {
        let t = Topic::of("harness2 by Google AI model or project");
        assert!(!t.matches("Harness (software) — a model project for AI"));
        assert!(t.matches("Harness2: Google's new agent harness"));
        assert!(t.matches("Google unveils Harness 2"));
        let t = Topic::of("SubQ Snap 1.1");
        assert!(!t.matches("Version 1.1 release notes"));
        assert!(!t.matches("Snap Inc. — Snapchat"));
        assert!(t.matches("SubQ releases Snap-1.1"));
        assert!(t.matches("Snap 1.1 benchmark"));
    }

    #[test]
    fn plain_lower_case_questions_keep_their_keywords() {
        let t = Topic::of("langchain erupted developre creator");
        assert!(t.matches("LangChain - Wikipedia: LangChain is a software framework"));
        assert!(!t.matches("Volcanic eruption"));
        let t = Topic::of("سلام پاکستان تاریخ");
        assert!(t.matches("پاکستان کی تاریخ"));
        assert!(!t.matches("India"));
    }

    #[test]
    fn filter_counts_what_it_dropped() {
        let t = Topic::of("SubQ Snap 1.1");
        let (kept, dropped) = t.filter(vec!["Snap 1.1 by SubQ", "Shogi", "Snap"], |s| {
            (*s).to_owned()
        });
        assert_eq!(kept, ["Snap 1.1 by SubQ"]);
        assert_eq!(dropped, 2);
    }

    #[test]
    fn versions_are_read_as_versions() {
        assert!(is_version("1.1"));
        assert!(is_version("v3"));
        assert!(is_version("2512"));
        assert!(is_version("3.5b"));
        assert!(!is_version("harness2"));
        assert!(!is_version("gpt"));
        assert_eq!(normalise("Snap-1.1, end."), "snap 1.1 end");
    }
}
