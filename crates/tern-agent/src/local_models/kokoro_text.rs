//! The text side of read aloud (#69), compiled in every build: which
//! text Kokoro can read (English), how a Markdown answer is spoken, and
//! how it is cut into sentences for streaming. The voice model itself is
//! in `kokoro` (the `tts` feature).

/// The notice for text Kokoro cannot read.
pub const ENGLISH_ONLY: &str = "Read aloud supports English for now.";

/// The voices Settings › Speech offers (the four the manifest downloads):
/// id and label.
pub const VOICES: [(&str, &str); 4] = [
    ("af_heart", "Heart (American, female)"),
    ("am_michael", "Michael (American, male)"),
    ("bf_emma", "Emma (British, female)"),
    ("bm_george", "George (British, male)"),
];

/// The voice used when none is saved.
pub const DEFAULT_VOICE: &str = "af_heart";

/// Speeds Settings offers.
pub const SPEEDS: [f32; 5] = [0.8, 0.9, 1.0, 1.15, 1.3];

/// A saved voice id, or the default when it is not one of [`VOICES`].
pub fn voice_or_default(saved: Option<&str>) -> &'static str {
    saved
        .and_then(|s| VOICES.iter().find(|(id, _)| *id == s.trim()))
        .map_or(DEFAULT_VOICE, |(id, _)| *id)
}

/// Whether Kokoro can read `text`: mostly Latin letters. A message that
/// is mostly another script (Urdu, Arabic, Hindi) is not.
pub fn readable(text: &str) -> bool {
    let mut latin = 0usize;
    let mut other = 0usize;
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        if c.is_ascii_alphabetic() || ('\u{00C0}'..='\u{024F}').contains(&c) {
            latin += 1;
        } else {
            other += 1;
        }
    }
    latin > 0 && latin >= other * 3
}

/// The text of a message as it should be spoken: Markdown marks, code
/// blocks, links' addresses, and the tags Tern adds are left out.
pub fn speakable(markdown: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for line in markdown.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_code = !in_code;
            if in_code {
                out.push_str("(code block.)\n");
            }
            continue;
        }
        if in_code || t.starts_with('|') && t.ends_with('|') && t.contains("---") {
            continue;
        }
        let t = t.trim_start_matches(['#', '>', ' ']);
        let t = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .unwrap_or(t);
        out.push_str(t);
        out.push('\n');
    }
    // [text](url) -> text; bare URLs -> "a link"; emphasis and code marks.
    let link = regex::Regex::new(r"\[([^\]]*)\]\([^)]*\)").expect("static regex");
    let out = link.replace_all(&out, "$1");
    let url = regex::Regex::new(r"https?://\S+").expect("static regex");
    let out = url.replace_all(&out, "a link");
    let tags = regex::Regex::new(r"</?[a-zA-Z_][^>]*>").expect("static regex");
    let out = tags.replace_all(&out, "");
    out.replace(['*', '`', '_', '|'], " ")
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` cut into sentences (at `.`, `!`, `?`, `…` and line ends), each
/// at most about 300 characters, so playback starts after the first one
/// and stops between any two.
pub fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut cur = String::new();
        let chars: Vec<char> = line.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            cur.push(c);
            let end = matches!(c, '.' | '!' | '?' | '…')
                && chars.get(i + 1).is_none_or(|n| n.is_whitespace())
                && !ends_with_abbreviation(&cur);
            let too_long = cur.chars().count() >= 300 && c.is_whitespace();
            if end || too_long {
                let s = cur.trim();
                if s.chars().any(char::is_alphanumeric) {
                    out.push(s.to_owned());
                }
                cur.clear();
            }
        }
        let s = cur.trim();
        if s.chars().any(char::is_alphanumeric) {
            out.push(s.to_owned());
        }
    }
    out
}

fn ends_with_abbreviation(s: &str) -> bool {
    let last = s
        .trim_end_matches('.')
        .rsplit(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("");
    let l = last.to_ascii_lowercase();
    [
        "e.g", "i.e", "mr", "mrs", "ms", "dr", "vs", "etc", "no", "st",
    ]
    .contains(&l.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urdu_is_not_read_as_english() {
        assert!(readable("Hello from Tern."));
        assert!(!readable("سلام پاکستان، آپ کیسے ہیں؟"));
        assert!(readable("Lahore (لاہور) is a city in Pakistan."));
        assert!(!readable("12345"));
    }

    #[test]
    fn sentences_split_for_streaming() {
        let s =
            sentences("Hello there. This is Tern! Is it working? Yes e.g. like this.\nNew line");
        assert_eq!(
            s,
            [
                "Hello there.",
                "This is Tern!",
                "Is it working?",
                "Yes e.g. like this.",
                "New line"
            ]
        );
    }

    #[test]
    fn markdown_is_spoken_plainly() {
        let s = speakable(
            "# Title\n- **Bold** item with [a link](https://x.org)\n```rust\nfn main() {}\n```\nSee https://tern.dev now.",
        );
        assert_eq!(
            s,
            "Title\nBold item with a link\n(code block.)\nSee a link now."
        );
    }

    #[test]
    fn a_saved_voice_falls_back_to_the_default() {
        assert_eq!(voice_or_default(Some("bm_george")), "bm_george");
        assert_eq!(voice_or_default(Some("zz_none")), DEFAULT_VOICE);
        assert_eq!(voice_or_default(None), DEFAULT_VOICE);
    }
}
