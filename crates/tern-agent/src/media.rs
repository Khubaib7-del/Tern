//! Routing work to the model that can do it (`docs/models.md`).
//!
//! | Job | Model | Endpoint |
//! | --- | --- | --- |
//! | The mic | a speech-to-text model on any configured provider | `/audio/transcriptions` (Groq, OpenAI, Mistral Voxtral, compatible servers) |
//! | A picture the chat model cannot see | a vision chat model, else an OCR model | chat with the picture, or Mistral `/ocr` |
//!
//! Choices come from the user's saved defaults (`default_stt_model`,
//! `default_vision_model`, `default_ocr_model`, as `provider/model`), else
//! an automatic pick from the listings. What another model read is passed
//! to the chat model as untrusted data, with a notice saying which model
//! read it. When nothing can do the job, the reason says what is missing.

use futures::future::join_all;
use tern_providers::{ChatMessage, ChatRequest, ImageInput, ModelInfo, Wire};

use crate::providers::{SecretStore, connect_by_id};

pub use tern_providers::{Capability, Category, Lifecycle};

/// A model on a provider, written `provider/model` in preferences.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    pub fn new(provider: &str, model: &str) -> Self {
        Self {
            provider: provider.to_owned(),
            model: model.to_owned(),
        }
    }

    /// Reads `provider/model`; the model part may hold more slashes
    /// (`groq/openai/gpt-oss-20b`).
    pub fn parse(s: &str) -> Option<Self> {
        let (p, m) = s.trim().split_once('/')?;
        (!p.is_empty() && !m.is_empty()).then(|| Self::new(p, m))
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

/// The provider word of a [`ModelRef`] naming a local model
/// (`local/whisper-small-q5_1`): it runs on this computer
/// (`crate::local_models`), not on a provider.
pub const LOCAL: &str = "local";

impl ModelRef {
    /// A local model by id.
    pub fn local(id: &str) -> Self {
        Self::new(LOCAL, id)
    }

    /// Whether it is a local model.
    pub fn is_local(&self) -> bool {
        self.provider == LOCAL
    }
}

impl std::fmt::Display for ModelRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_local() {
            return write!(f, "{} (on this computer)", self.model);
        }
        let name = tern_providers::find(&self.provider).map_or(self.provider.as_str(), |p| p.name);
        write!(f, "{} ({name})", self.model)
    }
}

/// A job Tern routes to a model other than the chat model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    /// The mic: speech to text.
    Transcribe,
    /// Describe a picture (a chat model that sees).
    Describe,
    /// Read the text out of a picture or document.
    Ocr,
}

impl Job {
    /// The model's job in words, for notices.
    fn verb(self) -> &'static str {
        match self {
            Self::Transcribe => "transcribed",
            Self::Describe => "described",
            Self::Ocr => "read",
        }
    }
}

/// Whether `m` on `provider` can do `job` through an endpoint Tern calls.
pub fn can_do(job: Job, provider: &str, m: &ModelInfo) -> bool {
    if m.is_retired() {
        return false;
    }
    let openai_style =
        tern_providers::find(provider).is_some_and(|p| p.wire == Wire::OpenAiCompatible);
    let id = m.id.to_ascii_lowercase();
    match job {
        // Realtime models take a WebSocket stream, not a file.
        Job::Transcribe => {
            m.has(Capability::SpeechToText) && openai_style && !id.contains("realtime")
        }
        Job::Describe => m.sees_images() && !is_not_vision(provider, &m.id),
        // Only Mistral's `/ocr` endpoint is wired up.
        Job::Ocr => m.has(Capability::Ocr) && provider == "mistral",
    }
}

/// Every configured provider's listing, as the routing sees it.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    /// In catalog order. A provider whose listing failed keeps its error.
    pub providers: Vec<(&'static str, Result<Vec<ModelInfo>, String>)>,
}

