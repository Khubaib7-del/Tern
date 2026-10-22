//! Small text helpers shared by the study modules: words without stop
//! words, sentences, overlap scores, slugs, and pulling JSON out of a
//! model's reply.

use serde_json::Value;

const STOP: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "had", "her", "was",
    "one", "our", "out", "has", "have", "his", "how", "its", "may", "new", "now", "old", "see",
    "two", "who", "did", "get", "use", "used", "uses", "using", "with", "that", "this", "from",
    "they", "them", "then", "than", "there", "their", "these", "those", "what", "when", "where",
    "which", "while", "will", "would", "could", "should", "into", "onto", "about", "also", "been",
    "being", "were", "each", "such", "some", "more", "most", "much", "many", "very", "only",
    "other", "over", "under", "between", "because", "does", "doing", "done", "your", "yours",
    "here", "just", "like", "make", "made", "same", "both", "well", "upon", "page",
];

/// Lowercase words of three or more letters that are not stop words.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '-')
        .map(|w| w.trim_matches('-').to_lowercase())
        .filter(|w| w.chars().count() >= 3 && !STOP.contains(&w.as_str()))
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
        .collect()
}

/// Share of `key`'s distinct words that also appear in `text` (0 to 1).
pub fn overlap(key: &str, text: &str) -> f32 {
    let k: std::collections::BTreeSet<String> = words(key).into_iter().collect();
    if k.is_empty() {
        return 0.0;
    }
    let t: std::collections::BTreeSet<String> = words(text).into_iter().collect();
    let hit = k
        .iter()
        .filter(|w| t.contains(*w) || t.iter().any(|x| near(w, x)))
        .count();
    hit as f32 / k.len() as f32
}

/// Two words that share a stem: one starts with the other and they differ
/// by at most three letters (`network`, `networks`, `networking`).
fn near(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    short.len() >= 4 && long.starts_with(short) && long.len() - short.len() <= 3
}

/// Sentences of `text`, trimmed, on one line each.
pub fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' && chars.get(i + 1) == Some(&'\n') {
            push_sentence(&mut out, &mut cur);
            continue;
        }
        cur.push(if c == '\n' { ' ' } else { c });
        if matches!(c, '.' | '?' | '!') {
            let next = chars.get(i + 1).copied();
            if next.is_none_or(char::is_whitespace) {
                push_sentence(&mut out, &mut cur);
            }
        }
    }
    push_sentence(&mut out, &mut cur);
    out
}

fn push_sentence(out: &mut Vec<String>, cur: &mut String) {
    let s = one_line(cur);
    if !s.is_empty() {
        out.push(s);
    }
    cur.clear();
}

/// Whitespace folded to single spaces.
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Lowercase, letters and digits only, single spaces: for comparing answers.
pub fn normalise(s: &str) -> String {
    let t: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let t = one_line(&t);
    t.strip_prefix("the ")
        .or_else(|| t.strip_prefix("a "))
        .or_else(|| t.strip_prefix("an "))
        .unwrap_or(&t)
        .to_owned()
}

/// Edit distance, for forgiving small typos in one-word answers.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// A short id from a title: `Neural networks` → `neural-networks`.
pub fn slug(s: &str) -> String {
    let t: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let t = t.split_whitespace().take(6).collect::<Vec<_>>().join("-");
    if t.is_empty() {
        "topic".into()
    } else {
        t.chars().take(48).collect()
    }
}

/// At most `n` characters, cut at a word with `…` when cut.
pub fn clip(s: &str, n: usize) -> String {
    let s = one_line(s);
    if s.chars().count() <= n {
        return s;
    }
    let cut: String = s.chars().take(n).collect();
    let cut = cut
        .rsplit_once(' ')
        .map(|(a, _)| a.to_owned())
        .unwrap_or(cut);
    format!("{cut}…")
}

