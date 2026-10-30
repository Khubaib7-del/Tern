//! What kind of request a message is, read from its words alone, so the
//! engine can route it before any model call (#83).
//!
//! - [`Intent::Files`]: work on the user's folder or files ("count words
//!   in every .md file here", a local path, "rename the folder"). Never
//!   answered with a web or scholarly search on Tern's own initiative.
//! - [`Intent::Academic`]: a question the literature answers (papers,
//!   studies, methods, a named technique such as FLARE, "do studies show
//!   ..."). `scholar_search` runs first; the web may follow.
//! - [`Intent::Casual`]: greetings and small talk.
//! - [`Intent::General`]: everything else.
//!
//! The classifier is deterministic on purpose: small models offered both
//! `web_search` and `scholar_search` picked the web for "How does FLARE
//! compare with retrieving before every sentence?" (#82 test).

use std::sync::LazyLock;

use regex::Regex;

/// The kind of request a message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    /// Work on files or a folder.
    Files,
    /// A question the scholarly literature answers.
    Academic,
    /// Greetings and small talk.
    Casual,
    /// Anything else.
    General,
}

/// A file name with an extension Tern's file tools deal with: `counts.md`,
/// `*.md`, `.md`, `main.rs`.
static FILE_EXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[\s(\[*'`]|\w)\.(?:md|markdown|txt|rs|py|js|jsx|ts|tsx|json|jsonl|csv|tsv|toml|ya?ml|html?|css|scss|pdf|docx?|xlsx?|pptx?|log|ini|cfg|conf|xml|sh|ps1|bat|cmd|c|cc|cpp|h|hpp|java|kt|go|rb|php|swift|sql|ipynb|tex|bib|png|jpe?g|gif|svg|zip|env)\b",
    )
    .expect("valid pattern")
});

/// Library names that look like file names ("Node.js", "Vue.js").
static JS_NAMES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:node|next|vue|nuxt|three|d3|express|react|chart|p5|ember|backbone|angular|svelte|solid|deno|bun)\.js\b")
        .expect("valid pattern")
});

/// Words that name files, folders or the workspace.
const FILE_WORDS: &[&str] = &[
    "file",
    "files",
    "folder",
    "folders",
    "directory",
    "directories",
    "dir",
    "subfolder",
    "subfolders",
    "workspace",
    "filename",
    "filenames",
    "codebase",
    "repo",
    "repository",
];

/// Words that only the literature answers.
const ACADEMIC_WORDS: &[&str] = &[
    "paper",
    "papers",
    "preprint",
    "preprints",
    "arxiv",
    "doi",
    "journal",
    "journals",
    "literature",
    "citation",
    "citations",
    "cited",
    "citing",
    "researcher",
    "researchers",
    "scientific",
    "scientists",
    "empirical",
    "empirically",
    "hypothesis",
    "experiments",
    "experimental",
    "clinical",
    "meta-analysis",
    "metaanalysis",
    "peer-reviewed",
    "thesis",
    "dissertation",
    "ablation",
    "scholarly",
    "academic",
    "peer",
];

/// Phrases that point at the literature.
const ACADEMIC_PHRASES: &[&str] = &[
    "studies show",
    "studies suggest",
    "studies found",
    "a study found",
    "a study shows",
    "the study found",
    "recent studies",
    "research on",
    "research paper",
    "clinical trial",
    "research shows",
    "research suggests",
    "according to research",
    "is there evidence",
    "what does the evidence",
    "what does research",
    "the evidence for",
    "the evidence on",
    "systematic review",
    "literature review",
    "state of the art",
    "state-of-the-art",
    "et al",
    "peer reviewed",
    "published research",
    "scientific evidence",
];

/// Words that ask about a method or technique.
const TECHNIQUE_WORDS: &[&str] = &[
    "method",
    "methods",
    "technique",
    "techniques",
    "approach",
    "approaches",
    "algorithm",
    "algorithms",
    "architecture",
    "framework",
    "model",
    "models",
    "mechanism",
    "compare",
    "compares",
    "compared",
    "comparison",
    "vs",
    "versus",
    "differ",
    "difference",
    "work",
    "works",
    "propose",
    "proposed",
    "introduce",
    "introduced",
    "outperform",
    "outperforms",
    "retrieval",
    "retrieving",
    "training",
    "trained",
];

