//! A guard for live dictation text (#68).
//!
//! Speech models invent text when they cannot follow: Moonshine (English
//! only) heard Urdu on the user's laptop and wrote "You will not be able
//! to do anything" over and over, and Whisper writes "Thank you." on
//! silence. [`clean`] drops what a window repeats (inside itself or from
//! the text just before it) and the phrases models write on silence.

/// Phrases speech models write on silence or noise, compared without
/// case or punctuation.
const ON_SILENCE: &[&str] = &[
    "thank you",
    "thank you so much",
    "thank you very much",
    "thanks",
    "thanks for watching",
    "thank you for watching",
    "thanks for listening",
    "thank you for listening",
    "please subscribe",
    "like and subscribe",
    "subscribe to my channel",
    "you",
    "bye",
    "bye bye",
    "okay",
    "so",
    "uh",
    "um",
    "hmm",
];

/// Phrases that are never speech in a dictation, loud or quiet (video
/// captions in Whisper's training data).
const NEVER: &[&str] = &[
    "thanks for watching",
    "thank you for watching",
    "please subscribe",
    "like and subscribe",
    "subscribe to my channel",
    "subtitles by the amara org community",
    "subtitles by",
    "transcribed by",
    "amara org",
];

/// Lower case, letters and digits only, single spaces.
fn norm(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sentences with their end marks (`.`, `!`, `?`, `؟`, `۔`).
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        cur.push(c);
        if matches!(c, '.' | '!' | '?' | '؟' | '۔' | '。') {
            let t = cur.trim().to_owned();
            if !t.is_empty() {
                out.push(t);
            }
            cur.clear();
        }
    }
    let t = cur.trim().to_owned();
    if !t.is_empty() {
        out.push(t);
    }
    out
}

/// Words with a run of the same 1–8 words collapsed to one copy: "you
/// will not be able to do anything you will not be able to do anything"
/// becomes one. Short runs (1–2 words) need three copies, so "very very"
/// stays.
fn collapse_runs(words: Vec<&str>) -> Vec<&str> {
    let key = |w: &str| norm(w);
    let mut out: Vec<&str> = Vec::new();
    for w in words {
        out.push(w);
        for n in 1..=8usize {
            let copies = if n <= 2 { 3 } else { 2 };
            if out.len() < n * copies {
                continue;
            }
            let tail = &out[out.len() - n..];
            let repeated = (1..copies).all(|k| {
                let start = out.len() - n * (k + 1);
                out[start..start + n]
                    .iter()
                    .zip(tail)
                    .all(|(a, b)| key(a) == key(b))
            });
            if repeated {
                let keep = out.len() - n * (copies - 1);
                out.truncate(keep);
                break;
            }
        }
    }
    out
}

/// `text` (one dictation window's transcript) without repeats and
/// silence phrases. `before` is the dictated text just before it (a
/// sentence it repeats is dropped); `quiet` says the window held little
/// sound, so a lone silence phrase in it is dropped too.
pub fn clean(text: &str, before: &str, quiet: bool) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let total = words.len();
    let words = collapse_runs(words);
    let looped = words.len() < total;
    let text = words.join(" ");
    let earlier: Vec<String> = sentences(before)
        .iter()
        .rev()
        .take(3)
        .map(|s| norm(s))
        .collect();
    let mut kept: Vec<String> = Vec::new();
    for s in sentences(&text) {
        let n = norm(&s);
        if n.is_empty()
            || NEVER
                .iter()
                .any(|p| n == *p || n.starts_with(&format!("{p} ")))
        {
            continue;
        }
        // The same sentence again, in this window or from the last one.
        if kept.last().is_some_and(|k| norm(k) == n) || earlier.contains(&n) {
            continue;
        }
        kept.push(s);
    }
    let all_silence =
        !kept.is_empty() && kept.iter().all(|s| ON_SILENCE.contains(&norm(s).as_str()));
    if all_silence && (quiet || looped || kept.len() > 1) {
        return String::new();
    }
    kept.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loops_from_the_users_laptop_are_cut() {
        let t = "You will not be able to do anything. You will not be able to do anything. You will not be able to do anything.";
        assert_eq!(clean(t, "", false), "You will not be able to do anything.");
        let t = "I will tell you that you will not be able to do anything you will not be able to do anything";
        assert_eq!(
            clean(t, "", false),
            "I will tell you that you will not be able to do anything"
        );
        // Repeated from the window before.
        assert_eq!(
            clean(
                "You will not be able to do anything.",
                "Hello. You will not be able to do anything.",
                false
            ),
            ""
        );
    }

    #[test]
    fn silence_phrases_go_when_the_window_was_quiet() {
        assert_eq!(clean("Thank you.", "", true), "");
        assert_eq!(clean("Thank you. Thank you. Thank you.", "", false), "");
        assert_eq!(clean("Thank you.", "", false), "Thank you.");
        assert_eq!(clean("Thanks for watching!", "", false), "");
        assert_eq!(clean("Subtitles by the Amara.org community", "", false), "");
        assert_eq!(
            clean("Thank you for the help with my code.", "", true),
            "Thank you for the help with my code."
        );
    }

    #[test]
    fn real_speech_stays() {
        assert_eq!(clean("آپ کیسے ہیں؟", "", false), "آپ کیسے ہیں؟");
        assert_eq!(clean("very very good", "", false), "very very good");
        assert_eq!(clean("no no no", "", false), "no");
        assert_eq!(clean("", "", true), "");
    }
}
