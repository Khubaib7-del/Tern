//! Where local speech runs: Settings › Speech (#67).
//!
//! - **Device**: Auto (the GPU when one works, else the CPU), GPU, or CPU.
//!   Whisper on whisper.cpp uses Vulkan on Windows and Linux and Metal on
//!   macOS; a build or machine without a usable GPU runs on the CPU and
//!   says why in `tern.log`.
//! - **CPU threads**: Auto or 2, 4, 6, 8. The choice saved in preferences
//!   wins over `TERN_WHISPER_THREADS`, which wins over Auto.
//!
//! Auto threads: one per logical core, at most twice the physical cores
//! and at most four. On the 4-vCPU CI runners 4 threads were fastest
//! (#66: Linux 7.8 s, Windows 9.6 s for a 12 s clip), and the user's
//! 4-core, 8-thread laptop still took 19-30 s for an 11.7 s clip with 6.

use std::path::PathBuf;
use std::sync::RwLock;

/// Where speech models run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Device {
    /// The GPU when one works, else the CPU.
    #[default]
    Auto,
    /// The GPU; the CPU only when no GPU can be started.
    Gpu,
    /// The CPU only.
    Cpu,
}

impl Device {
    pub const ALL: [Device; 3] = [Device::Auto, Device::Gpu, Device::Cpu];

    pub fn id(self) -> &'static str {
        match self {
            Device::Auto => "auto",
            Device::Gpu => "gpu",
            Device::Cpu => "cpu",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Device::Auto => "Auto",
            Device::Gpu => "GPU",
            Device::Cpu => "CPU",
        }
    }

    /// `auto`, `gpu`, `cpu` (any case); anything else is `None`.
    pub fn parse(s: &str) -> Option<Device> {
        Device::ALL
            .into_iter()
            .find(|d| d.id().eq_ignore_ascii_case(s.trim()))
    }
}

/// The thread counts Settings offers besides Auto.
pub const THREAD_CHOICES: [u32; 4] = [2, 4, 6, 8];

/// The most threads Auto picks.
pub const AUTO_THREAD_CAP: usize = 4;

/// The saved choice: device and CPU threads (`None` = Auto).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub device: Device,
    pub threads: Option<u32>,
}

static CHOICE: RwLock<Option<Choice>> = RwLock::new(None);
static LAST: RwLock<String> = RwLock::new(String::new());
static LAST_RUN: RwLock<Option<LastRun>> = RwLock::new(None);

/// The languages Settings › Speech offers for dictation (#68): Whisper
/// codes and names. `auto` first, English second (the fast Moonshine
/// model); every other one runs on Whisper.
pub const DICTATION_LANGUAGES: [(&str, &str); 19] = [
    ("auto", "Auto"),
    ("en", "English"),
    ("ur", "Urdu"),
    ("hi", "Hindi"),
    ("pa", "Punjabi"),
    ("ar", "Arabic"),
    ("fa", "Persian"),
    ("bn", "Bengali"),
    ("tr", "Turkish"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("ru", "Russian"),
    ("zh", "Chinese"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("id", "Indonesian"),
];

/// The saved dictation language as a Whisper code, `None` for Auto.
pub fn dictation_language(saved: Option<&str>) -> Option<&'static str> {
    let s = saved?.trim();
    DICTATION_LANGUAGES
        .iter()
        .find(|(code, _)| *code != "auto" && code.eq_ignore_ascii_case(s))
        .map(|(code, _)| *code)
}

/// "Urdu" for `ur`, "Auto" for `None`, the code itself when unknown.
pub fn language_name(code: Option<&str>) -> String {
    match code {
        None => "Auto".into(),
        Some(c) => DICTATION_LANGUAGES
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(c))
            .map_or_else(|| c.to_owned(), |(_, n)| (*n).to_owned()),
    }
}

