//! Whisper models on whisper.cpp (the `whisper` feature): transcribes
//! audio files, any language Whisper knows (Urdu included), on the CPU.
//! whisper.cpp is built from source into the binary by `whisper-rs`; no
//! extra library ships with Tern.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::{LocalEngine, ModelEntry, Runtime, Task};

/// Runs ggml Whisper models (`ggml-*.bin`).
#[derive(Default)]
pub struct WhisperCpp {
    /// The last model loaded, kept so a second file does not load it again.
    loaded: Mutex<Option<(PathBuf, Arc<WhisperContext>)>>,
}

impl std::fmt::Debug for WhisperCpp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhisperCpp").finish_non_exhaustive()
    }
}

/// The model file: the entry's `.bin`.
fn model_file(entry: &ModelEntry) -> Option<&str> {
    entry
        .files
        .iter()
        .map(|f| f.local())
        .find(|n| n.ends_with(".bin"))
}

/// Whether `entry` is a ggml Whisper model.
pub fn fits(entry: &ModelEntry) -> bool {
    entry.runtime == Runtime::WhisperCpp
        && matches!(entry.task, Task::SpeechToText | Task::Dictation)
        && model_file(entry).is_some()
}

fn threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(1, 8) as i32)
        .unwrap_or(4)
}

impl WhisperCpp {
    fn context(&self, path: &Path) -> Result<Arc<WhisperContext>, String> {
        let mut slot = self
            .loaded
            .lock()
            .map_err(|_| "the Whisper engine is unusable")?;
        if let Some((p, ctx)) = slot.as_ref()
            && p == path
        {
            return Ok(ctx.clone());
        }
        // whisper.cpp logs to stderr unless hooked; this silences it.
        whisper_rs::install_logging_hooks();
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| format!("cannot load the Whisper model: {e}"))?;
        let ctx = Arc::new(ctx);
        *slot = Some((path.to_owned(), ctx.clone()));
        Ok(ctx)
    }
}

impl LocalEngine for WhisperCpp {
    fn runtime(&self) -> Runtime {
        Runtime::WhisperCpp
    }

    fn name(&self) -> &'static str {
        "whisper.cpp"
    }

    fn runs(&self, entry: &ModelEntry) -> bool {
        fits(entry)
    }

    fn transcribe(
        &self,
        dir: &Path,
        entry: &ModelEntry,
        audio: &[f32],
        language: Option<&str>,
    ) -> Result<String, String> {
        let file = model_file(entry).ok_or_else(|| format!("{} has no .bin file", entry.name))?;
        let ctx = self.context(&dir.join(file))?;
        let mut state = ctx
            .create_state()
            .map_err(|e| format!("cannot start Whisper: {e}"))?;
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let lang = language
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .unwrap_or("auto");
        p.set_language(Some(lang));
        p.set_translate(false);
        p.set_n_threads(threads());
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_special(false);
        p.set_print_timestamps(false);
        // whisper.cpp wants at least a second; pad short clips with quiet.
        let min = super::resample::RATE as usize + 1_600;
        let mut pcm = audio.to_vec();
        if pcm.len() < min {
            pcm.resize(min, 0.0);
        }
        state
            .full(p, &pcm)
            .map_err(|e| format!("Whisper failed: {e}"))?;
        let text = state
            .as_iter()
            .filter_map(|s| s.to_str_lossy().ok().map(|t| t.trim().to_owned()))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_models::Manifest;

    #[test]
    fn runs_the_whisper_models() {
        let m = Manifest::builtin();
        let runs: Vec<&str> = m
            .models
            .iter()
            .filter(|e| fits(e))
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(runs, ["whisper-small-q5_1", "whisper-large-v3-turbo-q5_0"]);
    }

    /// Transcribes a real file with a downloaded Whisper model. Needs
    /// `TERN_WHISPER_MODEL_DIR` (an installed whisper folder),
    /// `TERN_WHISPER_ID`, and `TERN_AUDIO`; run with `--ignored`.
    #[test]
    #[ignore]
    fn transcribes_a_file_with_the_installed_model() {
        let dir = PathBuf::from(std::env::var("TERN_WHISPER_MODEL_DIR").unwrap());
        let id = std::env::var("TERN_WHISPER_ID").unwrap_or("whisper-small-q5_1".into());
        let path = std::env::var("TERN_AUDIO").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let pcm = crate::local_models::audio::decode_16k_mono(&bytes, &path).unwrap();
        let m = Manifest::builtin();
        let entry = m.find(&id).unwrap();
        let lang = std::env::var("TERN_LANG").ok();
        let text = WhisperCpp::default()
            .transcribe(&dir, entry, &pcm, lang.as_deref())
            .unwrap();
        println!("{text}");
        if let Ok(want) = std::env::var("TERN_EXPECT") {
            for w in want.split('|') {
                assert!(
                    text.to_lowercase().contains(&w.to_lowercase()),
                    "{w:?} not in {text:?}"
                );
            }
        }
    }
}
