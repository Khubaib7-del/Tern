//! The Study harness for front ends: materials on disk, the model calls,
//! and the saved history.
//!
//! The pure work (chunks and anchors, topics, mastery, prompts, parsing,
//! grading, fallbacks) lives in `tern-study`. This module gives it what
//! only the engine has: files read through the attachment reader (PDFs
//! with `[page N]` markers), vault notes, research reports, pages read
//! through Tern Crawl, the user's provider, and the data directory:
//!
//! ```text
//! <data>/study/library.json        materials, topics, mastery, history
//! <data>/study/materials/<id>.json each material's chunks
//! <data>/study/quizzes/<id>.json   a quiz, its key, and every attempt
//! <data>/study/vivas/<id>.json|md  a viva transcript and its rubric report
//! <data>/study/cards.json          the flashcard deck (see `crate::cards`)
//! <data>/study/explain/<id>.json   saved Explain in 3D scenes (`crate::explain`)
//! ```
//!
//! Every model step falls back to `tern-study`'s deterministic version
//! when no model is given or its call fails, so Study works offline.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tern_platform::AppPaths;
use tern_providers::{ChatMessage, ChatRequest};

pub use tern_study::grade::{blank_matches, grade_exact, grade_lexical};
pub use tern_study::library::{LIBRARY_VERSION, select_sources};
pub use tern_study::mastery::ago;
pub use tern_study::quiz::{Answered, letter};
pub use tern_study::text::clip;
pub use tern_study::tutor::{BEYOND, TutorTurn, opening as tutor_opening};
pub use tern_study::{
    Anchor, Chunk, Difficulty, Grade, GradeMethod, Level, Mark, Mastery, Material, MaterialKind,
    MaterialText, Question, QuestionKind, Quiz, QuizAttempt, QuizRecord, QuizSpec, QuizSummary,
    Rubric, Source, StudyLibrary, Topic, TutorMode, Viva, VivaReport, VivaSummary, VivaTurn,
};

use crate::engine::Model;
use crate::providers::{self, SecretStore};

/// Streamed reply text; return `false` to stop.
pub type StreamText<'a> = &'a (dyn Fn(&str) -> bool + Send + Sync);

/// Characters of passages sent with one tutor turn.
pub const TUTOR_CHARS: usize = 12_000;
/// Characters of passages a quiz is written from.
pub const QUIZ_CHARS: usize = 14_000;
/// Characters of passages the viva examiner sees.
pub const VIVA_CHARS: usize = 12_000;

/// Unix seconds now.
pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// The study folder in the data directory.
pub fn study_dir(paths: &AppPaths) -> PathBuf {
    paths.data_dir.join("study")
}

/// Where the study library and its records are kept.
#[derive(Clone, Debug)]
pub struct StudyStore {
    root: PathBuf,
}

pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Tern could not make {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)
        .map_err(|e| format!("Tern could not write {}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("{} could not be read: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Tern could not read {}: {e}", path.display())),
    }
}

