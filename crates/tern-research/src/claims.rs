//! Claim extraction: the sentences of an answer that cite a source.
//!
//! An answer cites pages inline as `[1]`, `[2][3]`, `[1, 4]`, or `[2-4]`,
//! and may end with a short list of the links it cited. A claim is one
//! sentence carrying at least one citation; its text is the sentence with
//! the citation marks and Markdown taken out. The closing link list is
//! read for which URL each number means, never as claims.

/// One cited sentence of an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    /// 1-based, in order of appearance.
    pub n: usize,
    /// The sentence as plain text, without citation marks.
    pub text: String,
    /// Source numbers it cites, in order, without repeats.
    pub citations: Vec<usize>,
    /// Byte range of the sentence (with its citations) in the answer.
    pub span: (usize, usize),
}

/// A citation group at `at`: its numbers and the byte just past its `]`.
/// Years such as `[2017]` and Markdown links `[1](…)` are not citations.
pub fn citation_at(s: &str, at: usize) -> Option<(Vec<usize>, usize)> {
    let rest = s.get(at..)?;
    let inner_end = rest.find(']')?;
    if !rest.starts_with('[') || inner_end > 24 {
        return None;
    }
    let inner = &rest[1..inner_end];
    if inner.trim().is_empty()
        || !inner
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ',' | '-' | '–' | ' '))
    {
        return None;
    }
    let end = at + inner_end + 1;
    if s[end..].starts_with('(') {
        return None;
    }
    let mut out = Vec::new();
    for part in inner.split(',') {
        let nums: Vec<&str> = part.split(['-', '–']).map(str::trim).collect();
        if nums.iter().any(|p| p.is_empty() || p.len() > 2) {
            return None;
        }
        let nums: Vec<usize> = nums.iter().filter_map(|p| p.parse().ok()).collect();
        match nums.as_slice() {
            [a] => out.push(*a),
            [a, b] if a <= b && b - a < 20 => out.extend(*a..=*b),
            _ => return None,
        }
    }
    Some((out, end))
}

/// Every citation number in `text`, in order of first use.
pub fn citations(text: &str) -> Vec<usize> {
    let mut seen = Vec::new();
    for (i, _) in text.match_indices('[') {
        if let Some((nums, _)) = citation_at(text, i) {
            for n in nums {
                if !seen.contains(&n) {
                    seen.push(n);
                }
            }
        }
    }
    seen
}

/// The answer without its closing link list, as a byte length.
fn body_len(answer: &str) -> usize {
    let mut cut = answer.len();
    let mut offset = 0;
    let mut seen_list = false;
    for line in answer.split_inclusive('\n') {
        let t = line.trim();
        let lower = t.trim_matches(['#', '*', ':', ' ']).to_lowercase();
        let heading = matches!(
            lower.as_str(),
            "sources" | "references" | "links" | "citations" | "sources cited"
        );
        if heading || (is_link_line(t) && !seen_list) {
            if !seen_list {
                cut = offset;
            }
            seen_list = true;
        } else if seen_list && !t.is_empty() && !is_link_line(t) {
            // Prose after the list: the list was not the end after all.
            seen_list = false;
            cut = answer.len();
        }
        offset += line.len();
    }
    cut
}

/// A line of the closing list: `[1] Title — https://…`, `1. https://…`,
/// `- [1](https://…)`, `[1]: https://…`.
fn is_link_line(t: &str) -> bool {
    let t = t.trim_start_matches(['-', '*', ' ']);
    list_number(t).is_some() && t.contains("http")
}

fn list_number(t: &str) -> Option<usize> {
    if let Some(rest) = t.strip_prefix('[') {
        let end = rest.find(']')?;
        return rest[..end].trim().parse().ok();
    }
    let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    let after = t[digits.len()..].chars().next()?;
    matches!(after, '.' | ')' | ':').then(|| digits.parse().ok())?
}

/// The answer without its closing link list.
pub fn body(answer: &str) -> &str {
    answer[..body_len(answer)].trim_end()
}

/// The numbered links an answer lists at its end, as (number, URL).
pub fn link_list(answer: &str) -> Vec<(usize, String)> {
    let body = body_len(answer);
    let mut out = Vec::new();
    for line in answer[body..].lines() {
        let t = line.trim().trim_start_matches(['-', '*', ' ']);
        let Some(n) = list_number(t) else { continue };
        if let Some(url) = first_url(t)
            && !out.iter().any(|(m, _)| *m == n)
        {
            out.push((n, url));
        }
    }
    out
}

fn first_url(t: &str) -> Option<String> {
    let start = t.find("https://").or_else(|| t.find("http://"))?;
    let url: String = t[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && !matches!(c, ')' | '>' | '<' | '"' | ']'))
        .collect();
    let url = url.trim_end_matches(['.', ',', ';']).to_owned();
    (url.len() > "https://".len()).then_some(url)
}

const ABBREVIATIONS: [&str; 12] = [
    "e.g", "i.e", "al", "vs", "etc", "fig", "no", "dr", "mr", "ms", "st", "approx",
];

