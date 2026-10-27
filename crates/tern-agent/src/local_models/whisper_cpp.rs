//! Whisper models on whisper.cpp (the `whisper` feature): transcribes
//! audio files, any language Whisper knows (Urdu included), on the CPU.
//! whisper.cpp is built from source into the binary by `whisper-rs`; no
//! extra library ships with Tern.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::{CANCELLED, LocalEngine, ModelEntry, Runtime, Task, TranscribeCtl};

/// Runs ggml Whisper models (`ggml-*.bin`).
#[derive(Default)]
pub struct WhisperCpp {
    /// The last model loaded (and whether on the GPU), kept so a second
    /// file does not load it again.
    loaded: Mutex<Option<(PathBuf, bool, Arc<WhisperContext>)>>,
    /// The language-check model (Whisper tiny), kept apart so checking
    /// does not unload the model that transcribes (#70).
    checker: Mutex<Option<(PathBuf, bool, Arc<WhisperContext>)>>,
}

type Slot = Mutex<Option<(PathBuf, bool, Arc<WhisperContext>)>>;

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

/// The last run's load and inference times, for the timings line.
static LAST: Mutex<String> = Mutex::new(String::new());

/// Threads for whisper.cpp: Settings › Speech, else
/// `TERN_WHISPER_THREADS`, else one per logical core, at most twice the
/// physical cores and at most four (`speech::threads`). Measured on CI
/// (#66, a 12 s clip, whisper small, 4-vCPU runners): Linux 8.3 s with 2
/// threads, 7.8 s with 4, 61 s with 6 and 93 s with 8; Windows 11.7 s,
/// 9.6 s, 295 s and 575 s. The user's 4-core, 8-thread laptop needed
/// 37.3 s for an 11.7 s clip with 8 and 19-30 s with 6, so ggml's thread
/// barriers cost far more than they gain once threads fill every core.
fn threads() -> i32 {
    super::speech::threads()
}

/// Flash attention: off unless `TERN_WHISPER_FLASH=1`. On the CI runners
/// whisper small was slower with it (4 threads: Linux 10.1 s on, 7.8 s
/// off; Windows 12.1 s on, 9.6 s off).
fn flash_attn() -> bool {
    std::env::var("TERN_WHISPER_FLASH").as_deref() == Ok("1")
}

/// Where a run goes: the GPU (and which), or the CPU and why.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Placement {
    Gpu(super::gpu::Gpu),
    Cpu(String),
}

impl Placement {
    /// The device line for `tern.log` and Settings › Speech.
    fn line(&self, threads: i32) -> String {
        match self {
            Placement::Gpu(g) => format!("engine=whisper.cpp gpu={} {}", g.api, g.name),
            Placement::Cpu(why) => {
                format!("engine=whisper.cpp cpu {threads} threads ({why})")
            }
        }
    }

    fn label(&self, threads: i32) -> String {
        match self {
            Placement::Gpu(g) => format!("GPU · {} · {}", api_name(g.api), g.name),
            Placement::Cpu(why) => format!("CPU · {threads} threads ({why})"),
        }
    }
}

fn api_name(api: &str) -> &str {
    match api {
        "vulkan" => "Vulkan",
        "metal" => "Metal",
        a => a,
    }
}

/// The device the saved choice and this machine allow.
fn placement() -> Placement {
    use super::speech::Device;
    super::gpu::keep_hook();
    let choice = super::speech::choice();
    match choice.device {
        Device::Cpu => Placement::Cpu("CPU chosen in Settings".into()),
        Device::Auto if super::speech::gpu_failed_before() => Placement::Cpu(
            "the GPU stopped an earlier run; choose GPU in Settings › Speech to try it again"
                .into(),
        ),
        Device::Auto | Device::Gpu => match super::gpu::first() {
            Ok(g) => Placement::Gpu(g),
            Err(why) => Placement::Cpu(why),
        },
    }
}

impl WhisperCpp {
    fn context(
        &self,
        path: &Path,
        gpu: Option<&super::gpu::Gpu>,
    ) -> Result<Arc<WhisperContext>, String> {
        self.context_in(&self.loaded, path, gpu)
    }