/// Subjects of scientific questions ("do octopuses dream ...").
const SCIENCE_WORDS: &[&str] = &[
    "dream",
    "dreams",
    "dreaming",
    "sleep",
    "brain",
    "brains",
    "neuron",
    "neurons",
    "neural",
    "cognition",
    "cognitive",
    "gene",
    "genes",
    "genetic",
    "dna",
    "rna",
    "protein",
    "proteins",
    "cell",
    "cells",
    "virus",
    "viruses",
    "bacteria",
    "disease",
    "diseases",
    "vaccine",
    "vaccines",
    "cancer",
    "tumor",
    "species",
    "evolution",
    "evolve",
    "evolved",
    "octopus",
    "octopuses",
    "octopi",
    "quantum",
    "molecule",
    "molecules",
    "enzyme",
    "hormone",
    "climate",
    "photosynthesis",
    "llm",
    "llms",
    "transformer",
    "transformers",
    "embedding",
    "embeddings",
    "reinforcement",
    "hallucination",
    "hallucinations",
];

/// All-capital words that are not techniques.
const COMMON_CAPS: &[&str] = &[
    "API", "APIS", "URL", "URLS", "USA", "UK", "EU", "CEO", "CTO", "FAQ", "PDF", "CSV", "JSON",
    "HTML", "CSS", "SQL", "XML", "HTTP", "HTTPS", "OK", "TV", "PC", "USB", "GPU", "CPU", "RAM",
    "SSD", "OS", "ID", "IDS", "TODO", "README", "ASAP", "FYI", "BTW", "LOL", "OMG", "DIY", "AM",
    "PM", "UTC", "PKT", "EST", "PST", "NASA", "UN", "WHO", "IT", "MD", "TXT", "I",
];

/// Greetings and small talk.
const CASUAL: &[&str] = &[
    "hi", "hello", "hey", "thanks", "thank", "thx", "ok", "okay", "cool", "nice", "great", "bye",
    "goodbye", "morning", "evening", "yo", "sup", "lol",
];

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .map(|w| w.trim_matches('-').to_owned())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Whether the message asks for work on files or a folder: a file name
/// with an extension, a local path, or a word for files or folders.
pub fn is_file_task(text: &str) -> bool {
    let plain = JS_NAMES.replace_all(text, " ");
    if FILE_EXT.is_match(&plain) || crate::paths::find_in_text(text).is_some() {
        return true;
    }
    let w = words(text);
    w.iter().any(|x| FILE_WORDS.contains(&x.as_str()))
}

/// A technique's name: an all-capital word of three to ten letters or
/// digits (FLARE, BERT, RAG, GPT-4), or a mixed-case acronym with more
/// capitals than small letters (LoRA, RoBERTa, Self-RAG). Product names
/// in camel case (LangChain, GitHub, OpenAI) are not techniques.
fn has_technique_name(text: &str) -> bool {
    text.split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .map(|w| w.trim_matches('-'))
        .filter(|w| !w.is_empty())
        .enumerate()
        .any(|(i, w)| {
            let letters = w.chars().filter(|c| c.is_alphabetic()).count();
            let caps = w.chars().filter(|c| c.is_uppercase()).count();
            let all_caps = letters >= 3
                && caps == letters
                && w.chars().count() <= 10
                && !COMMON_CAPS.contains(&w.to_uppercase().as_str());
            // Mixed case like "LoRA": not the first word (a capitalised
            // sentence start is not an acronym), and mostly capitals.
            let mixed = i > 0
                && caps >= 2
                && caps < letters
                && caps > letters - caps
                && w.chars().next().is_some_and(char::is_uppercase)
                && w.chars().count() <= 10;
            all_caps || mixed
        })
}

/// Whether the message is a question the scholarly literature answers.
pub fn is_academic(text: &str) -> bool {
    classify(text) == Intent::Academic
}

fn academic_signals(text: &str) -> bool {
    let lower = text.to_lowercase();
    let w = words(text);
    let has = |list: &[&str]| w.iter().any(|x| list.contains(&x.as_str()));
    if has(ACADEMIC_WORDS) || ACADEMIC_PHRASES.iter().any(|p| lower.contains(p)) {
        // "peer" alone is not enough ("peer to peer").
        let only_peer = w
            .iter()
            .filter(|x| ACADEMIC_WORDS.contains(&x.as_str()))
            .all(|x| x == "peer")
            && !ACADEMIC_PHRASES.iter().any(|p| lower.contains(p));
        if !only_peer {
            return true;
        }
    }
    let question = text.contains('?')
        || w.first().is_some_and(|f| {
            [
                "how",
                "why",
                "what",
                "do",
                "does",
                "did",
                "is",
                "are",
                "can",
                "could",
                "which",
                "compare",
                "explain",
                "summarize",
                "summarise",
            ]
            .contains(&f.as_str())
        });
    if has_technique_name(text) && (has(TECHNIQUE_WORDS) || question) {
        return true;
    }
    question && has(SCIENCE_WORDS)
}