impl Catalog {
    pub fn new(providers: Vec<(&'static str, Result<Vec<ModelInfo>, String>)>) -> Self {
        Self { providers }
    }

    /// Every listed model with its provider.
    pub fn models(&self) -> impl Iterator<Item = (&'static str, &ModelInfo)> {
        self.providers
            .iter()
            .filter_map(|(p, r)| r.as_ref().ok().map(|l| (*p, l)))
            .flat_map(|(p, l)| l.iter().map(move |m| (p, m)))
    }

    /// A model by provider and id (or alias).
    pub fn find(&self, provider: &str, model: &str) -> Option<&ModelInfo> {
        self.models()
            .find(|(p, m)| {
                *p == provider && (m.id == model || m.aliases.iter().any(|a| a == model))
            })
            .map(|(_, m)| m)
    }

    /// What `model` on `provider` can do: its listing, else Tern's table.
    pub fn info(&self, provider: &str, model: &str) -> ModelInfo {
        self.find(provider, model).cloned().unwrap_or_else(|| {
            let mut m = ModelInfo::new(model);
            m.finish(provider);
            m
        })
    }

    /// The model for `job`: the saved choice when it is listed and can do
    /// the job, else the best listed model. `near` (the chat's provider)
    /// is tried first.
    pub fn pick(&self, job: Job, saved: Option<&str>, near: Option<&str>) -> Option<ModelRef> {
        self.pick_other(job, saved, near, None)
    }

    /// [`Catalog::pick`] leaving out `except` (provider, model): the chat
    /// model itself, when another model is to read for it.
    pub fn pick_other(
        &self,
        job: Job,
        saved: Option<&str>,
        near: Option<&str>,
        except: Option<(&str, &str)>,
    ) -> Option<ModelRef> {
        let other = |p: &str, id: &str| except.is_none_or(|(ep, em)| !(ep == p && em == id));
        if let Some(r) = saved.and_then(ModelRef::parse)
            && let Some(m) = self.find(&r.provider, &r.model)
            && can_do(job, &r.provider, m)
            && other(&r.provider, &m.id)
        {
            return Some(ModelRef::new(&r.provider, &m.id));
        }
        let mut best: Option<(i32, &'static str, &ModelInfo)> = None;
        for (p, m) in self
            .models()
            .filter(|(p, m)| can_do(job, p, m) && other(p, &m.id))
        {
            let score = rank(job, p, m) + if Some(p) == near { 100 } else { 0 };
            if best.as_ref().is_none_or(|(s, _, _)| score > *s) {
                best = Some((score, p, m));
            }
        }
        best.map(|(_, p, m)| ModelRef::new(p, &m.id))
    }

    /// Whether a speech-to-text model is available anywhere.
    pub fn can_transcribe(&self) -> bool {
        self.models().any(|(p, m)| can_do(Job::Transcribe, p, m))
    }
}

/// Higher is a better default for the job. Small and fast for the mic and
/// for describing pictures; the newest OCR.
fn rank(job: Job, _provider: &str, m: &ModelInfo) -> i32 {
    let id = m.id.to_ascii_lowercase();
    let has = |k: &str| id.contains(k);
    match job {
        Job::Transcribe => {
            let mut s = 0;
            if has("turbo") || has("gpt-transcribe") || has("mini-transcribe") {
                s += 20;
            }
            if has("distil") || has("-en") {
                s -= 5;
            }
            s
        }
        Job::Describe => {
            let mut s = 0;
            if ["mini", "flash", "small", "8b", "lite", "haiku", "scout"]
                .iter()
                .any(|k| has(k))
            {
                s += 10;
            }
            if has("preview") || has("exp") {
                s -= 5;
            }
            s
        }
        // Newer OCR ids sort later (`mistral-ocr-4-1` after `-2512`).
        Job::Ocr => {
            let v: i32 = id
                .trim_start_matches("mistral-ocr-")
                .split('-')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            if v < 100 { v * 10_000 } else { v }
        }
    }
}

/// Lists every provider in `ids` at once. Each gets its saved key (or none,
/// for local servers) and keeps its own error.
pub async fn catalog(ids: &[&'static str], secrets: Option<&dyn SecretStore>) -> Catalog {
    let connected: Vec<_> = ids
        .iter()
        .map(|id| (*id, connect_by_id(id, secrets, None)))
        .collect();
    let lists = join_all(connected.iter().map(|(id, p)| async move {
        let r = match p {
            Ok(p) => p.list_models_cached().await.map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        (*id, r)
    }))
    .await;
    Catalog::new(lists)
}

/// Transcribes recorded speech (`audio` is a file such as a WAV; its name
/// tells the server the format).
/// A local model (`via.is_local()`) runs on this computer, off the async
/// threads.
pub async fn transcribe(
    secrets: Option<&dyn SecretStore>,
    via: &ModelRef,
    audio: &[u8],
    file_name: &str,
) -> Result<String, String> {
    let ctl = crate::local_models::TranscribeCtl::new();
    transcribe_with(secrets, via, audio, file_name, &ctl, None).await
}

/// How long a transcription may take before Tern stops it: a minute, or
/// four times the audio's length when that is longer (a slow computer on
/// a long file), at most 20 minutes.
pub fn transcribe_timeout(audio_secs: Option<f64>) -> std::time::Duration {
    let secs = audio_secs.unwrap_or(0.0).max(0.0) * 4.0;
    std::time::Duration::from_secs_f64(secs.clamp(60.0, 1_200.0))
}

/// [`transcribe`] with progress and cancel through `ctl`, and a time
/// limit ([`transcribe_timeout`] of `audio_secs`). A cancelled one ends
/// with [`crate::local_models::CANCELLED`]. Each run leaves a line in the
/// diagnostic log ([`crate::diag`]) with the model and its timings.
pub async fn transcribe_with(
    secrets: Option<&dyn SecretStore>,
    via: &ModelRef,
    audio: &[u8],
    file_name: &str,
    ctl: &crate::local_models::TranscribeCtl,
    audio_secs: Option<f64>,
) -> Result<String, String> {
    use crate::local_models::CANCELLED;
    let limit = transcribe_timeout(audio_secs);
    let started = std::time::Instant::now();
    // Which transcriber took the file (local models come first), so a
    // slow or failed transcript in tern.log names its engine.
    crate::diag::note(&format!(
        "transcribe: start {via} ({}) file={} audio={} limit={}s",
        if via.is_local() { "local" } else { "provider" },
        ext_of(file_name),
        audio_secs.map_or_else(|| "?".to_owned(), |s| format!("{s:.1}s")),
        limit.as_secs()
    ));
    let r: Result<String, String> = if via.is_local() {
        let (id, bytes, name, c) = (
            via.model.clone(),
            audio.to_vec(),
            file_name.to_owned(),
            ctl.clone(),
        );
        let job = tokio::task::spawn_blocking(move || transcribe_local(&id, &bytes, &name, &c));
        match tokio::time::timeout(limit, job).await {
            Ok(joined) => match joined.map_err(|e| e.to_string()) {
                Ok(Ok(t)) => {
                    crate::diag::note(&format!(
                        "transcribe: {via} file={} audio={:.1}s decode={}ms resample={}ms model={}ms total={}ms engine={}",
                        ext_of(file_name),
                        t.audio_secs,
                        t.decode_ms,
                        t.resample_ms,
                        t.model_ms,
                        started.elapsed().as_millis(),
                        t.engine
                    ));
                    Ok(t.text)
                }
                Ok(Err(e)) => {
                    crate::diag::note(&format!(
                        "transcribe: {via} file={} failed after {}ms: {e}",
                        ext_of(file_name),
                        started.elapsed().as_millis()
                    ));
                    Err(e)
                }
                Err(e) => Err(e),
            },
            Err(_) => {
                // whisper.cpp checks the flag between steps and stops.
                ctl.cancel();
                Err(timed_out(via, limit))
            }
        }
    } else {
        let remote = async {
            let p = connect_by_id(&via.provider, secrets, None).map_err(|e| e.to_string())?;
            p.transcribe(&via.model, audio, file_name, None)
                .await
                .map_err(|e| e.to_string())
        };
        let cancelled = async {
            while !ctl.is_cancelled() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
        tokio::select! {
            r = tokio::time::timeout(limit, remote) => r.unwrap_or_else(|_| Err(timed_out(via, limit))),
            _ = cancelled => Err(CANCELLED.into()),
        }
    };
    if ctl.is_cancelled() && r.is_err() {
        crate::diag::note(&format!(
            "transcribe: {via} file={} stopped after {}ms",
            ext_of(file_name),
            started.elapsed().as_millis()
        ));
        return Err(r
            .err()
            .filter(|e| e.contains("longer than"))
            .unwrap_or_else(|| CANCELLED.into()));
    }
    match r {
        Ok(text) if text.trim().is_empty() => {
            Err(format!("{via} heard no speech in the recording."))
        }
        Ok(text) => {
            if !via.is_local() {
                crate::diag::note(&format!(
                    "transcribe: {via} file={} total={}ms",
                    ext_of(file_name),
                    started.elapsed().as_millis()
                ));
            }
            Ok(text)
        }
        Err(e) => {
            crate::diag::note(&format!(
                "transcribe: {via} file={} failed after {}ms: {e}",
                ext_of(file_name),
                started.elapsed().as_millis()
            ));
            Err(e)
        }
    }
}

fn timed_out(via: &ModelRef, limit: std::time::Duration) -> String {
    format!(
        "{via} took longer than {} s, so Tern stopped it. Try a shorter clip, or another transcription model in Settings › Local models.",
        limit.as_secs()
    )
}

/// The file's extension only (names stay out of the log).
fn ext_of(name: &str) -> String {
    std::path::Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "?".into())
}

/// Transcribes an audio file with installed local model `id`.
#[cfg(feature = "audio")]
fn transcribe_local(
    id: &str,
    audio: &[u8],
    file_name: &str,
    ctl: &crate::local_models::TranscribeCtl,
) -> Result<crate::local_models::Transcript, String> {
    let (lm, _) = crate::local_models::LocalModels::discover()?;
    lm.transcribe_file_with(id, audio, file_name, None, ctl)
}

#[cfg(not(feature = "audio"))]
fn transcribe_local(
    id: &str,
    _audio: &[u8],
    _file_name: &str,
    _ctl: &crate::local_models::TranscribeCtl,
) -> Result<crate::local_models::Transcript, String> {
    Err(format!("this build cannot run the local model {id}"))
}

/// The local model that transcribes for `task` (Dictation for the mic,
/// SpeechToText for files): the one in use when it is installed, on, and
/// this build can run it.
pub fn local_transcriber(
    local: &crate::local_models::LocalModels,
    prefs: &crate::local_models::LocalModelPrefs,
    task: crate::local_models::Task,
) -> Option<ModelRef> {
    local
        .usable(prefs, task)
        .filter(|e| crate::local_models::can_run(e))
        .map(|e| ModelRef::local(&e.id))
}

/// Audio files the mic routing accepts as an attachment (and `.opus`,
/// which is recognised so it gets a plain "not supported yet").
pub fn is_audio_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        ".wav", ".mp3", ".m4a", ".aac", ".ogg", ".opus", ".webm", ".flac", ".mp4", ".mpga", ".mpeg",
    ]
    .iter()
    .any(|e| n.ends_with(e))
}

/// The extensions the + menu's Audio picker keeps (lower case).
pub const AUDIO_EXTENSIONS: [&str; 7] = ["m4a", "mp3", "wav", "ogg", "flac", "aac", "opus"];

/// Why an audio file cannot be transcribed yet, before trying: Opus
/// (`.opus`) has no decoder in Tern yet.
pub fn audio_not_supported(name: &str) -> Option<String> {
    name.to_ascii_lowercase().ends_with(".opus").then(|| {
        format!(
            "{name} is Opus audio, which Tern cannot transcribe yet. Save or convert it as M4A, MP3, WAV, Ogg Vorbis or FLAC and attach that."
        )
    })
}

/// How long an audio file plays, in seconds, read from its headers (no
/// decoding), when this build reads audio and the file says.
pub fn audio_duration(path: &std::path::Path) -> Option<f64> {
    #[cfg(feature = "audio")]
    {
        crate::local_models::audio::duration_secs(path)
    }
    #[cfg(not(feature = "audio"))]
    {
        let _ = path;
        None
    }
}

/// "0:42", "12:05", "1:02:03".
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// What to do with pictures attached to a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImagePlan {
    /// The chat model sees them.
    Send,
    /// Another model reads them first.
    Read { via: ModelRef, job: Job },
    /// Nothing can read them; the reason says what is missing.
    Missing(String),
}

/// One way to get a picture's content to a chat model that cannot see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reader {
    /// A local model on this computer: a vision model that describes
    /// (`describe`), else an OCR model that reads the text.
    Local {
        model: String,
        name: String,
        describe: bool,
    },
    /// A model on a configured provider (vision or OCR).
    Remote { via: ModelRef, job: Job },
}

