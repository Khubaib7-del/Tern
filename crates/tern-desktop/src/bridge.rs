//! Runs chat turns on a tokio runtime and reports back over one channel.
//!
//! The window never calls a provider or the store itself: it hands a
//! [`ChatSession`] to the bridge, which gives it back with the outcome.

use tern_agent::cards::{self as cards, CardSpec, Deck, Format, Rating};
use tern_agent::deep::{DeepControl, PlanAnswer, PlanApprover};
use tern_agent::explain::{self as explain, Scene};
use tern_agent::providers::{self as setup, ModelInfo, SecretStore};
use tern_agent::search;
use tern_agent::study::{
    self as st, Grade, Question, Quiz, QuizAttempt, QuizSummary, StudyEngine, StudyLibrary,
    StudyStore, TutorMode, TutorReply, TutorTurn, Viva, VivaReport,
};
use tern_agent::tools::BoxFuture;
use tern_agent::{
    ApprovalAnswer, ApprovalRequest, Approver, CancelFlag, ChatSession, ContextUse, PastTurn,
    Preferences, ResearchPlan, SessionSummary, TurnOutcome,
};
use tern_core::SecretString;
use tern_core::SessionId;
use tokio::sync::{mpsc, oneshot};

use crate::providers::{Kind, ServiceRow, rows_from};

mod cont;
mod ext;
mod jobs;
pub use cont::{ContMsg, LibraryChange};
pub use ext::{ExtMsg, McpChange, ProjectChange, SkillChange};
pub use jobs::SetupMsg;

/// Everything the background side tells the window.
pub enum Msg {
    Connected(Box<ChatSession>, Vec<SessionSummary>),
    /// A past chat reopened, with its turns for the transcript.
    Resumed(Box<ChatSession>, Vec<PastTurn>, Vec<SessionSummary>),
    /// A plain note for the transcript (a chat that would not reopen).
    Notice(String),
    /// A note that is information, never an error, whatever its words
    /// ("codestral can't see pictures, so Tern read …").
    Info(String),
    /// What the turn is doing before the model is asked ("Looking at
    /// image…", "Reading report.pdf…"; #69), or `None` when done.
    Doing(Option<String>),
    /// Every configured provider's listing, for routing.
    Catalog(Box<tern_agent::media::Catalog>),
    /// An attached audio file's transcript (file name, seconds it plays
    /// when known), or why not.
    /// Audio `id` (see `Model::audio_started`) finished: its transcript or
    /// why not.
    FileTranscribed(u64, Result<String, String>),
    /// One window of streaming dictation came back: its number and text.
    /// A dictation window's text (or why not) and the language heard.
    Dictated(u64, Result<String, String>, Option<String>),
    /// A past chat's turns, read before its model connects, so the
    /// transcript shows at once; `Resumed` (or a connect failure) follows.
    Opening(SessionId, Vec<PastTurn>),
    /// Reopening a chat could not connect its model: the chat, its
    /// turns, and why. The transcript stays; the user can pick a model.
    ResumeFailed(SessionId, String),
    /// A site's favicon on disk, or `None` when it could not be had.
    Icon(String, Option<std::path::PathBuf>),
    /// A file, folder, or archive read for the composer, or why not.
    Attached(Result<tern_agent::Attachment, String>),
    /// A page for the built-in browser: the load's number and the page.
    Browsed(u64, Result<tern_agent::browse::BrowsedPage, String>),
    /// A git operation finished: the repository as it now is, and what
    /// to tell the user.
    Git(crate::git::GitSnapshot, Result<Option<String>, String>),
    /// The chat list after archiving one.
    Sessions(Vec<SessionSummary>),
    /// The open chat after a model or search change, and why it could
    /// not change (it keeps the old model then).
    Swapped(Box<ChatSession>, Option<String>),
    ConnectFailed(String),
    /// "Compact now" finished: the session back, what it did, and the
    /// context after.
    Compacted(
        Box<ChatSession>,
        Result<Option<tern_agent::Compacted>, String>,
        ContextUse,
    ),
    /// An event from the running turn, as logged.
    Event(Box<tern_core::EventEnvelope>),
    /// Reply text streaming in: the model call's number and the new text.
    Text(u32, String),
    /// The model's reasoning streaming in, the same way.
    Reasoning(u32, String),
    /// The Archived chats view: archived chats, chats in the trash, and a
    /// line to tell the user.
    Archive(Vec<SessionSummary>, Vec<SessionSummary>, Option<String>),
    Approval(ApprovalRequest, oneshot::Sender<ApprovalAnswer>),
    /// A deep research plan for the user to run, edit, skip, or cancel.
    Plan(ResearchPlan, oneshot::Sender<PlanAnswer>),
    Done(
        Box<ChatSession>,
        TurnOutcome,
        ContextUse,
        Vec<SessionSummary>,
    ),
    /// The Providers screen's rows, the saved choices, and a keychain error.
    Providers(Vec<ServiceRow>, Box<Preferences>, Option<String>),
    /// A key (new or saved) was tried against its service.
    Checked(Kind, &'static str, Result<String, String>),
    Removed(Kind, &'static str, Result<bool, String>),
    Models(&'static str, Result<Vec<ModelInfo>, String>),
    /// Saving the choices failed; the chat still reconnects with them.
    PrefsFailed(String),
    /// The watch list, and a line to tell the user (a run's result or an
    /// error), if any. `finished` counts watch runs that just ended.
    Watches(Vec<tern_agent::watch::Watch>, Option<String>, usize),
    /// The Study harness: see [`StudyMsg`].
    Study(Box<StudyMsg>),
    /// The Decision model page.
    Decision(Box<DecisionMsg>),
    /// Message IDs, ranges, sub-sessions, handoff, side chat, highlights,
    /// and the Library.
    Cont(Box<ContMsg>),
    /// MCP servers, skills, instructions, and project folders.
    Ext(Box<ExtMsg>),
    /// The Evaluation page.
    Eval(Box<EvalMsg>),
    /// Page readers and custom providers (Settings).
    Setup(Box<SetupMsg>),
}

/// Sends a reply's reasoning to the window as it streams.
fn reasoning_sink(tx: &mpsc::UnboundedSender<Msg>) -> tern_agent::ReasoningSink {
    let tx = tx.clone();
    std::sync::Arc::new(move |call: u32, t: &str| {
        let _ = tx.send(Msg::Reasoning(call, t.to_owned()));
    })
}

/// What the Evaluation page's background work tells the window.
#[derive(Debug)]
pub enum EvalMsg {
    /// Saved runs, newest first; the run to show; the run before it.
    Loaded(
        Vec<tern_agent::eval::RunSummary>,
        Option<Box<tern_agent::eval::EvalRun>>,
        Option<Box<tern_agent::eval::EvalRun>>,
    ),
    /// An offline run finished: a line to show, or why it failed.
    Ran(Result<String, String>),
}

/// What the Decision model page's background work tells the window.
#[derive(Debug)]
pub enum DecisionMsg {
    /// The remote catalog with key status, the saved choices, a keychain error.
    Loaded(
        Vec<tern_agent::decide::DecisionRow>,
        tern_agent::decide::DecisionPrefs,
        Option<String>,
    ),
    /// A key was tested (and saved, when it worked).
    Checked(&'static str, Result<String, String>),
    Removed(&'static str, Result<bool, String>),
    /// The page's Test button: a sample decision with the choices shown.
    Tested(Result<String, String>),
}

/// What the Study harness's background work tells the window. Each is
/// sent once and boxed in [`Msg`], so the variants' sizes do not matter.
#[allow(clippy::large_enum_variant)]
pub enum StudyMsg {
    /// The library as saved, a line to tell the user, and who the
    /// passages go to (`None` when studying offline).
    Library(Result<StudyLibrary, String>, Option<String>, Option<String>),
    /// Tutor reply text streaming in.
    TutorText(String),
    /// The tutor's reply, and the library after it (the topic was seen).
    Tutor(Result<TutorReply, String>, Option<StudyLibrary>),
    /// A quiz to take, with a note when it was written without the model.
    Quiz(Result<(Quiz, Option<String>), String>),
    /// Question `n` graded: what was given and the grade.
    Graded(usize, String, Grade),
    /// The attempt saved: its summary and the library with new mastery.
    QuizDone(Result<QuizSummary, String>, Option<StudyLibrary>),
    /// The viva with its next question waiting, and a note.
    Viva(Result<Viva, String>, Option<String>),
    /// The viva ended: its report and where it was saved.
    VivaReport(
        Result<(Viva, VivaReport, Option<std::path::PathBuf>), String>,
        Option<String>,
        Option<StudyLibrary>,
    ),
    /// The flashcard deck as saved, and a line to tell the user (cards
    /// written, an export, an import).
    Deck(Result<Deck, String>, Option<String>),
    /// A review was saved; the library with its nudged mastery.
    Reviewed(Result<StudyLibrary, String>),
    /// An Explain in 3D scene for tab `key`.
    Explain(u32, Result<(Scene, Option<String>), String>),
    /// Saving or exporting tab `key`'s scene: where it went.
    ExplainSaved(u32, Result<String, String>),
}

impl std::fmt::Debug for StudyMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Library(r, ..) => write!(f, "Library(ok: {})", r.is_ok()),
            Self::TutorText(t) => write!(f, "TutorText({} chars)", t.len()),
            Self::Tutor(r, _) => write!(f, "Tutor(ok: {})", r.is_ok()),
            Self::Quiz(r) => write!(f, "Quiz(ok: {})", r.is_ok()),
            Self::Graded(n, ..) => write!(f, "Graded({n})"),
            Self::QuizDone(r, _) => write!(f, "QuizDone(ok: {})", r.is_ok()),
            Self::Viva(r, _) => write!(f, "Viva(ok: {})", r.is_ok()),
            Self::VivaReport(r, ..) => write!(f, "VivaReport(ok: {})", r.is_ok()),
            Self::Deck(r, _) => write!(f, "Deck(ok: {})", r.is_ok()),
            Self::Reviewed(r) => write!(f, "Reviewed(ok: {})", r.is_ok()),
            Self::Explain(k, r) => write!(f, "Explain({k}, ok: {})", r.is_ok()),
            Self::ExplainSaved(k, r) => write!(f, "ExplainSaved({k}, ok: {})", r.is_ok()),
        }
    }
}

impl std::fmt::Debug for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected(..) => f.write_str("Connected"),
            Self::Swapped(_, failed) => write!(f, "Swapped(ok: {})", failed.is_none()),
            Self::Attached(r) => write!(f, "Attached(ok: {})", r.is_ok()),
            Self::Resumed(_, turns, _) => write!(f, "Resumed({} turns)", turns.len()),
            Self::Notice(e) => write!(f, "Notice({e})"),
            Self::Info(e) => write!(f, "Info({e})"),
            Self::Compacted(_, r, _) => write!(f, "Compacted(ok: {})", r.is_ok()),
            Self::Catalog(c) => write!(f, "Catalog({} providers)", c.providers.len()),
            Self::FileTranscribed(id, r) => {
                write!(f, "FileTranscribed({id}, ok: {})", r.is_ok())
            }
            Self::Dictated(seq, r, _) => write!(f, "Dictated({seq}, ok: {})", r.is_ok()),
            Self::Opening(id, turns) => write!(f, "Opening({id}, {} turns)", turns.len()),
            Self::ResumeFailed(id, e) => write!(f, "ResumeFailed({id}, {e})"),
            Self::Git(s, r) => write!(f, "Git(repo: {}, ok: {})", s.status.is_repo, r.is_ok()),
            Self::Browsed(r, p) => write!(f, "Browsed({r}, ok: {})", p.is_ok()),
            Self::Sessions(s) => write!(f, "Sessions({})", s.len()),
            Self::ConnectFailed(e) => write!(f, "ConnectFailed({e})"),
            Self::Event(e) => write!(f, "Event({})", e.kind),
            Self::Icon(site, path) => write!(f, "Icon({site}, {})", path.is_some()),
            Self::Text(call, t) => write!(f, "Text({call}, {} chars)", t.len()),
            Self::Reasoning(call, t) => write!(f, "Reasoning({call}, {} chars)", t.len()),
            Self::Archive(a, d, _) => {
                write!(f, "Archive({} archived, {} deleted)", a.len(), d.len())
            }
            Self::Approval(r, _) => write!(f, "Approval({})", r.tool),
            Self::Plan(p, _) => write!(f, "Plan({} parts)", p.sub_questions.len()),
            Self::Done(_, out, ..) => write!(f, "Done({:?})", out.state),
            Self::Providers(rows, ..) => write!(f, "Providers({})", rows.len()),
            Self::Checked(k, id, r) => write!(f, "Checked({k:?}, {id}, {})", r.is_ok()),
            Self::Removed(k, id, r) => write!(f, "Removed({k:?}, {id}, {})", r.is_ok()),
            Self::Models(id, r) => write!(f, "Models({id}, {})", r.is_ok()),
            Self::PrefsFailed(e) => write!(f, "PrefsFailed({e})"),
            Self::Study(m) => write!(f, "Study({m:?})"),
            Self::Decision(m) => write!(f, "Decision({m:?})"),
            Self::Cont(m) => write!(f, "Cont({m:?})"),
            Self::Ext(m) => write!(f, "Ext({m:?})"),
            Self::Setup(m) => write!(f, "Setup({m:?})"),
            Self::Eval(m) => match m.as_ref() {
                EvalMsg::Loaded(runs, sel, _) => write!(
                    f,
                    "Eval(Loaded({} runs, showing {}))",
                    runs.len(),
                    sel.as_ref().map(|r| r.id.as_str()).unwrap_or("none")
                ),
                EvalMsg::Ran(r) => write!(f, "Eval(Ran(ok: {}))", r.is_ok()),
            },
            Self::Watches(w, note, n) => {
                write!(
                    f,
                    "Watches({}, note: {}, finished: {n})",
                    w.len(),
                    note.is_some()
                )
            }
        }
    }
}