impl StudyStore {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The app's study folder.
    pub fn discover() -> Result<Self, String> {
        let paths = AppPaths::discover().map_err(|e| e.to_string())?;
        Ok(Self::at(study_dir(&paths)))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn library_path(&self) -> PathBuf {
        self.root.join("library.json")
    }

    /// The library; a missing file is an empty library.
    pub fn load(&self) -> Result<StudyLibrary, String> {
        let lib: StudyLibrary = read_json(&self.library_path())?.unwrap_or_default();
        if lib.version > LIBRARY_VERSION {
            return Err("The study library was saved by a newer Tern.".into());
        }
        Ok(lib)
    }

    pub fn save(&self, lib: &StudyLibrary) -> Result<(), String> {
        let mut l = lib.clone();
        l.version = LIBRARY_VERSION;
        write_atomic(
            &self.library_path(),
            &serde_json::to_string_pretty(&l).unwrap_or_default(),
        )
    }

    fn text_path(&self, id: &str) -> PathBuf {
        self.root.join("materials").join(format!("{id}.json"))
    }

    pub fn save_text(&self, t: &MaterialText) -> Result<(), String> {
        write_atomic(
            &self.text_path(&t.material),
            &serde_json::to_string(t).unwrap_or_default(),
        )
    }

    pub fn load_text(&self, id: &str) -> Result<MaterialText, String> {
        Ok(
            read_json(&self.text_path(id))?.unwrap_or_else(|| MaterialText {
                material: id.to_owned(),
                chunks: Vec::new(),
            }),
        )
    }

    /// Every chunk of every material, in library order.
    pub fn chunks(&self, lib: &StudyLibrary) -> Vec<Chunk> {
        lib.materials
            .iter()
            .filter_map(|m| self.load_text(&m.id).ok())
            .flat_map(|t| t.chunks)
            .collect()
    }

    fn quiz_path(&self, id: &str) -> PathBuf {
        self.root.join("quizzes").join(format!("{id}.json"))
    }

    pub fn save_quiz(&self, rec: &QuizRecord) -> Result<PathBuf, String> {
        let p = self.quiz_path(&rec.quiz.id);
        write_atomic(&p, &serde_json::to_string_pretty(rec).unwrap_or_default())?;
        Ok(p)
    }

    pub fn load_quiz(&self, id: &str) -> Result<QuizRecord, String> {
        read_json(&self.quiz_path(id))?.ok_or_else(|| format!("No quiz {id} in the history."))
    }

    fn viva_path(&self, id: &str, ext: &str) -> PathBuf {
        self.root.join("vivas").join(format!("{id}.{ext}"))
    }

    /// Saves a viva as JSON, and its report as Markdown when it has one.
    /// Returns the Markdown path.
    pub fn save_viva(&self, viva: &Viva) -> Result<Option<PathBuf>, String> {
        write_atomic(
            &self.viva_path(&viva.id, "json"),
            &serde_json::to_string_pretty(viva).unwrap_or_default(),
        )?;
        match &viva.report {
            Some(r) => {
                let p = self.viva_path(&viva.id, "md");
                write_atomic(&p, &r.to_markdown(viva))?;
                Ok(Some(p))
            }
            None => Ok(None),
        }
    }

    pub fn load_viva(&self, id: &str) -> Result<Viva, String> {
        read_json(&self.viva_path(id, "json"))?
            .ok_or_else(|| format!("No viva {id} in the history."))
    }

    /// A fresh id for a quiz (`q`) or viva (`v`): the time, plus a suffix
    /// when two land in the same second.
    pub fn new_id(&self, prefix: &str, at: i64) -> String {
        let dir = if prefix == "v" { "vivas" } else { "quizzes" };
        let base = format!("{prefix}{at}");
        let mut id = base.clone();
        let mut n = 2;
        while self.root.join(dir).join(format!("{id}.json")).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }
}

// ---- Materials -------------------------------------------------------

/// Adds (or refreshes) a material from text. A material with the same
/// origin keeps its id, so its mastery carries over.
pub fn add_text(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    name: &str,
    kind: MaterialKind,
    origin: &str,
    text: &str,
) -> Result<Material, String> {
    let id = match lib.material_from(origin) {
        Some(m) => m.id.clone(),
        None => lib.new_material_id(),
    };
    let (m, t) = tern_study::material::build(&id, name, kind, origin, text, now());
    if t.chunks.is_empty() {
        return Err(format!("{name} has no text Tern can study from."));
    }
    store.save_text(&t)?;
    lib.add_material(m.clone());
    Ok(m)
}

/// Adds a file, folder, or `.zip` (each file in it becomes a material).
/// PDFs are read page by page, so anchors carry page numbers.
pub fn add_path(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    path: &Path,
) -> Result<Vec<Material>, String> {
    let a = crate::attach::Attachment::load(path)?;
    if a.is_image() {
        return Err(format!(
            "{} is a picture; Study reads text, PDFs, and notes.",
            a.name
        ));
    }
    let single = a.files.len() == 1;
    let mut out = Vec::new();
    for f in &a.files {
        let name = if single {
            a.name.clone()
        } else {
            format!("{}/{}", a.name, f.path)
        };
        let origin = if single {
            path.display().to_string()
        } else {
            format!("{}/{}", path.display(), f.path)
        };
        let kind = MaterialKind::for_name(&f.path);
        match add_text(store, lib, &name, kind, &origin, &f.text) {
            Ok(m) => out.push(m),
            Err(e) if single => return Err(e),
            Err(_) => {}
        }
    }
    if out.is_empty() {
        return Err(format!("{} has no text Tern can study from.", a.name));
    }
    Ok(out)
}

/// Adds a vault note.
pub fn add_note(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    vault: &crate::vault::Vault,
    note: &str,
) -> Result<Material, String> {
    let found = vault
        .find(note)
        .ok_or_else(|| format!("No note \"{note}\" in the vault."))?;
    let text = vault.read(&found.path)?;
    add_text(
        store,
        lib,
        found.name(),
        MaterialKind::Vault,
        &format!("vault:{}", found.path),
        &text,
    )
}

/// Adds a chat's latest research report (`last`, an id, or the start of one).
pub fn add_report(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    chat: &str,
) -> Result<Material, String> {
    let session = crate::report::find_chat(chat)?;
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let dir = crate::report::runs_dir(&paths, session);
    let latest = std::fs::read_dir(&dir)
        .map_err(|_| {
            format!("Chat {session} has no run report yet. `tern report {session}` writes one.")
        })?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .filter_map(|p| {
            let n: u32 = p
                .file_stem()?
                .to_str()?
                .strip_prefix("turn-")?
                .parse()
                .ok()?;
            Some((n, p))
        })
        .max_by_key(|(n, _)| *n)
        .ok_or_else(|| format!("Chat {session} has no run report yet."))?;
    let text = std::fs::read_to_string(&latest.1).map_err(|e| e.to_string())?;
    let title = text
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|t| tern_study::text::clip(t, 60))
        .unwrap_or_else(|| format!("Report {session}"));
    add_text(
        store,
        lib,
        &title,
        MaterialKind::Report,
        &latest.1.display().to_string(),
        &text,
    )
}