impl Reader {
    /// Who read it, for notes: "local PP-OCRv6 small", "pixtral-12b (Mistral)".
    pub fn label(&self) -> String {
        match self {
            Self::Local { name, .. } => format!("local {name}"),
            Self::Remote { via, .. } => via.to_string(),
        }
    }

    /// What it did, for the notice.
    fn did(&self, names: &str) -> String {
        match self {
            Self::Local {
                describe: true,
                name,
                ..
            } => format!("described {names} with local {name}"),
            Self::Local { name, .. } => format!("read the text in {names} with local {name}"),
            Self::Remote { via, job: Job::Ocr } => format!("read the text in {names} with {via}"),
            Self::Remote { via, .. } => format!("had {via} describe {names}"),
        }
    }
}

/// How the pictures attached to a message reach the chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageRoute {
    /// The chat model sees them. `fallback` reads them instead if the
    /// provider turns out to refuse pictures for this model.
    Send { fallback: Vec<Reader> },
    /// The chat model cannot see: these read them first, in order, each
    /// tried when the one before fails.
    Read(Vec<Reader>),
    /// Nothing can read them; the reason says what is missing. The
    /// message still goes, with a note.
    Missing(String),
}

/// Models the provider refused pictures for, by `provider/model`; they are
/// no longer treated as seeing (saved in `<data>/not-vision.json`).
fn not_vision() -> &'static std::sync::Mutex<std::collections::BTreeSet<String>> {
    static SET: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeSet<String>>> =
        std::sync::OnceLock::new();
    SET.get_or_init(|| std::sync::Mutex::new(load_not_vision()))
}

