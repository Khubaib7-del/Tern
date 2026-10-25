//! Local models: on-demand OCR, vision, speech, and embedding models that
//! run on the user's machine (`docs/local-models.md`).
//!
//! Nothing is bundled. A [`Manifest`] lists the models Tern knows (the
//! built-in one, plus `<config>/local-models.json` to add or replace
//! entries without a new build). Each entry names its task, its Hugging
//! Face repo and pinned revision, its files with sizes and SHA-256 hashes,
//! its licence, and the runtime that runs it. [`LocalModels::download`]
//! fetches the files into `<data>/models/<id>/` (resumable, with progress
//! and cancel), checks every hash, and writes `tern-model.json` last, so a
//! model counts as installed only when every file is whole. Which models
//! are switched off and which is the default per task live in
//! [`Preferences::local_models`](crate::Preferences).
//!
//! The runtimes themselves (whisper.cpp, ONNX Runtime, …) plug in later:
//! [`runtime_available`] says which ones this build can run.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use tern_providers::download::{CancelDownload, Progress};

/// The built-in manifest. Its entries are placeholders until the model
/// research picks Tern's models; replacing this file is all it takes.
const BUILTIN: &str = include_str!("manifest.json");

/// The file in a model's folder written once every file is verified.
pub const RECORD: &str = "tern-model.json";

/// The user's own manifest additions, in the config folder.
pub const OVERRIDES: &str = "local-models.json";

/// Hugging Face, unless `TERN_HF_ENDPOINT` points at a mirror.
pub const HF_ENDPOINT: &str = "https://huggingface.co";

/// What a local model does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    Ocr,
    DocParse,
    Vision,
    SpeechToText,
    TextToSpeech,
    Embedding,
}

impl Task {
    /// In Settings order.
    pub const ALL: [Task; 6] = [
        Task::Ocr,
        Task::DocParse,
        Task::Vision,
        Task::SpeechToText,
        Task::TextToSpeech,
        Task::Embedding,
    ];

    /// The saved word (`speech_to_text`).
    pub fn key(self) -> &'static str {
        match self {
            Self::Ocr => "ocr",
            Self::DocParse => "doc_parse",
            Self::Vision => "vision",
            Self::SpeechToText => "speech_to_text",
            Self::TextToSpeech => "text_to_speech",
            Self::Embedding => "embedding",
        }
    }

    /// Reads a task word, saved or typed (`stt`, `doc-parse`, `tts`).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase().replace('-', "_");
        Some(match s.as_str() {
            "ocr" => Self::Ocr,
            "doc_parse" | "docparse" | "documents" => Self::DocParse,
            "vision" => Self::Vision,
            "speech_to_text" | "stt" | "speech" | "transcribe" => Self::SpeechToText,
            "text_to_speech" | "tts" | "voice" => Self::TextToSpeech,
            "embedding" | "embeddings" => Self::Embedding,
            _ => return None,
        })
    }

    /// The section heading.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ocr => "OCR",
            Self::DocParse => "Document parsing",
            Self::Vision => "Vision",
            Self::SpeechToText => "Speech to text",
            Self::TextToSpeech => "Text to speech",
            Self::Embedding => "Embeddings",
        }
    }

    /// One line under the heading.
    pub fn blurb(self) -> &'static str {
        match self {
            Self::Ocr => "Reads the text in pictures for chat models that cannot see.",
            Self::DocParse => "Turns PDFs and scans into structured text and tables.",
            Self::Vision => "Describes pictures on your machine.",
            Self::SpeechToText => "Turns the mic into text without sending audio anywhere.",
            Self::TextToSpeech => "Reads answers aloud.",
            Self::Embedding => "Turns text into vectors for search over your sources.",
        }
    }
}

/// The engine that runs a model's files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    WhisperCpp,
    Onnx,
    LlamaCpp,
    Candle,
    Piper,
    #[serde(other)]
    Other,
}

