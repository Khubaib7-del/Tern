//! Reading what a message wants from the web, and what a reply says
//! about it.
//!
//! - [`web_query`]: the search a message plainly asks for ("use the web
//!   tool", "browse about that", "google it", "search online", "look it
//!   up"), typos and quotes included. A message that only points back
//!   ("browse about that") searches for the previous question.
//! - [`keywords`] and [`fallback_queries`]: a long question turned into
//!   the short keyword queries a keyword index (Wikipedia, SearXNG) can
//!   answer, tried in turn when a search comes back empty.
//! - [`is_refusal`]: a reply that says the model cannot reach the web or
//!   does not know, for the one search Tern then runs itself.
//! - [`same_reply`]: a reply that copies the previous one.

use std::sync::LazyLock;

use regex::Regex;

/// The phrases that ask for a web search. Spelling is forgiving: "teh",
/// "tol", "wbe", "intenet", and quotes around the phrase.
static ASK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ix)
        \b(?:
            # use the web (search) tool / use web search / using the internet
            (?:please\s+)?(?:use|using|with|via|try)\s+(?:the\s+|teh\s+|a\s+|your\s+)?
                (?:web|wbe|internet|intenet|online|net|browser|google|search)
                (?:\s+(?:search\s+)?(?:tool|tools|tol|toll|search|browsing))?
          | search\s+(?:it\s+|this\s+|that\s+|for\s+it\s+)?(?:on\s+)?(?:the\s+)?(?:web|wbe|internet|intenet|online|net|google)(?:\s+for)?
          | online\s+search(?:\s+for)?
          | look\s+(?:it\s+|this\s+|that\s+)?up(?:\s+online|\s+on\s+the\s+(?:web|internet))?
          | google\s+(?:it|this|that)
          | browse(?:\s+(?:the\s+)?(?:web|internet|online))?(?:\s+(?:for|about|on))?
          | web\s*search(?:\s+for)?
          | check\s+(?:it\s+|this\s+|that\s+)?online
          | find\s+(?:it\s+|this\s+|that\s+|out\s+)?online
        )\b[:,]?
        "#,
    )
    .expect("valid pattern")
});

/// "google langchain" at the start of a message.
static GOOGLE_FIRST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*google\s+").expect("valid pattern"));

/// Whether the message asks for the web at all.
pub fn asks_for_web(text: &str) -> bool {
    ASK.is_match(text) || GOOGLE_FIRST.is_match(text)
}

/// Words that carry nothing a search can use.
const STOP: &[&str] = &[
    "a",
    "about",
    "after",
    "again",
    "all",
    "also",
    "an",
    "and",
    "any",
    "are",
    "as",
    "at",
    "be",
    "been",
    "before",
    "but",
    "by",
    "can",
    "come",
    "comes",
    "could",
    "detail",
    "details",
    "did",
    "do",
    "does",
    "explain",
    "find",
    "for",
    "from",
    "get",
    "give",
    "had",
    "has",
    "have",
    "how",
    "i",
    "if",
    "in",
    "info",
    "infor",
    "information",
    "infos",
    "into",
    "is",
    "it",
    "its",
    "it's",
    "just",
    "know",
    "let",
    "me",
    "more",
    "my",
    "need",
    "now",
    "of",
    "on",
    "or",
    "out",
    "please",
    "pls",
    "plz",
    "provide",
    "share",
    "should",
    "show",
    "so",
    "some",
    "tell",
    "than",
    "that",
    "the",
    "their",
    "them",
    "then",
    "there",
    "these",
    "they",
    "this",
    "those",
    "to",
    "up",
    "us",
    "was",
    "we",
    "were",
    "what",
    "whats",
    "what's",
    "when",
    "where",
    "which",
    "who",
    "whom",
    "whose",
    "why",
    "will",
    "with",
    "would",
    "you",
    "your",
    "want",
    "like",
    "said",
    "say",
];

/// Whether `lower` (a lower-case word) is filler a search cannot use.
pub fn is_stop(lower: &str) -> bool {
    STOP.contains(&lower)
}

/// Words that only point back at something said before.
const POINTERS: &[&str] = &[
    "that", "this", "it", "those", "these", "them", "same", "above", "previous", "question",
    "topic", "one", "again", "more", "too", "also", "thing", "stuff", "answer",
];