fn not_vision_file() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    tern_platform::AppPaths::discover()
        .ok()
        .map(|p| p.data_dir.join("not-vision.json"))
}

fn load_not_vision() -> std::collections::BTreeSet<String> {
    not_vision_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Remembers that `provider` refused pictures for `model`, so it is never
/// picked to see them again (the chat model or a describer).
pub fn mark_not_vision(provider: &str, model: &str) {
    let key = format!("{provider}/{model}");
    let Ok(mut set) = not_vision().lock() else {
        return;
    };
    if set.insert(key)
        && let Some(path) = not_vision_file()
        && let Ok(json) = serde_json::to_string_pretty(&*set)
    {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, json);
    }
}

/// Whether `provider` refused pictures for `model` before.
pub fn is_not_vision(provider: &str, model: &str) -> bool {
    not_vision()
        .lock()
        .is_ok_and(|s| s.contains(&format!("{provider}/{model}")))
}

/// Whether a provider error says the model does not take pictures
/// (Mistral code 3051 "Image input is not enabled for this model", OpenAI
/// "does not support image", and the like).
pub fn rejects_images(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    e.contains("3051")
        || [
            "image input is not enabled",
            "image input is not supported",
            "does not support image",
            "does not support vision",
            "doesn't support image",
            "image_url is only supported",
            "images are not supported",
            "vision is not supported",
            "not a multimodal model",
            "unsupported content type: image",
        ]
        .iter()
        .any(|k| e.contains(k))
}

/// An error in plain words: a provider's JSON body becomes its message.
pub fn plain_error(error: &str) -> String {
    let e = error.trim();
    match e.find(" returned HTTP ") {
        Some(i) => {
            let (who, rest) = e.split_at(i);
            let rest = rest.trim_start_matches(" returned HTTP ");
            let (status, body) = rest.split_once(':').unwrap_or((rest, ""));
            format!(
                "{who}: {} (HTTP {})",
                tern_providers::error::plain_body(body),
                status.trim()
            )
        }
        None if e.contains("{\"") => {
            let start = e.find('{').unwrap_or(0);
            let head = e[..start].trim().trim_end_matches(':');
            let body = tern_providers::error::plain_body(&e[start..]);
            if head.is_empty() {
                body
            } else {
                format!("{head}: {body}")
            }
        }
        None => e.to_owned(),
    }
}

/// Every reader for a chat with `model` on `provider`, best first: the
/// local vision model, the local OCR model (installed, on, and runnable),
/// then a provider's vision model, then a provider's OCR model. A saved
/// OCR default goes before an automatic vision pick.
pub fn readers_for(
    catalog: &Catalog,
    provider: &str,
    model: &str,
    prefs: &crate::Preferences,
    local: Option<&crate::local_models::LocalModels>,
) -> Vec<Reader> {
    use crate::local_models::{Task, can_run};
    let mut out = Vec::new();
    if let Some(lm) = local {
        for (task, describe) in [(Task::Vision, true), (Task::Ocr, false)] {
            if let Some(e) = lm.usable(&prefs.local_models, task).filter(|e| can_run(e)) {
                out.push(Reader::Local {
                    model: e.id.clone(),
                    name: e.name.clone(),
                    describe,
                });
            }
        }
    }
    let near = Some(provider);
    let vision = catalog.pick_other(
        Job::Describe,
        prefs.default_vision_model.as_deref(),
        near,
        Some((provider, model)),
    );
    let ocr = catalog.pick(Job::Ocr, prefs.default_ocr_model.as_deref(), near);
    let saved_ocr_first = prefs.default_vision_model.is_none() && prefs.default_ocr_model.is_some();
    let mut remote = Vec::new();
    if let Some(via) = vision {
        remote.push(Reader::Remote {
            via,
            job: Job::Describe,
        });
    }
    if let Some(via) = ocr {
        let r = Reader::Remote { via, job: Job::Ocr };
        if saved_ocr_first {
            remote.insert(0, r);
        } else {
            remote.push(r);
        }
    }
    out.extend(remote);
    out
}

/// Decides how pictures reach a chat with `model` on `provider`: sent
/// when the chat model sees (and was never refused), else read first by
/// [`readers_for`], local models before a provider's.
pub fn route_images(
    catalog: &Catalog,
    provider: &str,
    model: &str,
    prefs: &crate::Preferences,
    local: Option<&crate::local_models::LocalModels>,
) -> ImageRoute {
    let readers = readers_for(catalog, provider, model, prefs, local);
    if catalog.info(provider, model).sees_images() && !is_not_vision(provider, model) {
        return ImageRoute::Send { fallback: readers };
    }
    if !readers.is_empty() {
        return ImageRoute::Read(readers);
    }
    let mut why = format!(
        "{model} cannot see pictures, and nothing can read them: no local OCR or vision model is installed and on, and no configured provider lists a vision or OCR model. Download one in Settings › Local models, add a provider key (Mistral, Groq, Gemini), or pick a chat model that can see."
    );
    if let Some(lm) = local {
        for task in [
            crate::local_models::Task::Vision,
            crate::local_models::Task::Ocr,
        ] {
            if let Some(e) = lm
                .usable(&prefs.local_models, task)
                .filter(|e| !crate::local_models::can_run(e))
            {
                why.push_str(&format!(
                    " The local model {} is downloaded, but this build cannot run {} models yet.",
                    e.name,
                    e.runtime.label()
                ));
            }
        }
    }
    ImageRoute::Missing(why)
}

