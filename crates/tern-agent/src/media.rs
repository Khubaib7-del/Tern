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
        Job::Describe => m.sees_images(),
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
        if let Some(r) = saved.and_then(ModelRef::parse)
            && let Some(m) = self.find(&r.provider, &r.model)
            && can_do(job, &r.provider, m)
        {
            return Some(ModelRef::new(&r.provider, &m.id));
        }
        let mut best: Option<(i32, &'static str, &ModelInfo)> = None;
        for (p, m) in self.models().filter(|(p, m)| can_do(job, p, m)) {
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
    if via.is_local() {
        let (id, audio, name) = (via.model.clone(), audio.to_vec(), file_name.to_owned());
        let text = tokio::task::spawn_blocking(move || transcribe_local(&id, &audio, &name))
            .await
            .map_err(|e| e.to_string())??;
        if text.trim().is_empty() {
            return Err(format!("{via} heard no speech in the recording."));
        }
        return Ok(text);
    }
    let p = connect_by_id(&via.provider, secrets, None).map_err(|e| e.to_string())?;
    let text = p
        .transcribe(&via.model, audio, file_name, None)
        .await
        .map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Err(format!("{via} heard no speech in the recording."));
    }
    Ok(text)
}

/// Transcribes an audio file with installed local model `id`.
#[cfg(feature = "audio")]
fn transcribe_local(id: &str, audio: &[u8], file_name: &str) -> Result<String, String> {
    let (lm, _) = crate::local_models::LocalModels::discover()?;
    lm.transcribe_file(id, audio, file_name, None)
}

#[cfg(not(feature = "audio"))]
fn transcribe_local(id: &str, _audio: &[u8], _file_name: &str) -> Result<String, String> {
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

/// Audio files the mic routing accepts as an attachment.
pub fn is_audio_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        ".wav", ".mp3", ".m4a", ".ogg", ".webm", ".flac", ".mp4", ".mpga", ".mpeg",
    ]
    .iter()
    .any(|e| n.ends_with(e))
}

/// What to do with pictures attached to a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImagePlan {
    /// The chat model sees them.
    Send,
    /// Another model reads them first.
    Read { via: ModelRef, job: Job },
    /// A local OCR model on this computer reads them first
    /// (`crate::local_models`): its id and display name.
    ReadLocal { model: String, name: String },
    /// Nothing can read them; the reason says what is missing.
    Missing(String),
}

/// [`plan_images`], falling back to an installed local OCR model when no
/// provider can read the pictures. A local model whose runtime this build
/// cannot run yet turns into a clear reason instead.
pub fn plan_images_local(
    catalog: &Catalog,
    provider: &str,
    model: &str,
    prefs: &crate::Preferences,
    local: Option<&crate::local_models::LocalModels>,
) -> ImagePlan {
    let plan = plan_images(catalog, provider, model, prefs);
    let ImagePlan::Missing(why) = &plan else {
        return plan;
    };
    let Some(e) =
        local.and_then(|lm| lm.usable(&prefs.local_models, crate::local_models::Task::Ocr))
    else {
        return plan;
    };
    if crate::local_models::can_run(e) {
        ImagePlan::ReadLocal {
            model: e.id.clone(),
            name: e.name.clone(),
        }
    } else {
        ImagePlan::Missing(format!(
            "{why} The local OCR model {} is downloaded, but this build cannot run {} models yet.",
            e.name,
            e.runtime.label()
        ))
    }
}

/// Has local model `id` read the text in `images`; the same blocks and
/// notice as [`read_images`].
pub fn read_images_local(
    local: &crate::local_models::LocalModels,
    id: &str,
    name: &str,
    images: &[ImageInput],
    chat_model: &str,
) -> Result<(String, String), String> {
    use base64::Engine as _;
    let mut blocks = Vec::new();
    for img in images {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&img.data)
            .map_err(|e| format!("{} is not readable: {e}", img.name))?;
        let text = local.ocr(id, &bytes)?;
        let source = format!("OCR text of {} by local model {id}", img.name);
        blocks.push(crate::untrusted::wrap(&source, text.trim()));
    }
    let names: Vec<&str> = images.iter().map(|i| i.name.as_str()).collect();
    Ok((
        blocks.join("\n\n"),
        format!(
            "{chat_model} cannot see pictures, so {name} read {} on this computer and passed the text on.",
            names.join(", ")
        ),
    ))
}

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
        assert!(!is_audio_name("notes.pdf"));
    }
}