/// Sentence spans of `text`, citations kept with the sentence they follow.
pub fn sentences(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        let c = bytes[i];
        if c == b'\n' {
            push_span(text, &mut out, start, i);
            start = i + 1;
            i += 1;
            continue;
        }
        if matches!(c, b'.' | b'?' | b'!') && !abbreviation_before(text, i) {
            // Take trailing citations and closing marks with the sentence.
            let mut end = i + 1;
            loop {
                let rest = &text[end..];
                let skip = rest.len() - rest.trim_start_matches([' ', '*', '_', ')', '"']).len();
                if let Some((_, after)) = citation_at(text, end + skip) {
                    end = after;
                    continue;
                }
                if rest.starts_with(['*', '_', ')', '"']) {
                    end += 1;
                    continue;
                }
                break;
            }
            let next = text[end..].chars().next();
            if next.is_none_or(char::is_whitespace) {
                push_span(text, &mut out, start, end);
                start = end;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    push_span(text, &mut out, start, text.len());
    out
}

fn abbreviation_before(text: &str, dot: usize) -> bool {
    let before = &text[..dot];
    let word: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '.')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let lower = word.to_lowercase();
    // A decimal ("3.5") never reaches here: no whitespace follows its dot.
    ABBREVIATIONS.contains(&lower.as_str())
        || (word.len() == 1 && word.chars().all(|c| c.is_uppercase()))
}

fn push_span(text: &str, out: &mut Vec<(usize, usize)>, start: usize, end: usize) {
    let s = &text[start..end];
    let lead = s.len() - s.trim_start().len();
    let trail = s.len() - s.trim_end().len();
    if s.trim().is_empty() {
        return;
    }
    out.push((start + lead, end - trail));
}

/// A sentence as plain text: citation marks, Markdown emphasis, list
/// bullets, headings, and link targets taken out.
pub fn plain(sentence: &str) -> String {
    let mut out = String::with_capacity(sentence.len());
    let mut i = 0;
    while i < sentence.len() {
        let rest = &sentence[i..];
        if rest.starts_with('[') {
            if let Some((_, end)) = citation_at(sentence, i) {
                i = end;
                continue;
            }
            // [text](url) keeps its text.
            if let Some(close) = rest.find("](")
                && let Some(paren) = rest[close..].find(')')
            {
                out.push_str(&rest[1..close]);
                i += close + paren + 1;
                continue;
            }
        }
        let c = rest.chars().next().expect("non-empty");
        if !matches!(c, '*' | '`') {
            out.push(c);
        }
        i += c.len_utf8();
    }
    let t = out.trim();
    let t = t.trim_start_matches(['#', '>', '-', ' ']);
    let t = strip_list_number(t);
    collapse_spaces(t)
        .replace(" .", ".")
        .replace(" ,", ",")
        .trim()
        .to_owned()
}

fn strip_list_number(t: &str) -> &str {
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && digits < 3 && t[digits..].starts_with(". ") {
        &t[digits + 2..]
    } else {
        t
    }
}

fn collapse_spaces(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The cited sentences of `answer`, in order.
pub fn extract(answer: &str) -> Vec<Claim> {
    let body = &answer[..body_len(answer)];
    let mut out = Vec::new();
    for (a, b) in sentences(body) {
        let raw = &body[a..b];
        let cites = citations(raw);
        if cites.is_empty() {
            continue;
        }
        let text = plain(raw);
        if text.chars().filter(|c| c.is_alphanumeric()).count() < 3 {
            continue;
        }
        out.push(Claim {
            n: out.len() + 1,
            text,
            citations: cites,
            span: (a, b),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citation_groups() {
        assert_eq!(
            citations("A [1]. B [2][3]. C [1, 4]. D [5-7]. E [9–8]."),
            [1, 2, 3, 4, 5, 6, 7]
        );
        assert!(citations("a year like [2017], a link [1](https://x), [ ]").is_empty());
    }

    #[test]
    fn claims_are_cited_sentences() {
        let answer = "**Retrieval-augmented generation** pairs a retriever with a generator [1]. \
It was introduced by Lewis et al. in 2020 [1][2]. Some think it is overrated.\n\n\
- Vector search finds passages by meaning [3].\n\nSources:\n[1] RAG paper https://arxiv.org/abs/2005.11401\n[2] https://ai.meta.com/blog/rag\n";
        let claims = extract(answer);
        let texts: Vec<_> = claims.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Retrieval-augmented generation pairs a retriever with a generator.",
                "It was introduced by Lewis et al. in 2020.",
                "Vector search finds passages by meaning.",
            ]
        );
        assert_eq!(claims[1].citations, [1, 2]);
        assert_eq!(claims[2].n, 3);
        assert_eq!(
            link_list(answer),
            [
                (1, "https://arxiv.org/abs/2005.11401".to_owned()),
                (2, "https://ai.meta.com/blog/rag".to_owned())
            ]
        );
    }

    #[test]
    fn citation_after_the_full_stop_stays_with_its_sentence() {
        let claims = extract("Water boils at 100 °C at sea level. [2] It freezes at 0 °C.[3]");
        assert_eq!(claims.len(), 2);
        assert_eq!(claims[0].citations, [2]);
        assert_eq!(claims[0].text, "Water boils at 100 °C at sea level.");
        assert_eq!(claims[1].citations, [3]);
    }

    #[test]
    fn markdown_links_and_decimals() {
        let claims = extract("Python [3.12](https://python.org) is 1.5x faster on this [4].");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].text, "Python 3.12 is 1.5x faster on this.");
    }

    #[test]
    fn list_styles() {
        let a = "Text [1].\n\n1. [Title](https://a.test/x)\n2) https://b.test/y.\n- [3]: https://c.test";
        assert_eq!(
            link_list(a),
            [
                (1, "https://a.test/x".to_owned()),
                (2, "https://b.test/y".to_owned()),
                (3, "https://c.test".to_owned())
            ]
        );
        assert_eq!(extract(a).len(), 1);
        assert!(link_list("No list here [1].").is_empty());
    }
}