/// What reading the pictures came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadOutcome {
    /// What joins the user's message: one untrusted block per picture
    /// read, and a plain note for each picture nobody could read.
    pub text: String,
    /// The notice for the chat (information, not an error).
    pub notice: String,
    /// The reader that read them, if one did.
    pub used: Option<Reader>,
    /// Each reader that failed, with the reason in plain words.
    pub failures: Vec<(Reader, String)>,
}

/// Tries `readers` in order on `images` until one reads them all; each
/// failure (a refused key, credits, a rate limit, a timeout, a server
/// error, a model that refuses pictures, a local model that will not
/// load or reads nothing) falls through to the next. `read` runs one
/// reader on the pictures and returns their texts in order. When every
/// reader fails the outcome still carries a note for the message, so the
/// turn goes on without the pictures instead of failing.
pub async fn read_chain<F, Fut>(
    readers: &[Reader],
    images: &[ImageInput],
    chat_model: &str,
    mut read: F,
) -> ReadOutcome
where
    F: FnMut(Reader, Vec<ImageInput>) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<String>, String>>,
{
    let mut out = ReadOutcome::default();
    // Pictures no reader takes (HEIC, junk) get their note up front.
    let mut ready = Vec::new();
    let mut notes = Vec::new();
    for img in images {
        match crate::pictures::prepare(img) {
            Ok(p) => ready.push(p),
            Err(e) => notes.push(format!("[Tern couldn't read {}: {e}]", img.name)),
        }
    }
    let names = |imgs: &[ImageInput]| {
        imgs.iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !ready.is_empty() {
        for r in readers {
            match read(r.clone(), ready.clone()).await {
                Ok(texts) => {
                    let blocks: Vec<String> = ready
                        .iter()
                        .zip(&texts)
                        .map(|(img, t)| {
                            let what = match r {
                                Reader::Local {
                                    describe: false, ..
                                }
                                | Reader::Remote { job: Job::Ocr, .. } => "OCR text",
                                _ => "description",
                            };
                            crate::untrusted::wrap(
                                &format!("{what} of {} by {}", img.name, r.label()),
                                t.trim(),
                            )
                        })
                        .collect();
                    out.text = blocks.join("\n\n");
                    out.notice = format!(
                        "{chat_model} cannot see pictures, so Tern {}.",
                        r.did(&names(&ready))
                    );
                    out.used = Some(r.clone());
                    break;
                }
                Err(e) => out.failures.push((r.clone(), plain_error(&e))),
            }
        }
        if out.used.is_none() {
            let why = match out.failures.last() {
                Some((r, e)) => format!("{} failed: {e}", r.label()),
                None => "no reader is set up for pictures".into(),
            };
            for img in &ready {
                notes.push(format!("[Tern couldn't read {}: {why}]", img.name));
            }
        }
    }
    if !out.failures.is_empty() && out.used.is_some() {
        let tried: Vec<String> = out
            .failures
            .iter()
            .map(|(r, e)| format!("{} failed: {e}", r.label()))
            .collect();
        out.notice.push_str(&format!(" ({})", tried.join("; ")));
    }
    if !notes.is_empty() {
        if out.used.is_none() {
            out.notice = format!(
                "{chat_model} cannot see pictures, and Tern couldn't read them, so the message went without them. {}",
                notes.join(" ")
            );
        }
        let joined = notes.join("\n");
        out.text = if out.text.is_empty() {
            joined
        } else {
            format!("{}\n\n{joined}", out.text)
        };
    }
    out
}

/// [`read_chain`] with the real readers: a provider's model through its
/// API, a local model on this computer (off the async threads). A
/// provider that refuses pictures for a model is remembered
/// ([`mark_not_vision`]).
pub async fn read_with(
    secrets: Option<&dyn SecretStore>,
    readers: &[Reader],
    images: &[ImageInput],
    chat_model: &str,
) -> ReadOutcome {
    read_chain(readers, images, chat_model, |r, imgs| async move {
        match r {
            Reader::Remote { via, job } => {
                let got = read_remote(secrets, &via, job, &imgs).await;
                if let Err(e) = &got
                    && job == Job::Describe
                    && rejects_images(e)
                {
                    mark_not_vision(&via.provider, &via.model);
                }
                got
            }
            Reader::Local {
                model, describe, ..
            } => tokio::task::spawn_blocking(move || read_local(&model, describe, &imgs))
                .await
                .unwrap_or_else(|e| Err(e.to_string())),
        }
    })
    .await
}

async fn read_remote(
    secrets: Option<&dyn SecretStore>,
    via: &ModelRef,
    job: Job,
    images: &[ImageInput],
) -> Result<Vec<String>, String> {
    let p = connect_by_id(&via.provider, secrets, None).map_err(|e| e.to_string())?;
    let mut texts = Vec::new();
    for img in images {
        let fut = async {
            match job {
                Job::Ocr => p
                    .ocr(&via.model, &img.media_type, &img.data)
                    .await
                    .map_err(|e| e.to_string()),
                _ => {
                    let msg = ChatMessage::user(DESCRIBE).with_images(vec![img.clone()]);
                    let req = ChatRequest::new(&via.model, vec![msg]);
                    p.chat(&req)
                        .await
                        .map(|r| r.text)
                        .map_err(|e| e.to_string())
                }
            }
        };
        let text = tokio::time::timeout(READ_WAIT, fut)
            .await
            .map_err(|_| format!("{via} did not answer in {} s", READ_WAIT.as_secs()))??;
        if text.trim().is_empty() {
            return Err(format!("{via} returned nothing for {}", img.name));
        }
        texts.push(text);
    }
    Ok(texts)
}

/// How long one picture may take at a provider.
const READ_WAIT: std::time::Duration = std::time::Duration::from_secs(90);

fn read_local(id: &str, describe: bool, images: &[ImageInput]) -> Result<Vec<String>, String> {
    use base64::Engine as _;
    let (lm, _) = crate::local_models::LocalModels::discover()?;
    let mut texts = Vec::new();
    for img in images {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&img.data)
            .map_err(|e| format!("{} is not readable: {e}", img.name))?;
        let text = if describe {
            lm.describe(id, &bytes, LOCAL_DESCRIBE)?
        } else {
            lm.ocr(id, &bytes)?
        };
        if text.trim().is_empty() {
            return Err(format!(
                "found no {} in {}",
                if describe { "description" } else { "text" },
                img.name
            ));
        }
        texts.push(text);
    }
    Ok(texts)
}

