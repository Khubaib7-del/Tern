//! One calibrated token estimator per model (#85, item A3b).
//!
//! Tern used to count four characters a token, often over JSON-escaped
//! text. Urdu written in Arabic script then came out several times too
//! high: a 67-character Urdu sentence is 19 tokens in a modern tokenizer,
//! but its `\uXXXX`-escaped JSON form is over 400 characters. The context
//! ring jumped to "nearly full" after a few Urdu turns.
//!
//! Here an estimate is `chars × r`, with one ratio `r` per script and per
//! (provider, model):
//!
//! - Seeds per script: Latin 0.27, Arabic script 0.30, CJK 0.9, other
//!   scripts 0.35 tokens a character.
//! - Every reply that reports its prompt tokens calibrates the ratio of the
//!   script that dominates the prompt: the ratio becomes the sum of the
//!   tokens attributed to that script over the sum of its characters, so a
//!   few calls are enough and one odd call cannot swing it.
//! - Text is never counted in its JSON-escaped form: escapes are decoded
//!   first ([`unescape_json`]).
//!
//! The governor's pre-fit, the context cascade and the context ring all
//! read the same estimator ([`global`]).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// A writing system, for the seed ratios.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Script {
    /// Latin letters, digits, punctuation and spaces.
    Latin,
    /// Arabic, Urdu, Persian (the Arabic script blocks).
    Arabic,
    /// Chinese, Japanese and Korean.
    Cjk,
    /// Everything else (Cyrillic, Devanagari, Greek, ...).
    Other,
}

impl Script {
    const ALL: [Script; 4] = [Script::Latin, Script::Arabic, Script::Cjk, Script::Other];

    /// Tokens a character before any calibration.
    pub fn seed(self) -> f64 {
        match self {
            Script::Latin => 0.27,
            Script::Arabic => 0.30,
            Script::Cjk => 0.9,
            Script::Other => 0.35,
        }
    }

    fn index(self) -> usize {
        match self {
            Script::Latin => 0,
            Script::Arabic => 1,
            Script::Cjk => 2,
            Script::Other => 3,
        }
    }
}

/// The script a character belongs to.
pub fn script_of(c: char) -> Script {
    let u = c as u32;
    match u {
        0..=0x024F => Script::Latin,
        // Combining marks and general punctuation count with Latin.
        0x0300..=0x036F | 0x2000..=0x206F | 0x20A0..=0x20CF => Script::Latin,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => {
            Script::Arabic
        }
        0x1100..=0x11FF
        | 0x2E80..=0x2FDF
        | 0x3000..=0x30FF
        | 0x3130..=0x318F
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
        | 0xFF00..=0xFFEF
        | 0x20000..=0x2FA1F => Script::Cjk,
        _ => Script::Other,
    }
}

/// Characters per script, in [`Script`] order.
pub fn census(text: &str) -> [u64; 4] {
    let mut n = [0u64; 4];
    for c in text.chars() {
        n[script_of(c).index()] += 1;
    }
    n
}

/// Whether `text` carries JSON `\uXXXX` escapes.
pub fn looks_json_escaped(text: &str) -> bool {
    let b = text.as_bytes();
    b.windows(6)
        .any(|w| w[0] == b'\\' && w[1] == b'u' && w[2..].iter().all(|c| c.is_ascii_hexdigit()))
}

