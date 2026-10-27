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

/// Sentences with their end marks (`.`, `!`, `?`, `؟`, `۔`). A mark
/// ends a sentence only before a space or the end, so "Amara.org" and
/// "3.5" stay whole.
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        cur.push(c);
        let at_break = chars.peek().is_none_or(|n| n.is_whitespace());
        if at_break && matches!(c, '.' | '!' | '?' | '؟' | '۔' | '。') {
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
                    .all(|(a, b)| norm(a) == norm(b))
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

/// Without the sound tags models write instead of words (#70: Moonshine
/// began a dictation with "[inaudible]"): `[inaudible]`, `[Music]`,
/// `(laughs)`, `[BLANK_AUDIO]`, `*cough*`.
pub fn drop_tags(text: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\[[^\]]{1,40}\]|\((?i:inaudible|laughs?|laughter|music|applause|silence|cough(?:s|ing)?|sighs?|noise|unintelligible|crosstalk|background noise)\)|\*[a-zA-Z ]{1,20}\*")
            .expect("static regex")
    });
    RE.replace_all(text, " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` (one dictation window's transcript) without repeats and
/// silence phrases. `before` is the dictated text just before it (a
/// sentence it repeats is dropped); `quiet` says the window held little
/// sound, so a lone silence phrase in it is dropped too.
pub fn clean(text: &str, before: &str, quiet: bool) -> String {
    let text = drop_tags(text);
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

/// A sentence that loops a few words ("Muslims and Muslims and the
/// Muslims and Muslims"): six words or more, under half of them
/// different (#71).
fn loops(s: &str) -> bool {
    let n = norm(s);
    let words: Vec<&str> = n.split_whitespace().collect();
    if words.len() < 6 {
        return false;
    }
    let mut uniq = words.clone();
    uniq.sort_unstable();
    uniq.dedup();
    uniq.len() * 2 < words.len()
}

/// Whether `s` has a letter of the Arabic script (Urdu, Arabic, Persian).
fn has_arabic_script(s: &str) -> bool {
    s.chars()
        .any(|c| ('\u{0600}'..='\u{06FF}').contains(&c) || ('\u{0750}'..='\u{077F}').contains(&c))
}

/// Whether `s` is written only in Latin letters (with digits and marks).
fn latin_only(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_alphabetic())
        && !s
            .chars()
            .any(|c| c.is_alphabetic() && !c.is_ascii_alphabetic())
}

/// Arabic letter forms Whisper writes for Urdu speech, as Urdu writes
/// them (#71: Urdu came out looking like Arabic): yeh, kaf, heh, teh
/// marbuta.
pub fn urdu_letters(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{064A}' | '\u{0649}' => '\u{06CC}',
            '\u{0643}' => '\u{06A9}',
            '\u{0647}' | '\u{0629}' => '\u{06C1}',
            other => other,
        })
        .collect()
}

/// [`clean`] for a window heard in `language` (#71): Urdu gets Urdu
/// letter forms, a sentence looping a few words is dropped, and in an
/// Urdu window a whole English sentence of five words or more (Whisper
/// inventing an English line over Urdu speech) is dropped.
pub fn clean_lang(text: &str, before: &str, quiet: bool, language: Option<&str>) -> String {
    let urdu = language == Some("ur");
    let text = if urdu {
        urdu_letters(text)
    } else {
        text.to_owned()
    };
    let cleaned = clean(&text, before, quiet);
    let urdu_heard = urdu && has_arabic_script(&cleaned);
    let kept: Vec<String> = sentences(&cleaned)
        .into_iter()
        .filter(|s| !loops(s))
        .filter(|s| !(urdu_heard && latin_only(s) && s.split_whitespace().count() >= 5))
        .collect();
    kept.join(" ")
}

#[cfg(test)]
mod lang_tests {
    use super::*;

    #[test]
    fn urdu_windows_get_urdu_letters_and_lose_invented_english() {
        // Arabic yeh and kaf become Urdu's.
        assert_eq!(
            urdu_letters("\u{0643}\u{064A}\u{0627}"),
            "\u{06A9}\u{06CC}\u{0627}"
        );
        let urdu = "\u{0645}\u{06CC}\u{06BA} \u{0679}\u{06BE}\u{06CC}\u{06A9} \u{06C1}\u{0648}\u{06BA}\u{06D4}";
        let mixed = format!("{urdu} We are not talking about the world today.");
        assert_eq!(clean_lang(&mixed, "", false, Some("ur")), urdu);
        // An English window is left alone.
        assert_eq!(
            clean_lang(
                "We are not talking about the world today.",
                "",
                false,
                Some("en")
            ),
            "We are not talking about the world today."
        );
        // Short English inside Urdu speech stays.
        let code = format!("{urdu} Open the file.");
        assert_eq!(clean_lang(&code, "", false, Some("ur")), code);
    }

    #[test]
    fn a_looping_sentence_is_dropped() {
        assert_eq!(
            clean_lang(
                "Muslims Muslims and Muslims the Muslims Muslims world Muslims.",
                "",
                false,
                None
            ),
            ""
        );
        assert_eq!(
            clean_lang("Paging maps pages to frames.", "", false, None),
            "Paging maps pages to frames."
        );
    }
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
    fn sound_tags_are_not_words() {
        assert_eq!(
            clean(
                "[inaudible] for the hill missions and things like that.",
                "",
                false
            ),
            "for the hill missions and things like that."
        );
        assert_eq!(clean("[BLANK_AUDIO]", "", true), "");
        assert_eq!(
            clean("(laughs) okay so the demo is ready", "", false),
            "okay so the demo is ready"
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