/// Adds a web page or online PDF, read through Tern Crawl.
pub async fn add_link(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    url: &str,
) -> Result<Material, String> {
    let page = crate::web::shared_reader()
        .fetch_for_user(url)
        .await
        .map_err(|e| format!("Tern could not read {url}: {e}"))?;
    let kind = if page.content_type.contains("pdf") {
        MaterialKind::Pdf
    } else {
        MaterialKind::Link
    };
    let text = if page.markdown.trim().is_empty() {
        &page.text
    } else {
        &page.markdown
    };
    let name = if page.title.trim().is_empty() {
        page.site.clone()
    } else {
        page.title.clone()
    };
    add_text(store, lib, &name, kind, &page.final_url, text)
}

/// Removes a material and its text. Topics are marked stale.
pub fn remove(store: &StudyStore, lib: &mut StudyLibrary, which: &str) -> Result<Material, String> {
    let m = lib.remove_material(which)?;
    let _ = std::fs::remove_file(store.text_path(&m.id));
    Ok(m)
}

// ---- The model -------------------------------------------------------

/// The user's model for study calls: plain completions, no tools.
#[derive(Clone)]
pub struct StudyEngine {
    model: Arc<dyn Model>,
    model_name: String,
    provider_name: String,
}

impl std::fmt::Debug for StudyEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "StudyEngine({} on {})",
            self.model_name, self.provider_name
        )
    }
}

impl StudyEngine {
    /// Connects with the chat's provider and model choices.
    pub async fn open(opts: crate::ChatOptions) -> Result<Self, String> {
        let keychain = providers::keychain();
        let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
        let (provider, model, _) = crate::chat::connect_model(&opts, secrets).await?;
        let provider_name = provider.spec().name.to_owned();
        Ok(Self {
            model: Arc::new(provider),
            model_name: model,
            provider_name,
        })
    }

    /// An engine over any model, for tests.
    pub fn with_model(model: Arc<dyn Model>, model_name: &str, provider_name: &str) -> Self {
        Self {
            model,
            model_name: model_name.to_owned(),
            provider_name: provider_name.to_owned(),
        }
    }

    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }

    /// One completion. `on_text` streams the reply when given.
    pub async fn ask(
        &self,
        system: String,
        messages: Vec<ChatMessage>,
        max_tokens: u32,
        temperature: f32,
        on_text: Option<StreamText<'_>>,
    ) -> Result<String, String> {
        let mut req = ChatRequest::new(&self.model_name, messages);
        req.system = Some(system);
        req.max_tokens = max_tokens;
        req.temperature = Some(temperature);
        let resp = match on_text {
            Some(f) => self.model.complete_stream(&req, f).await,
            None => self.model.complete(&req).await,
        }
        .map_err(|e| e.to_string())?;
        Ok(resp.text)
    }
}

// ---- Topics ----------------------------------------------------------

/// Rebuilds the topics from every material. Returns who wrote them
/// (`model` or `fallback`) and, when the model was tried and failed, why.
pub async fn build_topics(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    engine: Option<&StudyEngine>,
) -> Result<(String, Option<String>), String> {
    use tern_study::topics::{MAX_TOPICS, fallback, parse_topics, topics_prompt};
    let chunks = store.chunks(lib);
    let mut note = None;
    let mut topics = None;
    if let Some(e) = engine.filter(|_| !chunks.is_empty()) {
        let (system, user) = topics_prompt(&chunks, MAX_TOPICS);
        match e
            .ask(system, vec![ChatMessage::user(user)], 3000, 0.2, None)
            .await
        {
            Ok(text) => match parse_topics(&text, &chunks, MAX_TOPICS) {
                Some(t) => topics = Some(t),
                None => {
                    note = Some(
                        "The model's topics could not be read; Tern used the headings instead."
                            .into(),
                    )
                }
            },
            Err(err) => {
                note = Some(format!(
                    "The model could not be reached ({err}); Tern used the headings instead."
                ))
            }
        }
    }
    let by = if topics.is_some() {
        "model"
    } else {
        "fallback"
    };
    let topics = topics.unwrap_or_else(|| fallback(&chunks, MAX_TOPICS));
    lib.set_topics(topics, by);
    store.save(lib)?;
    Ok((by.to_owned(), note))
}