/// The kind of request `text` is. Files win over everything: a folder
/// task that mentions a paper is still a folder task.
pub fn classify(text: &str) -> Intent {
    let t = text.trim();
    if is_file_task(t) {
        return Intent::Files;
    }
    let w = words(t);
    if w.is_empty() {
        return Intent::Casual;
    }
    if academic_signals(t) {
        return Intent::Academic;
    }
    if w.len() <= 4 && w.iter().any(|x| CASUAL.contains(&x.as_str())) {
        return Intent::Casual;
    }
    Intent::General
}

/// Whether Tern may run its own web search after the model refused or
/// repeated itself: never for a folder or file task, that is a message
/// about files when file tools are on offer or it names a local path, or
/// any turn in which the model already used a file tool (#83).
pub fn safety_net_allowed(text: &str, file_tools: bool, used_file_tools: bool) -> bool {
    if used_file_tools {
        return false;
    }
    if !is_file_task(text) {
        return true;
    }
    !(file_tools || crate::paths::find_in_text(text).is_some())
}

/// Words a scholarly search cannot use beyond the web filler.
const SCHOLAR_FILLER: &[&str] = &[
    "compare",
    "compares",
    "compared",
    "comparison",
    "vs",
    "versus",
    "difference",
    "differences",
    "between",
    "work",
    "works",
    "really",
    "true",
    "actually",
    "studies",
    "study",
    "research",
    "paper",
    "papers",
    "evidence",
    "show",
    "shows",
    "suggest",
    "suggests",
    "according",
    "literature",
    "academic",
    "scientific",
    "scholarly",
    "summarize",
    "summarise",
    "find",
    "search",
    "look",
    "every",
    "each",
    "any",
];

/// The keyword query Tern sends `scholar_search` for an academic
/// question: its key terms without filler, at most eight.
pub fn scholar_query(text: &str) -> Option<String> {
    let kept: Vec<String> = crate::webintent::keywords(text)
        .into_iter()
        .filter(|w| !SCHOLAR_FILLER.contains(&w.to_lowercase().as_str()))
        .take(8)
        .collect();
    (!kept.is_empty()).then(|| kept.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn academic_questions() {
        for q in [
            "How does FLARE compare with retrieving before every sentence?",
            "Do octopuses dream in binary code?",
            "What do studies show about intermittent fasting and longevity?",
            "find papers on retrieval augmented generation",
            "Is there evidence that LoRA matches full fine-tuning?",
            "what method does Self-RAG use to decide when to retrieve",
            "Summarize the literature on spaced repetition",
            "Vaswani et al. attention mechanism explained",
        ] {
            assert_eq!(classify(q), Intent::Academic, "{q}");
        }
    }

    #[test]
    fn file_tasks() {
        for q in [
            "count words in every .md file here and save the counts to counts.md.",
            "Write one python script that counts the words in every .md file here and saves the counts to counts.md",
            "rename notes.txt to old-notes.txt",
            "list the folder",
            "read C:\\Users\\me\\Documents\\paper.pdf",
            "summarize the papers in this folder",
            "add a line to sum.md",
        ] {
            assert_eq!(classify(q), Intent::Files, "{q}");
            assert!(is_file_task(q), "{q}");
        }
    }

    #[test]
    fn casual_and_general() {
        for q in ["hi", "thanks!", "hello there", "ok cool"] {
            assert_eq!(classify(q), Intent::Casual, "{q}");
        }
        for q in [
            "Write a long essay about arrays",
            "what is the weather in Lahore today",
            "explain recursion",
            "who won the match yesterday",
            "What is an API key?",
            "peer to peer networking basics",
            "Make me a study plan for next week",
            "who is the creator of LangChain?",
            "what is new on GitHub and OpenAI?",
        ] {
            let got = classify(q);
            assert!(
                got == Intent::General || got == Intent::Casual,
                "{q}: {got:?}"
            );
        }
    }

    #[test]
    fn the_safety_net_never_searches_for_file_tasks() {
        let task = "count words in every .md file here and save the counts to counts.md.";
        assert!(!safety_net_allowed(task, true, false));
        assert!(!safety_net_allowed("who created langchain?", true, true));
        assert!(safety_net_allowed("who created langchain?", true, false));
        assert!(safety_net_allowed("who created langchain?", false, false));
        // "Node.js" is a name, not a file.
        assert!(safety_net_allowed("what is new in Node.js?", false, false));
        assert!(safety_net_allowed("what is new in Node.js?", true, false));
        assert!(!safety_net_allowed("fix the bug in server.js", true, false));
    }

    #[test]
    fn scholar_queries_keep_the_topic() {
        assert_eq!(
            scholar_query("How does FLARE compare with retrieving before every sentence?")
                .as_deref(),
            Some("FLARE retrieving sentence")
        );
        assert_eq!(
            scholar_query("Do octopuses dream in binary code?").as_deref(),
            Some("octopuses dream binary code")
        );
    }
}