/// One finished local or provider transcription, for Settings › Speech
/// › Last transcription.
#[derive(Clone, Debug, PartialEq)]
pub struct LastRun {
    /// The model (`moonshine-streaming-small`, `groq/whisper-large-v3`).
    pub model: String,
    /// Where it ran ("CPU · 4 threads", "GPU · Vulkan · Intel(R) UHD
    /// Graphics", "Groq (online)").
    pub device: String,
    /// When it finished, seconds since 1970 (UTC).
    pub at_unix: i64,
    pub audio_secs: Option<f64>,
    pub ms: u128,
    /// The language Whisper heard, when it says.
    pub language: Option<String>,
}

impl LastRun {
    /// "moonshine-streaming-small · CPU · 4 threads · 18:42 · 3.0 s of
    /// audio in 1.9 s", in the local time `offset` seconds from UTC.
    pub fn line(&self, offset: i64) -> String {
        let day = (self.at_unix + offset).rem_euclid(86_400);
        let mut s = format!(
            "{} · {} · {:02}:{:02}",
            self.model,
            self.device,
            day / 3600,
            (day / 60) % 60
        );
        match self.audio_secs {
            Some(a) => s.push_str(&format!(
                " · {a:.1} s of audio in {:.1} s",
                self.ms as f64 / 1000.0
            )),
            None => s.push_str(&format!(" · {:.1} s", self.ms as f64 / 1000.0)),
        }
        if let Some(l) = &self.language {
            s.push_str(&format!(" · heard {}", language_name(Some(l))));
        }
        s
    }
}

pub fn set_last_run(run: LastRun) {
    if let Ok(mut r) = LAST_RUN.write() {
        *r = Some(run);
    }
}

/// The last transcription this window ran, any engine.
pub fn last_run() -> Option<LastRun> {
    LAST_RUN.read().ok().and_then(|r| r.clone())
}

/// Puts the saved choice into effect (from `Preferences::apply_runtime`).
pub fn set(choice: Choice) {
    if let Ok(mut c) = CHOICE.write() {
        *c = Some(choice);
    }
}

/// The choice in effect: the one set, else the saved preferences (the
/// CLI never sets one), with `TERN_SPEECH_DEVICE` used when the saved
/// device is Auto.
pub fn choice() -> Choice {
    let set = CHOICE.read().ok().and_then(|c| *c);
    let mut c = match set {
        Some(c) => c,
        None => {
            let p = crate::prefs::Preferences::load();
            let c = from_prefs(p.speech_device.as_deref(), p.speech_threads);
            self::set(c);
            c
        }
    };
    if c.device == Device::Auto
        && let Some(d) = std::env::var("TERN_SPEECH_DEVICE")
            .ok()
            .and_then(|v| Device::parse(&v))
    {
        c.device = d;
    }
    c
}

/// The choice saved in preferences.
pub fn from_prefs(device: Option<&str>, threads: Option<u32>) -> Choice {
    Choice {
        device: device.and_then(Device::parse).unwrap_or_default(),
        threads: threads.filter(|n| *n > 0),
    }
}

/// CPU threads for whisper.cpp: the saved choice, else
/// `TERN_WHISPER_THREADS`, else Auto.
pub fn threads() -> i32 {
    let env = std::env::var("TERN_WHISPER_THREADS")
        .ok()
        .and_then(|v| v.trim().parse::<i32>().ok());
    pick_threads(
        choice().threads,
        env,
        num_cpus_physical(),
        std::thread::available_parallelism().map_or(1, |n| n.get()),
    )
}

#[cfg(feature = "whisper")]
fn num_cpus_physical() -> usize {
    num_cpus::get_physical()
}

#[cfg(not(feature = "whisper"))]
fn num_cpus_physical() -> usize {
    0
}