/// Builds topics when there are none or the materials changed, without a
/// model (cheap, for listing).
pub fn ensure_topics(store: &StudyStore, lib: &mut StudyLibrary) -> Result<(), String> {
    if lib.topics.is_empty() || lib.topics_stale {
        let chunks = store.chunks(lib);
        let t = tern_study::topics::fallback(&chunks, tern_study::topics::MAX_TOPICS);
        lib.set_topics(t, "fallback");
        store.save(lib)?;
    }
    Ok(())
}

fn find_topic(lib: &StudyLibrary, which: &str) -> Result<Topic, String> {
    lib.topic(which)
        .cloned()
        .ok_or_else(|| format!("No topic matches \"{which}\". `tern study topics` lists them."))
}

// ---- Tutor -----------------------------------------------------------

/// A tutor reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TutorReply {
    pub text: String,
    /// The passages it cited: `[S1] notes.pdf p. 4`.
    pub citations: Vec<String>,
    /// It flagged something beyond the materials.
    pub beyond: bool,
    /// No model answered (none given, or its call failed: see `note`).
    pub offline: bool,
    pub note: Option<String>,
}

/// One tutor turn on `topic`. `history` holds the conversation so far,
/// ending with the user's latest message; an empty history starts the
/// session with the mode's opening.
#[allow(clippy::too_many_arguments)]
pub async fn tutor_reply(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    engine: Option<&StudyEngine>,
    topic: &str,
    mode: TutorMode,
    history: &[TutorTurn],
    on_text: Option<StreamText<'_>>,
) -> Result<TutorReply, String> {
    use tern_study::tutor::{citations, fallback_reply, flags_beyond, opening, tutor_system};
    let topic = find_topic(lib, topic)?;
    let chunks = store.chunks(lib);
    let last = history
        .iter()
        .rev()
        .find(|t| t.is_user())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    let sources = select_sources(
        &chunks,
        Some(&topic),
        &format!("{} {last}", topic.title),
        TUTOR_CHARS,
    );
    let mut history = history.to_vec();
    if history.is_empty() {
        history.push(TutorTurn::user(opening(mode, &topic)));
    }
    let mut reply = TutorReply::default();
    if let Some(e) = engine {
        let system = tutor_system(mode, &topic, &lib.mastery_of(&topic.id), &sources);
        let messages: Vec<ChatMessage> = history
            .iter()
            .map(|t| {
                if t.is_user() {
                    ChatMessage::user(&t.text)
                } else {
                    ChatMessage::assistant(&t.text)
                }
            })
            .collect();
        match e.ask(system, messages, 1400, 0.4, on_text).await {
            Ok(text) if !text.trim().is_empty() => reply.text = text,
            Ok(_) => reply.note = Some("The model sent an empty reply.".into()),
            Err(err) => reply.note = Some(format!("The model could not be reached: {err}")),
        }
    }
    if reply.text.is_empty() {
        reply.text = fallback_reply(mode, &topic, &sources, &history);
        reply.offline = true;
        if let Some(f) = on_text {
            let _ = f(&reply.text);
        }
    }
    reply.citations = citations(&reply.text, &sources);
    reply.beyond = flags_beyond(&reply.text);
    lib.touch(&topic.id, now());
    store.save(lib)?;
    Ok(reply)
}

// ---- Quizzes ---------------------------------------------------------

/// Writes a quiz and saves it in the history (with no attempts yet).
pub async fn make_quiz(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    engine: Option<&StudyEngine>,
    spec: &QuizSpec,
) -> Result<(Quiz, Option<String>), String> {
    use tern_study::quiz::{fallback_quiz, parse_quiz, quiz_prompt};
    ensure_topics(store, lib)?;
    let topic = match &spec.topic {
        Some(t) => Some(find_topic(lib, t)?),
        None => None,
    };
    let title = topic
        .as_ref()
        .map(|t| t.title.clone())
        .unwrap_or_else(|| "All materials".into());
    let chunks = store.chunks(lib);
    if chunks.is_empty() {
        return Err("Add materials first: `tern study add <files>`.".into());
    }
    let sources = select_sources(&chunks, topic.as_ref(), &title, QUIZ_CHARS);
    let tid = topic.as_ref().map(|t| t.id.as_str());
    let mut note = None;
    let mut questions = Vec::new();
    let mut by = "fallback";
    if let Some(e) = engine {
        let (system, user) = quiz_prompt(spec, &title, &sources);
        let max = (spec.count() as u32 * 220).clamp(800, 6000);
        match e
            .ask(system, vec![ChatMessage::user(user)], max, 0.5, None)
            .await
        {
            Ok(text) => {
                questions = parse_quiz(&text, &sources, tid);
                if questions.is_empty() {
                    note = Some("The model's quiz could not be read; Tern wrote one from your materials instead.".into());
                } else {
                    by = "model";
                }
            }
            Err(err) => {
                note = Some(format!(
                    "The model could not be reached ({err}); Tern wrote a quiz from your materials instead."
                ))
            }
        }
    }
    if questions.is_empty() {
        questions = fallback_quiz(spec, &sources, tid);
    }
    if questions.is_empty() {
        return Err("Your materials are too short to write a quiz from yet.".into());
    }
    questions.truncate(spec.count());
    for (i, q) in questions.iter_mut().enumerate() {
        q.n = i + 1;
        // Whole-material quizzes: each question goes to the topic its
        // passage is in, so mastery lands on the right topic.
        if q.topic.is_none()
            && let Some(a) = &q.anchor
            && let Some(src) = sources.iter().find(|s| &s.anchor == a)
            && let Some(t) = lib.topics.iter().find(|t| t.chunks.contains(&src.chunk))
        {
            q.topic = Some(t.id.clone());
        }
    }
    let at = now();
    let quiz = Quiz {
        id: store.new_id("q", at),
        title,
        topic: tid.map(str::to_owned),
        difficulty: spec.difficulty,
        questions,
        created_at: at,
        written_by: by.into(),
    };
    store.save_quiz(&QuizRecord {
        quiz: quiz.clone(),
        attempts: Vec::new(),
    })?;
    Ok((quiz, note))
}