/// How many chats the sidebar lists.
pub const RECENT: usize = 30;

/// Sends approval requests to the window and waits for the button pressed.
struct UiApprover(mpsc::UnboundedSender<Msg>);

impl Approver for UiApprover {
    fn ask<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, ApprovalAnswer> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            if self.0.send(Msg::Approval(req.clone(), tx)).is_err() {
                return ApprovalAnswer::Deny;
            }
            // A closed window counts as a refusal.
            rx.await.unwrap_or(ApprovalAnswer::Deny)
        })
    }
}

/// Sends a drafted plan to the window and waits for the user's choice.
struct UiPlanApprover(mpsc::UnboundedSender<Msg>);

impl PlanApprover for UiPlanApprover {
    fn review<'a>(&'a self, plan: &'a ResearchPlan) -> BoxFuture<'a, PlanAnswer> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            if self.0.send(Msg::Plan(plan.clone(), tx)).is_err() {
                return PlanAnswer::Cancel;
            }
            // A closed window cancels the plan.
            rx.await.unwrap_or(PlanAnswer::Cancel)
        })
    }
}

/// How long the picker waits for a provider's model listing.
pub const MODELS_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// The tokio side of the app.
/// Pictures attached to a message, and how they reach the chat (see
/// [`Bridge::send`]).
#[derive(Debug)]
pub struct Pictures {
    /// Already normalised (PNG/JPEG, 2048 px, 5 MB) when `sent`.
    pub images: Vec<tern_agent::ImageInput>,
    /// Who reads them when the chat model cannot see, best first (local
    /// vision, local OCR, provider vision, provider OCR).
    pub readers: Vec<tern_agent::media::Reader>,
    /// The chat model sees them: they went with the message, and
    /// `readers` is the fallback if the provider refuses pictures.
    pub sent: bool,
    pub chat_model: String,
}

#[derive(Debug)]
pub struct Bridge {
    rt: tokio::runtime::Runtime,
    tx: mpsc::UnboundedSender<Msg>,
    /// The model study calls go to, opened on first use.
    study_engine: std::sync::Arc<std::sync::Mutex<Option<StudyEngine>>>,
    /// The open side chat.
    side: cont::SideSlot,
    /// Explain in 3D scenes being built, by tab key, to cancel.
    explain_jobs: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u32, explain::Cancel>>>,
}

