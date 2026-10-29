//! A paper's PDF text, page by page, cut into passages that carry their
//! page and section, and quotes found again with their locator.
//!
//! The pages come from Tern Crawl's PDF reader (`[page N]` markers). Section
//! headings are found by their shape: a numbered heading ("3 Method",
//! "2.1 Data", "IV. RESULTS") or a known name on a line of its own
//! ("Abstract", "Related Work", "Conclusions"). The bibliography is left out
//! of the passages.

use super::{Locator, Passage, PassageKind};

/// Headings papers use without a number.
const NAMED: &[&str] = &[
    "abstract",
    "introduction",
    "background",
    "related work",
    "related works",
    "preliminaries",
    "method",
    "methods",
    "methodology",
    "materials and methods",
    "approach",
    "model",
    "experiments",
    "experimental setup",
    "evaluation",
    "results",
    "results and discussion",
    "discussion",
    "analysis",
    "limitations",
    "conclusion",
    "conclusions",
    "future work",
    "acknowledgments",
    "acknowledgements",
    "references",
    "bibliography",
    "appendix",
];

/// Sections after which nothing is a passage worth quoting.
const END: &[&str] = &["references", "bibliography"];

/// Splits a reader's joined text (`[page N]` markers) back into pages.
pub fn pages_of(joined: &str) -> Vec<String> {
    let mut pages: Vec<String> = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for line in joined.lines() {
        let t = line.trim();
        if let Some(n) = t
            .strip_prefix("[page ")
            .and_then(|r| r.strip_suffix(']'))
            .and_then(|n| n.trim().parse::<usize>().ok())
        {
            if let Some((k, text)) = current.take() {
                while pages.len() + 1 < k {
                    pages.push(String::new());
                }
                pages.push(text);
            }
            current = Some((n, String::new()));
            continue;
        }
        match &mut current {
            Some((_, text)) => {
                text.push_str(line);
                text.push('\n');
            }
            None => current = Some((1, format!("{line}\n"))),
        }
    }
    if let Some((k, text)) = current {
        while pages.len() + 1 < k {
            pages.push(String::new());
        }
        pages.push(text);
    }
    pages
}

/// The heading a line is, if it is one: "3 Method", "2.1 Data sets",
/// "IV. RESULTS", "Abstract".
pub fn heading(line: &str) -> Option<String> {
    let t = line.trim();
    if t.is_empty() || t.chars().count() > 70 || t.ends_with(['.', ',', ';', ':']) {
        return None;
    }
    let lower = t.to_lowercase();
    if NAMED.contains(&lower.as_str()) {
        return Some(title_case_if_shouting(t));
    }
    // "3 Method", "3. Method", "2.1 Data", "IV. Results", "A Appendix".
    let (num, rest) = t.split_once(' ')?;
    let num_ok = {
        let n = num.trim_end_matches('.');
        let arabic = !n.is_empty()
            && n.split('.')
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            && n.split('.').count() <= 3
            && n.split('.').next().is_some_and(|p| p.len() <= 2);
        let roman = !n.is_empty()
            && n.len() <= 5
            && n.chars().all(|c| matches!(c, 'I' | 'V' | 'X'))
            && num.ends_with('.');
        arabic || roman
    };
    if !num_ok {
        return None;
    }
    let rest = rest.trim();
    let words: Vec<&str> = rest.split_whitespace().collect();
    let first_upper = rest.chars().next().is_some_and(char::is_uppercase);
    if words.is_empty() || words.len() > 8 || !first_upper {
        return None;
    }
    // A numbered line that is a sentence or a table row is not a heading.
    let letters = rest.chars().filter(|c| c.is_alphabetic()).count();
    let digits = rest.chars().filter(|c| c.is_ascii_digit()).count();
    if letters < 3 || digits > letters / 2 {
        return None;
    }
    Some(format!(
        "{} {}",
        num.trim_end_matches('.'),
        title_case_if_shouting(rest)
    ))
}

/// "RELATED WORK" as "Related Work"; anything else unchanged.
fn title_case_if_shouting(t: &str) -> String {
    let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() < 3 || letters.iter().any(|c| c.is_lowercase()) {
        return t.to_owned();
    }
    t.split_whitespace()
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_string() + &cs.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A heading and where it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// 1-based page.
    pub page: u32,
    /// Line on that page.
    pub line: usize,
    pub title: String,
}

/// Every heading in the pages, in order.
pub fn sections(pages: &[String]) -> Vec<Heading> {
    let mut out = Vec::new();
    for (pi, page) in pages.iter().enumerate() {
        for (li, line) in page.lines().enumerate() {
            if let Some(title) = heading(line) {
                out.push(Heading {
                    page: pi as u32 + 1,
                    line: li,
                    title,
                });
            }
        }
    }
    out
}

fn bare(title: &str) -> String {
    let t = title.trim();
    let t = match t.split_once(' ') {
        Some((n, rest))
            if n.trim_end_matches('.')
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || matches!(c, 'I' | 'V' | 'X')) =>
        {
            rest
        }
        _ => t,
    };
    t.to_lowercase()
}