/// Grades one answer: exactly against the key for multiple choice,
/// true/false, and blanks; by the model judge for short answers, with the
/// word-overlap check when there is no judge or its reply cannot be read.
pub async fn grade(engine: Option<&StudyEngine>, q: &Question, given: &str) -> Grade {
    use tern_study::grade::{judge_prompt, parse_judgment};
    if let Some(g) = grade_exact(q, given) {
        return g;
    }
    if given.trim().is_empty() {
        return grade_lexical(q, given);
    }
    if let Some(e) = engine {
        let (system, user) = judge_prompt(q, given);
        if let Ok(text) = e
            .ask(system, vec![ChatMessage::user(user)], 400, 0.0, None)
            .await
            && let Some(g) = parse_judgment(&text, q)
        {
            return g;
        }
    }
    grade_lexical(q, given)
}

/// Saves a finished attempt, applies it to mastery, and adds it to the
/// history list.
pub fn finish_quiz(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    quiz: &Quiz,
    mut attempt: QuizAttempt,
) -> Result<QuizSummary, String> {
    let at = now();
    attempt.finished_at = Some(at);
    for a in &attempt.answers {
        let topic = quiz
            .questions
            .iter()
            .find(|q| q.n == a.n)
            .and_then(|q| q.topic.clone())
            .or_else(|| quiz.topic.clone());
        if let Some(t) = topic {
            lib.record(&t, a.grade.credit, quiz.difficulty, at);
        }
    }
    let mut rec = store.load_quiz(&quiz.id).unwrap_or_else(|_| QuizRecord {
        quiz: quiz.clone(),
        attempts: Vec::new(),
    });
    let summary = QuizSummary {
        id: quiz.id.clone(),
        title: quiz.title.clone(),
        topic: quiz.topic.clone(),
        taken_at: at,
        score: attempt.score(),
        total: quiz.questions.len(),
    };
    rec.attempts.push(attempt);
    store.save_quiz(&rec)?;
    lib.quizzes.push(summary.clone());
    store.save(lib)?;
    Ok(summary)
}

// ---- Viva ------------------------------------------------------------

/// Starts a viva on the materials. The title defaults to the first
/// research report among them, else the first material.
pub fn start_viva(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    title: Option<&str>,
    planned: usize,
) -> Result<Viva, String> {
    if lib.materials.is_empty() {
        return Err("Add your project report or notes first: `tern study add <files>` (or `--report last`).".into());
    }
    ensure_topics(store, lib)?;
    let title = title
        .map(str::to_owned)
        .filter(|t| !t.trim().is_empty())
        .or_else(|| {
            lib.materials
                .iter()
                .find(|m| m.kind == MaterialKind::Report)
                .map(|m| m.name.clone())
        })
        .unwrap_or_else(|| lib.materials[0].name.clone());
    let at = now();
    Ok(Viva::new(&store.new_id("v", at), &title, planned, at))
}