impl Bridge {
    pub fn new() -> std::io::Result<(Self, mpsc::UnboundedReceiver<Msg>)> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            // Four workers: a turn, a model switch, a listing, and a chat
            // being reopened can all wait on the network at once without
            // holding each other up.
            .worker_threads(4)
            .enable_all()
            .thread_name("tern-desktop-rt")
            .build()?;
        let (tx, rx) = mpsc::unbounded_channel();
        Ok((
            Self {
                rt,
                tx,
                study_engine: Default::default(),
                side: Default::default(),
                explain_jobs: Default::default(),
            },
            rx,
        ))
    }

    /// Opens a new chat (a draft until its first message) with the
    /// user's saved choices.
    pub fn connect(&self) {
        self.connect_harness(tern_agent::Harness::Research);
    }

    /// [`Bridge::connect`] for a harness: a new Research or Study draft.
    pub fn connect_harness(&self, harness: tern_agent::Harness) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let prefs = tokio::task::spawn_blocking(Preferences::load)
                .await
                .unwrap_or_default();
            let mut opts = prefs.chat_options();
            opts.harness = harness;
            match ChatSession::open(opts).await {
                Ok(s) => {
                    let recent = s.recent_sessions(RECENT).unwrap_or_default();
                    let _ = tx.send(Msg::Connected(Box::new(s), recent));
                }
                Err(e) => {
                    let _ = tx.send(Msg::ConnectFailed(e));
                }
            }
        });
    }

    /// Opens the chat to work in for `harness`: `prefer` when given, else
    /// the last chat with messages (on launch, or switching harness), else
    /// a new draft. Never makes an empty row.
    pub fn launch(&self, harness: tern_agent::Harness, prefer: Option<SessionId>) {
        let start = prefer.map_or(crate::restore::Start::Last, crate::restore::Start::Chat);
        self.launch_start(harness, start);
    }

    /// Opens `harness` with the chat `start` names: a given chat (or, when
    /// it is gone, the last chat with messages), a new chat, or the last
    /// chat with messages.
    pub fn launch_start(&self, harness: tern_agent::Harness, start: crate::restore::Start) {
        use crate::restore::Start;
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let (prefs, tries) = tokio::task::spawn_blocking(move || {
                let tries: Vec<SessionId> = match start {
                    Start::Fresh => Vec::new(),
                    Start::Chat(id) => {
                        // Only a chat still listed (not archived) in this
                        // harness; otherwise the last chat with messages.
                        let listed = tern_agent::recent_sessions(500)
                            .unwrap_or_default()
                            .iter()
                            .any(|s| s.id == id && s.harness == harness);
                        listed
                            .then_some(id)
                            .into_iter()
                            .chain(tern_agent::chat_to_reopen(harness).filter(|r| *r != id))
                            .collect()
                    }
                    Start::Last => tern_agent::chat_to_reopen(harness).into_iter().collect(),
                };
                (Preferences::load(), tries)
            })
            .await
            .unwrap_or_default();
            for id in tries {
                // The chat shows at once; its model connects after.
                let turns = tokio::task::spawn_blocking(move || tern_agent::stored_turns(id))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                if let Ok(turns) = turns {
                    let _ = tx.send(Msg::Opening(id, turns));
                    resume_connect(&tx, &prefs, id).await;
                    return;
                }
            }
            let mut opts = prefs.chat_options();
            opts.harness = harness;
            match ChatSession::open(opts).await {
                Ok(s) => {
                    let recent = s.recent_sessions(RECENT).unwrap_or_default();
                    let _ = tx.send(Msg::Connected(Box::new(s), recent));
                }
                Err(e) => {
                    let _ = tx.send(Msg::ConnectFailed(e));
                }
            }
        });
    }

    /// Remembers the chat to reopen next launch.
    pub fn remember(&self, harness: tern_agent::Harness, id: SessionId) {
        self.rt.spawn_blocking(move || {
            let _ = tern_agent::remember_chat(harness, id);
        });
    }

    /// Saves the choices, then switches the open chat to them in place:
    /// same chat, same history, the next answer from the new model.
    pub fn swap(&self, mut session: Box<ChatSession>, prefs: Preferences) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let saved = {
                let p = prefs.clone();
                tokio::task::spawn_blocking(move || p.save())
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
            };
            if let Err(e) = saved {
                let _ = tx.send(Msg::PrefsFailed(e));
            }
            let result = session.set_model(&prefs.chat_options()).await;
            let _ = tx.send(Msg::Swapped(session, result.err()));
        });
    }

    /// Tests a search service with an address (SearXNG) before it is chosen.
    pub fn test_search_at(&self, id: &'static str, url: Option<String>) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let keychain = tern_agent::providers::keychain();
            let secrets = keychain.as_ref().ok().map(|b| b.as_ref());
            let r = tern_agent::search::test(id, secrets, url.as_deref())
                .await
                .map(|n| match n {
                    0 => "It answered, with no results for the test query".to_owned(),
                    1 => "Working · 1 result for the test query".to_owned(),
                    n => format!("Working · {n} results for the test query"),
                });
            let _ = tx.send(Msg::Checked(Kind::Search, id, r));
        });
    }

    /// Reopens a past chat with the saved provider choices. Its turns are
    /// read from the database first and sent on their own, so the
    /// transcript shows at once; the model connects after, and a model
    /// that cannot be reached leaves the transcript up with a reason.
    pub fn resume(&self, id: SessionId) {
        self.rt.spawn(resume_task(self.tx.clone(), id));
    }

    /// Makes a branch of chat `from` keeping its first `turns` turns, then
    /// opens the branch.
    pub fn branch(&self, from: SessionId, turns: usize) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let made = tokio::task::spawn_blocking(move || {
                tern_agent::branch::branch_session(from, turns)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            let id = match made {
                Ok(id) => id,
                Err(e) => {
                    let _ = tx.send(Msg::Notice(format!("Tern could not branch that chat: {e}")));
                    return;
                }
            };
            let prefs = tokio::task::spawn_blocking(Preferences::load)
                .await
                .unwrap_or_default();
            match ChatSession::resume(prefs.chat_options(), id).await {
                Ok((s, turns)) => {
                    let recent = s.recent_sessions(RECENT).unwrap_or_default();
                    let _ = tx.send(Msg::Resumed(Box::new(s), turns, recent));
                }
                Err(e) => {
                    let _ = tx.send(Msg::Notice(format!("Tern could not open the branch: {e}")));
                }
            }
        });
    }

    /// Saves a chat's new title, then sends the list again.
    pub fn rename(&self, id: SessionId, title: String) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            if let Err(e) = tern_agent::rename_session(id, &title) {
                let _ = tx.send(Msg::Notice(format!("Tern could not rename that chat: {e}")));
            }
            let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Sessions(recent));
        });
    }

    /// Fetches a site's favicon into the icon cache (once per site), through
    /// the same network guard as page reads.
    pub fn favicon(&self, site: String, url: String) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let found = tern_agent::sources::cached_favicon(&site, &url).await;
            let _ = tx.send(Msg::Icon(site, found));
        });
    }

    /// Hides a chat from the list, then sends the list again.
    pub fn archive(&self, id: SessionId) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            if let Err(e) = tern_agent::archive_session(id) {
                let _ = tx.send(Msg::Notice(format!(
                    "Tern could not archive that chat: {e}"
                )));
            }
            let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Sessions(recent));
        });
    }

    /// Reads files, folders, or archives off the UI thread, one message each.
    pub fn attach(&self, paths: Vec<std::path::PathBuf>) {
        for path in paths {
            let tx = self.tx.clone();
            self.rt.spawn(async move {
                let result =
                    tokio::task::spawn_blocking(move || tern_agent::Attachment::load(&path))
                        .await
                        .unwrap_or_else(|e| Err(format!("Tern could not read that file: {e}")));
                let _ = tx.send(Msg::Attached(result));
            });
        }
    }

    /// Folds the chat's older turns into a summary now.
    pub fn compact(&self, mut session: Box<ChatSession>) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let events = tx.clone();
            let observer = move |e: &tern_core::EventEnvelope| {
                let _ = events.send(Msg::Event(Box::new(e.clone())));
            };
            let result = session.compact(false, &observer).await;
            let usage = session.context();
            let _ = tx.send(Msg::Compacted(session, result, usage));
        });
    }

    /// Lists every configured provider's models for the routing.
    pub fn catalog(&self, ids: Vec<&'static str>) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let keychain = setup::keychain();
            let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
            let c = tern_agent::media::catalog(&ids, secrets).await;
            let _ = tx.send(Msg::Catalog(Box::new(c)));
        });
    }

    /// Reads an audio file off the UI thread and transcribes it (local
    /// models on a blocking thread), reporting progress through `ctl`,
    /// stopping when it is cancelled or runs past the time limit.
    pub fn transcribe_file(
        &self,
        via: tern_agent::media::ModelRef,
        path: std::path::PathBuf,
        secs: Option<f64>,
        id: u64,
        ctl: tern_agent::local_models::TranscribeCtl,
    ) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "audio.wav".into());
            match tokio::task::spawn_blocking({
                let path = path.clone();
                move || std::fs::read(path)
            })
            .await
            .unwrap_or_else(|e| Err(std::io::Error::other(e.to_string())))
            {
                Ok(bytes) => {
                    let keychain = setup::keychain();
                    let secrets: Option<&dyn SecretStore> =
                        keychain.as_ref().ok().map(|b| b.as_ref());
                    let r = tern_agent::media::transcribe_with(
                        secrets, &via, &bytes, &name, &ctl, secs,
                    )
                    .await;
                    let _ = tx.send(Msg::FileTranscribed(id, r));
                }
                Err(e) => {
                    let _ = tx.send(Msg::FileTranscribed(
                        id,
                        Err(format!("Tern could not read {name}: {e}")),
                    ));
                }
            }
        });
    }

    /// Runs one turn and hands the session back when it ends. With
    /// `pictures` the chat model cannot see, the readers read them first
    /// (each failure falls through to the next) and their text joins the
    /// message; when none can, the message still goes with a note. When
    /// the provider refuses pictures for the chat model itself, the model
    /// is remembered as not seeing, the pictures leave the chat's history,
    /// and the turn runs again through the readers, with no failed run.
    pub fn send(
        &self,
        mut session: Box<ChatSession>,
        text: String,
        cancel: CancelFlag,
        pictures: Option<Pictures>,
    ) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let approver = UiApprover(tx.clone());
            let events = tx.clone();
            let observer = move |e: &tern_core::EventEnvelope| {
                let _ = events.send(Msg::Event(Box::new(e.clone())));
            };
            let texts = tx.clone();
            let on_text = move |call: u32, t: &str| {
                let _ = texts.send(Msg::Text(call, t.to_owned()));
            };
            session.set_reasoning_sink(Some(reasoning_sink(&tx)));
            let text = match &pictures {
                Some(p) if !p.sent => read_pictures(&tx, p, &text).await,
                _ => text,
            };
            let mut out = session
                .send_streaming(&text, &approver, cancel.clone(), &observer, Some(&on_text))
                .await;
            // The chat model refused the pictures sent with this message:
            // it cannot see. The session already remembers that and drops
            // earlier pictures on its own; here the pictures go through the
            // readers and the turn runs again, with no failed run shown.
            let refused = out.answer.is_none()
                && out.state == tern_core::TurnState::Failed
                && out
                    .reason
                    .as_deref()
                    .is_some_and(tern_agent::media::rejects_images);
            if refused
                && !cancel.is_cancelled()
                && let Some(p) = pictures.as_ref().filter(|p| p.sent)
            {
                let provider = session.provider_id().to_owned();
                let model = session.model().to_owned();
                tern_agent::media::mark_not_vision(&provider, &model);
                session.set_sees_pictures(Some(false));
                session.drop_pictures();
                let text = read_pictures(&tx, p, &text).await;
                out = session
                    .send_streaming(&text, &approver, cancel, &observer, Some(&on_text))
                    .await;
            }
            let usage = session.context();
            // Check the answer's claims and write the run report; the
            // events reach the run panel before the turn is handed back.
            session.check_last_turn(&observer).await;
            let recent = session.recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Done(session, out, usage, recent));
        });
    }

    /// Runs one deep research turn (plan, approval, parallel workers,
    /// synthesis) and hands the session back when it ends. `control`
    /// stops one worker or the whole run.
    pub fn deep(&self, mut session: Box<ChatSession>, text: String, control: DeepControl) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let opts = tokio::task::spawn_blocking(|| Preferences::load().deep_options())
                .await
                .unwrap_or_default();
            let approver = UiPlanApprover(tx.clone());
            let events = tx.clone();
            let observer = move |e: &tern_core::EventEnvelope| {
                let _ = events.send(Msg::Event(Box::new(e.clone())));
            };
            let texts = tx.clone();
            let on_text = move |call: u32, t: &str| {
                let _ = texts.send(Msg::Text(call, t.to_owned()));
            };
            let out = session
                .deep_research(&text, &opts, &approver, &control, &observer, Some(&on_text))
                .await;
            let usage = session.context();
            session.check_last_turn(&observer).await;
            let recent = session.recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Done(session, out, usage, recent));
        });
    }

    /// Resumes the chat's stopped run from its checkpoint and hands the
    /// session back when it ends, like [`Bridge::send`].
    pub fn resume_run(&self, mut session: Box<ChatSession>, control: DeepControl) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let opts = tokio::task::spawn_blocking(|| Preferences::load().deep_options())
                .await
                .unwrap_or_default();
            let approver = UiApprover(tx.clone());
            let events = tx.clone();
            let observer = move |e: &tern_core::EventEnvelope| {
                let _ = events.send(Msg::Event(Box::new(e.clone())));
            };
            let texts = tx.clone();
            let on_text = move |call: u32, t: &str| {
                let _ = texts.send(Msg::Text(call, t.to_owned()));
            };
            session.set_reasoning_sink(Some(reasoning_sink(&tx)));
            let out = match session
                .resume_run(&opts, &approver, &control, &observer, Some(&on_text))
                .await
            {
                Ok(out) => out,
                Err(e) => TurnOutcome {
                    turn_id: tern_core::TurnId::new(),
                    state: tern_core::TurnState::Failed,
                    answer: None,
                    reason: Some(e),
                    states: Vec::new(),
                    usage: tern_agent::Usage::measured(0, 0),
                    model_calls: 0,
                    tool_calls: 0,
                },
            };
            let usage = session.context();
            if out.answer.is_some() {
                session.check_last_turn(&observer).await;
            }
            let recent = session.recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Done(session, out, usage, recent));
        });
    }

    /// Loads the Archived chats view.
    pub fn load_archive(&self) {
        self.trash(|_| Ok(None));
    }

    /// Runs `f` on the database off the UI thread, then sends the archive
    /// lists and the chat list.
    fn trash(
        &self,
        f: impl FnOnce(&mut tern_agent::Store) -> Result<Option<String>, String> + Send + 'static,
    ) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            match tern_agent::trash::open_store() {
                Ok(mut store) => {
                    let note = f(&mut store).unwrap_or_else(Some);
                    let (a, d) = tern_agent::trash::lists(&store).unwrap_or_default();
                    let _ = tx.send(Msg::Archive(a, d, note));
                }
                Err(e) => {
                    let _ = tx.send(Msg::Archive(Vec::new(), Vec::new(), Some(e)));
                }
            }
            let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Sessions(recent));
        });
    }

    /// Moves a chat to the trash.
    pub fn delete_chat(&self, id: SessionId) {
        self.trash(move |s| tern_agent::trash::delete(s, id).map(|_| None));
    }

    /// Takes a chat out of the trash.
    pub fn restore_chat(&self, id: SessionId) {
        self.trash(move |s| {
            tern_agent::trash::restore(s, id).map(|_| Some("Chat restored.".into()))
        });
    }

    /// Brings an archived chat back to the sidebar.
    pub fn unarchive_chat(&self, id: SessionId) {
        self.trash(move |s| {
            tern_agent::trash::unarchive(s, id).map(|_| Some("Chat back in the list.".into()))
        });
    }

    /// Erases a chat in the trash for good.
    pub fn erase_chat(&self, id: SessionId) {
        self.trash(move |s| {
            let data = tern_agent::trash::data_dir()?;
            tern_agent::trash::erase(s, &data, id).map(|_| Some("Chat deleted for good.".into()))
        });
    }

    /// Erases every chat in the trash.
    pub fn empty_trash(&self) {
        self.trash(move |s| {
            let data = tern_agent::trash::data_dir()?;
            let n = tern_agent::trash::empty(s, &data)?;
            Ok(Some(match n {
                0 => "The trash was already empty.".into(),
                1 => "Deleted 1 chat for good.".into(),
                n => format!("Deleted {n} chats for good."),
            }))
        });
    }

    /// Loads the watch list.
    pub fn load_watches(&self) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let (list, note) = match tern_agent::watch::WatchList::load() {
                Ok(l) => (l.watches, None),
                Err(e) => (Vec::new(), Some(e)),
            };
            let _ = tx.send(Msg::Watches(list, note, 0));
        });
    }

    /// Changes the watch list with `f` and saves it.
    fn edit_watches(
        &self,
        f: impl FnOnce(&mut tern_agent::watch::WatchList) -> Result<String, String> + Send + 'static,
    ) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let result = tern_agent::watch::WatchList::load().and_then(|mut l| {
                let note = f(&mut l)?;
                l.save()?;
                Ok((l.watches, note))
            });
            let msg = match result {
                Ok((list, note)) => Msg::Watches(list, Some(note), 0),
                Err(e) => {
                    let list = tern_agent::watch::WatchList::load()
                        .map(|l| l.watches)
                        .unwrap_or_default();
                    Msg::Watches(list, Some(e), 0)
                }
            };
            let _ = tx.send(msg);
        });
    }

    /// Puts a question on watch.
    pub fn add_watch(&self, question: String, every_hours: u32, deep: bool) {
        self.edit_watches(move |l| {
            let w = l.add(&question, every_hours, deep)?;
            Ok(format!("Watching “{}” {}.", w.question, w.every_label()))
        });
    }

    pub fn remove_watch(&self, id: String) {
        self.edit_watches(move |l| {
            let w = l.remove(&id)?;
            Ok(format!("Stopped watching “{}”.", w.question))
        });
    }

    /// Changes how often a watch runs.
    pub fn set_watch_every(&self, id: String, every_hours: u32) {
        self.edit_watches(move |l| {
            let i = l.position(&id)?;
            l.watches[i].every_hours = every_hours;
            Ok(format!(
                "“{}” now runs {}.",
                l.watches[i].question,
                l.watches[i].every_label()
            ))
        });
    }

    /// Runs watches in the background, one after another: `ids`, or every
    /// due one when empty. Each sends the list when it ends.
    pub fn run_watches(&self, ids: Vec<String>) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let prefs = tokio::task::spawn_blocking(Preferences::load)
                .await
                .unwrap_or_default();
            let ids = if ids.is_empty() {
                tern_agent::watch::due_now().unwrap_or_default()
            } else {
                ids
            };
            if ids.is_empty() {
                let list = tern_agent::watch::WatchList::load()
                    .map(|l| l.watches)
                    .unwrap_or_default();
                let _ = tx.send(Msg::Watches(list, None, 1));
                return;
            }
            let n = ids.len();
            for (k, id) in ids.into_iter().enumerate() {
                let result =
                    tern_agent::watch::run_watch(&id, prefs.chat_options(), prefs.deep_options())
                        .await;
                let note = match &result {
                    Ok(r) => format!("Watch “{}”: {}", r.watch.question, r.watch.status_line()),
                    Err(e) => format!("A watch could not run: {e}"),
                };
                let list = tern_agent::watch::WatchList::load()
                    .map(|l| l.watches)
                    .unwrap_or_default();
                // The last run tells the window the batch is over.
                let _ = tx.send(Msg::Watches(list, Some(note), usize::from(k + 1 == n)));
            }
        });
    }

    /// Saves the choices, then opens a chat with them.
    pub fn save_and_connect(&self, prefs: Preferences) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let saved = {
                let p = prefs.clone();
                tokio::task::spawn_blocking(move || p.save())
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
            };
            if let Err(e) = saved {
                let _ = tx.send(Msg::PrefsFailed(e));
            }
            match ChatSession::open(prefs.chat_options()).await {
                Ok(s) => {
                    let recent = s.recent_sessions(RECENT).unwrap_or_default();
                    let _ = tx.send(Msg::Connected(Box::new(s), recent));
                }
                Err(e) => {
                    let _ = tx.send(Msg::ConnectFailed(e));
                }
            }
        });
    }

    /// Saves the choices, then reopens chat `id` with them (a chat whose
    /// model could not be reached, after the user picked another).
    pub fn save_and_resume(&self, prefs: Preferences, id: SessionId) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let saved = tokio::task::spawn_blocking(move || prefs.save())
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            if let Err(e) = saved {
                let _ = tx.send(Msg::PrefsFailed(e));
            }
            resume_task(tx, id).await;
        });
    }

    /// Reads the catalog with key status and the saved choices.
    /// Runs a git operation in `dir`, then reads the repository again.
    pub fn git(&self, dir: std::path::PathBuf, op: crate::git::GitOp) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let repo = tern_agent::git::Repo::at(dir);
            let prefs = Preferences::load();
            let owner = prefs.git_name.clone().unwrap_or_default();
            let year = tern_agent::git::this_year();
            let result = crate::git::run(&repo, &op, &prefs.git_identity(), &owner, year);
            let snapshot = crate::git::GitSnapshot::read(&repo);
            let _ = tx.send(Msg::Git(snapshot, result));
        });
    }

    /// Saves the choices without reconnecting the chat.
    pub fn save_prefs(&self, prefs: Preferences) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            if let Err(e) = prefs.save() {
                let _ = tx.send(Msg::PrefsFailed(e));
            }
            // Custom providers, the page reader, the user's prompts, and the
            // decision jobs take effect at once, not at the next launch.
            prefs.apply_runtime();
        });
    }

    /// Reads a link (URL or DOI) through the guarded reader and attaches it.
    pub fn attach_link(&self, url: String) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let r = tern_agent::browse::open(&url).await.map(|p| p.attachment());
            let _ = tx.send(Msg::Attached(r));
        });
    }

    /// Reads a page for the built-in browser.
    pub fn browse(&self, request: u64, url: String) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let page = tern_agent::browse::open(&url).await;
            let _ = tx.send(Msg::Browsed(request, page));
        });
    }

    pub fn load_providers(&self) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let _ = tx.send(providers_msg());
        });
    }

    /// Tests a new key and saves it to the keychain only if it works.
    pub fn add_key(&self, kind: Kind, id: &'static str, key: SecretString) {
        let tx = self.tx.clone();
        let base = Preferences::load().base_url(id).map(str::to_owned);
        self.rt.spawn(async move {
            let result = match setup::keychain() {
                Err(e) => Err(format!("The keychain is not available: {e}")),
                Ok(store) => match kind {
                    Kind::Model => setup::add(id, key, store.as_ref(), base.as_deref())
                        .await
                        .map(|r| model_report(r.chat_models, r.models, r.latency_ms))
                        .map_err(|e| e.to_string()),
                    Kind::Search => search::add(id, key, store.as_ref(), None)
                        .await
                        .map(|_| "Key works and is saved".to_owned()),
                },
            };
            let _ = tx.send(Msg::Checked(kind, id, result));
        });
    }

    /// Tries the saved key, or the local server.
    pub fn test(&self, kind: Kind, id: &'static str) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let keychain = setup::keychain();
            let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
            let result = match kind {
                Kind::Model => setup::test(id, secrets, Preferences::load().base_url(id))
                    .await
                    .map(|r| model_report(r.chat_models, r.models, r.latency_ms))
                    .map_err(|e| e.to_string()),
                Kind::Search => {
                    search::test(id, secrets, Preferences::load().search_url.as_deref())
                        .await
                        .map(|n| format!("Works, {n} result{}", if n == 1 { "" } else { "s" }))
                }
            };
            let _ = tx.send(Msg::Checked(kind, id, result));
        });
    }

    pub fn remove(&self, kind: Kind, id: &'static str) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let result = setup::keychain()
                .map_err(|e| format!("The keychain is not available: {e}"))
                .and_then(|store| match kind {
                    Kind::Model => setup::remove(id, store.as_ref()).map_err(|e| e.to_string()),
                    Kind::Search => search::remove(id, store.as_ref()),
                });
            let _ = tx.send(Msg::Removed(kind, id, result));
        });
    }

    /// Reads the saved evaluation runs and the one to show (`last` when
    /// `which` is `None`) with the run before it.
    pub fn eval_load(&self, which: Option<String>) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let msg = match tern_agent::eval::EvalStore::discover() {
                Err(_) => EvalMsg::Loaded(Vec::new(), None, None),
                Ok(store) => {
                    let selected = store.find(which.as_deref().unwrap_or("last")).ok();
                    let previous = selected.as_ref().and_then(|r| store.previous(r));
                    EvalMsg::Loaded(
                        store.summaries(),
                        selected.map(Box::new),
                        previous.map(Box::new),
                    )
                }
            };
            let _ = tx.send(Msg::Eval(Box::new(msg)));
        });
    }

    /// Runs the offline (synthetic) suites, saves the run, and shows it.
    pub fn eval_run(&self, trials: usize) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            use tern_agent::eval::{self as ev, EvalRequest, EvalStore, TernSubject};
            let result = match EvalStore::discover() {
                Err(e) => Err(e),
                Ok(store) => {
                    let req = EvalRequest {
                        trials: Some(trials),
                        ..Default::default()
                    };
                    match Box::pin(ev::run(&store, &req, &TernSubject::offline())).await {
                        Ok((run, _)) => {
                            let (p, a) = run.passed();
                            let broken = ev::check_baseline(&run, &ev::baseline());
                            let previous = store.previous(&run);
                            let line = format!(
                                "{}: {p}/{a} case-trials passed in {:.1} s{}",
                                run.id,
                                run.seconds,
                                if broken.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · {} below the baseline", broken.len())
                                }
                            );
                            let _ = tx.send(Msg::Eval(Box::new(EvalMsg::Loaded(
                                store.summaries(),
                                Some(Box::new(run)),
                                previous.map(Box::new),
                            ))));
                            Ok(line)
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            let _ = tx.send(Msg::Eval(Box::new(EvalMsg::Ran(result))));
        });
    }

    pub fn decision_load(&self) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let keychain = setup::keychain();
            let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
            let rows = tern_agent::decide::list(secrets);
            let prefs = Preferences::load().decision;
            let _ = tx.send(Msg::Decision(Box::new(DecisionMsg::Loaded(
                rows,
                prefs,
                keychain.err(),
            ))));
        });
    }

    /// Tests a decision-model key with one small decision; saves it if it works.
    pub fn decision_add_key(&self, id: &'static str, key: SecretString) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let result = match setup::keychain() {
                Err(e) => Err(format!("The keychain is not available: {e}")),
                Ok(store) => tern_agent::decide::add(id, key, store.as_ref(), None, None)
                    .await
                    .map(|d| format!("Key works and is saved. {}", crate::decision::test_line(&d))),
            };
            let _ = tx.send(Msg::Decision(Box::new(DecisionMsg::Checked(id, result))));
        });
    }

    pub fn decision_test_key(&self, id: &'static str) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let result = match setup::keychain() {
                Err(e) => Err(format!("The keychain is not available: {e}")),
                Ok(store) => tern_agent::decide::test_saved(id, store.as_ref(), None, None)
                    .await
                    .map(|d| format!("Works. {}", crate::decision::test_line(&d))),
            };
            let _ = tx.send(Msg::Decision(Box::new(DecisionMsg::Checked(id, result))));
        });
    }

    pub fn decision_remove(&self, id: &'static str) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let result = setup::keychain()
                .map_err(|e| format!("The keychain is not available: {e}"))
                .and_then(|store| tern_agent::decide::remove(id, store.as_ref()));
            let _ = tx.send(Msg::Decision(Box::new(DecisionMsg::Removed(id, result))));
        });
    }

    /// One sample decision with the choices on the page.
    pub fn decision_test(&self, prefs: tern_agent::decide::DecisionPrefs) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let keychain = setup::keychain();
            let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
            let decider = tern_agent::decide::decider_from(&prefs, secrets);
            let result = tern_agent::decide::ask(
                &decider,
                tern_agent::decide::DecisionKind::SourceRank,
                "Which page should be read first to learn what the Transformer architecture replaced?",
                &[
                    "arxiv.org: Attention Is All You Need, the Transformer, based solely on attention, dispensing with recurrence".into(),
                    "pinterest.com: Transformers wallpaper ideas".into(),
                    "a cooking blog: weekend bread recipes".into(),
                ],
                "",
            )
            .await
            .map(|d| crate::decision::test_line(&d));
            let _ = tx.send(Msg::Decision(Box::new(DecisionMsg::Tested(result))));
        });
    }

    /// Lists a provider's models for the picker (a recent listing is
    /// reused unless `fresh`). Always answers within [`MODELS_WAIT`], with
    /// the list or why not, so the picker never stays on "Loading models…".
    pub fn models(&self, id: &'static str, fresh: bool) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let keychain = setup::keychain();
            let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
            let base = Preferences::load().base_url(id).map(str::to_owned);
            let fetch = async {
                if fresh {
                    setup::models_fresh(id, secrets, base.as_deref()).await
                } else {
                    setup::models(id, secrets, base.as_deref()).await
                }
            };
            let result = match tokio::time::timeout(MODELS_WAIT, fetch).await {
                Ok(r) => r.map_err(|e| e.to_string()),
                Err(_) => Err(format!(
                    "no answer in {} s. Check the connection and try again.",
                    MODELS_WAIT.as_secs()
                )),
            };
            let _ = tx.send(Msg::Models(id, result));
        });
    }

    /// Transcribes one window of streaming dictation (a WAV).
    pub fn dictate(
        &self,
        via: tern_agent::media::ModelRef,
        seq: u64,
        wav: Vec<u8>,
        language: Option<String>,
    ) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let keychain = setup::keychain();
            let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
            let (r, heard) =
                tern_agent::media::dictate(secrets, &via, &wav, language.as_deref()).await;
            let _ = tx.send(Msg::Dictated(seq, r, heard));
        });
    }

    // ---- Study ----

    fn study_send(tx: &mpsc::UnboundedSender<Msg>, m: StudyMsg) {
        let _ = tx.send(Msg::Study(Box::new(m)));
    }

    /// Loads the library (building topics if the materials changed) and
    /// connects the study model, forgetting any connected before.
    pub fn study_load(&self) {
        let tx = self.tx.clone();
        let cache = self.study_engine.clone();
        if let Ok(mut c) = cache.lock() {
            *c = None;
        }
        self.rt.spawn(async move {
            let lib = tokio::task::spawn_blocking(|| {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                st::ensure_topics(&store, &mut lib)?;
                Ok::<_, String>(lib)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            let engine = study_engine(&cache).await;
            let who = engine
                .as_ref()
                .map(|e| format!("{} · {}", e.provider_name(), e.model_name()));
            Self::study_send(&tx, StudyMsg::Library(lib, None, who));
        });
    }

    /// Adds files, folders, or archives as materials.
    pub fn study_add(&self, paths: Vec<std::path::PathBuf>) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = (|| {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                let mut added = 0;
                let mut errors = Vec::new();
                for p in &paths {
                    match st::add_path(&store, &mut lib, p) {
                        Ok(ms) => added += ms.len(),
                        Err(e) => errors.push(e),
                    }
                }
                st::ensure_topics(&store, &mut lib)?;
                let note = if errors.is_empty() {
                    Some(format!(
                        "Added {added} material{}. Topics are grouped by headings; Rebuild topics asks the model.",
                        if added == 1 { "" } else { "s" }
                    ))
                } else {
                    Some(errors.join(" "))
                };
                Ok::<_, String>((lib, note))
            })();
            let m = match r {
                Ok((lib, note)) => StudyMsg::Library(Ok(lib), note, None),
                Err(e) => StudyMsg::Library(Err(e), None, None),
            };
            Self::study_send(&tx, m);
        });
    }

    /// Adds a web page or online PDF as a material (read with Tern Crawl).
    pub fn study_add_link(&self, url: String) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let r = async {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                let m = st::add_link(&store, &mut lib, &url).await?;
                st::ensure_topics(&store, &mut lib)?;
                Ok::<_, String>((lib, format!("Added {}.", m.name)))
            }
            .await;
            let m = match r {
                Ok((lib, note)) => StudyMsg::Library(Ok(lib), Some(note), None),
                Err(e) => StudyMsg::Library(Err(e), None, None),
            };
            Self::study_send(&tx, m);
        });
    }

    /// Adds the latest research report of `chat` (`last` for the newest).
    pub fn study_add_report(&self, chat: String) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = (|| {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                let m = st::add_report(&store, &mut lib, &chat)?;
                st::ensure_topics(&store, &mut lib)?;
                Ok::<_, String>((lib, format!("Added the report \"{}\".", m.name)))
            })();
            let m = match r {
                Ok((lib, note)) => StudyMsg::Library(Ok(lib), Some(note), None),
                Err(e) => StudyMsg::Library(Err(e), None, None),
            };
            Self::study_send(&tx, m);
        });
    }

    pub fn study_remove(&self, id: String) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = (|| {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                let m = st::remove(&store, &mut lib, &id)?;
                st::ensure_topics(&store, &mut lib)?;
                Ok::<_, String>((lib, format!("Removed {}.", m.name)))
            })();
            let m = match r {
                Ok((lib, note)) => StudyMsg::Library(Ok(lib), Some(note), None),
                Err(e) => StudyMsg::Library(Err(e), None, None),
            };
            Self::study_send(&tx, m);
        });
    }

    /// Regroups the materials into topics with the model.
    pub fn study_rebuild_topics(&self) {
        let tx = self.tx.clone();
        let cache = self.study_engine.clone();
        self.rt.spawn(async move {
            let engine = study_engine(&cache).await;
            let r = async {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                let (by, note) = st::build_topics(&store, &mut lib, engine.as_ref()).await?;
                let note = note.or_else(|| {
                    Some(if by == "model" {
                        format!("{} topics, grouped by the model.", lib.topics.len())
                    } else {
                        format!(
                            "{} topics, grouped by headings (no model).",
                            lib.topics.len()
                        )
                    })
                });
                Ok::<_, String>((lib, note))
            }
            .await;
            let m = match r {
                Ok((lib, note)) => StudyMsg::Library(Ok(lib), note, None),
                Err(e) => StudyMsg::Library(Err(e), None, None),
            };
            Self::study_send(&tx, m);
        });
    }

    /// One tutor turn; the reply streams back as [`StudyMsg::TutorText`].
    pub fn study_tutor(&self, topic: String, mode: TutorMode, history: Vec<TutorTurn>) {
        let tx = self.tx.clone();
        let cache = self.study_engine.clone();
        self.rt.spawn(async move {
            let engine = study_engine(&cache).await;
            let stream_tx = tx.clone();
            let on_text = move |t: &str| {
                Self::study_send(&stream_tx, StudyMsg::TutorText(t.to_owned()));
                true
            };
            let r = async {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                let reply = st::tutor_reply(
                    &store,
                    &mut lib,
                    engine.as_ref(),
                    &topic,
                    mode,
                    &history,
                    Some(&on_text),
                )
                .await?;
                Ok::<_, String>((reply, lib))
            }
            .await;
            let m = match r {
                Ok((reply, lib)) => StudyMsg::Tutor(Ok(reply), Some(lib)),
                Err(e) => StudyMsg::Tutor(Err(e), None),
            };
            Self::study_send(&tx, m);
        });
    }

    /// Writes a quiz.
    pub fn study_quiz(&self, spec: st::QuizSpec) {
        let tx = self.tx.clone();
        let cache = self.study_engine.clone();
        self.rt.spawn(async move {
            let engine = study_engine(&cache).await;
            let r = async {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                st::make_quiz(&store, &mut lib, engine.as_ref(), &spec).await
            }
            .await;
            Self::study_send(&tx, StudyMsg::Quiz(r));
        });
    }

    /// Grades one answer.
    pub fn study_grade(&self, q: Question, given: String) {
        let tx = self.tx.clone();
        let cache = self.study_engine.clone();
        self.rt.spawn(async move {
            // Keyed questions never need the model.
            let g = match st::grade_exact(&q, &given) {
                Some(g) => g,
                None => {
                    let engine = study_engine(&cache).await;
                    st::grade(engine.as_ref(), &q, &given).await
                }
            };
            Self::study_send(&tx, StudyMsg::Graded(q.n, given, g));
        });
    }

    /// Saves a finished attempt and applies it to mastery.
    pub fn study_finish_quiz(&self, quiz: Quiz, attempt: QuizAttempt) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = (|| {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                let s = st::finish_quiz(&store, &mut lib, &quiz, attempt)?;
                Ok::<_, String>((s, lib))
            })();
            let m = match r {
                Ok((s, lib)) => StudyMsg::QuizDone(Ok(s), Some(lib)),
                Err(e) => StudyMsg::QuizDone(Err(e), None),
            };
            Self::study_send(&tx, m);
        });
    }

    /// Starts a viva and asks its first question.
    pub fn study_viva_start(&self, title: Option<String>, planned: usize) {
        let tx = self.tx.clone();
        let cache = self.study_engine.clone();
        self.rt.spawn(async move {
            let engine = study_engine(&cache).await;
            let r = async {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                let mut v = st::start_viva(&store, &mut lib, title.as_deref(), planned)?;
                let note = st::viva_question(&store, &lib, engine.as_ref(), &mut v).await?;
                Ok::<_, String>((v, note))
            }
            .await;
            let m = match r {
                Ok((v, note)) => StudyMsg::Viva(Ok(v), note),
                Err(e) => StudyMsg::Viva(Err(e), None),
            };
            Self::study_send(&tx, m);
        });
    }

    /// Records an answer, then asks the next question, or ends the viva
    /// when every question has been asked (or `end` is set).
    pub fn study_viva_answer(&self, mut viva: Viva, answer: String, seconds: u32, end: bool) {
        let tx = self.tx.clone();
        let cache = self.study_engine.clone();
        self.rt.spawn(async move {
            let engine = study_engine(&cache).await;
            if !answer.trim().is_empty() {
                viva.answer(&answer, seconds);
            }
            let r = async move {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                if end || viva.is_done() {
                    let (report, path, note) =
                        st::finish_viva(&store, &mut lib, engine.as_ref(), &mut viva).await?;
                    return Ok::<_, String>((viva, Some((report, path, lib)), note));
                }
                let note = st::viva_question(&store, &lib, engine.as_ref(), &mut viva).await?;
                Ok((viva, None, note))
            }
            .await;
            let m = match r {
                Ok((v, Some((report, path, lib)), note)) => {
                    StudyMsg::VivaReport(Ok((v, report, path)), note, Some(lib))
                }
                Ok((v, None, note)) => StudyMsg::Viva(Ok(v), note),
                Err(e) => StudyMsg::VivaReport(Err(e), None, None),
            };
            Self::study_send(&tx, m);
        });
    }

    // ---- Flashcards ----

    /// Loads the deck.
    pub fn cards_load(&self) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = StudyStore::discover().and_then(|s| cards::load_deck(&s));
            Self::study_send(&tx, StudyMsg::Deck(r, None));
        });
    }

    /// Writes cards on a topic (or all materials).
    pub fn cards_gen(&self, spec: CardSpec) {
        let tx = self.tx.clone();
        let cache = self.study_engine.clone();
        self.rt.spawn(async move {
            let engine = study_engine(&cache).await;
            let r = async {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                let g = cards::generate(&store, &mut lib, engine.as_ref(), &spec).await?;
                let deck = cards::load_deck(&store)?;
                Ok::<_, String>((g, deck))
            }
            .await;
            let m = match r {
                Ok((g, deck)) => {
                    let n = g.added.len();
                    let mut line = format!(
                        "{n} new card{} written by {}",
                        if n == 1 { "" } else { "s" },
                        if g.written_by == "model" {
                            "the model"
                        } else {
                            "Tern from your materials"
                        }
                    );
                    if g.repeats > 0 {
                        line.push_str(&format!(
                            "; {} repeat{} skipped",
                            g.repeats,
                            if g.repeats == 1 { "" } else { "s" }
                        ));
                    }
                    line.push('.');
                    if let Some(note) = g.note {
                        line = format!("{note} {line}");
                    }
                    StudyMsg::Deck(Ok(deck), Some(line))
                }
                Err(e) => StudyMsg::Deck(Err(e), None),
            };
            Self::study_send(&tx, m);
        });
    }

    /// Saves an answer and nudges the card's topic.
    pub fn cards_review(&self, card: String, rating: Rating, at: i64) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = (|| {
                let store = StudyStore::discover()?;
                let mut lib = store.load()?;
                let mut deck = cards::load_deck(&store)?;
                cards::review(&store, &mut lib, &mut deck, &card, rating, at)?;
                Ok::<_, String>(lib)
            })();
            Self::study_send(&tx, StudyMsg::Reviewed(r));
        });
    }

    /// Changes the deck: edit, suspend, or delete one card.
    pub fn cards_change(&self, change: CardChange) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = (|| {
                let store = StudyStore::discover()?;
                let mut deck = cards::load_deck(&store)?;
                let line = match change {
                    CardChange::Edit(id, front, back) => {
                        deck.edit(&id, &front, &back)?;
                        "Card saved.".to_owned()
                    }
                    CardChange::Suspend(id, on) => {
                        deck.suspend(&id, on)?;
                        if on {
                            "Card suspended; it will not come up until you bring it back.".into()
                        } else {
                            "Card back in review.".into()
                        }
                    }
                    CardChange::Delete(id) => {
                        deck.delete(&id)?;
                        "Card deleted.".into()
                    }
                };
                cards::save_deck(&store, &deck)?;
                Ok::<_, String>((deck, line))
            })();
            let m = match r {
                Ok((d, line)) => StudyMsg::Deck(Ok(d), Some(line)),
                Err(e) => StudyMsg::Deck(Err(e), None),
            };
            Self::study_send(&tx, m);
        });
    }

    /// Exports the deck (or one topic's cards).
    pub fn cards_export(&self, path: std::path::PathBuf, format: Format, topic: Option<String>) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = (|| {
                let store = StudyStore::discover()?;
                let deck = cards::load_deck(&store)?;
                let line = cards::export(&deck, topic.as_deref(), &path, format)?;
                Ok::<_, String>((deck, line))
            })();
            let m = match r {
                Ok((d, line)) => StudyMsg::Deck(Ok(d), Some(format!("Exported {line}"))),
                Err(e) => StudyMsg::Deck(Err(e), None),
            };
            Self::study_send(&tx, m);
        });
    }

    /// Imports cards from CSV or TSV.
    pub fn cards_import(&self, path: std::path::PathBuf) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = (|| {
                let store = StudyStore::discover()?;
                let mut deck = cards::load_deck(&store)?;
                let (added, skipped) = cards::import(&store, &mut deck, &path)?;
                Ok::<_, String>((deck, added, skipped))
            })();
            let m = match r {
                Ok((d, added, skipped)) => StudyMsg::Deck(
                    Ok(d),
                    Some(format!(
                        "{added} card{} imported{}.",
                        if added == 1 { "" } else { "s" },
                        if skipped > 0 {
                            format!(", {skipped} already in the deck")
                        } else {
                            String::new()
                        }
                    )),
                ),
                Err(e) => StudyMsg::Deck(Err(e), None),
            };
            Self::study_send(&tx, m);
        });
    }

    // ---- Explain in 3D ----

    /// Builds a scene for tab `key` on the runtime, never on the UI
    /// thread: a build already running for the tab is cancelled first, a
    /// cancelled build sends nothing, and a panic while building comes
    /// back as an error for the tab, not a crash.
    pub fn explain_build(&self, key: u32, ask: explain::Ask) {
        let tx = self.tx.clone();
        let cache = self.study_engine.clone();
        let cancel = explain::Cancel::new();
        if let Ok(mut jobs) = self.explain_jobs.lock()
            && let Some(old) = jobs.insert(key, cancel.clone())
        {
            old.cancel();
        }
        let jobs = self.explain_jobs.clone();
        let inner = cancel.clone();
        let job = self.rt.spawn(async move {
            let engine = study_engine(&cache).await;
            let store = StudyStore::discover()?;
            let mut lib = store.load()?;
            explain::build_with(&store, &mut lib, engine.as_ref(), &ask, &inner).await
        });
        self.rt.spawn(async move {
            let r = match job.await {
                Ok(r) => r,
                Err(e) if e.is_panic() => Err(
                    "Tern hit an internal error while building this scene. Try again, or explain a shorter answer.".into(),
                ),
                Err(_) => Err(explain::CANCELLED.into()),
            };
            if let Ok(mut j) = jobs.lock()
                && j.get(&key).is_some_and(|c| c.same(&cancel))
            {
                j.remove(&key);
            }
            if cancel.is_cancelled() {
                return;
            }
            Self::study_send(&tx, StudyMsg::Explain(key, r));
        });
    }

    /// Where tab `key`'s build is, while it runs.
    pub fn explain_stage(&self, key: u32) -> Option<explain::Stage> {
        self.explain_jobs.lock().ok()?.get(&key).map(|c| c.stage())
    }

    /// Stops the scene being built for tab `key` (its tab closed, or
    /// Cancel was pressed).
    pub fn explain_cancel(&self, key: u32) {
        if let Ok(mut jobs) = self.explain_jobs.lock()
            && let Some(c) = jobs.remove(&key)
        {
            c.cancel();
        }
    }

    /// Saves a scene to the study library (JSON and SVG), and to the vault
    /// when `vault` is set.
    pub fn explain_save(&self, key: u32, scene: Scene, vault: bool) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = (|| {
                let store = StudyStore::discover()?;
                explain::save(&store, &scene)?;
                if vault {
                    let v = tern_agent::vault::Vault::open_default()?;
                    let name = explain::save_to_vault(&v, &scene)?;
                    return Ok(format!(
                        "Saved to your study library and the vault as {name}."
                    ));
                }
                Ok::<_, String>("Saved to your study library.".into())
            })();
            Self::study_send(&tx, StudyMsg::ExplainSaved(key, r));
        });
    }

    /// Writes an SVG snapshot of the scene at `step`.
    pub fn explain_svg(&self, key: u32, scene: Scene, step: usize, path: std::path::PathBuf) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = explain::export_svg(&scene, Some(step), &path)
                .map(|()| format!("Snapshot saved to {}.", path.display()));
            Self::study_send(&tx, StudyMsg::ExplainSaved(key, r));
        });
    }
}

