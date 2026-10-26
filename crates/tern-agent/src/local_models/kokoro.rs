//! Read aloud (#69): Kokoro-82M on ONNX Runtime, through `loqui-kokoro`
//! (MIT) with `loqui-g2p` (MIT) for the phonemes: misaki's English
//! lexicons (Apache-2.0) and a small neural model for unknown words. No
//! espeak-ng (GPL-3.0) is linked or downloaded.
//!
//! English only: Kokoro's American (`a*`) and British (`b*`) voices.
//! Text in another script (Urdu) gets [`ENGLISH_ONLY`] instead of
//! garbled English phonemes; see [`readable`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{LocalEngine, ModelEntry, Runtime, Task};

/// Kokoro's output rate.
pub const SAMPLE_RATE: u32 = 24_000;

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

/// Kokoro on ONNX Runtime; the loaded model is kept for the next call.
#[derive(Default)]
pub struct KokoroTts {
    loaded: Mutex<Option<(PathBuf, Arc<loqui_kokoro::Kokoro>)>>,
}

impl std::fmt::Debug for KokoroTts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KokoroTts").finish_non_exhaustive()
    }
}

/// Whether `entry` is a Kokoro voice model (an ONNX graph and voice packs).
pub fn fits(entry: &ModelEntry) -> bool {
    entry.runtime == Runtime::Onnx
        && entry.task == Task::TextToSpeech
        && entry.id.starts_with("kokoro")
        && model_file(entry).is_some()
}

fn model_file(entry: &ModelEntry) -> Option<String> {
    entry
        .files
        .iter()
        .map(|f| f.local().to_owned())
        .find(|n| n.ends_with(".onnx"))
}

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
    ["e.g", "i.e", "mr", "mrs", "ms", "dr", "vs", "etc", "no", "st"].contains(&l.as_str())
}

impl KokoroTts {
    fn model(&self, dir: &Path, entry: &ModelEntry) -> Result<Arc<loqui_kokoro::Kokoro>, String> {
        let mut slot = self.loaded.lock().map_err(|_| "the voice model lock broke".to_owned())?;
        if let Some((d, k)) = slot.as_ref()
            && d == dir
        {
            return Ok(k.clone());
        }
        let file = model_file(entry).ok_or_else(|| format!("{} has no model file", entry.name))?;
        let threads = super::speech::threads().max(1) as usize;
        let t0 = Instant::now();
        let model = loqui_kokoro::KokoroModel::load(
            &dir.join(&file),
            loqui_kokoro::Device::Cpu,
            Some(threads),
        )
        .map_err(|e| e.to_string())?;
        let k = Arc::new(loqui_kokoro::Kokoro::new(
            model,
            dir.join("voices"),
            loqui_g2p::OovFallback::Neural,
        ));
        crate::diag::note(&format!(
            "tts: loaded {} on CPU, {threads} threads, in {:.1}s",
            entry.id,
            t0.elapsed().as_secs_f64()
        ));
        *slot = Some((dir.to_path_buf(), k.clone()));
        Ok(k)
    }
}

impl LocalEngine for KokoroTts {
    fn runtime(&self) -> Runtime {
        Runtime::Onnx
    }

    fn name(&self) -> &'static str {
        "Kokoro on ONNX Runtime"
    }

    fn runs(&self, entry: &ModelEntry) -> bool {
        fits(entry)
    }

    fn speak(
        &self,
        dir: &Path,
        entry: &ModelEntry,
        text: &str,
        voice: &str,
        speed: f32,
    ) -> Result<Vec<f32>, String> {
        if !readable(text) {
            return Err(ENGLISH_ONLY.into());
        }
        let k = self.model(dir, entry)?;
        let blend: loqui_kokoro::Blend = voice_or_default(Some(voice))
            .parse()
            .map_err(|e: loqui_kokoro::Error| e.to_string())?;
        let t0 = Instant::now();
        let audio = k
            .speak(text, &blend, speed.clamp(0.5, 2.0))
            .map_err(|e| e.to_string())?;
        crate::diag::note(&format!(
            "tts: {} voice={voice} speed={speed:.2} chars={} audio={:.1}s took={:.2}s",
            entry.id,
            text.chars().count(),
            audio.len() as f64 / f64::from(SAMPLE_RATE),
            t0.elapsed().as_secs_f64()
        ));
        Ok(audio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_models::Manifest;

    #[test]
    fn runs_kokoro_only() {
        let m = Manifest::builtin();
        let ids: Vec<&str> = m
            .models
            .iter()
            .filter(|e| fits(e))
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(ids, ["kokoro-82m-int8"]);
    }

    #[test]
    fn urdu_is_not_read_as_english() {
        assert!(readable("Hello from Tern."));
        assert!(!readable("سلام پاکستان، آپ کیسے ہیں؟"));
        assert!(readable("Lahore (لاہور) is a city in Pakistan."));
        assert!(!readable("12345"));
    }

    #[test]
    fn sentences_split_for_streaming() {
        let s = sentences("Hello there. This is Tern! Is it working? Yes e.g. like this.\nNew line");
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
        let s = speakable("# Title\n- **Bold** item with [a link](https://x.org)\n```rust\nfn main() {}\n```\nSee https://tern.dev now.");
        assert_eq!(s, "Title\nBold item with a link\n(code block.)\nSee a link now.");
    }

    #[test]
    fn a_saved_voice_falls_back_to_the_default() {
        assert_eq!(voice_or_default(Some("bm_george")), "bm_george");
        assert_eq!(voice_or_default(Some("zz_none")), DEFAULT_VOICE);
        assert_eq!(voice_or_default(None), DEFAULT_VOICE);
    }
}