/// What a small local vision model is asked.
const LOCAL_DESCRIBE: &str =
    "Describe this picture in detail. Transcribe any visible text exactly.";
/// Decides how pictures reach a chat with `model` on `provider`.
pub fn plan_images(
    catalog: &Catalog,
    provider: &str,
    model: &str,
    prefs: &crate::Preferences,
) -> ImagePlan {
    if catalog.info(provider, model).sees_images() {
        return ImagePlan::Send;
    }
    let near = Some(provider);
    if let Some(via) = catalog.pick(Job::Describe, prefs.default_vision_model.as_deref(), near) {
        // A saved OCR default wins over an automatic vision pick.
        if prefs.default_vision_model.is_none()
            && let Some(ocr) = prefs
                .default_ocr_model
                .as_deref()
                .and_then(|s| catalog.pick(Job::Ocr, Some(s), near))
        {
            return ImagePlan::Read {
                via: ocr,
                job: Job::Ocr,
            };
        }
        return ImagePlan::Read {
            via,
            job: Job::Describe,
        };
    }
    if let Some(via) = catalog.pick(Job::Ocr, prefs.default_ocr_model.as_deref(), near) {
        return ImagePlan::Read { via, job: Job::Ocr };
    }
    ImagePlan::Missing(format!(
        "{model} cannot see pictures, and no configured provider lists a vision or OCR model to read them. Add a key for one (Mistral OCR or Pixtral, a Groq vision model, Gemini), or pick a chat model that can see. The pictures were not sent."
    ))
}

const DESCRIBE: &str = "Describe this picture for someone who cannot see it, so another model can answer questions about it. Transcribe every piece of visible text exactly, keep tables as Markdown tables, give the numbers in charts, and say what the picture shows. Do not add opinions.";

/// Has `via` read `images`; returns one untrusted block per picture, ready
/// to append to the user's message, and the notice for the chat.
pub async fn read_images(
    secrets: Option<&dyn SecretStore>,
    via: &ModelRef,
    job: Job,
    images: &[ImageInput],
    chat_model: &str,
) -> Result<(String, String), String> {
    let p = connect_by_id(&via.provider, secrets, None).map_err(|e| e.to_string())?;
    let mut blocks = Vec::new();
    for img in images {
        let text = match job {
            Job::Ocr => p
                .ocr(&via.model, &img.media_type, &img.data)
                .await
                .map_err(|e| e.to_string())?,
            _ => {
                let msg = ChatMessage::user(DESCRIBE).with_images(vec![img.clone()]);
                let req = ChatRequest::new(&via.model, vec![msg]);
                p.chat(&req).await.map_err(|e| e.to_string())?.text
            }
        };
        let source = format!("{} of {} by {}", job_noun(job), img.name, via.key());
        blocks.push(crate::untrusted::wrap(&source, text.trim()));
    }
    let names: Vec<&str> = images.iter().map(|i| i.name.as_str()).collect();
    let notice = format!(
        "{chat_model} cannot see pictures, so {via} {} {} first and passed the text on.",
        job.verb(),
        names.join(", ")
    );
    Ok((blocks.join("\n\n"), notice))
}

