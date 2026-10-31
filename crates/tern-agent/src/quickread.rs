//! Quick-path auto-read (#85, A13).
//!
//! A general factual question answered from `web_search` alone has only
//! snippets to quote, so its answers carried no quotes from read pages
//! ("How tall is the Eiffel Tower?" cited bare links). After such a
//! search Tern reads the top three results itself and hands back, with
//! the search, the opening of each page (about 512 tokens) and its one
//! passage that best matches the question.

/// Results read after a general question's search.
pub const TOP: usize = 3;

/// Characters of a page's opening handed back (about 512 tokens).
pub const HEAD_CHARS: usize = 2_000;

/// Characters of the best-matching passage handed back.
pub const CHUNK_CHARS: usize = 1_000;

/// Whether a turn's search may be followed by quick reads: a general
/// question (not a folder task, not a scholarly one, not small talk)
/// that does not point back at the previous answer.
pub fn applies(message: &str) -> bool {
    let (own, _) = crate::attach::split(message);
    let (_, own) = crate::reply::split(own);
    crate::intent::classify(own) == crate::intent::Intent::General
        && !crate::followup::points_back(own)
}

/// The result links of a `web_search` text, in rank order, at most `n`.
/// Each hit is listed as `n. title (site)` then its link on its own
/// indented line.
pub fn links(search: &str, n: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in search.lines() {
        let t = line.trim();
        if line.starts_with("   ")
            && (t.starts_with("https://") || t.starts_with("http://"))
            && !t.contains(char::is_whitespace)
            && !out.iter().any(|u| u == t)
        {
            out.push(t.to_owned());
            if out.len() >= n {
                break;
            }
        }
    }
    out
}

/// A page cut for the model: its opening, then the passage after it that
/// shares the most words with `question`, when that passage is not
/// already in the opening.
pub fn excerpt(page: &str, question: &str) -> String {
    let head = cut(page, HEAD_CHARS);
    if page.chars().count() <= HEAD_CHARS {
        return head;
    }
    let terms: Vec<String> = crate::webintent::keywords(question)
        .into_iter()
        .map(|w| w.to_lowercase())
        .collect();
    let rest: String = page.chars().skip(HEAD_CHARS).collect();
    let best = rest
        .split("\n\n")
        .flat_map(|para| chunks(para, CHUNK_CHARS))
        .map(|c| (score(&c, &terms), c))
        .filter(|(s, _)| *s > 0)
        .max_by_key(|(s, _)| *s)
        .map(|(_, c)| c);
    match best {
        Some(c) => format!("{head}…\n[best match further down] …{}…", c.trim()),
        None => format!("{head}…"),
    }
}

fn cut(t: &str, n: usize) -> String {
    t.chars().take(n).collect()
}

fn chunks(t: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = t.chars().collect();
    chars
        .chunks(n.max(1))
        .map(|c| c.iter().collect::<String>())
        .filter(|c| !c.trim().is_empty())
        .collect()
}

fn score(chunk: &str, terms: &[String]) -> usize {
    let lower = chunk.to_lowercase();
    terms.iter().filter(|t| lower.contains(t.as_str())).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_questions_only() {
        assert!(applies(
            "How tall is the Eiffel Tower, and when was it finished?"
        ));
        assert!(!applies(
            "How does FLARE compare with retrieving before every sentence?"
        ));
        assert!(!applies("hi there"));
        assert!(!applies("summarise those results"));
    }

    #[test]
    fn links_in_rank_order() {
        let s = "Web search results for \"eiffel tower height\" from Brave.\n\
                 1. Eiffel Tower (wikipedia.org)\n   https://en.wikipedia.org/wiki/Eiffel_Tower\n   330 m tall\n\
                 2. Facts (toureiffel.paris)\n   https://www.toureiffel.paris/en\n   snippet\n\
                 3. Again (wikipedia.org)\n   https://en.wikipedia.org/wiki/Eiffel_Tower\n   dup\n\
                 4. History (history.com)\n   https://www.history.com/eiffel\n   s\n\
                 5. More (x.org)\n   https://x.org/e\n   s\n";
        assert_eq!(
            links(s, TOP),
            vec![
                "https://en.wikipedia.org/wiki/Eiffel_Tower",
                "https://www.toureiffel.paris/en",
                "https://www.history.com/eiffel"
            ]
        );
    }

    #[test]
    fn excerpt_keeps_the_opening_and_the_best_passage() {
        let page = format!(
            "{}\n\nThe tower was completed in March 1889 for the World's Fair.\n\n{}",
            "Intro text. ".repeat(300),
            "Unrelated. ".repeat(200)
        );
        let e = excerpt(&page, "when was the Eiffel Tower completed");
        assert!(e.starts_with("Intro text."));
        assert!(e.contains("completed in March 1889"), "{e}");
        assert!(e.chars().count() < HEAD_CHARS + CHUNK_CHARS + 100);
        assert_eq!(excerpt("short page", "q"), "short page");
    }
}
