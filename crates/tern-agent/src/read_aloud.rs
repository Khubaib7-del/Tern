//! Read aloud (#69): an answer spoken by the local voice model (Kokoro),
//! sentence by sentence. The first sentence plays as soon as it is made
//! while the next ones are made behind it; pause keeps the place, stop
//! (or leaving the chat) drops the rest.
//!
//! The front end polls [`ReadAloud::status`] for the button's state and
//! the sentence being spoken.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::local_models::kokoro_text::{
    CODE_BLOCK, ENGLISH_ONLY, chunks, readable, sentences, speakable,
};
use crate::local_models::{LocalModelPrefs, LocalModels, Task, can_run};
use crate::platform::speaker::Player;

/// Rate of the voice model's samples (Kokoro: 24 kHz).
const VOICE_RATE: u32 = 24_000;

/// A short pause between sentences (seconds).
const GAP: f32 = 0.18;

/// Where reading stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Making the first sentence.
    Preparing,
    Playing,
    Paused,
    /// Everything was read (or it was stopped).
    Done,
    /// Nothing could be read; the reason in plain words.
    Failed(String),
}

/// What the button shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub phase: Phase,
    /// The sentence being spoken (0-based).
    pub sentence: usize,
    pub sentences: usize,
}

/// Why read aloud cannot start, and what fixes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Missing {
    /// The text is not English (Urdu): [`ENGLISH_ONLY`].
    NotEnglish,
    /// No voice model is downloaded and on: `id` is the one to download.
    Download { id: String, name: String },
    /// One is downloaded, but this build cannot run it (`name`).
    CannotRun { name: String },
    /// Nothing to read (an empty answer, only code).
    Empty,
}

impl Missing {
    pub fn message(&self) -> String {
        match self {
            Missing::NotEnglish => ENGLISH_ONLY.into(),
            Missing::Download { name, .. } => format!(
                "Read aloud needs the local voice model {name}. Download it in Settings › Local models."
            ),
            Missing::CannotRun { name } => {
                format!("This build cannot run {name} yet, so it cannot read aloud.")
            }
            Missing::Empty => "There is nothing in this answer to read aloud.".into(),
        }
    }
}

/// The voice model read aloud uses: the one picked for the task when it
/// is installed, on and runnable, else the reason it cannot.
pub fn voice_model(lm: &LocalModels, prefs: &LocalModelPrefs) -> Result<String, Missing> {
    match lm.usable(prefs, Task::TextToSpeech) {
        Some(e) if can_run(e) => Ok(e.id.clone()),
        Some(e) => {
            // The alternative may run where the pick does not.
            let alt = lm
                .alternative_to(e)
                .filter(|a| can_run(a) && lm.installed(&a.id).is_some() && prefs.is_enabled(&a.id));
            match alt {
                Some(a) => Ok(a.id.clone()),
                None => Err(Missing::CannotRun {
                    name: e.name.clone(),
                }),
            }
        }
        None => {
            let pick = lm.chosen(prefs, Task::TextToSpeech);
            let runnable = pick.filter(|e| can_run(e)).or_else(|| {
                lm.manifest()
                    .for_task(Task::TextToSpeech)
                    .find(|e| can_run(e))
            });
            match runnable.or(pick) {
                Some(e) => Err(Missing::Download {
                    id: e.id.clone(),
                    name: e.name.clone(),
                }),
                None => Err(Missing::CannotRun {
                    name: "a voice model".into(),
                }),
            }
        }
    }
}

/// Checks `text` before reading: the spoken sentences, or why not.
pub fn prepare(text: &str) -> Result<Vec<String>, Missing> {
    let spoken = speakable(text);
    let all = sentences(&spoken);
    let words: Vec<&String> = all.iter().filter(|s| *s != CODE_BLOCK).collect();
    if words.is_empty() {
        // Nothing, or only code (the placeholder alone).
        return Err(Missing::Empty);
    }
    // English sentences of a mixed answer are read; an answer with none
    // (Urdu) gets the note.
    if !words.iter().any(|s| readable(s)) {
        return Err(Missing::NotEnglish);
    }
    // Long sentences go in clause-sized pieces (#70), so the first audio
    // starts sooner and a slow computer leaves short gaps, not long ones.
    Ok(all
        .into_iter()
        .filter(|s| readable(s))
        .flat_map(|s| chunks(&s))
        .collect())
}

struct Inner {
    cancel: AtomicBool,
    made_all: AtomicBool,
    player: Player,
    /// The frame each made sentence starts at, in order.
    starts: Mutex<Vec<u64>>,
    error: Mutex<Option<String>>,
    total: usize,
}

/// One answer being read aloud. Dropping it stops the reading.
pub struct ReadAloud {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ReadAloud {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadAloud")
            .field("sentences", &self.inner.total)
            .finish_non_exhaustive()
    }
}