/// One change to the deck from the deck screen.
#[derive(Clone, Debug)]
pub enum CardChange {
    /// Card id, new front, new back.
    Edit(String, String, String),
    Suspend(String, bool),
    Delete(String),
}

/// The study model: the cached one, or the chat's provider and model
/// from the saved choices; `None` when Study is set to offline or no
/// model connects (Study then uses its own fallbacks).
/// [`Bridge::resume`]: the turns first, then the model.
async fn resume_task(tx: mpsc::UnboundedSender<Msg>, id: SessionId) {
    let (prefs, turns) =
        tokio::task::spawn_blocking(move || (Preferences::load(), tern_agent::stored_turns(id)))
            .await
            .unwrap_or_else(|_| (Preferences::default(), Err("the read failed".into())));
    if let Ok(turns) = turns {
        let _ = tx.send(Msg::Opening(id, turns));
    }
    resume_connect(&tx, &prefs, id).await;
}

/// Connects a shown chat's model: `Resumed`, or `ResumeFailed` with why.
async fn resume_connect(tx: &mpsc::UnboundedSender<Msg>, prefs: &Preferences, id: SessionId) {
    match ChatSession::resume(prefs.chat_options(), id).await {
        Ok((s, turns)) => {
            let recent = s.recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Resumed(Box::new(s), turns, recent));
        }
        Err(e) => {
            let _ = tx.send(Msg::ResumeFailed(id, e));
        }
    }
}