    fn context_in(
        &self,
        slot: &Slot,
        path: &Path,
        gpu: Option<&super::gpu::Gpu>,
    ) -> Result<Arc<WhisperContext>, String> {
        let mut slot = slot.lock().map_err(|_| "the Whisper engine is unusable")?;
        if let Some((p, on_gpu, ctx)) = slot.as_ref()
            && p == path
            && *on_gpu == gpu.is_some()
        {
            return Ok(ctx.clone());
        }
        *slot = None;
        // whisper.cpp logs to stderr unless hooked; this silences it.
        whisper_rs::install_logging_hooks();
        let mut params = WhisperContextParameters::default();
        params.flash_attn(flash_attn());
        params.use_gpu(gpu.is_some());
        if let Some(g) = gpu {
            params.gpu_device(g.index);
        }
        let ctx = WhisperContext::new_with_params(path, params)
            .map_err(|e| format!("cannot load the Whisper model: {e}"))?;
        let ctx = Arc::new(ctx);
        *slot = Some((path.to_owned(), gpu.is_some(), ctx.clone()));
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
        self.transcribe_with(dir, entry, audio, language, &TranscribeCtl::new())
    }

    fn build_info(&self) -> String {
        format!(
            "({}; {}; {})",
            super::speech::last_device().unwrap_or_else(|| format!("{} threads", threads())),
            LAST.lock().map(|t| t.clone()).unwrap_or_default(),
            whisper_rs::print_system_info().trim()
        )
    }

    fn detect_language(
        &self,
        dir: &Path,
        entry: &ModelEntry,
        audio: &[f32],
    ) -> Result<(String, f32), String> {
        let file = model_file(entry).ok_or_else(|| format!("{} has no .bin file", entry.name))?;
        let n_threads = threads();
        let place = placement();
        let gpu = match &place {
            Placement::Gpu(g) => Some(g.clone()),
            Placement::Cpu(_) => None,
        };
        let slot = if entry.id.starts_with("whisper-tiny") {
            &self.checker
        } else {
            &self.loaded
        };
        let path = dir.join(file);
        let t0 = std::time::Instant::now();
        let ctx = match self.context_in(slot, &path, gpu.as_ref()) {
            Ok(c) => c,
            Err(_) if gpu.is_some() => self.context_in(slot, &path, None)?,
            Err(e) => return Err(e),
        };
        let mut state = ctx
            .create_state()
            .map_err(|e| format!("cannot start Whisper: {e}"))?;
        // The encoder and one decoder step: no transcription (#70: the
        // full transcription Auto used took 9-21 s on the user's laptop).
        let min = super::resample::RATE as usize + 1_600;
        let mut pcm = audio.to_vec();
        if pcm.len() < min {
            pcm.resize(min, 0.0);
        }
        let threads = usize::try_from(n_threads.max(1)).unwrap_or(1);
        state
            .pcm_to_mel(&pcm, threads)
            .map_err(|e| format!("Whisper could not read the audio: {e}"))?;
        let (id, probs) = state
            .lang_detect(0, threads)
            .map_err(|e| format!("Whisper could not tell the language: {e}"))?;
        let code = whisper_rs::get_lang_str(id).unwrap_or("?").to_owned();
        let p = usize::try_from(id)
            .ok()
            .and_then(|i| probs.get(i).copied())
            .unwrap_or(0.0);
        crate::diag::note(&format!(
            "whisper: language check {code} p={p:.2} model={} audio={:.1}s took={}ms {}",
            entry.id,
            audio.len() as f64 / f64::from(super::resample::RATE),
            t0.elapsed().as_millis(),
            place.line(n_threads)
        ));
        Ok((code, p))
    }