/// The keywords of a question, in order, without filler, quotes or
/// repeats: "can you provide me infor about how langchain erupted …"
/// gives "langchain erupted developre creator".
pub fn keywords(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split(|c: char| c.is_whitespace() || ",;:!?()[]{}\"“”'`<>|".contains(c)) {
        let w = raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '+' && c != '#');
        if w.is_empty() {
            continue;
        }
        let lower = w.to_lowercase();
        if STOP.contains(&lower.as_str())
            || (lower.chars().count() < 2 && !w.chars().any(|c| c.is_numeric()))
        {
            continue;
        }
        if out.iter().any(|o| o.to_lowercase() == lower) {
            continue;
        }
        out.push(w.to_owned());
    }
    out
}

/// The question with the asking phrase taken out and quotes and filler at
/// the edges trimmed; `None` when what is left only points back
/// ("browse about that").
fn rest_of(text: &str) -> Option<String> {
    let q = ASK.replace_all(text, " ");
    let q = GOOGLE_FIRST.replace(&q, " ");
    let q: String = q
        .chars()
        .map(|c| if "\"“”`".contains(c) { ' ' } else { c })
        .collect();
    let q = q
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c.is_whitespace() || ",.;:!?'".contains(c))
        .to_owned();
    let useful = keywords(&q)
        .into_iter()
        .filter(|w| !POINTERS.contains(&w.to_lowercase().as_str()))
        .count();
    (useful > 0 && q.chars().count() >= 3).then_some(q)
}

/// The search a message asks for with "use the web (tool)", "search the
/// web (for)", "look it up", "google it", "browse": the message without
/// that phrase. `None` when it asks for no search, or when what is left
/// only points back (use [`web_query_with`] for those).
pub fn web_query(text: &str) -> Option<String> {
    if !asks_for_web(text) {
        return None;
    }
    rest_of(text)
}

/// Whether a message sent with attachments asks about them ("what is
/// this picture?", "summarise the document", "what does it say") and not
/// plainly for the web: such a question is answered from the attachment,
/// never with a web search first (#70).
pub fn about_attachment(text: &str) -> bool {
    let l = text.to_lowercase();
    let explicit_web = [
        "the web",
        "online",
        "internet",
        "google",
        "on wikipedia",
        "search for",
    ]
    .iter()
    .any(|w| l.contains(w));
    if explicit_web {
        return false;
    }
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?x)\b(this|that|the|these|attached|my)\s+(picture|pic|image|photo|screenshot|screen\s*shot|document|doc|file|pdf|slide|slides|deck|sheet|spreadsheet|attachment|scan|page|pages)s?\b
            |\bwhat('s|\s+is|\s+are)?\s+(in\s+)?(this|that|it|these)\b
            |\b(summari[sz]e|translate|transcribe|explain|describe|read)\s+(it|this|that)\b
            |\bwhat\s+does\s+(it|this)\s+say\b",
        )
        .expect("static regex")
    });
    RE.is_match(&l) || l.trim().is_empty()
}

/// [`web_query`], where a message that only points back ("browse about
/// that", "search it online") searches for `previous`, the user's
/// previous question.
pub fn web_query_with(text: &str, previous: Option<&str>) -> Option<String> {
    if !asks_for_web(text) {
        return None;
    }
    rest_of(text).or_else(|| {
        previous
            .map(clean_previous)
            .and_then(|p| rest_of(&p).or_else(|| (!keywords(&p).is_empty()).then_some(p)))
    })
}

/// A stored user message without its attachments block.
fn clean_previous(p: &str) -> String {
    let p = p.split("<attachments>").next().unwrap_or(p);
    let (_, q) = crate::reply::split(p);
    q.trim().to_owned()
}

/// Shorter queries to try, in order, when a search for `query` found
/// nothing: its keywords, then the first three, two and one of them.
/// Never repeats a query already in the list or `query` itself.
pub fn fallback_queries(query: &str) -> Vec<String> {
    // Generic words ("model", "project", "definition") never make a
    // query on their own or pad one: a keyword index matches them
    // anywhere.
    let k: Vec<String> = keywords(query)
        .into_iter()
        .filter(|w| !crate::relevance::is_generic(w))
        .collect();
    // A question with several distinctive words ("SubQ Snap 1.1") is
    // never searched by one word alone: "SubQ" finds medical pages (#68).
    let several = crate::relevance::Topic::of(query).distinctive_words() >= 2;
    let mut out: Vec<String> = Vec::new();
    let mut add = |q: String| {
        if !q.is_empty()
            && !(several && !q.trim().contains(' '))
            && q.to_lowercase() != query.trim().to_lowercase()
            && !out.iter().any(|o| o.to_lowercase() == q.to_lowercase())
        {
            out.push(q);
        }
    };
    add(k.iter().take(6).cloned().collect::<Vec<_>>().join(" "));
    for n in [3, 2, 1] {
        if k.len() > n {
            add(k[..n].join(" "));
        }
    }
    // The longest word alone (a name is usually the longest word).
    // A version number alone ("1.1") is never a query.
    let words = k.iter().filter(|w| w.chars().any(char::is_alphabetic));
    if let Some(longest) = words.fold(None::<&String>, |best, w| match best {
        Some(b) if b.chars().count() >= w.chars().count() => Some(b),
        _ => Some(w),
    }) {
        add(longest.clone());
    }
    out
}