/// `text` with JSON string escapes decoded (`\uXXXX` with surrogate pairs,
/// `\n`, `\t`, `\"`, `\\`, `\/`), so it is counted as the model reads it.
/// Text without escapes comes back unchanged.
pub fn unescape_json(text: &str) -> Cow<'_, str> {
    if !text.contains('\\') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let hex4 = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| -> Option<u32> {
        let mut s = String::new();
        for _ in 0..4 {
            let c = *chars.peek()?;
            if !c.is_ascii_hexdigit() {
                return None;
            }
            s.push(c);
            chars.next();
        }
        u32::from_str_radix(&s, 16).ok()
    };
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('u') => {
                chars.next();
                let Some(hi) = hex4(&mut chars) else {
                    out.push_str("\\u");
                    continue;
                };
                if (0xD800..0xDC00).contains(&hi) {
                    // A surrogate pair: `\uD83D\uDE00`.
                    let mut look = chars.clone();
                    if look.next() == Some('\\')
                        && look.next() == Some('u')
                        && let Some(lo) = hex4(&mut look)
                        && (0xDC00..0xE000).contains(&lo)
                        && let Some(ch) =
                            char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00))
                    {
                        out.push(ch);
                        chars = look;
                        continue;
                    }
                    out.push('\u{FFFD}');
                } else {
                    out.push(char::from_u32(hi).unwrap_or('\u{FFFD}'));
                }
            }
            Some('n') => {
                chars.next();
                out.push('\n');
            }
            Some('t') => {
                chars.next();
                out.push('\t');
            }
            Some('r') => {
                chars.next();
                out.push('\r');
            }
            Some(q @ ('"' | '\\' | '/')) => {
                chars.next();
                out.push(q);
            }
            _ => out.push('\\'),
        }
    }
    Cow::Owned(out)
}

/// One model's calibration: per script, the tokens attributed and the
/// characters seen.
#[derive(Clone, Copy, Debug, Default)]
struct Calibration {
    tokens: [f64; 4],
    chars: [f64; 4],
    samples: u32,
}

impl Calibration {
    fn ratio(&self, s: Script) -> f64 {
        let i = s.index();
        if self.chars[i] >= 1.0 && self.tokens[i] > 0.0 {
            self.tokens[i] / self.chars[i]
        } else {
            s.seed()
        }
    }
}

/// Share of a prompt's characters one script needs before a reported
/// count calibrates that script.
const DOMINANT: f64 = 0.6;

/// The estimator: seeded ratios, calibrated per (provider, model).
#[derive(Debug, Default)]
pub struct Estimator {
    models: Mutex<HashMap<(String, String), Calibration>>,
}

impl Estimator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Estimated tokens for `text` sent to `model` at `provider`. Escaped
    /// JSON is decoded first.
    pub fn estimate(&self, provider: &str, model: &str, text: &str) -> u64 {
        let text = unescape_json(text);
        self.estimate_census(provider, model, &census(&text))
    }

    /// Estimated tokens for a character census ([`census`]).
    pub fn estimate_census(&self, provider: &str, model: &str, n: &[u64; 4]) -> u64 {
        let cal = self.calibration(provider, model);
        let t: f64 = Script::ALL
            .iter()
            .map(|s| n[s.index()] as f64 * cal.ratio(*s))
            .sum();
        t.ceil() as u64
    }

    /// The ratio in use for `script` on `model`.
    pub fn ratio(&self, provider: &str, model: &str, script: Script) -> f64 {
        self.calibration(provider, model).ratio(script)
    }

    /// Calls that calibrated `model` so far.
    pub fn samples(&self, provider: &str, model: &str) -> u32 {
        self.calibration(provider, model).samples
    }

    fn calibration(&self, provider: &str, model: &str) -> Calibration {
        self.models
            .lock()
            .ok()
            .and_then(|m| m.get(&(provider.to_owned(), model.to_owned())).copied())
            .unwrap_or_default()
    }

    /// Learns from a reply: `sent` is the prompt text as sent (decoded
    /// here if escaped) and `reported` the prompt tokens the provider
    /// counted. Only the dominant script's ratio moves; the other scripts'
    /// share is taken off at their current ratios first.
    pub fn observe(&self, provider: &str, model: &str, sent: &str, reported: u64) {
        let text = unescape_json(sent);
        self.observe_census(provider, model, &census(&text), reported);
    }

    /// [`Estimator::observe`] for a character census.
    pub fn observe_census(&self, provider: &str, model: &str, n: &[u64; 4], reported: u64) {
        let total: u64 = n.iter().sum();
        if total == 0 || reported == 0 {
            return;
        }
        let Some(dom) = Script::ALL
            .iter()
            .copied()
            .max_by_key(|s| n[s.index()])
            .filter(|s| n[s.index()] as f64 / total as f64 >= DOMINANT)
        else {
            return;
        };
        let Ok(mut models) = self.models.lock() else {
            return;
        };
        let cal = models
            .entry((provider.to_owned(), model.to_owned()))
            .or_default();
        let others: f64 = Script::ALL
            .iter()
            .filter(|s| **s != dom)
            .map(|s| n[s.index()] as f64 * cal.ratio(*s))
            .sum();
        let mine = (reported as f64 - others).max(reported as f64 * 0.1);
        let i = dom.index();
        cal.tokens[i] += mine;
        cal.chars[i] += n[i] as f64;
        cal.samples += 1;
    }
}