impl Runtime {
    pub fn label(self) -> &'static str {
        match self {
            Self::WhisperCpp => "whisper.cpp",
            Self::Onnx => "ONNX Runtime",
            Self::LlamaCpp => "llama.cpp",
            Self::Candle => "Candle",
            Self::Piper => "Piper",
            Self::Other => "another runtime",
        }
    }
}

/// An engine that runs one runtime's models. None ship yet: the download,
/// state, and defaults work today, and an engine (whisper.cpp, ONNX
/// Runtime, …) plugs in by calling [`register_engine`] at startup.
pub trait LocalEngine: Send + Sync {
    /// The runtime this engine runs.
    fn runtime(&self) -> Runtime;
    /// Reads the text in a picture (PNG, JPEG, GIF, or WebP bytes) with
    /// the model installed in `dir`.
    fn ocr(&self, dir: &Path, entry: &ModelEntry, image: &[u8]) -> Result<String, String> {
        let _ = (dir, image);
        Err(format!("{} cannot read pictures", entry.name))
    }
}

type Engines = std::sync::RwLock<Vec<std::sync::Arc<dyn LocalEngine>>>;

fn engines() -> &'static Engines {
    static ENGINES: std::sync::OnceLock<Engines> = std::sync::OnceLock::new();
    ENGINES.get_or_init(Default::default)
}

/// Makes `engine` run its runtime's models from now on.
pub fn register_engine(engine: std::sync::Arc<dyn LocalEngine>) {
    if let Ok(mut list) = engines().write() {
        list.retain(|e| e.runtime() != engine.runtime());
        list.push(engine);
    }
}

fn engine_for(runtime: Runtime) -> Option<std::sync::Arc<dyn LocalEngine>> {
    engines()
        .read()
        .ok()?
        .iter()
        .find(|e| e.runtime() == runtime)
        .cloned()
}

/// Whether this build can run `runtime`'s models (an engine is registered).
pub fn runtime_available(runtime: Runtime) -> bool {
    engine_for(runtime).is_some()
}

/// One file of a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFile {
    /// Its path in the repo, and under the model's folder.
    pub name: String,
    /// A full URL; `None` means the repo's `resolve/<revision>/<name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Bytes.
    pub size: u64,
    /// Lowercase hex. `None` skips the check (only for user entries).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// A model Tern can download.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub task: Task,
    /// Display name.
    pub name: String,
    /// `owner/repo` on Hugging Face.
    pub hf_repo: String,
    /// A commit or branch; pinned commits keep hashes stable.
    #[serde(default = "main_branch")]
    pub revision: String,
    /// SPDX id where there is one (`MIT`, `Apache-2.0`).
    pub licence: String,
    pub runtime: Runtime,
    /// A brand key for the row's logo; `None` uses the task's icon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    /// A stand-in until the model research picks the real model.
    #[serde(default)]
    pub placeholder: bool,
    /// One line about it.
    #[serde(default)]
    pub note: String,
    pub files: Vec<ModelFile>,
}

fn main_branch() -> String {
    "main".into()
}

impl ModelEntry {
    /// Every file's bytes together.
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Where `file` comes from, given the Hugging Face endpoint.
    pub fn file_url(&self, file: &ModelFile, endpoint: &str) -> String {
        file.url.clone().unwrap_or_else(|| {
            format!(
                "{}/{}/resolve/{}/{}",
                endpoint.trim_end_matches('/'),
                self.hf_repo,
                self.revision,
                file.name
            )
        })
    }

    /// The model's page on Hugging Face.
    pub fn page(&self) -> String {
        format!("{HF_ENDPOINT}/{}", self.hf_repo)
    }
}

/// The models Tern knows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub models: Vec<ModelEntry>,
}

impl Manifest {
    /// The manifest compiled into Tern.
    pub fn builtin() -> Self {
        Self::parse(BUILTIN).expect("the built-in local model manifest is valid")
    }

