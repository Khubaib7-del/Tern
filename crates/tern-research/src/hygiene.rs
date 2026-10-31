//! Evidence hygiene (#85, A14): the small cleanups that keep checks
//! honest. Footnote and citation markers come off a passage before its
//! numbers are compared (a paper's `[130]` is not the number 130); titles
//! from Crossref and OpenAlex have their HTML entities decoded and their
//! tags dropped, once, however many times they were escaped; a refuting
//! passage must name the claim's main entity; and an acronym the answer
//! spells out is flagged when no source spells it out the same way.

/// `text` without bracketed reference markers (`[12]`, `[3, 4]`,
/// `[5–7]`) and superscript digits (`¹²`, `^12`). Brackets holding words
/// stay.
pub fn strip_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find(']') {
            Some(j) if is_marker(&after[..j]) => rest = &after[j + 1..],
            _ => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    let mut clean = String::with_capacity(out.len());
    let mut chars = out.chars().peekable();
    while let Some(c) = chars.next() {
        if is_superscript_digit(c) {
            continue;
        }
        if c == '^' && chars.peek().is_some_and(char::is_ascii_digit) {
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
            }
            continue;
        }
        clean.push(c);
    }
    clean
}

/// A bracket's inside is a reference marker: digits with commas, spaces
/// and dashes only.
fn is_marker(inside: &str) -> bool {
    let t = inside.trim();
    !t.is_empty()
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ',' | ' ' | '-' | '–' | '—' | ';'))
}

fn is_superscript_digit(c: char) -> bool {
    matches!(c, '⁰' | '¹' | '²' | '³' | '⁴' | '⁵' | '⁶' | '⁷' | '⁸' | '⁹')
}

/// HTML entities decoded once per layer, until none are left (at most
/// three layers): `&amp;lt;i&amp;gt;` becomes `<i>`, never `&lt;i&gt;`.
pub fn decode_entities(text: &str) -> String {
    let mut t = text.to_owned();
    for _ in 0..3 {
        let next = decode_layer(&t);
        if next == t {
            break;
        }
        t = next;
    }
    t
}

fn decode_layer(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let end = after.find(';').filter(|&e| e <= 10);
        let decoded = end.and_then(|e| entity(&after[1..e]).map(|c| (c, e)));
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &after[e + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ndash" => '–',
        "mdash" => '—',
        _ => {
            let n = name.strip_prefix('#')?;
            let code = match n.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(code)?
        }
    })
}

/// A title as one line of plain text: entities decoded (every layer),
/// inline tags (`<i>`, `<sub>`, JATS) dropped, spaces collapsed.
pub fn clean_title(title: &str) -> String {
    let decoded = decode_entities(title);
    let mut out = String::with_capacity(decoded.len());
    let mut in_tag = false;
    for c in decoded.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The claim's main entity: its first acronym (`FLARE`, `GPT-4`), else
/// its first capitalised word after the first, looked for in the claim's
/// first half only, where its subject is. `None` when it names none
/// there (a refutation is then judged on its words alone): in "The theory
/// of general relativity was published by Niels Bohr in 1915" the name is
/// what the claim says about its subject, and a passage naming another
/// author still refutes it.
pub fn main_entity(claim: &str) -> Option<String> {
    let words: Vec<&str> = claim
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ':' | '(' | ')' | '"'))
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();
    let half = words.len().div_ceil(2);
    let subject = &words[..half];
    if let Some(a) = subject.iter().find(|w| is_acronym(w)) {
        return Some((*a).to_owned());
    }
    subject
        .iter()
        .skip(1)
        .find(|w| {
            w.chars().next().is_some_and(char::is_uppercase)
                && w.chars().count() >= 3
                && !MONTHS.contains(*w)
        })
        .map(|w| (*w).to_owned())
}