/// Asks the next question: pushes it onto `viva` and returns whether the
/// model wrote it (`None` note) or the templates did (with why).
pub async fn viva_question(
    store: &StudyStore,
    lib: &StudyLibrary,
    engine: Option<&StudyEngine>,
    viva: &mut Viva,
) -> Result<Option<String>, String> {
    use tern_study::viva::{
        examiner_system, fallback_question, next_step, parse_question, question_prompt,
    };
    let chunks = store.chunks(lib);
    let query = viva
        .turns
        .last()
        .map(|t| format!("{} {}", t.question, t.answer))
        .unwrap_or_else(|| viva.title.clone());
    let sources = select_sources(&chunks, None, &query, VIVA_CHARS);
    let (level, follow_up) = next_step(viva, &sources);
    let mut note = None;
    let mut question = None;
    if let Some(e) = engine {
        let system = examiner_system(&viva.title, &lib.topics, &sources);
        let user = question_prompt(viva, level, follow_up);
        match e
            .ask(system, vec![ChatMessage::user(user)], 300, 0.6, None)
            .await
        {
            Ok(text) => question = parse_question(&text),
            Err(err) => {
                note = Some(format!(
                    "The model could not be reached ({err}); the examiner is using its own questions."
                ))
            }
        }
    }
    let question = match question {
        Some(q) => q,
        None => {
            let mut q = fallback_question(viva, level, follow_up, &lib.topics);
            // A follow-up points at the passage the answer touched.
            if follow_up
                && let Some(last) = viva.turns.last()
                && let Some(s) = sources
                    .iter()
                    .map(|s| (tern_study::text::overlap(&last.question, &s.text), s))
                    .filter(|(o, _)| *o > 0.0)
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, s)| s)
            {
                q.push_str(&format!(" Your material covers this in {}.", s.label()));
            }
            if engine.is_some() && note.is_none() {
                note = Some(
                    "The model's question could not be read; the examiner used its own.".into(),
                );
            }
            q
        }
    };
    let topic = lib
        .topics
        .iter()
        .find(|t| question.to_lowercase().contains(&t.title.to_lowercase()))
        .map(|t| t.id.clone());
    viva.turns.push(VivaTurn {
        level,
        question,
        answer: String::new(),
        follow_up,
        topic,
        seconds: 0,
    });
    Ok(note)
}