    /// Reads and checks a manifest: unique ids, safe file paths, a file
    /// per model, and hex hashes.
    pub fn parse(json: &str) -> Result<Self, String> {
        let m: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut ids = BTreeSet::new();
        for e in &m.models {
            if !safe_id(&e.id) {
                return Err(format!("`{}` is not a usable model id", e.id));
            }
            if !ids.insert(e.id.as_str()) {
                return Err(format!("`{}` is listed twice", e.id));
            }
            if e.files.is_empty() {
                return Err(format!("`{}` lists no files", e.id));
            }
            for f in &e.files {
                if !safe_path(&f.name) || f.name == RECORD {
                    return Err(format!("`{}` has an unsafe file path `{}`", e.id, f.name));
                }
                if let Some(h) = &f.sha256
                    && (h.len() != 64 || !h.bytes().all(|b| b.is_ascii_hexdigit()))
                {
                    return Err(format!("`{}`: `{}` has a malformed sha256", e.id, f.name));
                }
            }
        }
        Ok(m)
    }

    /// `other`'s entries replace ones with the same id; new ones follow.
    pub fn merged(mut self, other: Manifest) -> Self {
        for e in other.models {
            match self.models.iter_mut().find(|m| m.id == e.id) {
                Some(slot) => *slot = e,
                None => self.models.push(e),
            }
        }
        self
    }

    /// The built-in manifest with the user's `local-models.json` from
    /// `config_dir` merged in. A broken file is reported, not fatal.
    pub fn load(config_dir: &Path) -> (Self, Option<String>) {
        let base = Self::builtin();
        let path = config_dir.join(OVERRIDES);
        match std::fs::read_to_string(&path) {
            Err(_) => (base, None),
            Ok(s) => match Self::parse(&s) {
                Ok(o) => (base.merged(o), None),
                Err(e) => (base, Some(format!("{} was ignored: {e}", path.display()))),
            },
        }
    }

    pub fn find(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn for_task(&self, task: Task) -> impl Iterator<Item = &ModelEntry> {
        self.models.iter().filter(move |m| m.task == task)
    }
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !id.starts_with('.')
}

/// A relative path with plain segments: no `..`, no root, no backslashes.
fn safe_path(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains(':')
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

/// The user's choices: models switched off and the default per task.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalModelPrefs {
    /// Installed models the user switched off, by id. Installed models are
    /// on unless listed here.
    pub disabled: Vec<String>,
    /// The default model per task, by task key (`ocr`) to model id.
    pub defaults: BTreeMap<String, String>,
}

impl LocalModelPrefs {
    pub fn is_enabled(&self, id: &str) -> bool {
        !self.disabled.iter().any(|d| d == id)
    }

    pub fn set_enabled(&mut self, id: &str, on: bool) {
        self.disabled.retain(|d| d != id);
        if !on {
            self.disabled.push(id.to_owned());
        }
    }

    pub fn default_for(&self, task: Task) -> Option<&str> {
        self.defaults.get(task.key()).map(String::as_str)
    }

    /// Sets or clears the task's default.
    pub fn set_default(&mut self, task: Task, id: Option<&str>) {
        match id {
            Some(id) => {
                self.defaults.insert(task.key().into(), id.into());
            }
            None => {
                self.defaults.remove(task.key());
            }
        }
    }

    /// Drops every mention of a deleted model.
    pub fn forget(&mut self, id: &str) {
        self.disabled.retain(|d| d != id);
        self.defaults.retain(|_, v| v != id);
    }
}

/// A verified file on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledFile {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// What `tern-model.json` holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub id: String,
    pub files: Vec<InstalledFile>,
    pub installed_at: String,
}

/// Where a model stands on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    NotInstalled,
    /// An interrupted download left this many bytes; Download resumes.
    Partial(u64),
    Installed,
}

/// A manifest entry with its state and the user's choices, for lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelStatus {
    pub entry: ModelEntry,
    pub state: State,
    pub enabled: bool,
    pub is_default: bool,
}

impl ModelStatus {
    pub fn installed(&self) -> bool {
        self.state == State::Installed
    }
}