    fn transcribe_with(
        &self,
        dir: &Path,
        entry: &ModelEntry,
        audio: &[f32],
        language: Option<&str>,
        ctl: &TranscribeCtl,
    ) -> Result<String, String> {
        let file = model_file(entry).ok_or_else(|| format!("{} has no .bin file", entry.name))?;
        let loading = std::time::Instant::now();
        let n_threads = threads();
        let mut place = placement();
        // A marker while the GPU runs: if the GPU takes the app down, the
        // next start finds it and Auto stays on the CPU.
        let marker = match &place {
            Placement::Gpu(_) => super::speech::gpu_trial_marker(),
            Placement::Cpu(_) => None,
        };
        if let Some(m) = &marker {
            if let Some(dir) = m.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(m, place.line(n_threads));
        }
        let gpu = match &place {
            Placement::Gpu(g) => Some(g.clone()),
            Placement::Cpu(_) => None,
        };
        let ctx = match self.context(&dir.join(file), gpu.as_ref()) {
            Ok(c) => c,
            // The GPU would not take the model: the CPU then.
            Err(e) if gpu.is_some() => {
                crate::diag::note(&format!("whisper: GPU load failed ({e}); using the CPU"));
                place = Placement::Cpu(format!("the GPU could not load the model: {e}"));
                self.context(&dir.join(file), None)?
            }
            Err(e) => return Err(e),
        };
        let load_ms = loading.elapsed().as_millis();
        crate::diag::note(&format!("whisper: {}", place.line(n_threads)));
        super::speech::set_last_device(&place.label(n_threads));
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
        p.set_n_threads(n_threads);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_special(false);
        p.set_print_timestamps(false);
        // One clip at a time: no text carried from an earlier window, and
        // a short clip (up to 30 s) decoded as one segment without
        // timestamps, which skips the timestamp tokens.
        p.set_no_context(true);
        let short = audio.len() <= 30 * super::resample::RATE as usize;
        p.set_single_segment(short);
        p.set_no_timestamps(short);
        // Progress for the chip (whisper.cpp reports 0–100 as it goes);
        // the first 5% was decoding the file.
        let progress = ctl.clone();
        p.set_progress_callback_safe(move |pc: i32| {
            progress.set_progress((5 + pc.clamp(0, 100) * 95 / 100) as u8)
        });
        // Checked between the model's steps: × on the chip, or the timeout.
        // whisper-rs 0.16 reads the closure back as the type it was given
        // but stores it boxed as `Box<dyn FnMut() -> bool>`; a plain
        // closure is then read as garbage and aborts every run (error -6,
        // seen on CI). Handing it that boxed type makes both sides agree.
        let stop = ctl.clone();
        let abort: Box<dyn FnMut() -> bool> = Box::new(move || stop.is_cancelled());
        p.set_abort_callback_safe(abort);
        // whisper.cpp wants at least a second; pad short clips with quiet.
        let min = super::resample::RATE as usize + 1_600;
        let mut pcm = audio.to_vec();
        if pcm.len() < min {
            pcm.resize(min, 0.0);
        }
        let running = std::time::Instant::now();
        let ran = state.full(p, &pcm);
        if let Some(m) = &marker {
            let _ = std::fs::remove_file(m);
        }
        if let Ok(mut t) = LAST.lock() {
            *t = format!(
                "load {load_ms} ms, whisper_full {} ms, {}",
                running.elapsed().as_millis(),
                place.line(n_threads)
            );
        }
        if ctl.is_cancelled() {
            return Err(CANCELLED.into());
        }
        ran.map_err(|e| format!("Whisper failed: {e}"))?;
        ctl.set_progress(100);
        // The language whisper heard (auto-detect unless one was given).
        let heard = whisper_rs::get_lang_str(state.full_lang_id_from_state()).unwrap_or("?");
        if heard != "?" {
            ctl.set_heard(heard);
        }
        if let Ok(mut t) = LAST.lock() {
            t.push_str(&format!(", language {heard} (asked {lang})"));
        }
        crate::diag::note(&format!(
            "whisper: language detected={heard} asked={lang} threads={n_threads}"
        ));
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
        assert_eq!(
            runs,
            [
                "whisper-small-q5_1",
                "whisper-large-v3-turbo-q5_0",
                "whisper-tiny-q5_1"
            ]
        );
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