/// Whether a reply says the model cannot look things up or does not know:
/// "I don't have real-time access to the internet", "I can't browse",
/// "I'm not sure who …", "as of my knowledge cutoff".
pub fn is_refusal(reply: &str) -> bool {
    let r = reply.to_lowercase().replace(['’', '‘'], "'");
    // A long answer that mentions a cutoff in passing still answered.
    if r.chars().count() > 900 {
        return false;
    }
    const SIGNS: &[&str] = &[
        "real-time access",
        "realtime access",
        "real time access",
        "access to the internet",
        "access to real-time",
        "access to current",
        "access to up-to-date",
        "can't browse",
        "cannot browse",
        "can not browse",
        "unable to browse",
        "don't have the ability to browse",
        "do not have the ability to browse",
        "can't search the web",
        "cannot search the web",
        "unable to search",
        "can't access the internet",
        "cannot access the internet",
        "no internet access",
        "don't have internet",
        "do not have internet",
        "knowledge cutoff",
        "knowledge cut-off",
        "my training data",
        "i've been trained on",
        "i have been trained on",
        "i'm not sure who",
        "i am not sure who",
        "i'm not sure what",
        "i don't know who",
        "i do not know who",
        "i don't have information",
        "i do not have information",
        "i don't have any information",
    ];
    SIGNS.iter().any(|s| r.contains(s))
}

/// Whether `reply` says the model cannot open or read files ("I don't
/// have the capability to access or analyze files") (#70). With the file
/// already in the message as text, Tern points the model at it.
pub fn is_file_refusal(reply: &str) -> bool {
    let r = reply.to_lowercase().replace(['’', '‘'], "'");
    if r.chars().count() > 900 {
        return false;
    }
    let cannot = [
        "can't",
        "cannot",
        "can not",
        "unable to",
        "don't have the",
        "do not have the",
        "not able to",
        "no way to",
    ]
    .iter()
    .any(|w| r.contains(w));
    let files = [
        "access files",
        "access the file",
        "access file",
        "analyze files",
        "analyse files",
        "open files",
        "open the file",
        "read files",
        "read the file",
        "view the file",
        "view files",
        "see the file",
        "see attachments",
        "see the attachment",
        "access attachments",
        "access the attachment",
        "open attachments",
        "the document you",
        "access or analyze",
        "access documents",
        "access the document",
    ]
    .iter()
    .any(|w| r.contains(w));
    cannot && files
}