/// Ends a viva: scores it, saves the transcript and the Markdown report,
/// and adds it to the history. Returns the report's path.
pub async fn finish_viva(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    engine: Option<&StudyEngine>,
    viva: &mut Viva,
) -> Result<(VivaReport, Option<PathBuf>, Option<String>), String> {
    use tern_study::viva::{fallback_report, parse_report, report_prompt};
    // An unanswered last question is dropped.
    if viva.waiting().is_some() {
        viva.turns.pop();
    }
    if viva.turns.is_empty() {
        return Err("Answer at least one question before ending the viva.".into());
    }
    let chunks = store.chunks(lib);
    let all: String = viva
        .turns
        .iter()
        .map(|t| t.question.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let sources = select_sources(&chunks, None, &all, VIVA_CHARS);
    let mut note = None;
    let mut report = None;
    if let Some(e) = engine {
        let (system, user) = report_prompt(viva, &sources);
        match e
            .ask(system, vec![ChatMessage::user(user)], 1500, 0.2, None)
            .await
        {
            Ok(text) => {
                report = parse_report(&text);
                if report.is_none() {
                    note = Some(
                        "The model's scores could not be read; this is Tern's offline estimate."
                            .into(),
                    );
                }
            }
            Err(err) => {
                note = Some(format!(
                    "The model could not be reached ({err}); this is Tern's offline estimate."
                ))
            }
        }
    }
    let report = report.unwrap_or_else(|| fallback_report(viva, &sources, &lib.topics));
    let at = now();
    viva.ended_at = Some(at);
    viva.report = Some(report.clone());
    let md = store.save_viva(viva)?;
    for t in viva.turns.iter().filter_map(|t| t.topic.clone()) {
        lib.touch(&t, at);
    }
    lib.vivas.push(VivaSummary {
        id: viva.id.clone(),
        title: viva.title.clone(),
        taken_at: at,
        overall: report.overall(),
        questions: viva.turns.len(),
    });
    store.save(lib)?;
    Ok((report, md, note))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tern_providers::{ChatResponse, ProviderResult};

    /// Replies in turn from a list (the last one repeats).
    struct Script(Mutex<Vec<String>>);

    impl Model for Script {
        fn provider_id(&self) -> &str {
            "test"
        }
        fn complete<'a>(
            &'a self,
            _req: &'a ChatRequest,
        ) -> crate::tools::BoxFuture<'a, ProviderResult<ChatResponse>> {
            Box::pin(async move {
                let mut v = self.0.lock().unwrap();
                let text = if v.len() > 1 {
                    v.remove(0)
                } else {
                    v[0].clone()
                };
                Ok(ChatResponse {
                    text,
                    usage: tern_providers::Usage::measured(10, 5),
                    finish_reason: None,
                    model: None,
                })
            })
        }
    }

    fn engine(replies: &[&str]) -> StudyEngine {
        StudyEngine::with_model(
            Arc::new(Script(Mutex::new(
                replies.iter().map(|s| s.to_string()).collect(),
            ))),
            "m",
            "Test",
        )
    }

    const NOTES: &str = "# Graphs\nA graph is a set of vertices joined by edges. Graphs model road networks and social networks.\n\n# Shortest paths\nDijkstra's algorithm is a greedy method that finds shortest paths when every edge weight is non-negative. It keeps a priority queue of tentative distances.\n\n# Spanning trees\nA minimum spanning tree is the cheapest set of edges that connects every vertex. Kruskal's algorithm adds edges in order of weight while avoiding cycles.";

    fn setup() -> (tempfile::TempDir, StudyStore, StudyLibrary) {
        let dir = tempfile::tempdir().unwrap();
        let store = StudyStore::at(dir.path().join("study"));
        let file = dir.path().join("algo.md");
        std::fs::write(&file, NOTES).unwrap();
        let mut lib = store.load().unwrap();
        let added = add_path(&store, &mut lib, &file).unwrap();
        assert_eq!(added.len(), 1);
        store.save(&lib).unwrap();
        (dir, store, lib)
    }

    #[tokio::test]
    async fn materials_topics_and_persistence_offline() {
        let (dir, store, mut lib) = setup();
        let (by, note) = build_topics(&store, &mut lib, None).await.unwrap();
        assert_eq!(by, "fallback");
        assert!(note.is_none());
        assert_eq!(lib.topics.len(), 3);
        let again = store.load().unwrap();
        assert_eq!(again.topics, lib.topics);
        assert_eq!(store.chunks(&again).len(), 3);
        // Adding the same file again keeps its id.
        let file = dir.path().join("algo.md");
        let m = add_path(&store, &mut lib, &file).unwrap();
        assert_eq!(m[0].id, "m1");
        assert_eq!(lib.materials.len(), 1);
        let gone = remove(&store, &mut lib, "1").unwrap();
        assert_eq!(gone.id, "m1");
        assert!(store.chunks(&lib).is_empty());
    }

    #[tokio::test]
    async fn model_topics_are_used_and_bad_replies_fall_back() {
        let (_d, store, mut lib) = setup();
        let e = engine(&[
            "{\"topics\": [{\"title\": \"Graph algorithms\", \"objectives\": [\"Compare Dijkstra and Kruskal\"], \"chunks\": [\"c1\", \"c2\", \"c3\"]}]}",
        ]);
        let (by, _) = build_topics(&store, &mut lib, Some(&e)).await.unwrap();
        assert_eq!(by, "model");
        assert_eq!(lib.topics.len(), 1);
        let bad = engine(&["sorry"]);
        let (by, note) = build_topics(&store, &mut lib, Some(&bad)).await.unwrap();
        assert_eq!(by, "fallback");
        assert!(note.unwrap().contains("could not be read"));
    }

    #[tokio::test]
    async fn tutor_cites_and_falls_back_offline() {
        let (_d, store, mut lib) = setup();
        build_topics(&store, &mut lib, None).await.unwrap();
        let e = engine(&[
            "Dijkstra keeps tentative distances in a priority queue [S1]. Beyond your materials: it was published in 1959.",
        ]);
        let r = tutor_reply(
            &store,
            &mut lib,
            Some(&e),
            "shortest",
            TutorMode::Explain,
            &[],
            None,
        )
        .await
        .unwrap();
        assert!(!r.offline);
        assert_eq!(r.citations, vec!["[S1] algo.md · Shortest paths"]);
        assert!(r.beyond);
        assert!(lib.mastery_of("shortest-paths").last_seen.is_some());
        let r = tutor_reply(
            &store,
            &mut lib,
            None,
            "2",
            TutorMode::Socratic,
            &[TutorTurn::user("what about priority queues?")],
            None,
        )
        .await
        .unwrap();
        assert!(r.offline);
        assert!(r.text.contains("algo.md · Shortest paths"));
        assert!(
            tutor_reply(
                &store,
                &mut lib,
                None,
                "quantum",
                TutorMode::Explain,
                &[],
                None
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn quiz_round_trip_updates_mastery_and_history() {
        let (_d, store, mut lib) = setup();
        let e = engine(&[
            "{\"questions\": [{\"type\": \"mcq\", \"question\": \"Which algorithm builds a minimum spanning tree?\", \"choices\": [\"Dijkstra\", \"Kruskal\", \"BFS\", \"DFS\"], \"answer\": \"B\", \"source\": \"S3\"}, {\"type\": \"short\", \"question\": \"When does Dijkstra work?\", \"answer\": \"When every edge weight is non-negative.\", \"source\": \"S2\"}]}",
            "{\"mark\": \"partly\", \"feedback\": \"Say non-negative weights.\"}",
        ]);
        let spec = QuizSpec {
            count: 2,
            ..QuizSpec::default()
        };
        let (quiz, note) = make_quiz(&store, &mut lib, Some(&e), &spec).await.unwrap();
        assert!(note.is_none());
        assert_eq!(quiz.written_by, "model");
        assert_eq!(quiz.questions[0].topic.as_deref(), Some("spanning-trees"));
        assert_eq!(quiz.questions[1].topic.as_deref(), Some("shortest-paths"));
        let g1 = grade(Some(&e), &quiz.questions[0], "b").await;
        assert_eq!(g1.mark, Mark::Correct);
        assert_eq!(g1.method, GradeMethod::Exact);
        let g2 = grade(Some(&e), &quiz.questions[1], "when weights are fine").await;
        assert_eq!(g2.mark, Mark::Partly);
        assert_eq!(g2.method, GradeMethod::Model);
        let attempt = QuizAttempt {
            started_at: 0,
            finished_at: None,
            answers: vec![
                Answered {
                    n: 1,
                    given: "b".into(),
                    grade: g1,
                },
                Answered {
                    n: 2,
                    given: "x".into(),
                    grade: g2,
                },
            ],
        };
        let s = finish_quiz(&store, &mut lib, &quiz, attempt).unwrap();
        assert_eq!(s.score, 1.5);
        let back = store.load().unwrap();
        assert_eq!(back.quizzes.len(), 1);
        assert_eq!(back.mastery_of("spanning-trees").leitner, 2);
        assert_eq!(back.mastery_of("shortest-paths").attempts, 1);
        assert_eq!(store.load_quiz(&quiz.id).unwrap().attempts.len(), 1);
    }

    #[tokio::test]
    async fn offline_quiz_is_deterministic() {
        let (_d, store, mut lib) = setup();
        let spec = QuizSpec {
            topic: Some("spanning".into()),
            count: 3,
            ..QuizSpec::default()
        };
        let (a, _) = make_quiz(&store, &mut lib, None, &spec).await.unwrap();
        let (b, _) = make_quiz(&store, &mut lib, None, &spec).await.unwrap();
        assert_eq!(a.questions, b.questions);
        assert_ne!(a.id, b.id);
        assert_eq!(a.written_by, "fallback");
        assert!(
            a.questions
                .iter()
                .all(|q| q.topic.as_deref() == Some("spanning-trees"))
        );
    }

    #[tokio::test]
    async fn viva_runs_offline_and_saves_a_report() {
        let (_d, store, mut lib) = setup();
        let mut v = start_viva(&store, &mut lib, Some("Graph algorithms"), 3).unwrap();
        viva_question(&store, &lib, None, &mut v).await.unwrap();
        assert_eq!(v.turns[0].level, 1);
        v.answer("Graphs.", 4);
        viva_question(&store, &lib, None, &mut v).await.unwrap();
        assert!(v.turns[1].follow_up);
        assert!(v.turns[1].question.contains("You said"));
        v.answer("Dijkstra's algorithm finds shortest paths with a priority queue of tentative distances when every edge weight is non-negative.", 40);
        viva_question(&store, &lib, None, &mut v).await.unwrap();
        assert_eq!(v.turns[2].level, 2);
        // The open question is dropped when the viva ends.
        let (report, md, _) = finish_viva(&store, &mut lib, None, &mut v).await.unwrap();
        assert_eq!(v.turns.len(), 2);
        assert_eq!(report.method, "offline estimate");
        let md = md.unwrap();
        assert!(
            std::fs::read_to_string(md)
                .unwrap()
                .contains("# Viva practice: Graph algorithms")
        );
        assert_eq!(store.load().unwrap().vivas.len(), 1);
        assert!(store.load_viva(&v.id).unwrap().report.is_some());
    }

    #[tokio::test]
    async fn viva_uses_model_questions_and_scores() {
        let (_d, store, mut lib) = setup();
        let e = engine(&[
            "Q1: What problem does Dijkstra's algorithm solve?",
            "{\"clarity\": 4, \"correctness\": 4, \"depth\": 3, \"sources\": 2, \"strengths\": [\"Clear\"], \"weaknesses\": [\"No page refs\"], \"suggested_topics\": [\"Spanning trees\"]}",
        ]);
        let mut v = start_viva(&store, &mut lib, None, 2).unwrap();
        assert_eq!(v.title, "algo.md");
        viva_question(&store, &lib, Some(&e), &mut v).await.unwrap();
        assert_eq!(
            v.turns[0].question,
            "What problem does Dijkstra's algorithm solve?"
        );
        v.answer(
            "Shortest paths from one source when weights are non-negative.",
            20,
        );
        let (r, _, note) = finish_viva(&store, &mut lib, Some(&e), &mut v)
            .await
            .unwrap();
        assert!(note.is_none());
        assert_eq!(r.overall(), 65);
        assert_eq!(r.suggested_topics, vec!["Spanning trees"]);
    }
}