/// The most frequent content words of `text`, most frequent first.
pub fn keywords(text: &str, n: usize) -> Vec<String> {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for w in words(text) {
        if w.chars().count() >= 4 {
            *counts.entry(w).or_default() += 1;
        }
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter().take(n).map(|(w, _)| w).collect()
}

/// Where `needle` is in `hay`, ignoring case, as a byte range safe to
/// slice with.
pub fn find_ci(hay: &str, needle: &str) -> Option<std::ops::Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let lower = hay.to_lowercase();
    let n = needle.to_lowercase();
    if lower.len() == hay.len()
        && let Some(at) = lower.find(&n)
    {
        let end = at + n.len();
        if hay.is_char_boundary(at) && hay.is_char_boundary(end) {
            return Some(at..end);
        }
    }
    let at = hay.find(needle)?;
    Some(at..at + needle.len())
}

/// The first word capitalised.
pub fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The first JSON value in `text`: the whole text, a fenced block, or from
/// the first `{` or `[` to its matching close.
pub fn json_in(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        return Some(v);
    }
    let mut best: Option<(usize, Value)> = None;
    for (open, close) in [('{', '}'), ('[', ']')] {
        let Some(start) = text.find(open) else {
            continue;
        };
        if best.as_ref().is_some_and(|(s, _)| *s < start) {
            continue;
        }
        let mut depth = 0i32;
        let mut in_str = false;
        let mut escaped = false;
        for (i, c) in text[start..].char_indices() {
            if in_str {
                match (escaped, c) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, '"') => in_str = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                c if c == open => depth += 1,
                c if c == close => {
                    depth -= 1;
                    if depth == 0 {
                        if let Ok(v) = serde_json::from_str(&text[start..start + i + 1]) {
                            best = Some((start, v));
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    best.map(|(_, v)| v)
}

/// A string field under any of `keys`.
pub fn field<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// A list of strings under any of `keys` (a single string counts as one).
pub fn list(v: &Value, keys: &[&str]) -> Vec<String> {
    for k in keys {
        match v.get(*k) {
            Some(Value::Array(a)) => {
                return a
                    .iter()
                    .filter_map(|x| match x {
                        Value::String(s) => Some(one_line(s)),
                        Value::Number(n) => Some(n.to_string()),
                        Value::Bool(b) => Some(b.to_string()),
                        _ => None,
                    })
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            Some(Value::String(s)) if !s.trim().is_empty() => return vec![one_line(s)],
            _ => {}
        }
    }
    Vec::new()
}

/// The source numbers a reply cites as `[S2]` or `[S2, S4]`.
pub fn cited_sources(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' {
            if let Some(end) = text[i..].find(']') {
                let inner = &text[i + 1..i + end];
                for part in inner.split([',', ';']) {
                    let p = part.trim();
                    if let Some(n) = p.strip_prefix('S').or_else(|| p.strip_prefix('s'))
                        && let Ok(n) = n.trim().parse::<usize>()
                        && !out.contains(&n)
                    {
                        out.push(n);
                    }
                }
                i += end;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_drop_stop_words_and_numbers() {
        assert_eq!(words("The cat and 42 dogs"), vec!["cat", "dogs"]);
    }

    #[test]
    fn overlap_counts_stems() {
        assert!(overlap("neural networks learn", "a network that learns weights") > 0.6);
        assert_eq!(overlap("", "x"), 0.0);
    }

    #[test]
    fn sentences_split_on_stops_and_blank_lines() {
        let s = sentences("One here. Two there!\n\nHeading\nThree e.g. more");
        assert_eq!(s[0], "One here.");
        assert_eq!(s[1], "Two there!");
        assert!(s[2].starts_with("Heading Three"));
    }

    #[test]
    fn json_found_in_prose_and_fences() {
        let v = json_in("Sure:\n```json\n{\"a\": [1, 2]}\n```").unwrap();
        assert_eq!(v["a"][1], 2);
        let v = json_in("x [1, {\"b\": \"]\"}] y").unwrap();
        assert!(v.is_array());
        assert!(json_in("no json").is_none());
    }

    #[test]
    fn cited_sources_parse() {
        assert_eq!(
            cited_sources("as shown [S2] and [S1, S3]. [4]"),
            vec![2, 1, 3]
        );
    }

    #[test]
    fn normalise_and_edit_distance() {
        assert_eq!(normalise("  The Mitochondria! "), "mitochondria");
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(
            slug("Neural Networks: an intro"),
            "neural-networks-an-intro"
        );
    }
}