/// What a download reports as it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadEvent {
    /// Bytes on disk across all of the model's files.
    Progress { done: u64, total: u64 },
    /// Checking a file's hash.
    Verifying(String),
}

/// Why a local model action failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelError {
    Unknown(String),
    Cancelled,
    Download(String),
    Checksum(String),
    Io(String),
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(id) => write!(f, "no local model is called `{id}`"),
            Self::Cancelled => write!(f, "the download was cancelled"),
            Self::Download(e) => write!(f, "the download failed: {e}"),
            Self::Checksum(file) => write!(
                f,
                "{file} did not match its checksum, so it was deleted; try the download again"
            ),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ModelError {}

impl From<std::io::Error> for ModelError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// The models folder and the manifest.
#[derive(Clone, Debug)]
pub struct LocalModels {
    root: PathBuf,
    manifest: Manifest,
    endpoint: String,
}

impl LocalModels {
    /// Models under `root` (normally `<data>/models`).
    pub fn new(root: impl Into<PathBuf>, manifest: Manifest) -> Self {
        let endpoint = std::env::var("TERN_HF_ENDPOINT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| HF_ENDPOINT.to_owned());
        Self {
            root: root.into(),
            manifest,
            endpoint,
        }
    }

    /// `<data>/models` with the built-in manifest and the user's additions.
    /// The second value is a warning about a broken `local-models.json`.
    pub fn discover() -> Result<(Self, Option<String>), String> {
        let paths = tern_platform::AppPaths::discover().map_err(|e| e.to_string())?;
        let (manifest, warn) = Manifest::load(&paths.config_dir);
        Ok((Self::new(paths.data_dir.join("models"), manifest), warn))
    }

    /// Downloads from `endpoint` instead of Hugging Face (tests, mirrors).
    pub fn with_endpoint(mut self, endpoint: &str) -> Self {
        self.endpoint = endpoint.to_owned();
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// `<root>/<id>`.
    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// The model's record when every file it lists is on disk at its size.
    pub fn installed(&self, id: &str) -> Option<Installed> {
        let dir = self.dir(id);
        let rec: Installed =
            serde_json::from_str(&std::fs::read_to_string(dir.join(RECORD)).ok()?).ok()?;
        rec.files
            .iter()
            .all(|f| std::fs::metadata(dir.join(&f.name)).is_ok_and(|m| m.len() == f.size))
            .then_some(rec)
    }

    pub fn state(&self, entry: &ModelEntry) -> State {
        if self.installed(&entry.id).is_some() {
            return State::Installed;
        }
        let dir = self.dir(&entry.id);
        let bytes: u64 = entry
            .files
            .iter()
            .map(|f| {
                let dest = dir.join(&f.name);
                std::fs::metadata(&dest)
                    .or_else(|_| std::fs::metadata(tern_providers::download::part_path(&dest)))
                    .map(|m| m.len())
                    .unwrap_or(0)
            })
            .sum();
        if bytes > 0 {
            State::Partial(bytes)
        } else {
            State::NotInstalled
        }
    }

    /// Every manifest entry with its state, in manifest order.
    pub fn statuses(&self, prefs: &LocalModelPrefs) -> Vec<ModelStatus> {
        self.manifest
            .models
            .iter()
            .map(|e| ModelStatus {
                state: self.state(e),
                enabled: prefs.is_enabled(&e.id),
                is_default: prefs.default_for(e.task) == Some(e.id.as_str()),
                entry: e.clone(),
            })
            .collect()
    }

    /// The model to use for `task`: the default when it is installed and
    /// on, else the first installed model that is on.
    pub fn usable(&self, prefs: &LocalModelPrefs, task: Task) -> Option<&ModelEntry> {
        let ok = |e: &&ModelEntry| {
            e.task == task && prefs.is_enabled(&e.id) && self.installed(&e.id).is_some()
        };
        prefs
            .default_for(task)
            .and_then(|id| self.manifest.find(id))
            .filter(ok)
            .or_else(|| self.manifest.models.iter().find(ok))
    }

    /// Downloads every file of `id` into its folder, resuming partial
    /// files, checks the hashes, and writes the record. An installed model
    /// returns its record at once.
    pub async fn download(
        &self,
        id: &str,
        cancel: &CancelDownload,
        on_event: &mut (dyn FnMut(DownloadEvent) + Send),
    ) -> Result<Installed, ModelError> {
        let entry = self
            .manifest
            .find(id)
            .ok_or_else(|| ModelError::Unknown(id.to_owned()))?
            .clone();
        if let Some(rec) = self.installed(id) {
            return Ok(rec);
        }
        let dir = self.dir(id);
        std::fs::create_dir_all(&dir)?;
        let total = entry.size();
        let client = tern_providers::download::client_for(&self.endpoint)
            .map_err(|e| ModelError::Download(e.to_string()))?;
        let mut before = 0u64;
        let mut files = Vec::new();
        for f in &entry.files {
            let dest = dir.join(&f.name);
            let whole = std::fs::metadata(&dest).is_ok_and(|m| m.len() == f.size);
            if !whole {
                let url = entry.file_url(f, &self.endpoint);
                let offset = before;
                let mut progress = |p: Progress| {
                    on_event(DownloadEvent::Progress {
                        done: offset + p.done,
                        total,
                    })
                };
                tern_providers::download::fetch(
                    &client,
                    &url,
                    &dest,
                    Some(f.size),
                    cancel,
                    &mut progress,
                )
                .await
                .map_err(|e| match e {
                    tern_providers::download::DownloadError::Cancelled => ModelError::Cancelled,
                    e => ModelError::Download(format!("{}: {e}", f.name)),
                })?;
            }
            before += f.size;
            on_event(DownloadEvent::Progress {
                done: before,
                total,
            });
            on_event(DownloadEvent::Verifying(f.name.clone()));
            let got = sha256_file(&dest)?;
            if let Some(want) = &f.sha256
                && !want.eq_ignore_ascii_case(&got)
            {
                let _ = std::fs::remove_file(&dest);
                return Err(ModelError::Checksum(f.name.clone()));
            }
            files.push(InstalledFile {
                name: f.name.clone(),
                size: f.size,
                sha256: got,
            });
        }
        let rec = Installed {
            id: id.to_owned(),
            files,
            installed_at: crate::checkpoint::now_rfc3339(),
        };
        let json = serde_json::to_string_pretty(&rec).map_err(|e| ModelError::Io(e.to_string()))?;
        std::fs::write(dir.join(RECORD), json)?;
        Ok(rec)
    }

    /// Reads the text in a picture with installed model `id`.
    pub fn ocr(&self, id: &str, image: &[u8]) -> Result<String, String> {
        let entry = self
            .manifest
            .find(id)
            .ok_or_else(|| ModelError::Unknown(id.to_owned()).to_string())?;
        if self.installed(id).is_none() {
            return Err(format!("{} is not downloaded", entry.name));
        }
        let engine = engine_for(entry.runtime)
            .ok_or_else(|| format!("this build cannot run {} models yet", entry.runtime.label()))?;
        engine.ocr(&self.dir(id), entry, image)
    }

    /// Deletes the model's folder, finished or partial. The caller drops it
    /// from preferences with [`LocalModelPrefs::forget`].
    pub fn remove(&self, id: &str) -> Result<(), ModelError> {
        if self.manifest.find(id).is_none() && !self.dir(id).exists() {
            return Err(ModelError::Unknown(id.to_owned()));
        }
        // Only ids that are safe folder names reach the filesystem.
        if !safe_id(id) {
            return Err(ModelError::Unknown(id.to_owned()));
        }
        match std::fs::remove_dir_all(self.dir(id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

/// A file's SHA-256 as lowercase hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// `148 MB`, `4.7 MB`, `586 KB`.
pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e8 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests;