/// The estimator every part of Tern shares.
pub fn global() -> &'static Estimator {
    static E: OnceLock<Estimator> = OnceLock::new();
    E.get_or_init(Estimator::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    const URDU: &str = "یہ ایک مختصر جملہ ہے جو اردو میں لکھا گیا ہے تاکہ گنتی جانچی جا سکے۔";
    const ENGLISH: &str =
        "Retrieval-augmented generation reads sources before it writes an answer to a question.";

    #[test]
    fn scripts_are_told_apart() {
        assert_eq!(script_of('a'), Script::Latin);
        assert_eq!(script_of('ی'), Script::Arabic);
        assert_eq!(script_of('中'), Script::Cjk);
        assert_eq!(script_of('д'), Script::Other);
        let n = census(URDU);
        assert!(n[1] > n[0], "mostly Arabic script: {n:?}");
    }

    /// A-T14 (part): no estimate is computed over JSON-escaped text.
    #[test]
    fn escaped_json_counts_like_the_text_it_encodes() {
        let escaped = serde_json::to_string(&serde_json::json!(URDU)).unwrap();
        // serde_json keeps non-ASCII as is; escape it the way some wires do.
        let ascii: String = escaped
            .chars()
            .flat_map(|c| {
                if c.is_ascii() {
                    vec![c]
                } else {
                    let mut buf = [0u16; 2];
                    c.encode_utf16(&mut buf)
                        .iter()
                        .flat_map(|u| format!("\\u{u:04x}").chars().collect::<Vec<_>>())
                        .collect()
                }
            })
            .collect();
        assert!(looks_json_escaped(&ascii));
        assert!(ascii.len() > URDU.chars().count() * 5, "the old overcount");
        let decoded = unescape_json(&ascii);
        assert_eq!(decoded.trim_matches('"'), URDU);
        let e = Estimator::new();
        let raw = e.estimate("p", "m", URDU);
        let esc = e.estimate("p", "m", &ascii);
        assert!(esc <= raw + 1, "escaped {esc} vs raw {raw}");
        assert_eq!(unescape_json("\\ud83d\\ude00"), "😀");
        assert_eq!(unescape_json("plain"), "plain");
        assert_eq!(unescape_json("a\\nb \\\"q\\\""), "a\nb \"q\"");
    }

    /// A-T14: an Urdu chat and an English chat, three calls each, against
    /// a fake provider that counts like a real tokenizer: within ±20%.
    #[test]
    fn three_calls_calibrate_urdu_and_english_within_twenty_percent() {
        // The fake provider's own ratios, deliberately far from the seeds
        // for Urdu (0.45 against 0.30) and close for English.
        let count = |text: &str| -> u64 {
            let n = census(text);
            (n[0] as f64 * 0.24 + n[1] as f64 * 0.45 + n[3] as f64 * 0.35).round() as u64
        };
        let e = Estimator::new();
        let mut urdu_chat = String::new();
        let mut english_chat = String::new();
        for _ in 0..3 {
            urdu_chat.push_str(URDU);
            urdu_chat.push(' ');
            english_chat.push_str(ENGLISH);
            english_chat.push(' ');
            e.observe("fake", "m", &urdu_chat, count(&urdu_chat));
            e.observe("fake", "m", &english_chat, count(&english_chat));
        }
        for chat in [&urdu_chat, &english_chat] {
            let next = format!("{chat}{chat}");
            let est = e.estimate("fake", "m", &next) as f64;
            let real = count(&next) as f64;
            let err = (est - real).abs() / real;
            assert!(err <= 0.20, "estimate {est} vs reported {real} ({err:.2})");
        }
        assert_eq!(e.samples("fake", "m"), 6);
        // An uncalibrated model keeps the seeds.
        assert!((e.ratio("fake", "other", Script::Arabic) - 0.30).abs() < 1e-9);
    }
}