const MONTHS: &[&str] = &[
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn is_acronym(w: &str) -> bool {
    let letters = w.chars().filter(char::is_ascii_alphabetic).count();
    letters >= 2
        && w.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && w.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
}

/// Whether a passage may refute the claim: it names the claim's main
/// entity, or the claim names none.
pub fn may_refute(claim: &str, passage: &str) -> bool {
    match main_entity(claim) {
        Some(e) => passage.to_lowercase().contains(&e.to_lowercase()),
        None => true,
    }
}

/// Acronym expansions in an answer (`Forward-Looking Active REtrieval
/// (FLARE)`, or `FLARE (Forward-Looking ...)`) that no source text spells
/// out: `(acronym, expansion)`, to be flagged "not from sources".
pub fn unsupported_expansions(answer: &str, sources: &[&str]) -> Vec<(String, String)> {
    let joined = sources.join("\n").to_lowercase();
    let squash = |s: &str| {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let squashed_sources = squash(&joined);
    let mut out: Vec<(String, String)> = Vec::new();
    for (acr, exp) in expansions(answer) {
        if squashed_sources.contains(&squash(&exp)) {
            continue;
        }
        if !out.iter().any(|(a, _)| *a == acr) {
            out.push((acr, exp));
        }
    }
    out
}

/// Every `Expansion (ACR)` and `ACR (Expansion)` in a text whose words'
/// initials spell the acronym.
pub fn expansions(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(o) = text[from..].find('(') {
        let open = from + o;
        let Some(c) = text[open..].find(')') else {
            break;
        };
        let close = open + c;
        let inside = text[open + 1..close].trim();
        let before = text[..open].trim_end();
        if is_acronym(inside) {
            // `Expansion (ACR)`: as many words before as the acronym has
            // letters, give or take small words.
            if let Some(exp) = expansion_before(before, inside) {
                out.push((inside.to_owned(), exp));
            }
        } else if let Some(acr) = before
            .rsplit(|c: char| c.is_whitespace())
            .next()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-'))
            .filter(|w| is_acronym(w))
            && initials_match(inside, acr)
        {
            out.push((acr.to_owned(), inside.to_owned()));
        }
        from = close + 1;
    }
    out
}

const SMALL: &[&str] = &[
    "of", "and", "for", "the", "in", "on", "to", "a", "an", "with",
];

/// The initials of `phrase`'s words (small words skipped, hyphenated parts
/// counted) spell `acr`'s letters, or `acr` starts the phrase's words.
fn initials_match(phrase: &str, acr: &str) -> bool {
    let want: String = acr
        .chars()
        .filter(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_lowercase();
    let parts: Vec<String> = phrase
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|w| !w.is_empty() && !SMALL.contains(&w.to_lowercase().as_str()))
        .map(str::to_lowercase)
        .collect();
    let initials: String = parts.iter().filter_map(|w| w.chars().next()).collect();
    if initials == want {
        return true;
    }
    // Acronyms that take two letters from one word (`REtrieval`): the
    // letters appear in order across the words, each word giving one or
    // more from its start.
    let mut need = want.chars().peekable();
    for w in &parts {
        let mut took = false;
        for ch in w.chars() {
            match need.peek() {
                Some(&n) if n == ch => {
                    need.next();
                    took = true;
                }
                _ => break,
            }
        }
        if !took {
            return false;
        }
    }
    need.peek().is_none()
}

fn expansion_before(before: &str, acr: &str) -> Option<String> {
    let words: Vec<&str> = before.split_whitespace().collect();
    let letters = acr.chars().filter(char::is_ascii_alphabetic).count();
    // Try the shortest run of trailing words whose initials spell it.
    for n in 1..=words.len().min(letters + 4) {
        let phrase = words[words.len() - n..].join(" ");
        let phrase = phrase.trim_matches(|c: char| !c.is_alphanumeric());
        let first = phrase.split_whitespace().next().unwrap_or_default();
        if SMALL.contains(&first.to_lowercase().as_str()) {
            continue;
        }
        if initials_match(phrase, acr) {
            return Some(phrase.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_come_off_before_numbers_are_read() {
        // A-T12 (#85): "[130]" is never the number 130.
        assert_eq!(
            strip_markers("Accuracy rose 12%[130] in 2023 [3, 4]."),
            "Accuracy rose 12% in 2023 ."
        );
        assert_eq!(strip_markers("see [5–7] and x²"), "see  and x");
        assert_eq!(strip_markers("a^12 b [Table 2]"), "a b [Table 2]");
        assert_eq!(strip_markers("open [ only"), "open [ only");
    }

    #[test]
    fn titles_are_decoded_once_and_untagged() {
        // A-T12: `&amp;lt;i&amp;gt;` becomes `<i>`, which is dropped.
        assert_eq!(
            clean_title("Growth of &amp;lt;i&amp;gt;E. coli&amp;lt;/i&amp;gt; in milk"),
            "Growth of E. coli in milk"
        );
        assert_eq!(clean_title("Costs &amp; totals"), "Costs & totals");
        assert_eq!(decode_entities("&#8211; &#x2014; &amp;amp;"), "– — &");
        assert_eq!(decode_entities("AT&T R&D"), "AT&T R&D");
    }

    #[test]
    fn a_refutation_must_name_the_main_entity() {
        assert_eq!(
            main_entity("FLARE retrieves when tokens are uncertain."),
            Some("FLARE".into())
        );
        assert_eq!(
            main_entity("The tower in Paris is 330 metres tall."),
            Some("Paris".into())
        );
        assert_eq!(main_entity("Sales rose in May."), None);
        // A name in the claim's second half is what it says, not whom it
        // is about: another author in the passage still refutes it.
        assert_eq!(
            main_entity("The theory of general relativity was published by Niels Bohr in 1915."),
            None
        );
        assert!(may_refute(
            "FLARE retrieves every 64 tokens.",
            "Unlike FLARE, the baseline retrieves every 32 tokens."
        ));
        assert!(!may_refute(
            "FLARE retrieves every 64 tokens.",
            "Self-RAG retrieves every 32 tokens."
        ));
    }

    #[test]
    fn an_expansion_no_source_spells_out_is_flagged() {
        // A-T12: the acronym guard.
        let answer = "Forward-Looking Active Retrieval (FLARE) is one method. RAG (Retrieval-Augmented Generation) is another.";
        let sources = ["FLARE stands for Forward-Looking Active REtrieval, a method."];
        assert_eq!(
            unsupported_expansions(answer, &sources),
            [(
                "RAG".to_owned(),
                "Retrieval-Augmented Generation".to_owned()
            )]
        );
        assert_eq!(
            expansions("Low-Rank Adaptation (LoRA) helps"),
            Vec::<(String, String)>::new(),
            "mixed-case short forms are not acronyms here"
        );
        assert_eq!(
            expansions("We use the Large Language Model (LLM) here."),
            [("LLM".to_owned(), "Large Language Model".to_owned())]
        );
    }
}