fn job_noun(job: Job) -> &'static str {
    match job {
        Job::Transcribe => "transcript",
        Job::Describe => "description",
        Job::Ocr => "OCR text",
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn transcription_time_limit_scales_with_the_audio() {
        use std::time::Duration;
        assert_eq!(transcribe_timeout(None), Duration::from_secs(60));
        assert_eq!(transcribe_timeout(Some(12.0)), Duration::from_secs(60));
        assert_eq!(transcribe_timeout(Some(60.0)), Duration::from_secs(240));
        assert_eq!(
            transcribe_timeout(Some(36_000.0)),
            Duration::from_secs(1_200)
        );
    }

    #[test]
    fn a_cancelled_remote_transcription_ends_at_once() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ctl = crate::local_models::TranscribeCtl::new();
        ctl.cancel();
        // A provider that is not set up fails fast either way; the
        // cancel wins or the error comes back, never a wait.
        let t = std::time::Instant::now();
        let via = ModelRef {
            provider: "groq".into(),
            model: "whisper-large-v3".into(),
        };
        let r = rt.block_on(transcribe_with(
            None,
            &via,
            b"RIFF",
            "a.wav",
            &ctl,
            Some(12.0),
        ));
        assert!(r.is_err());
        assert!(t.elapsed().as_secs() < 5);
    }

    use super::*;

    fn listed(provider: &str, ids: &[&str]) -> Vec<ModelInfo> {
        ids.iter()
            .map(|id| {
                let mut m = ModelInfo::new(*id);
                m.finish(provider);
                m
            })
            .collect()
    }

    fn catalog() -> Catalog {
        Catalog::new(vec![
            (
                "groq",
                Ok(listed(
                    "groq",
                    &[
                        "llama-3.1-8b-instant",
                        "whisper-large-v3",
                        "whisper-large-v3-turbo",
                        "qwen/qwen3.8-27b",
                    ],
                )),
            ),
            (
                "mistral",
                Ok(listed(
                    "mistral",
                    &[
                        "ministral-8b-latest",
                        "mistral-ocr-2512",
                        "mistral-ocr-4-1",
                        "voxtral-mini-transcribe-realtime-3-0",
                        "pixtral-12b-2409",
                    ],
                )),
            ),
            ("openai", Err("refused the key".into())),
        ])
    }

    #[test]
    fn refs_parse_with_slashes_in_the_model() {
        let r = ModelRef::parse("groq/openai/gpt-oss-20b").unwrap();
        assert_eq!(
            (r.provider.as_str(), r.model.as_str()),
            ("groq", "openai/gpt-oss-20b")
        );
        assert_eq!(r.key(), "groq/openai/gpt-oss-20b");
        assert_eq!(r.to_string(), "openai/gpt-oss-20b (Groq)");
        assert!(ModelRef::parse("nothing").is_none());
    }

    #[test]
    fn the_mic_gets_a_fast_file_transcriber() {
        let c = catalog();
        assert!(c.can_transcribe());
        let r = c.pick(Job::Transcribe, None, None).unwrap();
        assert_eq!(r.key(), "groq/whisper-large-v3-turbo");
        // A saved choice wins when it is listed and can transcribe.
        let r = c
            .pick(Job::Transcribe, Some("groq/whisper-large-v3"), None)
            .unwrap();
        assert_eq!(r.model, "whisper-large-v3");
        // A saved choice that cannot transcribe is ignored.
        let r = c
            .pick(
                Job::Transcribe,
                Some("mistral/voxtral-mini-transcribe-realtime-3-0"),
                None,
            )
            .unwrap();
        assert_eq!(r.model, "whisper-large-v3-turbo");
        let none = Catalog::new(vec![(
            "mistral",
            Ok(listed("mistral", &["ministral-8b-latest"])),
        )]);
        assert!(!none.can_transcribe() && none.pick(Job::Transcribe, None, None).is_none());
    }

    #[test]
    fn pictures_go_to_a_model_that_can_read_them() {
        let c = catalog();
        let prefs = crate::Preferences::default();
        assert_eq!(
            plan_images(&c, "groq", "qwen/qwen3.8-27b", &prefs),
            ImagePlan::Send
        );
        // The chat's own provider is tried first.
        match plan_images(&c, "mistral", "ministral-8b-latest", &prefs) {
            ImagePlan::Read { via, job } => {
                assert_eq!(
                    (via.key().as_str(), job),
                    ("mistral/pixtral-12b-2409", Job::Describe)
                )
            }
            other => panic!("{other:?}"),
        }
        let prefs = crate::Preferences {
            default_ocr_model: Some("mistral/mistral-ocr-latest".into()),
            ..Default::default()
        };
        // An alias resolves to its model only when listed; here the OCR
        // default falls back to the newest OCR model.
        let ocr_only = Catalog::new(vec![(
            "mistral",
            Ok(listed(
                "mistral",
                &["ministral-8b-latest", "mistral-ocr-2512", "mistral-ocr-4-1"],
            )),
        )]);
        match plan_images(&ocr_only, "mistral", "ministral-8b-latest", &prefs) {
            ImagePlan::Read { via, job } => {
                assert_eq!((via.model.as_str(), job), ("mistral-ocr-4-1", Job::Ocr))
            }
            other => panic!("{other:?}"),
        }
        let bare = Catalog::new(vec![(
            "groq",
            Ok(listed("groq", &["llama-3.1-8b-instant"])),
        )]);
        let ImagePlan::Missing(why) = plan_images(&bare, "groq", "llama-3.1-8b-instant", &prefs)
        else {
            panic!()
        };
        assert!(
            why.contains("llama-3.1-8b-instant cannot see pictures"),
            "{why}"
        );
    }

    fn seer(provider: &str, id: &str, sees: bool) -> ModelInfo {
        let mut m = ModelInfo::new(id);
        m.finish(provider);
        m.capabilities.retain(|c| *c != Capability::Vision);
        if !m.capabilities.contains(&Capability::Chat) {
            m.capabilities.push(Capability::Chat);
        }
        if sees {
            m.capabilities.push(Capability::Vision);
        }
        m
    }

    fn png(name: &str) -> ImageInput {
        use base64::Engine as _;
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        ImageInput {
            name: name.into(),
            media_type: "image/png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(out.into_inner()),
        }
    }

    fn local(name: &str) -> Reader {
        Reader::Local {
            model: name.to_ascii_lowercase(),
            name: name.into(),
            describe: false,
        }
    }

    #[test]
    fn a_chat_model_refused_pictures_is_never_sent_them_again() {
        let c = Catalog::new(vec![(
            "acme",
            Ok(vec![
                seer("acme", "chatty-vision-1", true),
                seer("acme", "eyes-2", true),
            ]),
        )]);
        let prefs = crate::Preferences::default();
        match route_images(&c, "acme", "chatty-vision-1", &prefs, None) {
            ImageRoute::Send { fallback } => assert_eq!(
                fallback,
                vec![Reader::Remote {
                    via: ModelRef::new("acme", "eyes-2"),
                    job: Job::Describe
                }]
            ),
            other => panic!("{other:?}"),
        }
        // The provider said "Image input is not enabled for this model".
        assert!(rejects_images(
            "Acme: Image input is not enabled for this model (code 3051) (HTTP 400)"
        ));
        mark_not_vision("acme", "chatty-vision-1");
        assert!(is_not_vision("acme", "chatty-vision-1"));
        match route_images(&c, "acme", "chatty-vision-1", &prefs, None) {
            ImageRoute::Read(r) => assert_eq!(r.len(), 1),
            other => panic!("{other:?}"),
        }
        // A describer that refused is not picked either.
        mark_not_vision("acme", "eyes-2");
        assert!(matches!(
            route_images(&c, "acme", "chatty-vision-1", &prefs, None),
            ImageRoute::Missing(_)
        ));
    }

    #[test]
    fn errors_that_mean_no_pictures() {
        for e in [
            "Mistral returned HTTP 400: {\"message\":\"Image input is not enabled for this model\",\"code\":\"3051\"}",
            "OpenAI: Invalid content type. image_url is only supported by certain models (HTTP 400)",
            "this model does not support image input",
        ] {
            assert!(rejects_images(e), "{e}");
        }
        for e in [
            "Groq is rate limiting this key",
            "Mistral: Unauthorized (HTTP 401)",
        ] {
            assert!(!rejects_images(e), "{e}");
        }
    }

    #[test]
    fn raw_provider_json_reads_as_plain_words() {
        assert_eq!(
            plain_error(
                r#"Mistral returned HTTP 400: {"object":"error","message":"Image input is not enabled for this model","type":"invalid_request_invalid_args","param":null,"code":"3051","raw_status_code":400}"#
            ),
            "Mistral: Image input is not enabled for this model (code 3051) (HTTP 400)"
        );
        assert_eq!(
            plain_error(r#"Run failed: {"error":{"message":"Insufficient credits"}}"#),
            "Run failed: Insufficient credits"
        );
        assert_eq!(
            plain_error("Could not reach Groq: timed out"),
            "Could not reach Groq: timed out"
        );
    }

    #[tokio::test]
    async fn a_failing_reader_falls_through_to_the_next() {
        let readers = vec![
            Reader::Remote {
                via: ModelRef::new("mistral", "ministral-14b-2512"),
                job: Job::Describe,
            },
            local("PP-OCRv6 small"),
        ];
        let calls = std::sync::Mutex::new(Vec::new());
        let out = read_chain(&readers, &[png("img.jpg")], "codestral-2508", |r, imgs| {
            calls.lock().unwrap().push(r.label());
            async move {
                match r {
                    Reader::Remote { .. } => Err(r#"Mistral returned HTTP 400: {"message":"Image input is not enabled for this model","code":"3051"}"#.to_owned()),
                    Reader::Local { .. } => Ok(imgs.iter().map(|_| "سلام پاکستان".to_owned()).collect()),
                }
            }
        })
        .await;
        assert_eq!(calls.lock().unwrap().len(), 2);
        assert_eq!(out.used, Some(local("PP-OCRv6 small")));
        assert!(out.text.contains("سلام پاکستان"), "{}", out.text);
        assert!(
            out.notice
                .contains("read the text in img.jpg with local PP-OCRv6 small"),
            "{}",
            out.notice
        );
        // The failure is said in plain words, not raw JSON.
        assert!(
            out.notice
                .contains("Image input is not enabled for this model (code 3051)")
        );
        assert!(!out.notice.contains('{'));
    }

    #[tokio::test]
    async fn every_kind_of_reader_error_falls_through() {
        for err in [
            "Mistral refused the key (HTTP 401).",
            "Mistral: Payment required (HTTP 402)",
            "Groq is rate limiting this key. Wait and try again.",
            "pixtral-12b (Mistral) did not answer in 90 s",
            "Mistral: Internal server error (HTTP 503)",
            "Could not reach Mistral: connection reset",
            "PP-OCRv6 small is not downloaded",
            "cannot load the OCR model: ORT error",
            "found no text in img.jpg",
        ] {
            let readers = vec![local("Broken"), local("Good")];
            let out = read_chain(&readers, &[png("a.png")], "m", |r, _| async move {
                if r == local("Broken") {
                    Err(err.to_owned())
                } else {
                    Ok(vec!["text".to_owned()])
                }
            })
            .await;
            assert_eq!(out.used, Some(local("Good")), "{err}");
            assert_eq!(out.failures.len(), 1);
        }
    }

    #[tokio::test]
    async fn when_every_reader_fails_the_message_goes_with_a_note() {
        let readers = vec![local("One"), local("Two")];
        let out = read_chain(
            &readers,
            &[png("img.jpg")],
            "codestral-2508",
            |_, _| async { Err::<Vec<String>, _>("Could not reach Mistral: timed out".to_owned()) },
        )
        .await;
        assert!(out.used.is_none());
        assert_eq!(out.failures.len(), 2);
        assert_eq!(
            out.text,
            "[Tern couldn't read img.jpg: local Two failed: Could not reach Mistral: timed out]"
        );
        assert!(out.notice.contains("went without them"), "{}", out.notice);
        // No reader at all still gives a note.
        let none = read_chain(&[], &[png("x.png")], "m", |_, _| async { Ok(vec![]) }).await;
        assert!(none.text.contains("couldn't read x.png"), "{}", none.text);
    }

    #[tokio::test]
    async fn a_heic_picture_gets_a_note_and_the_rest_are_read() {
        use base64::Engine as _;
        let mut heic = vec![0, 0, 0, 24];
        heic.extend(b"ftypheic\0\0\0\0mif1heic");
        let photo = ImageInput {
            name: "IMG_1.HEIC".into(),
            media_type: "image/heic".into(),
            data: base64::engine::general_purpose::STANDARD.encode(heic),
        };
        let out =
            read_chain(
                &[local("Good")],
                &[photo, png("b.png")],
                "m",
                |_, imgs| async move {
                    Ok(imgs.iter().map(|i| format!("text of {}", i.name)).collect())
                },
            )
            .await;
        assert!(out.text.contains("text of b.png"));
        assert!(
            out.text.contains("[Tern couldn't read IMG_1.HEIC:"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("text of IMG_1.HEIC"));
    }

    #[test]
    fn local_refs_name_this_computer() {
        let r = ModelRef::local("whisper-small-q5_1");
        assert!(r.is_local());
        assert_eq!(r.key(), "local/whisper-small-q5_1");
        assert_eq!(r.to_string(), "whisper-small-q5_1 (on this computer)");
        assert!(!ModelRef::new("groq", "whisper-large-v3").is_local());
    }

    #[test]
    fn audio_names() {
        assert!(is_audio_name("Memo.M4A") && is_audio_name("speech.wav"));
        assert!(is_audio_name("a.flac") && is_audio_name("b.ogg") && is_audio_name("c.mp3"));
        assert!(is_audio_name("voice.opus"));
        assert!(!is_audio_name("notes.pdf"));
        for e in AUDIO_EXTENSIONS {
            assert!(is_audio_name(&format!("x.{e}")), "{e}");
        }
        assert!(
            audio_not_supported("Voice.OPUS")
                .unwrap()
                .contains("cannot transcribe yet")
        );
        assert_eq!(audio_not_supported("memo.m4a"), None);
        assert_eq!(clock(42.4), "0:42");
        assert_eq!(clock(725.0), "12:05");
        assert_eq!(clock(3723.0), "1:02:03");
    }
}