/// The thread rule: `saved` > `env` > one per logical core, at most twice
/// the physical cores and at most [`AUTO_THREAD_CAP`].
pub fn pick_threads(saved: Option<u32>, env: Option<i32>, physical: usize, logical: usize) -> i32 {
    if let Some(n) = saved.filter(|n| *n > 0) {
        return n.min(64) as i32;
    }
    if let Some(n) = env.filter(|n| *n > 0) {
        return n.min(64);
    }
    let cap = if physical > 0 { physical * 2 } else { logical };
    logical.min(cap).clamp(1, AUTO_THREAD_CAP) as i32
}

/// What the last transcription ran on ("GPU · Vulkan · Intel(R) UHD
/// Graphics", "CPU · 4 threads (no GPU found)"), for Settings › Speech.
pub fn last_device() -> Option<String> {
    LAST.read()
        .ok()
        .map(|s| s.clone())
        .filter(|s| !s.is_empty())
}

pub fn set_last_device(line: &str) {
    if let Ok(mut s) = LAST.write() {
        *s = line.to_owned();
    }
}

/// The file that is there while a GPU transcription runs. Still there at
/// the next start means the GPU took the app down: Auto then stays on
/// the CPU (choosing GPU tries again).
pub fn gpu_trial_marker() -> Option<PathBuf> {
    crate::platform::AppPaths::discover()
        .ok()
        .map(|a| a.data_dir.join("logs").join("speech-gpu-trial"))
}

/// Whether an earlier GPU transcription never finished.
pub fn gpu_failed_before() -> bool {
    gpu_trial_marker().is_some_and(|m| m.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_threads_follow_logical_cores_up_to_four() {
        // The user's laptop: 4 cores, 8 threads.
        assert_eq!(pick_threads(None, None, 4, 8), 4);
        // A 4-vCPU CI runner (2 cores, 4 threads).
        assert_eq!(pick_threads(None, None, 2, 4), 4);
        assert_eq!(pick_threads(None, None, 16, 32), 4);
        assert_eq!(pick_threads(None, None, 1, 2), 2);
        assert_eq!(pick_threads(None, None, 0, 1), 1);
    }

    #[test]
    fn the_saved_choice_beats_the_environment() {
        assert_eq!(pick_threads(Some(6), Some(2), 4, 8), 6);
        assert_eq!(pick_threads(None, Some(2), 4, 8), 2);
        assert_eq!(pick_threads(Some(0), Some(8), 4, 8), 8);
        assert_eq!(pick_threads(None, Some(-1), 4, 8), 4);
        assert_eq!(pick_threads(Some(200), None, 4, 8), 64);
    }

    #[test]
    fn dictation_languages_and_the_last_run_line() {
        assert_eq!(dictation_language(None), None);
        assert_eq!(dictation_language(Some("auto")), None);
        assert_eq!(dictation_language(Some("UR")), Some("ur"));
        assert_eq!(dictation_language(Some("xx")), None);
        assert_eq!(language_name(Some("ur")), "Urdu");
        assert_eq!(language_name(None), "Auto");
        let r = LastRun {
            model: "moonshine-streaming-small".into(),
            device: "CPU · 4 threads".into(),
            at_unix: 1_761_486_120, // 13:42 UTC
            audio_secs: Some(3.0),
            ms: 1_900,
            language: Some("ur".into()),
        };
        assert_eq!(
            r.line(5 * 3600),
            "moonshine-streaming-small · CPU · 4 threads · 18:42 · 3.0 s of audio in 1.9 s · heard Urdu"
        );
    }

    #[test]
    fn devices_parse_from_prefs() {
        assert_eq!(Device::parse("GPU"), Some(Device::Gpu));
        assert_eq!(Device::parse(" cpu "), Some(Device::Cpu));
        assert_eq!(Device::parse("tpu"), None);
        let c = from_prefs(Some("cpu"), Some(4));
        assert_eq!(c.device, Device::Cpu);
        assert_eq!(c.threads, Some(4));
        assert_eq!(from_prefs(None, Some(0)), Choice::default());
    }
}