/// The pages cut into passages (paragraphs, or about 120 words where a
/// page has no blank lines), each with its page and section. Stops at the
/// references.
pub fn passages(pages: &[String], paper: &str) -> Vec<Passage> {
    const WORDS: usize = 120;
    let mut out = Vec::new();
    let mut section: Option<String> = None;
    'pages: for (pi, page) in pages.iter().enumerate() {
        let mut buf: Vec<&str> = Vec::new();
        let flush = |buf: &mut Vec<&str>, section: &Option<String>, out: &mut Vec<Passage>| {
            let text = buf.join(" ");
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            buf.clear();
            if text.split_whitespace().count() < 6 {
                return;
            }
            out.push(Passage {
                paper: paper.to_owned(),
                text,
                kind: if section.as_deref().is_some_and(|s| bare(s) == "abstract") {
                    PassageKind::Abstract
                } else {
                    PassageKind::Body
                },
                locator: Locator {
                    page: Some(pi as u32 + 1),
                    section: section.clone(),
                    offset: None,
                },
            });
        };
        for line in page.lines() {
            if let Some(h) = heading(line) {
                flush(&mut buf, &section, &mut out);
                if END.contains(&bare(&h).as_str()) {
                    break 'pages;
                }
                section = Some(h);
                continue;
            }
            if line.trim().is_empty() {
                flush(&mut buf, &section, &mut out);
                continue;
            }
            buf.push(line.trim());
            if buf
                .iter()
                .map(|l| l.split_whitespace().count())
                .sum::<usize>()
                >= WORDS
            {
                flush(&mut buf, &section, &mut out);
            }
        }
        flush(&mut buf, &section, &mut out);
    }
    out
}

fn squash(t: &str) -> String {
    t.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where `quote` is in the pages: its page and the section in effect
/// there. Matched on letters and digits, so line breaks, hyphenation
/// spaces and punctuation do not matter; `None` when it is not there
/// (a quote that is not in the paper has no locator, and must not be
/// cited as if it were).
pub fn locate(pages: &[String], quote: &str) -> Option<Locator> {
    let q = squash(quote);
    if q.split(' ').count() < 3 {
        return None;
    }
    let heads = sections(pages);
    for (pi, page) in pages.iter().enumerate() {
        let lines: Vec<&str> = page.lines().collect();
        // The quote may run across lines: test each line with the text
        // after it, and take the first line it starts on.
        let whole = squash(page);
        if !whole.contains(&q) {
            continue;
        }
        let line_at = (0..lines.len())
            .take_while(|&li| squash(&lines[li..].join(" ")).contains(&q))
            .last()
            .unwrap_or(0);
        let page_no = pi as u32 + 1;
        let section = heads
            .iter()
            .rev()
            .find(|h| h.page < page_no || (h.page == page_no && h.line <= line_at))
            .map(|h| h.title.clone());
        return Some(Locator {
            page: Some(page_no),
            section,
            offset: None,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages() -> Vec<String> {
        vec![
            "Attention Is All You Need\nAshish Vaswani\nAbstract\nThe dominant sequence transduction models are based on complex recurrent networks.\nWe propose a new simple network architecture, the Transformer.\n\n1 Introduction\nRecurrent neural networks have been firmly established as state of the art.\n".into(),
            "3 Model Architecture\nMost competitive neural sequence transduction models have an\nencoder-decoder structure. The encoder maps an input sequence.\n\n3.1 Encoder and Decoder Stacks\nThe encoder is composed of a stack of N = 6 identical layers.\n".into(),
            "References\n[1] Jimmy Lei Ba, Jamie Ryan Kiros, and Geoffrey E Hinton. Layer normalization.\n".into(),
        ]
    }

    #[test]
    fn headings_by_shape() {
        assert_eq!(
            heading("3 Model Architecture").as_deref(),
            Some("3 Model Architecture")
        );
        assert_eq!(heading("2.1 Data").as_deref(), Some("2.1 Data"));
        assert_eq!(heading("IV. RESULTS").as_deref(), Some("IV Results"));
        assert_eq!(heading("ABSTRACT").as_deref(), Some("Abstract"));
        assert_eq!(heading("Related Work").as_deref(), Some("Related Work"));
        assert_eq!(heading("3 we show that this works."), None);
        assert_eq!(heading("12 34 56 78"), None);
        assert_eq!(heading("2017 The year it all began"), None);
        assert_eq!(heading("The encoder is composed of a stack."), None);
    }

    #[test]
    fn pages_round_trip_from_markers() {
        let joined = "[page 1]\nfirst\n\n[page 3]\nthird";
        let p = pages_of(joined);
        assert_eq!(p.len(), 3);
        assert!(p[0].contains("first"));
        assert!(p[1].is_empty());
        assert!(p[2].contains("third"));
    }

    #[test]
    fn passages_carry_page_and_section_and_skip_references() {
        let ps = passages(&pages(), "arxiv:1706.03762");
        assert!(ps.iter().all(|p| !p.text.contains("Layer normalization")));
        let enc = ps.iter().find(|p| p.text.contains("N = 6")).unwrap();
        assert_eq!(enc.locator.page, Some(2));
        assert_eq!(
            enc.locator.section.as_deref(),
            Some("3.1 Encoder and Decoder Stacks")
        );
        let abs = ps.iter().find(|p| p.text.contains("dominant")).unwrap();
        assert_eq!(abs.kind, PassageKind::Abstract);
        assert_eq!(abs.locator.label(), "p. 1 · Abstract");
    }

    #[test]
    fn quotes_are_found_across_line_breaks() {
        let l = locate(
            &pages(),
            "neural sequence transduction models have an encoder-decoder structure",
        )
        .unwrap();
        assert_eq!(l.page, Some(2));
        assert_eq!(l.section.as_deref(), Some("3 Model Architecture"));
        let l = locate(&pages(), "We propose a new simple network architecture").unwrap();
        assert_eq!(l.label(), "p. 1 · Abstract");
        assert_eq!(
            locate(&pages(), "transformers solve protein folding outright"),
            None
        );
    }
}