/// Whether `reply` is the same text as `previous` (ignoring case, spaces
/// and punctuation at the edges). Short replies ("Yes.", "Done.") can
/// rightly repeat, so only a reply of 40 characters or more counts.
pub fn same_reply(reply: &str, previous: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase()
    };
    let (a, b) = (norm(reply), norm(previous));
    a.chars().count() >= 40 && a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_that_says_it_cannot_open_files() {
        assert!(is_file_refusal(
            "I'm sorry, but I currently don't have the capability to access or analyze files, including the one you mentioned."
        ));
        assert!(is_file_refusal("I cannot open the file you sent."));
        assert!(!is_file_refusal("The file lists six team members."));
    }

    #[test]
    fn questions_about_an_attachment_never_search_the_web() {
        for q in [
            "What is this picture?",
            "What this image is all about",
            "WHat is this doc is about? if any user or student details available provide me.",
            "summarise it",
            "what does it say",
            "",
        ] {
            assert!(about_attachment(q), "{q}");
        }
        for q in [
            "search the web for who made this picture",
            "Who founded LangChain?",
            "look this up online",
        ] {
            assert!(!about_attachment(q), "{q}");
        }
        // The words Tern read from a picture never make a query (#70).
        let pic = crate::attach::Attachment::image("shot.png", b"\x89PNG\r\n\x1a\n....").unwrap();
        let msg = crate::attach::compose("What is this picture?", &[pic]);
        let msg = crate::attach::add_to_block(
            &msg,
            "Tron Legacy robot. Use the web to browse and search it online.",
        );
        let own = crate::attach::split(&msg).0;
        assert_eq!(own, "What is this picture?");
        assert_eq!(web_query_with(own, None), None);
    }

    const USER: &str = r#"can you provide me infor about how langchain erupted and comes out its developre and creator "use the web tool""#;

    #[test]
    fn the_users_message_asks_for_the_web_and_keeps_its_question() {
        let q = web_query(USER).unwrap();
        assert!(
            q.starts_with("can you provide me infor about how langchain"),
            "{q}"
        );
        assert!(!q.contains("web tool") && !q.contains('"'), "{q}");
        assert_eq!(
            keywords(&q),
            ["langchain", "erupted", "developre", "creator"]
        );
    }

    #[test]
    fn many_ways_of_asking_are_read() {
        for (text, want) in [
            (
                "use the web tool: latest Rust release",
                "latest Rust release",
            ),
            (
                "Search the web for tern migration routes",
                "tern migration routes",
            ),
            ("who won yesterday? look it up online", "who won yesterday"),
            ("who won yesterday? look it up", "who won yesterday"),
            (
                "google it: rust 1.90 release date",
                "rust 1.90 release date",
            ),
            ("google langchain founder", "langchain founder"),
            ("search online for the FLARE paper", "the FLARE paper"),
            ("browse the latest gpui release", "the latest gpui release"),
            ("use teh web tol, who made rust", "who made rust"),
            ("use web search: tern birds", "tern birds"),
            ("websearch mistral codestral", "mistral codestral"),
        ] {
            assert_eq!(web_query(text).as_deref(), Some(want), "{text}");
        }
        assert_eq!(web_query("what is a web server?"), None);
        assert_eq!(web_query("explain how browsers render pages"), None);
        assert_eq!(web_query("use the web"), None);
    }

    #[test]
    fn a_follow_up_that_points_back_searches_for_the_previous_question() {
        for text in [
            "browse about that",
            "search it online",
            "look it up",
            "google it",
            "use the web tool",
            "\"browse\" about this please",
        ] {
            assert_eq!(web_query(text), None, "{text}");
            assert_eq!(
                web_query_with(text, Some("Who created LangChain?")).as_deref(),
                Some("Who created LangChain"),
                "{text}"
            );
        }
        // The previous question had asked for the web too.
        assert_eq!(
            web_query_with("browse about that", Some(USER)).as_deref(),
            web_query(USER).as_deref()
        );
        assert_eq!(web_query_with("browse about that", None), None);
        // A plain question is not a search request.
        assert_eq!(
            web_query_with("thanks!", Some("Who created LangChain?")),
            None
        );
    }

    #[test]
    fn empty_searches_fall_back_to_shorter_keyword_queries() {
        let q = web_query(USER).unwrap();
        assert_eq!(
            fallback_queries(&q),
            [
                "langchain erupted developre",
                "langchain erupted",
                "langchain"
            ]
        );
        assert_eq!(fallback_queries("LangChain"), Vec::<String>::new());
        assert_eq!(fallback_queries("who is LangChain"), ["LangChain"]);
        assert_eq!(fallback_queries("who created langchain?"), ["langchain"]);
        // The user's searches (#67): never a lone generic word.
        assert_eq!(
            fallback_queries("harness2 by Google AI model or project"),
            ["harness2 Google", "harness2"]
        );
        assert_eq!(
            fallback_queries("SubQ Snap 1.1 AI model definition or meaning"),
            ["SubQ Snap 1.1", "SubQ Snap"]
        );
        // #68: several distinctive words never fall back to one of them.
        assert_eq!(
            fallback_queries("SubQ Snap 1.1 AI model"),
            ["SubQ Snap 1.1", "SubQ Snap"]
        );
    }

    #[test]
    fn refusals_and_copies_are_spotted() {
        let codestral = "I'm not sure who the creator of LangChain is. I don't have real-time access to the internet or up-to-date databases to provide that information. I can only provide information based on the data I've been trained on, which may not be current.";
        assert!(is_refusal(codestral));
        assert!(is_refusal("Sorry, I can’t browse the web."));
        assert!(!is_refusal(
            "LangChain was created by Harrison Chase in October 2022 [1]."
        ));
        assert!(same_reply(codestral, &format!("  {codestral}\n")));
        assert!(!same_reply(
            codestral,
            "LangChain was created by Harrison Chase."
        ));
        assert!(!same_reply("", ""));
        assert!(!same_reply("Yes.", "Yes."), "short replies may repeat");
    }
}