impl ReadAloud {
    /// Starts reading `sentences` (from [`prepare`]) with installed voice
    /// model `model` in `voice` at `speed`. Fails when the speakers
    /// cannot be opened.
    pub fn start(
        model: String,
        sentences: Vec<String>,
        voice: String,
        speed: f32,
    ) -> Result<Self, String> {
        let player = Player::start()?;
        let inner = Arc::new(Inner {
            cancel: AtomicBool::new(false),
            made_all: AtomicBool::new(false),
            player,
            starts: Mutex::new(Vec::new()),
            error: Mutex::new(None),
            total: sentences.len(),
        });
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("tern-read-aloud".into())
            .spawn(move || {
                // A panic in the voice model ends this reading with a
                // reason, never the app.
                let guard = worker.clone();
                let run =
                    std::panic::AssertUnwindSafe(|| make(worker, model, sentences, voice, speed));
                if std::panic::catch_unwind(run).is_err() {
                    crate::diag::note("tts: the voice model panicked");
                    if let Ok(mut slot) = guard.error.lock() {
                        *slot = Some("the voice model stopped unexpectedly".into());
                    }
                    guard.made_all.store(true, Ordering::Relaxed);
                }
            })
            .map_err(|e| format!("Could not start reading: {e}"))?;
        Ok(Self { inner })
    }

    pub fn status(&self) -> Status {
        let i = &self.inner;
        let starts = i.starts.lock().map(|s| s.clone()).unwrap_or_default();
        let played = i.player.played();
        let sentence = starts.iter().rposition(|&s| s <= played).unwrap_or(0);
        let error = i.error.lock().ok().and_then(|e| e.clone());
        let phase = if let Some(e) = error.filter(|_| starts.is_empty()) {
            Phase::Failed(e)
        } else if let Some(e) = i.player.error() {
            Phase::Failed(format!("Playback stopped: {e}"))
        } else if i.cancel.load(Ordering::Relaxed)
            || (i.made_all.load(Ordering::Relaxed) && i.player.drained())
        {
            Phase::Done
        } else if starts.is_empty() {
            Phase::Preparing
        } else if i.player.is_paused() {
            Phase::Paused
        } else {
            Phase::Playing
        };
        Status {
            phase,
            sentence,
            sentences: i.total,
        }
    }

    /// Pauses, or resumes a paused reading.
    pub fn toggle_pause(&self) {
        let p = &self.inner.player;
        p.set_paused(!p.is_paused());
    }

    /// Stops and drops the rest.
    pub fn stop(&self) {
        self.inner.cancel.store(true, Ordering::Relaxed);
        self.inner.player.stop();
    }
}

impl Drop for ReadAloud {
    fn drop(&mut self) {
        self.stop();
    }
}

fn make(inner: Arc<Inner>, model: String, sentences: Vec<String>, voice: String, speed: f32) {
    let lm = match LocalModels::discover() {
        Ok((lm, _)) => lm,
        Err(e) => {
            if let Ok(mut slot) = inner.error.lock() {
                *slot = Some(e);
            }
            inner.made_all.store(true, Ordering::Relaxed);
            return;
        }
    };
    let gap = vec![0.0_f32; (VOICE_RATE as f32 * GAP) as usize];
    let short_gap = vec![0.0_f32; (VOICE_RATE as f32 * GAP / 3.0) as usize];
    let t0 = std::time::Instant::now();
    let mut audio_secs = 0.0_f64;
    for s in &sentences {
        if inner.cancel.load(Ordering::Relaxed) {
            break;
        }
        match lm.speak(&model, s, &voice, speed) {
            Ok(audio) => {
                audio_secs += audio.len() as f64 / f64::from(VOICE_RATE);
                let at = inner.player.push(&audio, VOICE_RATE);
                // A piece of a sentence runs on after a shorter pause.
                let sentence_end = s.ends_with(['.', '!', '?', '…', ':']);
                inner
                    .player
                    .push(if sentence_end { &gap } else { &short_gap }, VOICE_RATE);
                if let Ok(mut st) = inner.starts.lock() {
                    st.push(at);
                }
            }
            Err(e) => {
                crate::diag::note(&format!("tts: failed: {e}"));
                if let Ok(mut slot) = inner.error.lock() {
                    *slot = Some(e);
                }
                break;
            }
        }
    }
    let took = t0.elapsed().as_secs_f64();
    if audio_secs > 0.0 {
        crate::diag::note(&format!(
            "tts: made {} pieces, {audio_secs:.1}s of audio in {took:.1}s ({:.1}x real time{})",
            sentences.len(),
            took / audio_secs,
            if took > audio_secs {
                "; slower than speech, so short pauses between pieces"
            } else {
                ""
            }
        ));
    }
    inner.made_all.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urdu_answers_get_the_english_only_note() {
        assert_eq!(
            prepare("سلام پاکستان، آپ کیسے ہیں؟"),
            Err(Missing::NotEnglish)
        );
        assert_eq!(Missing::NotEnglish.message(), ENGLISH_ONLY);
        assert_eq!(prepare("```\nfn main() {}\n```"), Err(Missing::Empty));
        let s = prepare("Hello there. سلام پاکستان۔ How are you?").unwrap();
        assert_eq!(s, ["Hello there.", "How are you?"]);
    }

    #[test]
    fn a_missing_voice_model_says_which_to_download() {
        let dir = tempfile::tempdir().unwrap();
        let lm = LocalModels::new(dir.path(), crate::local_models::Manifest::builtin());
        let prefs = LocalModelPrefs::default();
        match voice_model(&lm, &prefs) {
            // With the tts engine compiled in, Kokoro is the one to get;
            // without it nothing can run.
            Err(Missing::Download { id, .. }) => assert_eq!(id, "kokoro-82m-int8"),
            Err(Missing::CannotRun { .. }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}