async fn study_engine(
    cache: &std::sync::Arc<std::sync::Mutex<Option<StudyEngine>>>,
) -> Option<StudyEngine> {
    if let Some(e) = cache.lock().ok().and_then(|c| c.clone()) {
        return Some(e);
    }
    let prefs = tokio::task::spawn_blocking(Preferences::load)
        .await
        .unwrap_or_default();
    if prefs.study_offline {
        return None;
    }
    let e = StudyEngine::open(prefs.chat_options()).await.ok()?;
    if let Ok(mut c) = cache.lock() {
        *c = Some(e.clone());
    }
    Some(e)
}

/// "Works, 12 chat models, 180 ms".
/// The Providers rows with key status, the saved choices (custom
/// providers registered first, so they are listed), and a keychain error.
fn providers_msg() -> Msg {
    let prefs = Preferences::load();
    let keychain = setup::keychain();
    let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
    let rows = rows_from(setup::list(secrets), search::list(secrets));
    Msg::Providers(rows, Box::new(prefs), keychain.err())
}

fn model_report(chat: usize, all: usize, ms: u128) -> String {
    let n = if chat > 0 { chat } else { all };
    let what = if chat > 0 { "chat model" } else { "model" };
    format!(
        "Works, {n} {what}{}, {ms} ms",
        if n == 1 { "" } else { "s" }
    )
}

