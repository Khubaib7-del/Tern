//! Read aloud (#69): Kokoro-82M on ONNX Runtime, through `loqui-kokoro`
//! (MIT) with `loqui-g2p` (MIT) for the phonemes: misaki's English
//! lexicons (Apache-2.0) and a small neural model for unknown words. No
//! espeak-ng (GPL-3.0) is linked or downloaded.
//!
//! English only: Kokoro's American (`a*`) and British (`b*`) voices.
//! Text in another script (Urdu) gets [`ENGLISH_ONLY`] instead of
//! garbled English phonemes (see [`super::kokoro_text`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::kokoro_text::{ENGLISH_ONLY, readable, voice_or_default};
use super::{LocalEngine, ModelEntry, Runtime, Task};

/// Kokoro's output rate.
pub const SAMPLE_RATE: u32 = 24_000;

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

impl KokoroTts {
    fn model(&self, dir: &Path, entry: &ModelEntry) -> Result<Arc<loqui_kokoro::Kokoro>, String> {
        let mut slot = self
            .loaded
            .lock()
            .map_err(|_| "the voice model lock broke".to_owned())?;
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
}