/// Has `p.readers` read the pictures (first that works) and returns the
/// message with their text, or with a note for each picture none could
/// read. Says which reader did it in an info toast, and records a local
/// model's failure on its Settings row.
async fn read_pictures(tx: &mpsc::UnboundedSender<Msg>, p: &Pictures, text: &str) -> String {
    let keychain = setup::keychain();
    let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
    let doing = match p.readers.first() {
        Some(tern_agent::media::Reader::Local { describe: true, .. }) => "Looking at image…",
        Some(tern_agent::media::Reader::Local { .. }) => "Reading the text in the image…",
        _ => "Reading the image…",
    };
    let _ = tx.send(Msg::Doing(Some(doing.to_owned())));
    let out = tern_agent::media::read_with(secrets, &p.readers, &p.images, &p.chat_model).await;
    let _ = tx.send(Msg::Doing(None));
    for (r, e) in &out.failures {
        if let tern_agent::media::Reader::Local { model, .. } = r {
            let _ = tx.send(Msg::Setup(Box::new(SetupMsg::LocalRunFailed(
                model.clone(),
                e.clone(),
            ))));
        }
    }
    if !out.notice.is_empty() {
        let _ = tx.send(if out.used.is_some() {
            Msg::Info(out.notice.clone())
        } else {
            Msg::Notice(out.notice.clone())
        });
    }
    if out.text.is_empty() {
        text.to_owned()
    } else {
        format!("{text}\n\n{}", out.text)
    }
}

#[cfg(test)]
mod tests {
    use super::model_report;

    #[test]
    fn reports_read_plainly() {
        assert_eq!(model_report(12, 40, 180), "Works, 12 chat models, 180 ms");
        assert_eq!(model_report(0, 1, 9), "Works, 1 model, 9 ms");
    }
}
