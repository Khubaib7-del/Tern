//! The hidden scripted UI tour, for recording the app on screen.
//!
//! Inert unless `TERN_TOUR=1` is set or `--tour` is passed. Then Tern runs
//! in a throwaway data folder (a fresh temp `TERN_HOME`, keys kept in
//! memory only, provider key variables cleared), seeds demo material
//! (Study notes with topics and flashcards, a vault note, a run report),
//! and walks every screen on a timer, roughly in the v2 concept video's
//! scene order, holding each one a few seconds. No model is called: the
//! chat and the deep research run are fed as the scripted events, text
//! and plan a provider would send, through the same handlers real ones
//! go through. A timestamped scene log goes to stdout (`TOUR …` lines) so
//! a recording can be cut per scene. It quits at the end.
//!
//! CI records it under Xvfb (`.github/workflows/ui-tour.yml`).

use std::time::{Duration, Instant};

use gpui_kit::{Context, Window, px, size};
use serde_json::{Value, json};
use tern_agent::Harness;
use tern_core::{EffectClass, EventEnvelope, SessionId, TurnId, TurnState};

use super::Workspace;
use crate::bridge::Msg;
use crate::model::{Connection, RunView, Screen};
use crate::settings::Section;
use crate::study::StudyTab;

/// Whether this launch is a tour (`TERN_TOUR` set to anything but `0`,
/// or `--tour`).
pub(crate) fn enabled() -> bool {
    std::env::var_os("TERN_TOUR").is_some_and(|v| !v.is_empty() && v != "0")
        || std::env::args().any(|a| a == "--tour")
}

/// The tour window's size: `TERN_TOUR_SIZE` (`1024x728`), else 1600×1000,
/// the screen the Linux recording uses.
pub(crate) fn window_size() -> (f32, f32) {
    std::env::var("TERN_TOUR_SIZE")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        })
        .unwrap_or((1600., 1000.))
}

/// A tour never touches the user's data or keys: without its own
/// `TERN_TOUR_HOME`, the app runs itself again as a child with a fresh
/// temp `TERN_HOME`, keys in memory (`TERN_SECRETS=memory`) and every
/// `*_API_KEY` variable removed, and returns the child's exit code.
/// `None` when this process is the one to run (no tour, or the child).
pub(crate) fn isolate() -> Option<i32> {
    if !enabled() || std::env::var_os("TERN_TOUR_CHILD").is_some() {
        return None;
    }
    let home = std::env::var_os("TERN_TOUR_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("tern-tour-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&home);
    if let Err(e) = std::fs::create_dir_all(&home) {
        eprintln!("The tour could not make its data folder: {e}");
        return Some(1);
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("The tour could not find its own program: {e}");
            return Some(1);
        }
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(std::env::args().skip(1))
        .env("TERN_HOME", &home)
        .env("TERN_SECRETS", "memory")
        .env("TERN_TOUR", "1")
        .env("TERN_TOUR_CHILD", "1")
        .env("TERN_NO_WATCHDOG", "1");
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy().to_string();
        if k.ends_with("_API_KEY") || k.ends_with("_API_TOKEN") {
            cmd.env_remove(k);
        }
    }
    match cmd.status() {
        Ok(s) => Some(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("The tour could not start: {e}");
            Some(1)
        }
    }
}

const QUESTION: &str =
    "How do agentic RAG systems decide when to retrieve, and which retrieval policies work best?";

/// The sources the scripted run reads: (title, url, site).
const SOURCES: [(&str, &str, &str); 6] = [
    (
        "Self-RAG: Learning to Retrieve, Generate, and Critique through Self-Reflection",
        "https://arxiv.org/abs/2310.11511",
        "arxiv.org",
    ),
    (
        "Active Retrieval Augmented Generation (FLARE)",
        "https://arxiv.org/abs/2305.06983",
        "arxiv.org",
    ),
    (
        "Adaptive-RAG: Learning to Adapt Retrieval-Augmented LLMs through Question Complexity",
        "https://arxiv.org/abs/2403.14403",
        "arxiv.org",
    ),
    (
        "Corrective Retrieval Augmented Generation",
        "https://arxiv.org/abs/2401.15884",
        "arxiv.org",
    ),
    (
        "When Not to Trust Language Models: Investigating Effectiveness of Parametric and Non-Parametric Memories",
        "https://arxiv.org/abs/2212.10511",
        "arxiv.org",
    ),
    (
        "LangGraph: agentic RAG",
        "https://github.com/langchain-ai/langgraph",
        "github.com",
    ),
];

const ANSWER: &str = "**Short answer:** the strongest agentic RAG systems treat retrieval as a decision, not a default.\n\n\
1. **Decide per step.** Self-RAG trains the model to emit reflection tokens that decide when to retrieve and to critique what came back [1]. FLARE retrieves only when the sentence it is about to write contains low-confidence tokens [2].\n\
2. **Route by difficulty.** Adaptive-RAG classifies each question's complexity and routes it to no retrieval, single-step or multi-step retrieval [3].\n\
3. **Check what came back.** Corrective RAG scores retrieved documents with a lightweight evaluator and falls back to web search when they look wrong [4].\n\
4. **Retrieve for the long tail.** Retrieval helps most for less popular entities; skipping it for popular ones saves cost with little loss in accuracy [5].\n\n\
For your FYP, start with a confidence-triggered policy (FLARE-style) plus a CRAG-style evaluator, and compare both against always-retrieve on cost and accuracy.";

const REASONING: &str = "Three workers came back. Self-RAG and FLARE both make retrieval a per-step choice; Adaptive-RAG routes per question; CRAG adds a check after retrieval. The steer asked about cost, so I will end with the long-tail finding and a recommendation.";

const QUICK_Q: &str = "Which of these is cheapest to run?";
const QUICK_A: &str = "Adaptive-RAG is usually cheapest: simple questions skip retrieval entirely, so most of the saving comes from the calls it never makes [1]. FLARE costs more because it re-checks confidence while writing [2].";

const NOTES: &str = "# Retrieval policies for agentic RAG\n\n\
## When to retrieve\n\
Always-retrieve pipelines fetch passages for every question. Adaptive policies decide per question or per step. FLARE looks ahead one sentence and retrieves when the next sentence contains low-probability tokens. Self-RAG trains reflection tokens such as Retrieve, IsRel, IsSup and IsUse so the model itself decides when retrieval is needed and whether passages support the answer.\n\n\
## Query routing\n\
Adaptive-RAG trains a small classifier that predicts question complexity. Simple questions are answered without retrieval, moderate ones with a single retrieval step, and complex multi-hop questions with iterative retrieval. Routing saves cost because most questions in a workload are simple.\n\n\
## Corrective retrieval\n\
Corrective RAG (CRAG) adds a lightweight retrieval evaluator that labels retrieved documents as correct, incorrect or ambiguous. Incorrect retrieval triggers a web search; ambiguous retrieval combines both. A decompose-then-recompose step filters irrelevant strips of text.\n\n\
## The long tail\n\
Mallen et al. found that language models remember popular entities well but fail on the long tail. Retrieval helps most for less popular entities, so retrieving only when popularity is low keeps accuracy while cutting retrieval calls.\n\n\
## Evaluating a policy\n\
Compare policies on answer accuracy, retrieval calls per question, tokens and latency. Report the share of questions that skipped retrieval and the accuracy on long-tail questions separately.\n";

/// What one beat of the tour does.
#[derive(Clone, Copy, Debug)]
enum Act {
    Nothing,
    OnboardWelcome,
    Onboard(crate::onboarding::Step),
    OnboardDone,
    Connect,
    Type(&'static str),
    ClearComposer,
    DeepMode,
    StartDeep,
    Plan,
    ApprovePlan,
    WorkerStart(usize),
    Search(usize, &'static str),
    Read(usize, usize),
    WorkerDone(usize),
    RollupOpen,
    RollupDetailed,
    Steer,
    SteerApplied,
    Approval,
    ApprovalYes,
    Checking,
    Stream(usize),
    Finish,
    OpenSources,
    View(RunView),
    ShowClaim,
    SideChat,
    CloseSide,
    Pause(bool),
    Editor,
    Git,
    Library,
    Archived,
    Watches,
    Chat,
    StudyOpen,
    StudyTab(StudyTab),
    StudyMaterials,
    ReviewStart,
    ReviewFlip,
    ReviewRate,
    QuizStart,
    QuizAnswer,
    QuizNext,
    ExplainStart,
    ExplainStep,
    ExplainPlay,
    ExplainZoom,
    Research,
    Slash,
    Palette(&'static str),
    PaletteClose,
    Mention,
    TabPicker,
    Settings(Section),
    CustomizeTab(crate::extensions::CustomizeTab),
    CloseSettings,
    Theme,
    Switcher(bool),
    Tabs,
    Sidebar(bool),
    RightPanel(bool),
    Scout(Option<crate::pet::State>),
    /// Holds Scout's hover toolbar open, as if the pointer were on it.
    PetToolbar(bool),
    ModelMenu,
    EffortMenu,
    CloseMenu,
    Browser,
    CloseBrowser,
    QuickTurn,
    QuickFinish,
    Toasts,
    ToastsClear,
    Dictation,
    DictationEnd,
    ContextRing,
    Narrow(bool),
    /// Claim review: 0 starts, 1 rejects claim 3, 2 notes claim 4, 3 finishes.
    Review(u8),
    /// Shows or clears the budget card (a run paused at its token limit).
    Budget(bool),
    /// Shows or clears a proposed change to a vault note.
    VaultDiff(bool),
    /// Profiling: Mistral with a key and a 20-model list open on
    /// Settings › Providers, as on the user's screen.
    SeedModels,
    /// Profiling: wheel notches over the Settings page (down, then up),
    /// 40 ms apart, as a mouse wheel spun on Windows.
    Wheel(u8),
    /// Profiling: a line in the log between measured phases.
    Mark(&'static str),
    /// Profiling: leave parts of Settings › Providers out
    /// (`Workspace::perf_strip`).
    Strip(u8),
    Quit,
    /// Composer tour: attaches a file from the tour's folder.
    CAttach(&'static str),
    /// Composer tour: the last chip's ×.
    CRemoveChip,
    /// Composer tour: the toast for audio with no transcription model.
    CNoModelToast,
    /// Composer tour: makes sure the mock's `whisper-1` is listed.
    CSeedStt,
    /// Composer tour: Enter.
    CSend,
    /// Composer tour: remembers the open chat.
    CRemember,
    /// Composer tour: New chat.
    CNewChat,
    /// Composer tour: opens the remembered chat.
    COpenRemembered,
    /// Composer tour: types text with `{dir}` as the tour's folder.
    CTypePath(&'static str),
    /// Composer tour: picks the `@` row with this label.
    CChooseMention(&'static str),
    /// Composer tour: allows a pending tool approval, if one is asked.
    CApprove,
    /// Composer tour: a scene-log line with what the composer and chat show.
    CState(&'static str),
    /// Composer tour: the chat's search service (`wikipedia`, `searxng`).
    CSearch(&'static str),
    /// Composer tour: Settings › Speech choices (device id, threads).
    CSpeech(&'static str, Option<u32>),
    /// Composer tour: a scene-log line with the GPU probe, the speech
    /// choice in effect and what the last transcription ran on.
    CSpeechState(&'static str),
    /// Composer tour: types text key by key at the caret, as a keyboard
    /// does (not the whole value replaced, as `Type` does).
    CKeys(&'static str),
    /// Composer tour: the Left (`true`) or Right arrow key in the composer.
    CArrow(bool),
    /// Settings › Speech › Dictation language (`auto`, `en`, `ur`, ...).
    DictationLanguage(&'static str),
    /// Sets the composer's text with the caret at this byte offset, and
    /// starts a mocked recording there (#68: dictation at the caret).
    DictateFrom(&'static str, usize),
    /// A mocked dictated phrase, through the same path as the mic's.
    Dictate(&'static str),
    /// The run panel's Auto-open switch.
    RunAutoOpen(bool),
    /// #69: a picture read by a local vision model before the model is
    /// asked: `true` starts a turn with "Looking at image…" leading the
    /// working line, `false` finishes it with the local description.
    CLocalPicture(bool),
    /// #69: the read-aloud control under the last answer (mock, no
    /// audio): 0 Preparing, 1 Playing sentence 2, 2 Paused, 3 stopped.
    CReadAloud(u8),
    /// #69: the local-model banner over the composer (SmolVLM 500M timed
    /// out, SmolVLM 256M downloaded), or cleared.
    CLocalFailed(bool),
    /// #69: Auto dictation's first recording: where the route starts, a
    /// silent first window, then a window heard as English (the route
    /// logic the mic uses, without audio).
    CDictationAuto,
    /// #70: a sent message with a picture whose OCR text Tern added: the
    /// bubble shows only the typed words and the chip.
    CCleanBubble,
    /// #70: what Tern would search before answering a picture question
    /// (nothing) and a plain "search the web" question (logged).
    CPrefetchCheck,
    /// #70: the model picker on Ollama with no Ollama running (the real
    /// listing, which says "Not running" within about 2 s).
    COllamaDown,
    /// #70: how a long answer is split for read aloud (logged).
    CReadAloudPieces,
    /// #71: Stop, as the stop button.
    CStop,
    /// #71: asks the open side chat.
    CSideAsk(&'static str),
    /// #71: a converting chip for `name` at page `done` of `total` (0
    /// clears it), as Granite-Docling reading a scan reports it.
    CProgress(&'static str, usize, usize),
    /// #73: clicks the newest block control whose id starts with the first
    /// text and ends with the second (`blockexport::tour_click`): a
    /// Rendered | LaTeX or Diagram | Code half, Export ▾, a Save row.
    CClick(&'static str, &'static str),
    /// #73: the Down arrow in the composer (the @ menu's next row).
    CDown,
    /// #73: copies the files the tour saved from blocks into `exports/`
    /// beside the scene log, so the workflow uploads them.
    CCollectExports,
}

/// One beat: an optional new scene (id, the v2 scene it maps to or
/// `tern-only`, a title), what to do, and how long to hold after.
struct Beat {
    scene: Option<(&'static str, &'static str, &'static str)>,
    act: Act,
    hold_ms: u64,
}

fn b(act: Act, hold_ms: u64) -> Beat {
    Beat {
        scene: None,
        act,
        hold_ms,
    }
}

fn scene(id: &'static str, v2: &'static str, title: &'static str, act: Act, hold_ms: u64) -> Beat {
    Beat {
        scene: Some((id, v2, title)),
        act,
        hold_ms,
    }
}

/// The frame-cost run (`TERN_TOUR_ONLY=perf`): Settings › Providers
/// (with a provider's model list open) against Settings › Tools, each
/// idle, through two theme wipes and a wheel spin down and up. The frame
/// meter's `PERF` lines (`crate::perf`) say what each costs.
fn perf_script() -> Vec<Beat> {
    let mut v = vec![b(Act::Nothing, 1500)];
    for (id, s) in [
        ("perf-providers", Section::Providers),
        ("perf-tools", Section::Tools),
        ("perf-providers-again", Section::Providers),
    ] {
        v.push(scene(id, "tern-only", id, Act::Settings(s), 1500));
        if s == Section::Providers {
            v.push(b(Act::SeedModels, 500));
        }
        v.push(b(Act::Mark("idle"), 4200));
        v.push(b(Act::Mark("theme"), 0));
        v.push(b(Act::Theme, 1500));
        v.push(b(Act::Theme, 2500));
        v.push(b(Act::Mark("wheel"), 0));
        v.push(b(Act::Wheel(24), 4500));
        v.push(b(Act::Mark("done"), 2100));
    }
    // What Providers' frames cost without each part: 1 logos, 2 the open
    // model list, 4 the row buttons, 7 all three.
    for (mark, mask) in [
        ("strip-logos", 1),
        ("strip-list", 2),
        ("strip-buttons", 4),
        ("strip-all", 7),
        ("strip-none", 0),
    ] {
        v.push(b(Act::Strip(mask), 300));
        v.push(b(Act::Mark(mark), 0));
        v.push(b(Act::Wheel(24), 4500));
        v.push(b(Act::Mark("done"), 2100));
    }
    v.push(b(Act::Quit, 0));
    v
}

/// The whole tour, in order.
fn script() -> Vec<Beat> {
    if std::env::var("TERN_TOUR_ONLY").as_deref() == Ok("perf") {
        return perf_script();
    }
    if composer_tour() {
        return composer_script();
    }
    use crate::onboarding::Step;
    let mut v = vec![
        b(Act::Nothing, 1500),
        scene(
            "t01-splash",
            "s01",
            "Splash: Tern mark on the welcome page",
            Act::OnboardWelcome,
            3500,
        ),
        scene(
            "t02-onboard-provider",
            "s02",
            "Onboarding: pick a provider",
            Act::Onboard(Step::Provider),
            3500,
        ),
        scene(
            "t03-onboard-key",
            "s02",
            "Onboarding: API key and live test",
            Act::Onboard(Step::Key),
            3500,
        ),
        scene(
            "t04-onboard-model",
            "s02",
            "Onboarding: default model",
            Act::Onboard(Step::Model),
            3500,
        ),
        scene(
            "t05-onboard-search",
            "s02",
            "Onboarding: where searches go",
            Act::Onboard(Step::Search),
            3000,
        ),
        scene(
            "t06-onboard-theme",
            "s02",
            "Onboarding: pick a look",
            Act::Onboard(Step::Theme),
            3000,
        ),
        scene(
            "t07-empty-chat",
            "s03",
            "Empty chat, composer, sidebar",
            Act::OnboardDone,
            400,
        ),
        b(Act::Connect, 3500),
        scene(
            "t08-plan-card",
            "s04",
            "Deep research: typing the question",
            Act::DeepMode,
            600,
        ),
        b(Act::Type(QUESTION), 1800),
        b(Act::StartDeep, 900),
        scene(
            "t09-plan-card",
            "s04",
            "Plan card and plan in the right rail",
            Act::Plan,
            4500,
        ),
        scene(
            "t10-run",
            "s05",
            "Run: workers, searches, reads",
            Act::ApprovePlan,
            700,
        ),
        b(Act::WorkerStart(1), 300),
        b(Act::WorkerStart(2), 300),
        b(Act::WorkerStart(3), 500),
        b(
            Act::Search(1, "adaptive retrieval when to retrieve LLM"),
            600,
        ),
        b(Act::Search(2, "query complexity routing RAG"), 600),
        b(Act::Read(1, 0), 600),
        b(
            Act::Search(3, "corrective retrieval evaluator web search fallback"),
            600,
        ),
        b(Act::Read(2, 2), 600),
        b(Act::Read(1, 1), 600),
        b(Act::Read(3, 3), 900),
        scene(
            "t11-rollup",
            "s05",
            "Live rollup, Summary then Detailed",
            Act::RollupOpen,
            2000,
        ),
        b(Act::RollupDetailed, 2500),
        scene(
            "t12-steering",
            "s06",
            "Steering a running research",
            Act::Type("add: How much do adaptive policies save in tokens?"),
            1500,
        ),
        b(Act::Steer, 2000),
        b(Act::SteerApplied, 700),
        b(Act::WorkerStart(4), 400),
        b(Act::Read(4, 4), 1600),
        scene(
            "t13-needs-you",
            "s07",
            "Needs you: approval card",
            Act::Approval,
            4000,
        ),
        b(Act::ApprovalYes, 600),
        scene(
            "t14-budget-pause",
            "s11",
            "Paused run (budget pause stand-in)",
            Act::Pause(true),
            3000,
        ),
        b(Act::Pause(false), 400),
        b(Act::WorkerDone(1), 300),
        b(Act::WorkerDone(2), 300),
        b(Act::WorkerDone(3), 300),
        b(Act::WorkerDone(4), 600),
        scene(
            "t15-checking",
            "s08",
            "Checking claims, answer streaming",
            Act::Checking,
            1200,
        ),
    ];
    for i in 0..8 {
        v.push(b(Act::Stream(i), 350));
    }
    v.extend([
        b(Act::Finish, 2500),
        scene(
            "t16-citations",
            "s08",
            "Report with citations and sources",
            Act::OpenSources,
            3500,
        ),
        scene(
            "t17-claim-review",
            "s09",
            "Claim verdicts in the Report view",
            Act::View(RunView::Report),
            1500,
        ),
        b(Act::ShowClaim, 3000),
        scene(
            "t17b-review-each",
            "s09",
            "Review claims: keep, reject, comment",
            Act::Review(0),
            1500,
        ),
        b(Act::Review(1), 1500),
        b(Act::Review(2), 2500),
        scene(
            "t17c-reviewed",
            "s09",
            "Review done: report updated toast, note in the composer",
            Act::Review(3),
            3000,
        ),
        scene(
            "t17d-budget-card",
            "s11",
            "Paused at a safe point: raise the limit and resume",
            Act::Budget(true),
            3500,
        ),
        b(Act::Budget(false), 300),
        scene(
            "t18-panel-summary",
            "s05",
            "Right panel: Summary",
            Act::View(RunView::Summary),
            2200,
        ),
        scene(
            "t19-panel-workers",
            "s05",
            "Right panel: Workers",
            Act::View(RunView::Workers),
            2200,
        ),
        scene(
            "t20-panel-detailed",
            "s05",
            "Right panel: Detailed",
            Act::View(RunView::Detailed),
            2200,
        ),
        scene(
            "t21-panel-raw",
            "tern-only",
            "Right panel: Raw events",
            Act::View(RunView::Raw),
            2200,
        ),
        scene(
            "t22-panel-ledger",
            "tern-only",
            "Right panel: Ledger",
            Act::View(RunView::Ledger),
            2200,
        ),
        scene(
            "t23-graph",
            "s12",
            "Graph: run map",
            Act::View(RunView::Graph),
            4500,
        ),
        scene(
            "t23b-context",
            "s05",
            "Right rail: Context",
            Act::View(RunView::Context),
            2200,
        ),
        scene(
            "t24-side-chat",
            "s10",
            "Side chat about this chat",
            Act::SideChat,
            3500,
        ),
        b(Act::CloseSide, 300),
        b(Act::View(RunView::Summary), 300),
        scene(
            "t25-timeline",
            "tern-only",
            "Second question; timeline rail",
            Act::QuickTurn,
            1500,
        ),
        b(Act::QuickFinish, 3000),
        // The Graph tab is built from the run it shows: the quick turn's
        // map (one page read, its own key terms) is not the deep run's.
        scene(
            "t25b-graph-changes",
            "s12",
            "Graph: a different run draws a different map",
            Act::View(RunView::Graph),
            3500,
        ),
        b(Act::View(RunView::Summary), 300),
        scene(
            "t26-theme-light",
            "s17",
            "Theme wipe dark to light",
            Act::Theme,
            3500,
        ),
        scene(
            "t27-theme-light-hold",
            "s17",
            "Light theme",
            Act::Nothing,
            2000,
        ),
        scene(
            "t28-theme-dark",
            "s17",
            "Theme wipe light to dark",
            Act::Theme,
            3000,
        ),
        scene(
            "t29-switcher",
            "s14",
            "Harness switcher",
            Act::Switcher(true),
            3000,
        ),
        b(Act::Switcher(false), 300),
        scene(
            "t30-study-home",
            "s14",
            "Study home: materials and topics",
            Act::StudyOpen,
            1500,
        ),
        b(Act::StudyMaterials, 3500),
        scene(
            "t31-flashcards",
            "s14",
            "Flashcards: review",
            Act::StudyTab(StudyTab::Cards),
            1200,
        ),
        b(Act::ReviewStart, 2500),
        scene(
            "t32-flashcard-flip",
            "s14",
            "Flashcards: flip and rate",
            Act::ReviewFlip,
            2500,
        ),
        b(Act::ReviewRate, 2000),
        scene(
            "t33-quiz",
            "s14",
            "Quiz",
            Act::StudyTab(StudyTab::Quiz),
            800,
        ),
        b(Act::QuizStart, 3500),
        scene(
            "t34-quiz-graded",
            "s14",
            "Quiz: answer and grade",
            Act::QuizAnswer,
            3500,
        ),
        b(Act::QuizNext, 2000),
        scene(
            "t35-viva",
            "s14",
            "Viva",
            Act::StudyTab(StudyTab::Viva),
            3000,
        ),
        scene(
            "t36-explain3d",
            "s14",
            "Explain in 3D: building and scene",
            Act::ExplainStart,
            6000,
        ),
        scene(
            "t37-explain-step",
            "s14",
            "Explain in 3D: next step",
            Act::ExplainStep,
            3000,
        ),
        scene(
            "t38-explain-play",
            "s14",
            "Explain in 3D: play",
            Act::ExplainPlay,
            5000,
        ),
        b(Act::ExplainPlay, 300),
        scene(
            "t39-explain-zoom",
            "s14",
            "Explain in 3D: zoom",
            Act::ExplainZoom,
            3500,
        ),
        scene(
            "t40-study-chat",
            "s14",
            "Study chat tab",
            Act::StudyTab(StudyTab::Chat),
            2500,
        ),
        b(Act::Research, 1500),
        scene(
            "t41-palette",
            "s15",
            "Command palette (Ctrl+K)",
            Act::Palette("rail"),
            3000,
        ),
        b(Act::PaletteClose, 300),
        scene(
            "t41b-slash",
            "tern-only",
            "/ command menu",
            Act::Slash,
            2500,
        ),
        scene(
            "t42-mention",
            "tern-only",
            "@ reference menu",
            Act::Mention,
            3000,
        ),
        b(Act::ClearComposer, 300),
        scene(
            "t43-tab-picker",
            "s15",
            "Tab picker (closest to the palette)",
            Act::TabPicker,
            3000,
        ),
        b(Act::TabPicker, 300),
        scene("t44-chat-tabs", "tern-only", "Chat tabs", Act::Tabs, 3000),
        scene(
            "t45-sidebar-closed",
            "shell",
            "Sidebar hidden",
            Act::Sidebar(false),
            2000,
        ),
        b(Act::Sidebar(true), 1500),
        scene(
            "t46-right-closed",
            "shell",
            "Right panel hidden",
            Act::RightPanel(false),
            2000,
        ),
        b(Act::RightPanel(true), 1200),
        // #68: the run panel opens by itself only for deep research and
        // only with Auto-open on; the switch is in its header.
        scene(
            "t46b-run-auto-open-off",
            "tern-only",
            "Run panel header: Auto-open off (it stays closed until opened)",
            Act::RunAutoOpen(false),
            2500,
        ),
        b(Act::RunAutoOpen(true), 800),
        scene(
            "t47-model-picker",
            "tern-only",
            "Model picker and catalogue",
            Act::ModelMenu,
            3500,
        ),
        b(Act::CloseMenu, 300),
        scene(
            "t48-effort",
            "tern-only",
            "Effort slider",
            Act::EffortMenu,
            3000,
        ),
        b(Act::CloseMenu, 300),
        scene("t49-toasts", "tern-only", "Toasts", Act::Toasts, 3500),
        b(Act::ToastsClear, 600),
        scene(
            "t50-dictation",
            "tern-only",
            "Dictation UI (interim text)",
            Act::Dictation,
            3000,
        ),
        b(Act::DictationEnd, 300),
        // #68: Settings › Speech › Dictation language, then dictated
        // phrases go in at the caret and the caret follows them.
        b(Act::Settings(Section::Speech), 1200),
        scene(
            "t50b-dictation-language",
            "tern-only",
            "Settings › Speech › Dictation language: Urdu",
            Act::DictationLanguage("ur"),
            2500,
        ),
        b(Act::DictationLanguage("auto"), 600),
        b(Act::CloseSettings, 800),
        scene(
            "t50c-dictation-caret",
            "tern-only",
            "Dictation at the caret: phrases go in between hello and world, the caret follows",
            Act::DictateFrom("hello world", 5),
            1500,
        ),
        b(Act::Dictate("compare retrieval"), 900),
        b(Act::Dictate("policies by cost"), 900),
        b(Act::Dictate("per question"), 2500),
        b(Act::ClearComposer, 300),
        scene(
            "t51-context-ring",
            "tern-only",
            "Context ring",
            Act::ContextRing,
            3000,
        ),
        scene(
            "t52-browser",
            "tern-only",
            "Built-in browser",
            Act::Browser,
            6000,
        ),
        b(Act::CloseBrowser, 400),
        scene("t53-vault", "s13", "Vault editor", Act::Editor, 3500),
        scene(
            "t53b-vault-diff",
            "s13",
            "Vault diff: a change to a note waits for Accept or Reject",
            Act::VaultDiff(true),
            3500,
        ),
        b(Act::VaultDiff(false), 300),
        scene("t54-git", "tern-only", "Git panel", Act::Git, 3000),
        scene("t55-library", "tern-only", "Library", Act::Library, 3000),
        scene("t56-watches", "tern-only", "Watches", Act::Watches, 2500),
        scene(
            "t57-archived",
            "tern-only",
            "Archived chats",
            Act::Archived,
            2500,
        ),
        b(Act::Chat, 500),
    ]);
    let pages: [(&'static str, &'static str, &'static str, Section); 14] = [
        (
            "t58-settings-providers",
            "s16",
            "Settings: Providers",
            Section::Providers,
        ),
        (
            "t59-settings-models",
            "s16",
            "Settings: Models and catalogue",
            Section::Models,
        ),
        (
            "t59b-settings-local-models",
            "tern-only",
            "Settings: Local models",
            Section::LocalModels,
        ),
        (
            "t59c-settings-speech",
            "tern-only",
            "Settings: Speech (device and CPU threads)",
            Section::Speech,
        ),
        (
            "t60-settings-tools",
            "tern-only",
            "Settings: Tools",
            Section::Tools,
        ),
        (
            "t61-settings-decision",
            "s16",
            "Settings: Decision model",
            Section::Decision,
        ),
        (
            "t62-settings-eval",
            "tern-only",
            "Settings: Evaluation",
            Section::Evaluation,
        ),
        (
            "t63-settings-mcp-skills",
            "tern-only",
            "Settings: MCP and skills",
            Section::Extensions,
        ),
        (
            "t64-settings-instructions",
            "tern-only",
            "Settings: Instructions",
            Section::Instructions,
        ),
        (
            "t65-settings-search",
            "tern-only",
            "Settings: Search and crawlers",
            Section::Search,
        ),
        (
            "t66-settings-appearance",
            "s16",
            "Settings: Appearance",
            Section::Appearance,
        ),
        (
            "t67-settings-study",
            "tern-only",
            "Settings: Study",
            Section::Study,
        ),
        (
            "t68-settings-privacy",
            "s16",
            "Settings: Privacy",
            Section::Privacy,
        ),
        (
            "t69-settings-about",
            "s16",
            "Settings: About",
            Section::About,
        ),
    ];
    for (id, v2, title, s) in pages {
        v.push(scene(id, v2, title, Act::Settings(s), 3000));
        if s == Section::Extensions {
            v.push(b(
                Act::CustomizeTab(crate::extensions::CustomizeTab::Mcp),
                2000,
            ));
        }
    }
    // Settings › Scout: every sprite state on the preview card (and on
    // the floating pet), then each state again over the chat.
    v.push(scene(
        "t70-scout-settings",
        "s99",
        "Settings: Scout (every state)",
        Act::Settings(Section::Scout),
        800,
    ));
    for st in crate::pet::State::ALL {
        v.push(b(Act::Scout(Some(st)), 1300));
    }
    v.push(b(Act::CloseSettings, 300));
    v.push(scene(
        "t70b-scout-pet",
        "s99",
        "Scout pet over the chat (every state)",
        Act::Nothing,
        300,
    ));
    for st in crate::pet::State::ALL {
        v.push(b(Act::Scout(Some(st)), 1300));
    }
    v.extend([
        b(Act::Scout(None), 1500),
        scene(
            "t70c-scout-toolbar",
            "tern-only",
            "Scout: the hover toolbar (stays while the pointer crosses to it)",
            Act::PetToolbar(true),
            2500,
        ),
        b(Act::PetToolbar(false), 300),
        scene(
            "t71-narrow",
            "tern-only",
            "Narrow window (900 px)",
            Act::Narrow(true),
            3500,
        ),
        b(Act::Narrow(false), 1500),
        scene("t72-outro", "s18", "End", Act::Nothing, 2500),
        b(Act::Quit, 0),
    ]);
    v
}

/// The tour's state while it runs.
#[derive(Debug)]
pub(super) struct Tour {
    started: Instant,
    /// The demo Research chat, so the scripted events have a session.
    chat: SessionId,
    /// Chats listed in the sidebar and opened as tabs.
    others: Vec<(SessionId, &'static str)>,
    /// Keep the window connected (a failed launch is ignored).
    connected: bool,
    /// Text of the answer streamed so far.
    streamed: String,
    /// The Research transcript, put back after the Study visit (a harness
    /// switch opens a fresh chat, and the tour has no stored one).
    saved: Option<(Vec<crate::model::Entry>, tern_agent::Run)>,
    /// The composer tour's first chat, to switch back to.
    remembered: Option<SessionId>,
    _task: Option<gpui_kit::Task<()>>,
}

impl Tour {
    pub(super) fn new() -> Self {
        Self {
            started: Instant::now(),
            chat: SessionId::new(),
            others: vec![
                (SessionId::new(), "Chunking strategies for long PDFs"),
                (SessionId::new(), "Reranker comparison: Cohere vs BGE"),
                (SessionId::new(), "FYP literature map"),
            ],
            connected: false,
            streamed: String::new(),
            saved: None,
            remembered: None,
            _task: None,
        }
    }
}

/// Seeds the throwaway data folder before the window opens: preferences,
/// Study notes with topics and flashcards, and a vault note.
pub(crate) fn seed() {
    if !enabled() {
        return;
    }
    let mut prefs = tern_agent::Preferences::load();
    prefs.onboarded = true;
    prefs.study_offline = true;
    prefs.reduced_motion = false;
    prefs.deep_workers = Some(3);
    if composer_tour() {
        seed_composer(&mut prefs);
    }
    if let Err(e) = prefs.save() {
        eprintln!("tour: preferences not saved: {e}");
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tour: no runtime to seed with: {e}");
            return;
        }
    };
    let seeded: Result<(), String> = rt.block_on(async {
        use tern_agent::study;
        let store = study::StudyStore::discover()?;
        let mut lib = store.load()?;
        study::add_text(
            &store,
            &mut lib,
            "Retrieval policies.md",
            study::MaterialKind::Note,
            "tour://retrieval-policies.md",
            NOTES,
        )?;
        study::build_topics(&store, &mut lib, None).await?;
        let ratings = [0.82_f32, 0.55, 0.34, 0.68, 0.2];
        for (t, r) in lib.topics.clone().iter().zip(ratings) {
            let m = lib.mastery.entry(t.id.clone()).or_default();
            m.rating = r;
            m.attempts = 4;
        }
        store.save(&lib)?;
        let spec = tern_agent::cards::CardSpec {
            count: 10,
            ..Default::default()
        };
        tern_agent::cards::generate(&store, &mut lib, None, &spec).await?;
        Ok(())
    });
    if let Err(e) = seeded {
        eprintln!("tour: study material not seeded: {e}");
    }
    if let Ok(v) = tern_agent::vault::Vault::open_default() {
        let _ = v.write(
            "Retrieval policies",
            "# Retrieval policies\n\nNotes for the FYP chapter on agentic RAG.\n\n- Always-retrieve baseline\n- FLARE: retrieve on low confidence\n- Self-RAG: reflection tokens\n- Adaptive-RAG: route by complexity\n- CRAG: evaluate, then fall back to web search\n\nSee [[Evaluation plan]].\n",
        );
        let _ = v.write(
            "Evaluation plan",
            "# Evaluation plan\n\nAccuracy, retrieval calls per question, tokens, latency. Long-tail questions reported separately.\n",
        );
    }
}

fn page(i: usize) -> Value {
    let (title, url, site) = SOURCES[i];
    json!({"title": title, "url": url, "site": site})
}

fn plan_json() -> Value {
    json!({
        "question": QUESTION,
        "sub_questions": [
            {"n": 1, "question": "When do adaptive RAG methods decide to retrieve?",
             "intent": "Find the triggers (confidence, reflection tokens) papers use.",
             "queries": ["adaptive retrieval when to retrieve LLM"], "sources": ["papers"], "share": 34},
            {"n": 2, "question": "How do systems route questions by difficulty?",
             "intent": "Compare classifiers that pick no, single or multi-step retrieval.",
             "queries": ["query complexity routing RAG"], "sources": ["papers"], "share": 33},
            {"n": 3, "question": "How do they check and correct what was retrieved?",
             "intent": "Evaluators and fallbacks after retrieval.",
             "queries": ["corrective retrieval evaluator web search fallback"], "sources": ["papers", "docs"], "share": 33}
        ],
        "budget": {"tokens": 240000, "pages": 24, "seconds": 600},
        "source": "model"
    })
}

fn claims_json(report: &std::path::Path) -> Value {
    let c = |n: usize, text: &str, src: usize, verdict: &str, quote: &str, overlap: u8| {
        let (title, url, site) = SOURCES[src - 1];
        json!({
            "claim": n, "text": text, "citations": [src], "verdict": verdict,
            "evidence": {"source": src, "url": url, "title": title, "site": site,
                         "quote": quote, "anchor": format!("#claim-{n}"), "page": null},
            "overlap": overlap, "method": "model", "note": ""
        })
    };
    json!({
        "checks": [
            c(1, "Self-RAG trains the model to emit reflection tokens that decide when to retrieve and to critique what came back.", 1, "supported", "Self-RAG ... learns to retrieve, generate and critique ... using special tokens, called reflection tokens.", 81),
            c(2, "FLARE retrieves only when the sentence it is about to write contains low-confidence tokens.", 2, "supported", "If the upcoming sentence contains low-confidence tokens, we retrieve relevant documents ...", 77),
            c(3, "Adaptive-RAG routes each question to no retrieval, single-step or multi-step retrieval by its complexity.", 3, "supported", "... dynamically selects the most suitable strategy ... from the simplest to the most sophisticated ones based on the query complexity.", 72),
            c(4, "Corrective RAG falls back to web search when retrieved documents look wrong.", 4, "partly", "... large-scale web searches are utilized as an extension for augmenting the retrieval results.", 58),
            c(5, "Skipping retrieval for popular entities saves cost with little loss in accuracy.", 5, "partly", "... retrieve non-parametric memories only when necessary ... improves performance while reducing inference costs.", 55)
        ],
        "report": {"markdown": report, "json": report.with_extension("json")}
    })
}

impl Workspace {
    /// Starts the tour's timer. Called once from `Workspace::new` when
    /// the tour is on.
    pub(super) fn start_tour(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let beats = script();
        log(&format!(
            "TOUR start unix_ms={} beats={} v2_order=s01..s18",
            unix_ms(),
            beats.len()
        ));
        let task = cx.spawn_in(window, async move |this, cx| {
            for beat in &beats {
                let go = this.update_in(cx, |this, window, cx| {
                    this.tour_beat(beat, window, cx);
                });
                if go.is_err() {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(beat.hold_ms))
                    .await;
            }
        });
        if let Some(t) = self.tour.as_mut() {
            t._task = Some(task);
        }
    }

    /// Messages a tour leaves out: the throwaway home has no provider, so
    /// a launch's connection result would undo the scripted screen.
    pub(super) fn tour_drops(&self, msg: &Msg) -> bool {
        // The composer tour runs a real chat against the mock model.
        self.tour.is_some()
            && !composer_tour()
            && matches!(
                msg,
                Msg::Connected(..)
                    | Msg::Resumed(..)
                    | Msg::Swapped(..)
                    | Msg::ConnectFailed(_)
                    | Msg::ResumeFailed(..)
                    | Msg::Notice(_)
                    | Msg::Opening(..)
                    | Msg::Sessions(_)
                    | Msg::Done(..)
            )
    }

    fn tour_beat(&mut self, beat: &Beat, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.tour.as_ref() else {
            return;
        };
        if let Some((id, v2, title)) = beat.scene {
            log(&format!(
                "TOUR scene unix_ms={} t={:.2} id={id} v2={v2} title={title}",
                unix_ms(),
                t.started.elapsed().as_secs_f32()
            ));
        }
        // A harness switch drops the connection; the tour keeps one.
        if t.connected
            && matches!(
                self.model.phase,
                crate::model::Phase::Connecting | crate::model::Phase::Offline(_)
            )
        {
            self.tour_connect();
        }
        let act = beat.act;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.tour_act(act, window, cx)
        }));
        if r.is_err() {
            log(&format!("TOUR error act={act:?} panicked; continuing"));
        }
        super::frame::next_frame(window, cx);
        cx.notify();
    }

    fn tour_connect(&mut self) {
        self.model.connected(Connection {
            provider: "Anthropic".into(),
            model: "claude-sonnet-4-5".into(),
            search: Some("Tavily".into()),
            context_window: Some(200_000),
        });
    }

    fn tour_event(
        &mut self,
        kind: &str,
        summary: &str,
        payload: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let chat = self.tour.as_ref().map(|t| t.chat).unwrap_or_default();
        let effect = match kind {
            "tool.executed" | "tool.started" => EffectClass::Network,
            _ => EffectClass::Analysis,
        };
        let e = EventEnvelope::new(chat, kind, effect, summary, payload);
        self.on_msg(Msg::Event(Box::new(e)), window, cx);
    }

    fn set_composer(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_owned();
        self.composer.update(cx, |c, cx| {
            crate::view::set_end(c, text.clone(), window, cx)
        });
        self.model.chat_here = self.current;
        self.model.on_composer_text(&text);
    }

    fn tour_act(&mut self, act: Act, window: &mut Window, cx: &mut Context<Self>) {
        use crate::onboarding::{KeyTest, Onboarding, Step};
        match act {
            Act::Nothing => {}
            Act::OnboardWelcome => {
                self.onboarding = Some(Onboarding::new());
            }
            Act::Onboard(step) => {
                let model_row = self
                    .model
                    .providers
                    .rows
                    .iter()
                    .filter(|r| r.kind == crate::providers::Kind::Model)
                    .find(|r| r.id == "anthropic")
                    .or_else(|| {
                        self.model
                            .providers
                            .rows
                            .iter()
                            .find(|r| r.kind == crate::providers::Kind::Model)
                    })
                    .map(|r| r.id);
                let search_row = self
                    .model
                    .providers
                    .rows
                    .iter()
                    .find(|r| r.kind == crate::providers::Kind::Search)
                    .map(|r| r.id);
                let o = self.onboarding.get_or_insert_with(Onboarding::new);
                if let Some(id) = model_row {
                    o.pick_provider(id, true, false);
                }
                if matches!(step, Step::Key | Step::Model | Step::Search | Step::Theme) {
                    o.set_test(KeyTest::Passed("Connected · 14 models".into()));
                }
                if matches!(step, Step::Model | Step::Search | Step::Theme) {
                    o.model = Some("claude-sonnet-4-5".into());
                }
                if matches!(step, Step::Search | Step::Theme) {
                    o.search = search_row;
                }
                o.direction = 1;
                o.step = step;
                if step == Step::Key {
                    // A visibly fake value in the masked field; never saved.
                    self.key_input.update(cx, |i, cx| {
                        i.set_value("sk-tour-not-a-real-key-0000", window, cx)
                    });
                }
            }
            Act::OnboardDone => {
                self.onboarding = None;
                self.key_input.update(cx, |i, cx| i.set_value("", window, cx));
            }
            Act::Connect => {
                let Some(t) = self.tour.as_mut() else {
                    return;
                };
                t.connected = true;
                let chat = t.chat;
                let others = t.others.clone();
                self.tour_connect();
                let mut list = vec![summary(chat, "Agentic RAG: retrieval policies", Harness::Research)];
                list.extend(others.iter().map(|(id, title)| summary(*id, title, Harness::Research)));
                list.push(summary(SessionId::new(), "Operating systems: paging", Harness::Study));
                self.model.set_sessions(list);
                self.current = Some(chat);
                self.tabs.set_active(chat, "Agentic RAG: retrieval policies");
                self.model.sidebar_open = true;
            }
            Act::Type(text) => self.set_composer(text, window, cx),
            Act::ClearComposer => self.set_composer("", window, cx),
            Act::DeepMode => {
                self.model.set_mode(crate::composer::Mode::Deep, None);
            }
            Act::StartDeep => {
                let text = self.composer.read(cx).value().to_string();
                if self.model.start_turn(&text).is_some() {
                    self.set_composer("", window, cx);
                    self.start_ticker(cx);
                    self.tour_event("turn.started", "Started deep research", json!({"mode": "deep", "workers": 3}), window, cx);
                    self.on_msg(
                        Msg::Reasoning(1, "Splitting the question into three parts a worker each can answer from papers.".into()),
                        window,
                        cx,
                    );
                }
            }
            Act::Plan => {
                let plan = plan_json();
                self.tour_event(
                    "plan.drafted",
                    "Drafted a plan with 3 sub-questions",
                    json!({"plan": plan, "estimate": {"low": 90000, "high": 160000}}),
                    window,
                    cx,
                );
                if let Ok(p) = serde_json::from_value::<tern_agent::ResearchPlan>(plan) {
                    let (tx, _rx) = tokio::sync::oneshot::channel();
                    self.on_msg(Msg::Plan(p, tx), window, cx);
                }
            }
            Act::ApprovePlan => {
                if let Some((plan, _)) = self.model.pending_plan() {
                    let plan = plan.clone();
                    self.answer_plan(tern_agent::PlanAnswer::Run(plan), cx);
                }
                self.tour_event("plan.answered", "Plan approved", json!({"answer": "run", "plan": plan_json()}), window, cx);
            }
            Act::WorkerStart(n) => {
                if n == 4 {
                    // Added by the steer: a fourth sub-question.
                }
                self.tour_event(
                    "worker.started",
                    &format!("Worker {n} started"),
                    json!({"worker": n, "budget": {"tokens": 60000, "pages": 6, "seconds": 150}}),
                    window,
                    cx,
                );
            }
            Act::Search(n, q) => {
                self.tour_event("tool.started", "Searching", json!({"tool": "web_search", "worker": n, "args": {"query": q}}), window, cx);
                self.tour_event(
                    "tool.executed",
                    &format!("Searched “{q}”"),
                    json!({"tool": "web_search", "worker": n, "args": {"query": q}, "meta": {"results": 8}}),
                    window,
                    cx,
                );
            }
            Act::Read(n, i) => {
                let url = SOURCES[i].1;
                self.tour_event("tool.started", "Reading", json!({"tool": "read_url", "worker": n, "args": {"url": url}}), window, cx);
                self.tour_event(
                    "tool.executed",
                    &format!("Read {}", SOURCES[i].2),
                    json!({"tool": "read_url", "worker": n, "args": {"url": url}, "meta": {"page": page(i)}}),
                    window,
                    cx,
                );
                self.request_icons();
            }
            Act::WorkerDone(n) => {
                self.tour_event(
                    "worker.finished",
                    &format!("Worker {n} finished"),
                    json!({"worker": n, "status": "done", "note": "", "took_ms": 21000 + n as u64 * 3700}),
                    window,
                    cx,
                );
            }
            Act::RollupOpen => self.model.live_rollup_open = true,
            Act::RollupDetailed => self.model.rollup_view = crate::chatflow::RollupView::Detailed,
            Act::Steer => {
                let text = "add: How much do adaptive policies save in tokens?";
                self.set_composer("", window, cx);
                self.tour_event("steer.received", "Steering note received", json!({"text": text}), window, cx);
                let what = tern_agent::steer::parse_steer(text).describe();
                self.model.toast_info(format!(
                    "Steer sent: {what}. It lands at the run's next safe point."
                ));
            }
            Act::SteerApplied => {
                self.model.rollup_view = crate::chatflow::RollupView::Summary;
                self.tour_event(
                    "steer.applied",
                    "Added sub-question 4",
                    json!({"text": "add: How much do adaptive policies save in tokens?", "applied": true,
                           "added": {"n": 4, "question": "How much do adaptive policies save in tokens?",
                                     "budget": {"tokens": 40000, "pages": 4, "seconds": 100}}}),
                    window,
                    cx,
                );
            }
            Act::Approval => {
                let req = tern_agent::ApprovalRequest {
                    tool: "vault_write".into(),
                    summary: "Save the report to your vault as “Retrieval policies – sources.md”".into(),
                    effect: EffectClass::Write,
                    target: Some("vault/Retrieval policies – sources.md".into()),
                    args: json!({"note": "Retrieval policies – sources"}),
                };
                self.tour_event("approval.requested", "Asked to write to the vault", json!({"tool": "vault_write"}), window, cx);
                let (tx, _rx) = tokio::sync::oneshot::channel();
                self.on_msg(Msg::Approval(req, tx), window, cx);
            }
            Act::ApprovalYes => {
                self.pending = None;
                self.model.approval_answered();
                self.tour_event("approval.answered", "Allowed once", json!({"tool": "vault_write", "answer": "allow"}), window, cx);
            }
            Act::Pause(on) => self.model.paused = on,
            Act::Checking => {
                self.tour_event("claims.checking", "Checking 5 claims", json!({}), window, cx);
                self.on_msg(Msg::Reasoning(2, REASONING.into()), window, cx);
            }
            Act::Stream(i) => {
                let words: Vec<&str> = ANSWER.split_inclusive(' ').collect();
                let per = words.len().div_ceil(8);
                let chunk: String = words.iter().skip(i * per).take(per).copied().collect();
                if let Some(t) = self.tour.as_mut() {
                    t.streamed.push_str(&chunk);
                }
                self.on_msg(Msg::Text(3, chunk), window, cx);
            }
            Act::Finish => {
                let report = write_report();
                self.tour_event(
                    "model.replied",
                    "Answered",
                    json!({"purpose": "answer", "reasoning": REASONING, "thought_ms": 2400, "finish_reason": "stop"}),
                    window,
                    cx,
                );
                self.tour_event("claims.checked", "3 supported, 2 partly", claims_json(&report), window, cx);
                self.tour_event("turn.finished", "Answered", json!({"state": "completed"}), window, cx);
                self.model.finish_turn(&outcome(ANSWER), context(41_200, 3_100));
                self.model.reveal_all();
                self.scroll.scroll_to_bottom();
            }
            Act::OpenSources => {
                if let Some(i) = self.last_answer() {
                    self.open_sources.insert(i);
                }
            }
            Act::View(v) => {
                self.model.run_panel_open = true;
                self.model.set_run_view(v);
            }
            Act::ShowClaim => {
                if let Some(i) = self.last_answer() {
                    self.model.show_claim(i, 4);
                }
            }
            Act::SideChat => self.open_side(Some("FLARE retrieves only when the sentence it is about to write contains low-confidence tokens".into()), window, cx),
            Act::CloseSide => self.model.cont.side.open = false,
            Act::QuickTurn => {
                self.model.set_mode(crate::composer::Mode::Chat, None);
                if self.model.start_turn(QUICK_Q).is_some() {
                    self.start_ticker(cx);
                    self.tour_event("turn.started", "Started", json!({"mode": "chat"}), window, cx);
                    self.tour_event("tool.executed", "Read arxiv.org", json!({"tool": "read_url", "args": {"url": SOURCES[2].1}, "meta": {"page": page(2)}}), window, cx);
                    self.on_msg(Msg::Text(1, QUICK_A.into()), window, cx);
                }
            }
            Act::QuickFinish => {
                self.tour_event("turn.finished", "Answered", json!({"state": "completed"}), window, cx);
                self.model.finish_turn(&outcome(QUICK_A), context(52_000, 400));
                self.model.reveal_all();
                self.scroll.scroll_to_bottom();
            }
            Act::Editor => self.open_editor(window, cx),
            Act::Review(step) => self.tour_review(step, window, cx),
            Act::Budget(on) => {
                self.model.resume_offer = on.then(|| crate::model::ResumeOffer {
                    question: QUESTION.into(),
                    headline: "Run paused: it reached its token limit".into(),
                    progress: "Deep research · 3 of 4 done".into(),
                    saved_at: "just now".into(),
                    kind: tern_agent::OfferKind::Resume,
                    about_search: false,
                    budget: Some(crate::model::BudgetStop {
                        used: 231_400,
                        limit: Some(240_000),
                    }),
                });
                self.scroll.scroll_to_bottom();
            }
            Act::VaultDiff(on) => {
                self.model.editor.proposal = on.then(|| {
                    let path = self
                        .model
                        .editor
                        .open
                        .clone()
                        .unwrap_or_else(|| "Retrieval policies.md".into());
                    crate::editor::Proposal {
                        path,
                        before: "# Retrieval policies\n\nFLARE retrieves on low-confidence tokens.\nSelf-RAG reflects with special tokens.".into(),
                        after: "# Retrieval policies\n\nFLARE retrieves on low-confidence tokens.\nSelf-RAG decides with reflection tokens whether to retrieve.\nAdaptive-RAG routes by query complexity.".into(),
                        source: "Changed outside the editor".into(),
                        on_disk: false,
                    }
                });
            }
            Act::Git => self.open_git(window, cx),
            Act::Library => self.open_library(cx),
            Act::Archived => self.open_archived(cx),
            Act::Watches => self.model.screen = Screen::Watches,
            Act::Chat => self.model.screen = Screen::Chat,
            Act::StudyOpen => {
                let kept = (self.model.entries.clone(), self.model.last_run.clone());
                if let Some(t) = self.tour.as_mut() {
                    t.saved = Some(kept);
                }
                self.choose_harness(Harness::Study, cx);
            }
            Act::StudyTab(tab) => self.study_tab(tab, cx),
            Act::StudyMaterials => {
                self.study_tab(StudyTab::Chat, cx);
                self.model.study.materials_open = true;
            }
            Act::ReviewStart => self.start_review(None, window, cx),
            Act::ReviewFlip => self.flip_card(cx),
            Act::ReviewRate => self.rate_card(tern_agent::cards::Rating::Good, cx),
            Act::QuizStart => self.start_quiz(cx),
            Act::QuizAnswer => {
                let mut typed = None;
                if let Some(run) = self.model.study.quiz.as_mut()
                    && let Some(q) = run.question().cloned()
                {
                    match q.kind {
                        tern_agent::study::QuestionKind::MultipleChoice
                        | tern_agent::study::QuestionKind::TrueFalse => {
                            run.picked = Some(q.answer.clone());
                        }
                        _ => typed = Some(q.answer.clone()),
                    }
                }
                if let Some(text) = typed {
                    self.study_inputs
                        .answer
                        .update(cx, |i, cx| i.set_value(text, window, cx));
                }
                self.quiz_submit(window, cx);
            }
            Act::QuizNext => self.quiz_next(cx),
            Act::ExplainStart => {
                let ask = tern_agent::explain::Ask {
                    concept: "Adaptive retrieval".into(),
                    topic: None,
                    context: None,
                };
                self.study_explain(ask, "Adaptive retrieval", window, cx);
            }
            Act::ExplainStep => {
                if let Some(k) = self.model.study.explain {
                    self.explain_step(k, 1, cx);
                }
            }
            Act::ExplainPlay => {
                if let Some(k) = self.model.study.explain {
                    self.explain_play(k, cx);
                }
            }
            Act::ExplainZoom => {
                if let Some(k) = self.model.study.explain
                    && let Some(s) = self.explains.get_mut(&k)
                {
                    let [_, _, w, h] = s.bounds.get();
                    if w > 0. && h > 0. {
                        s.wheel(-360., w / 2., h / 2., w, h);
                    }
                    self.explain_at = None;
                }
            }
            Act::Research => {
                self.choose_harness(Harness::Research, cx);
                if let Some((entries, run)) = self.tour.as_mut().and_then(|t| t.saved.take()) {
                    self.model.entries = entries;
                    self.model.last_run = run;
                    self.model.run_panel_open = true;
                }
                self.tour_connect();
            }
            Act::Slash => self.set_composer("/", window, cx),
            Act::Palette(q) => {
                self.cmdk = Some(super::palette_view::CmdK {
                    query: q.to_string(),
                    sel: 0,
                })
            }
            Act::PaletteClose => self.cmdk = None,
            Act::Mention => self.set_composer("Compare these @", window, cx),
            Act::TabPicker => self.toggle_tab_picker(window, cx),
            Act::Settings(s) => self.open_settings(s, window, cx),
            Act::CustomizeTab(tab) => self.set_customize_tab(tab, cx),
            Act::CloseSettings => self.close_settings(window, cx),
            Act::Theme => self.toggle_theme(window, cx),
            Act::Switcher(on) => self.switcher = on.then(Instant::now),
            Act::Tabs => {
                if let Some(t) = self.tour.as_ref() {
                    let chat = t.chat;
                    for (id, title) in t.others.clone() {
                        self.tabs.open(id, title);
                    }
                    self.tabs.set_active(chat, "Agentic RAG: retrieval policies");
                }
            }
            Act::Sidebar(on) => self.model.sidebar_open = on,
            Act::RightPanel(on) => self.model.run_panel_open = on,
            Act::Scout(m) => self.pet_preview = m,
            Act::PetToolbar(on) => self.pet_tour_toolbar = on,
            Act::ModelMenu => {
                self.model
                    .composer
                    .toggle_at(crate::composer::Menu::Model, Instant::now());
                self.refresh_catalog();
            }
            Act::EffortMenu => {
                self.model
                    .composer
                    .toggle_at(crate::composer::Menu::Effort, Instant::now());
            }
            Act::CloseMenu => self.model.composer.close(),
            Act::Browser => self.browse("https://arxiv.org/abs/2310.11511", window, cx),
            Act::CloseBrowser => {
                self.model.browser.visible = false;
            }
            Act::ToastsClear => {
                let now = Instant::now();
                let ids: Vec<u64> = self.model.toasts.shown().iter().map(|t| t.id).collect();
                for id in ids {
                    self.model.toasts.dismiss(id, now);
                }
            }
            Act::Toasts => {
                self.model.toast_info("Report saved to your vault: Retrieval policies – sources.md");
                self.model.toast_error(
                    "Tavily did not answer in time. Tern used Wikipedia search instead.",
                    Some(crate::toast::Retry::Resend),
                );
            }
            Act::Dictation => {
                let mut d = crate::dictation::Dictation::new();
                let samples: Vec<f32> = (0..32_000)
                    .map(|i| (i as f32 * 0.05).sin() * 0.3)
                    .collect();
                if let Some(r) = d.next_request(&samples) {
                    d.on_result(r.seq, Ok("compare retrieval policies by cost per question".into()));
                }
                self.dictation = Some(d);
            }
            Act::DictationEnd => self.dictation = None,
            Act::ContextRing => self.ctx_opened.set(Some(Instant::now())),
            Act::Narrow(on) => {
                let (full_w, h) = window_size();
                let w = if on { full_w.min(900.) } else { full_w };
                window.resize(size(px(w), px(h)));
            }
            Act::SeedModels => self.tour_seed_models(),
            Act::Strip(mask) => self.perf_strip = mask,
            Act::Wheel(n) => self.tour_wheel(n, window, cx),
            Act::Mark(what) => log(&format!(
                "PERF mark {what} on {} unix_ms={}",
                self.perf_tag(),
                unix_ms()
            )),
            Act::CAttach(name) => {
                self.attach_paths(vec![tour_files().join(name)], cx);
            }
            Act::CRemoveChip => {
                if let Some(i) = self.model.attachments.len().checked_sub(1) {
                    self.model.remove_attachment(i);
                }
            }
            Act::CNoModelToast => self.model.audio_needs_model("memo.wav"),
            Act::CSeedStt => {
                if self.file_stt_model().is_none() {
                    use tern_agent::providers::{Capability, ModelInfo, custom};
                    if let Some(spec) = custom::find("tourmock") {
                        let mut m = ModelInfo::new("whisper-1");
                        if !m.has(Capability::SpeechToText) {
                            m.capabilities.push(Capability::SpeechToText);
                        }
                        let chat = ModelInfo::new("tour-chat");
                        self.model.catalog = tern_agent::media::Catalog::new(vec![(
                            spec.id,
                            Ok(vec![chat, m]),
                        )]);
                        log("TOUR note the catalog had no speech model; listed tourmock/whisper-1");
                    }
                }
                log(&format!(
                    "TOUR note audio files go to {}",
                    self.file_stt_model()
                        .map(|m| m.to_string())
                        .unwrap_or_else(|| "nothing".into())
                ));
            }
            Act::CSend => {
                self.model.chat_here = self.current;
                self.submit(window, cx);
            }
            Act::CRemember => {
                let here = self.current;
                if let Some(t) = self.tour.as_mut() {
                    t.remembered = here;
                }
            }
            Act::CNewChat => self.new_chat(window, cx),
            Act::COpenRemembered => {
                if let Some(id) = self.tour.as_ref().and_then(|t| t.remembered) {
                    self.open_chat(id, cx);
                } else {
                    log("TOUR error no chat remembered");
                }
            }
            Act::CTypePath(text) => self.set_composer(&with_dir(text), window, cx),
            Act::CChooseMention(label) => {
                let rows = self.model.mention_candidates();
                match rows.iter().position(|r| r.label == label) {
                    Some(i) => self.choose_mention(Some(i), window, cx),
                    None => log(&format!("TOUR error no @ row {label}")),
                }
            }
            Act::CApprove => {
                if self.pending.is_some() {
                    log("TOUR note approval asked; allowing once");
                    self.answer(tern_agent::ApprovalAnswer::AllowOnce, cx);
                }
            }
            Act::CState(what) => self.log_composer_state(what, cx),
            Act::CKeys(text) => {
                use gpui_kit::EntityInputHandler as _;
                self.composer.update(cx, |c, cx| {
                    c.focus(window, cx);
                    for ch in text.chars() {
                        c.replace_text_in_range(None, &ch.to_string(), window, cx);
                    }
                });
                let value = self.composer.read(cx).value().to_string();
                self.model.chat_here = self.current;
                self.model.on_composer_text(&value);
            }
            Act::CArrow(left) => {
                use gpui_kit::component::input::{MoveLeft, MoveRight};
                self.composer.update(cx, |c, cx| c.focus(window, cx));
                if left {
                    window.dispatch_action(Box::new(MoveLeft), cx);
                } else {
                    window.dispatch_action(Box::new(MoveRight), cx);
                }
            }
            // After this update, as a real click runs: a Save calls back
            // into the workspace, which is being updated here.
            Act::CClick(prefix, suffix) => window.defer(cx, move |window, cx| {
                match crate::blockexport::tour_click(prefix, suffix, window, cx) {
                    Some(id) => log(&format!("TOUR note clicked {id}")),
                    None => log(&format!("TOUR error no control {prefix}…{suffix}")),
                }
                window.refresh();
            }),
            Act::CDown => {
                use gpui_kit::component::input::MoveDown;
                self.composer.update(cx, |c, cx| c.focus(window, cx));
                window.dispatch_action(Box::new(MoveDown), cx);
                // The action runs after this update; read the row then.
                cx.defer_in(window, |this, _, _| {
                    let selected = this.model.mention.as_ref().map(|m| m.selected);
                    log(&format!("TOUR note Down; @ row selected {selected:?}"));
                });
            }
            Act::CCollectExports => self.tour_collect_exports(),
            Act::CSearch(id) => {
                self.choose_search(Some(id), cx);
                log(&format!("TOUR note search service {id}"));
            }
            Act::CSpeech(device, threads) => {
                self.save_speech(Some(device), Some(threads));
                cx.notify();
            }
            Act::CSpeechState(what) => {
                use tern_agent::local_models::{gpu, speech};
                let probe = match gpu::first() {
                    Ok(g) => format!("gpu {} {}", g.api, g.name),
                    Err(why) => format!("cpu ({why})"),
                };
                let c = speech::choice();
                log(&format!(
                    "TOUR speech {what}: build_api={:?} probe={probe} device={} threads_choice={:?} threads_in_effect={} gpu_failed_before={} last={:?}",
                    gpu::API,
                    c.device.id(),
                    c.threads,
                    speech::threads(),
                    speech::gpu_failed_before(),
                    speech::last_device()
                ));
                cx.notify();
            }
            Act::DictationLanguage(code) => {
                self.save_dictation_language(code);
                log(&format!(
                    "TOUR note dictation language {:?}",
                    self.model.providers.prefs.dictation_language
                ));
                cx.notify();
            }
            Act::DictateFrom(text, at) => {
                self.composer.update(cx, |c, cx| {
                    c.set_value(text, window, cx);
                    c.set_selected_range(at..at, cx);
                    c.focus(window, cx);
                });
                self.dictation_caret = crate::dictation::Caret::start(at);
                self.log_dictation("caret placed");
                self.log_caret(cx);
            }
            Act::Dictate(phrase) => {
                self.insert_dictated(phrase, window, cx);
                self.composer.update(cx, |c, cx| c.focus(window, cx));
                self.log_caret(cx);
            }
            Act::CLocalPicture(start) => {
                if start {
                    self.model.set_mode(crate::composer::Mode::Chat, None);
                    if self.model.start_turn(LOCAL_PICTURE_Q).is_some() {
                        self.model.doing = Some("Looking at image…".to_owned());
                        self.start_ticker(cx);
                    } else {
                        log("TOUR error the picture turn did not start");
                    }
                } else {
                    self.model.doing = None;
                    self.model
                        .finish_turn(&outcome(LOCAL_PICTURE_A), context(3_200, 120));
                    self.model.reveal_all();
                    self.scroll.scroll_to_bottom();
                }
                log(&format!(
                    "TOUR note working line {:?}",
                    self.model.working_line().map(|w| w.phrase)
                ));
                cx.notify();
            }
            Act::CReadAloud(step) => {
                use tern_agent::read_aloud::{Phase, Status};
                let Some(entry) = self.last_answer() else {
                    log("TOUR error no answer to read aloud");
                    return;
                };
                let session = self.current;
                let phase = match step {
                    0 => Some((Phase::Preparing, 0)),
                    1 => Some((Phase::Playing, 1)),
                    2 => Some((Phase::Paused, 1)),
                    _ => None,
                };
                self.read_aloud = phase.map(|(phase, sentence)| super::read_aloud_view::ReadingNow {
                    entry,
                    session,
                    model: "kokoro-82m-int8".into(),
                    player: None,
                    status: Status {
                        phase,
                        sentence,
                        sentences: 3,
                    },
                });
                log(&format!(
                    "TOUR note read aloud {}",
                    self.read_aloud
                        .as_ref()
                        .map(|r| super::read_aloud_view::status_label(&r.status))
                        .unwrap_or_else(|| "stopped (speaker button)".into())
                ));
                cx.notify();
            }
            Act::CLocalFailed(on) => {
                self.model.local_failed = on.then(|| crate::chatflow::LocalFailure {
                    id: "smolvlm-500m".into(),
                    name: "SmolVLM 500M Instruct (Q8)".into(),
                    task: tern_agent::local_models::Task::Vision,
                    error: "the vision helper did not finish in 240 s".into(),
                    alternative: Some((
                        "smolvlm-256m".into(),
                        "SmolVLM 256M Instruct (Q8)".into(),
                        true,
                    )),
                    missing: false,
                });
                log(&format!(
                    "TOUR note local banner {:?}",
                    self.model.local_failed.as_ref().map(|f| f.line())
                ));
                cx.notify();
            }
            Act::CDictationAuto => {
                use crate::dictation::{Dictation, Pace, RATE, Route, language_window};
                let start = self.dictation_route.clone();
                let silent = start.after_window(None, true, false);
                // #70: live text from the fast model at once; one language
                // check on the first 3-4 s; Urdu starts over on Whisper.
                let tone: Vec<f32> = (0..RATE * 5).map(|i| ((i as f32) * 0.05).sin() * 0.3).collect();
                let check = language_window(&tone, 0, false);
                let mut d = Dictation::with_pace(Route::Detect.pace());
                let first = d.next_request(&tone[..RATE * 3]).map(|r| r.seq);
                if let Some(seq) = first {
                    d.on_result(seq, Ok("Hello from the first".into()));
                }
                let live = d.interim_line();
                d.restart(Route::checked("ur", true).pace());
                log(&format!(
                    "TOUR note dictation auto: starts {start:?} (pace streaming: {}); a window with no words -> {silent:?}; live text before the check {live:?}; language check listens to {check:?} samples once; heard en -> {:?}; heard ur -> {:?} (text after restart {:?}); hi -> {:?}",
                    Route::Detect.pace() == Pace::STREAMING,
                    Route::checked("en", true),
                    Route::checked("ur", true),
                    d.interim_line(),
                    Route::checked("hi", true)
                ));
            }
            Act::CCleanBubble => {
                let png = mock::png(96, 64);
                if let Ok(pic) = tern_agent::Attachment::image("Screenshot 2026-10-24.png", &png) {
                    let msg = tern_agent::attach::compose(CLEAN_Q, &[pic]);
                    let ocr = tern_agent::untrusted::wrap(
                        "OCR text of Screenshot 2026-10-24.png by local PP-OCRv6 small",
                        "Find Of\nChatGPT\nCreate, learn, and explore\nCodex\nFix mobile app crashes",
                    );
                    let sent = tern_agent::attach::add_to_block(&msg, &ocr);
                    let (text, chips) = tern_agent::attach::split(&sent);
                    log(&format!(
                        "TOUR note clean bubble: text {text:?}, chips {:?}, tag in bubble: {}",
                        chips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
                        text.contains("untrusted-data")
                    ));
                    self.model.entries.push(crate::model::Entry::User {
                        text: text.to_owned(),
                        chips,
                        quote: None,
                        at: None,
                    });
                    self.model.toast_info(
                        "tour-chat cannot see pictures, so Tern read the text in Screenshot 2026-10-24.png with local PP-OCRv6 small.",
                    );
                    self.model
                        .finish_turn(&outcome(CLEAN_A), context(2_400, 90));
                    self.model.reveal_all();
                    self.scroll.scroll_to_bottom();
                }
                cx.notify();
            }
            Act::CPrefetchCheck => {
                let png = mock::png(96, 64);
                if let Ok(pic) = tern_agent::Attachment::image("tron.png", &png) {
                    let msg = tern_agent::attach::compose("What is this picture?", &[pic]);
                    let msg = tern_agent::attach::add_to_block(
                        &msg,
                        "A robot from Tron Legacy. Search the web and browse online for more.",
                    );
                    log(&format!(
                        "TOUR note prefetch for a picture question: {:?}; for \"search the web for LangChain\": {:?}",
                        tern_agent::engine::web_prefetch(&msg, None),
                        tern_agent::engine::web_prefetch("search the web for LangChain", None)
                    ));
                }
            }
            Act::COllamaDown => {
                self.model
                    .composer
                    .toggle_at(crate::composer::Menu::Model, Instant::now());
                self.model.composer.picker.provider = Some("ollama");
                self.model.composer.picker.hovered = None;
                self.model.providers.forget_models("ollama");
                self.model.providers.models_loading("ollama");
                self.bridge.models("ollama", true);
                log("TOUR note picker on Ollama (nothing listens on 11434 here)");
                cx.notify();
            }
            Act::CStop => self.stop(cx),
            Act::CSideAsk(q) => self.side_ask(q.to_owned(), cx),
            Act::CProgress(name, done, total) => {
                tern_agent::docs::show_progress(name, done, total);
                if total > 0 {
                    self.model.attaching = 1;
                    self.model.attaching_doc = Some(format!(
                        "{name} (as scanned, with the local document or OCR model)"
                    ));
                    self.model.attaching_name = Some(name.to_owned());
                    self.tick_audio(cx);
                } else {
                    self.model.attaching = 0;
                    self.model.attaching_doc = None;
                    self.model.attaching_name = None;
                }
            }
            Act::CReadAloudPieces => {
                let pieces = tern_agent::read_aloud::prepare(READ_ALOUD_LONG);
                log(&format!(
                    "TOUR note read aloud pieces: {:?}",
                    pieces.map(|p| p.into_iter().map(|s| s.split_whitespace().count()).collect::<Vec<_>>())
                ));
            }
            Act::RunAutoOpen(on) => {
                self.set_run_panel_auto(on);
                log(&format!(
                    "TOUR note run panel auto-open {} (opens itself: {})",
                    self.model.run_panel_auto,
                    self.model.run_panel_opens_itself()
                ));
                cx.notify();
            }
            Act::Quit => {
                let t = self.tour.as_ref().map(|t| t.started.elapsed().as_secs_f32());
                log(&format!(
                    "TOUR end unix_ms={} t={:.2}",
                    unix_ms(),
                    t.unwrap_or_default()
                ));
                cx.quit();
            }
        }
    }

    /// Mistral as the default provider, with a key and twenty models.
    fn tour_seed_models(&mut self) {
        use crate::providers::{Kind, ModelChoice, Models};
        let st = &mut self.model.providers;
        for r in st.rows.iter_mut() {
            if r.kind == Kind::Model && matches!(r.id, "mistral" | "groq") {
                r.key_saved = true;
            }
        }
        st.prefs.provider = Some("mistral".to_owned());
        let list = (0..20)
            .map(|i| {
                let mut m = ModelChoice::new(tern_agent::providers::ModelInfo::new(format!(
                    "mistral-model-{i:02}-2512"
                )));
                m.context_window = Some(32_768 << (i % 4));
                m
            })
            .collect();
        st.models = Some(Models::Loaded("mistral", list));
    }

    /// `n` wheel notches down over the Settings page, then `n` up, 40 ms
    /// apart, through GPUI's own input dispatch (hit test, listeners).
    fn tour_wheel(&mut self, n: u8, window: &mut Window, cx: &mut Context<Self>) {
        let vp = window.viewport_size();
        // Right of the section list, a little above the middle: over the
        // provider rows and, once scrolled, the open model list.
        let at = gpui_kit::point(vp.width * 0.55, vp.height * 0.45);
        let n = usize::from(n);
        cx.spawn_in(window, async move |this, cx| {
            for i in 0..2 * n {
                let lines = if i < n { -3. } else { 3. };
                let ok = this.update_in(cx, |_, window, cx| {
                    window.dispatch_event(
                        gpui_kit::PlatformInput::ScrollWheel(gpui_kit::ScrollWheelEvent {
                            position: at,
                            delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., lines)),
                            modifiers: Default::default(),
                            touch_phase: gpui_kit::TouchPhase::Moved,
                        }),
                        cx,
                    );
                });
                if ok.is_err() {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
            }
        })
        .detach();
    }

    /// The transcript index of the last answer.
    /// A scene-log line with the composer's text, chips, audio in flight,
    /// the open @ rows, the chat's last answer, and the toasts, so a
    /// recording's frames can be checked against what the app held.
    fn log_dictation(&self, what: &str) {
        log(&format!(
            "TOUR dictation {what}: language={:?}",
            self.model.providers.prefs.dictation_language
        ));
    }

    /// A scene-log line with the composer's text and caret.
    fn log_caret(&self, cx: &mut Context<Self>) {
        let c = self.composer.read(cx);
        let text = c.value().to_string();
        let at = c.cursor();
        let (head, tail) = text.split_at(at.min(text.len()));
        log(&format!("TOUR caret {head}|{tail} (at {at})"));
    }

    /// Copies what the tour saved from blocks (formula, diagram, table and
    /// code files in the save folder) next to the scene log, and logs each
    /// file's name and size.
    fn tour_collect_exports(&self) {
        let dir = self.export_dir();
        let Some(out) = std::env::var_os("TERN_TOUR_LOG")
            .map(std::path::PathBuf::from)
            .and_then(|l| l.parent().map(|p| p.join("exports")))
        else {
            log("TOUR error no TERN_TOUR_LOG for exports");
            return;
        };
        let _ = std::fs::create_dir_all(&out);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            log(&format!("TOUR error can't list {}", dir.display()));
            return;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let ours = [
                "formula", "diagram", "table", "answer", "code", "snippet", "sort",
            ]
            .iter()
            .any(|s| name.starts_with(s))
                || name.ends_with(".rs");
            if !ours || !e.path().is_file() {
                continue;
            }
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            let _ = std::fs::copy(e.path(), out.join(&name));
            log(&format!(
                "TOUR note exported {} ({size} bytes)",
                dir.join(&name).display()
            ));
        }
    }

    fn log_composer_state(&self, what: &str, cx: &mut Context<Self>) {
        use crate::model::Entry;
        let text = self.composer.read(cx).value().to_string();
        let chips: Vec<String> = self
            .model
            .attachments
            .iter()
            .map(|a| format!("{} [{}]", a.name, a.detail()))
            .collect();
        let audio: Vec<&str> = self
            .model
            .audio_pending
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        let rows: Vec<String> = self
            .model
            .mention_candidates()
            .into_iter()
            .map(|r| format!("{} ({})", r.label, r.detail))
            .collect();
        let answer = self
            .last_answer()
            .and_then(|i| match &self.model.entries[i] {
                Entry::Assistant { text, run, .. } => Some(format!(
                    "{} | steps: {}",
                    text.chars().take(160).collect::<String>(),
                    run.steps
                        .iter()
                        .map(|s| s.summary.clone())
                        .collect::<Vec<_>>()
                        .join("; ")
                )),
                _ => None,
            });
        let toasts: Vec<&str> = self
            .model
            .toasts
            .shown()
            .iter()
            .map(|t| t.text.as_str())
            .collect();
        let (caret, rtl) = {
            let c = self.composer.read(cx);
            (c.selected_range(), c.is_right_to_left())
        };
        let title = self
            .model
            .sessions
            .iter()
            .find(|s| Some(s.id) == self.current)
            .map(|s| s.title.clone());
        log(&format!(
            "TOUR state {what}: caret={caret:?} rtl={rtl} mentions={:?} title={title:?}",
            crate::composer::mention_spans(&text)
        ));
        log(&format!(
            "TOUR state {what}: chat={:?} status={:?} phase={:?} composer={text:?} chips={chips:?} audio={audio:?} at_rows={rows:?} entries={} answer={answer:?} toasts={toasts:?} notices={:?} wiki_empty={:?}",
            self.current,
            self.model.working_line().map(|w| w.phrase),
            self.model.phase,
            self.model.entries.len(),
            self.model.active_notices(),
            self.model.wiki_empty,
        ));
        // #71: what belongs to which chat.
        let background: Vec<String> = self
            .background
            .iter()
            .map(|(id, bg)| {
                format!(
                    "{id}: phase={:?} entries={} session_back={}",
                    bg.slot.phase,
                    bg.slot.entries.len(),
                    bg.session.is_some()
                )
            })
            .collect();
        let side: Vec<String> = self
            .model
            .cont
            .side
            .lines
            .iter()
            .map(|l| format!("{} -> {:?}", l.question, l.answer))
            .collect();
        let tail: Vec<String> = self
            .model
            .entries
            .iter()
            .rev()
            .take(2)
            .map(|e| match e {
                Entry::Assistant { text, .. } => {
                    format!("answer {:?}", text.chars().take(120).collect::<String>())
                }
                Entry::Notice(n) => format!("notice {n:?}"),
                Entry::User { text, chips, .. } => format!("user {text:?} chips={}", chips.len()),
            })
            .collect();
        log(&format!(
            "TOUR state {what}: attaching={} converting={:?} background={background:?} side={side:?} last={tail:?}",
            self.model.attaching, self.model.attaching_doc,
        ));
        if let Some(Entry::Assistant { run, .. }) =
            self.last_answer().map(|i| &self.model.entries[i])
        {
            log(&format!(
                "TOUR state {what}: rollup={:?}",
                crate::chatflow::rollup(run)
            ));
        }
    }

    fn last_answer(&self) -> Option<usize> {
        self.model
            .entries
            .iter()
            .rposition(|e| matches!(e, crate::model::Entry::Assistant { .. }))
    }
}

fn summary(id: SessionId, title: &str, harness: Harness) -> tern_agent::SessionSummary {
    tern_agent::SessionSummary {
        id,
        title: title.into(),
        harness,
        created_at: tern_agent::checkpoint::now_rfc3339(),
        events: 40,
        parent: None,
        anchor: None,
        plan: None,
        project: None,
    }
}

fn outcome(answer: &str) -> tern_agent::TurnOutcome {
    tern_agent::TurnOutcome {
        turn_id: TurnId::new(),
        state: TurnState::Completed,
        answer: Some(answer.into()),
        reason: None,
        states: Vec::new(),
        usage: tern_agent::Usage::measured(41_200, 3_100),
        model_calls: 9,
        tool_calls: 14,
    }
}

fn context(prompt: u64, reply: u64) -> tern_agent::ContextUse {
    tern_agent::ContextUse {
        prompt,
        reply,
        window: Some(200_000),
        estimated: false,
        session_tokens: prompt + reply + 40_000,
        calls: 9,
        parts: tern_agent::ContextParts::estimate(4_000, 8_000, 0, 1_000).scaled_to(prompt, reply),
    }
}

/// Writes the run report the Report view links to, in the throwaway home.
fn write_report() -> std::path::PathBuf {
    let dir = tern_agent::platform::AppPaths::discover()
        .map(|p| p.data_dir.join("runs").join("tour"))
        .unwrap_or_else(|_| std::env::temp_dir().join("tern-tour-runs"));
    let _ = std::fs::create_dir_all(&dir);
    let md = dir.join("turn-1.md");
    let mut text = format!("# {QUESTION}\n\n{ANSWER}\n\n## Sources\n\n");
    for (i, (title, url, _)) in SOURCES.iter().enumerate() {
        text.push_str(&format!("{}. [{title}]({url})\n", i + 1));
    }
    let _ = std::fs::write(&md, text);
    let _ = std::fs::write(md.with_extension("json"), "{}");
    md
}

/// One scene-log line: to stdout, and to `TERN_TOUR_LOG` when set (a
/// Windows release build has no console).
fn log(line: &str) {
    use std::io::Write as _;
    println!("{line}");
    let _ = std::io::stdout().flush();
    if let Some(path) = std::env::var_os("TERN_TOUR_LOG")
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

/// The composer tour (`TERN_TOUR_ONLY=composer`): a real chat with a mock
/// OpenAI-compatible model on localhost, so attachments, transcripts,
/// typed paths, and `file_read` go through the real code.
fn composer_tour() -> bool {
    std::env::var("TERN_TOUR_ONLY").as_deref() == Ok("composer")
}

/// The folder the composer tour's files live in.
fn tour_files() -> std::path::PathBuf {
    std::env::temp_dir().join("tern-tour-files")
}

/// `{dir}` in a scripted line becomes the tour's files folder.
fn with_dir(text: &str) -> String {
    let mut dir = tour_files().display().to_string();
    dir.push(std::path::MAIN_SEPARATOR);
    text.replace("{dir}/", &dir).replace("{dir}", &dir)
}

/// #69: the question and the local vision model's description in the
/// local picture scene.
const LOCAL_PICTURE_Q: &str = "What is in this picture?";
const LOCAL_PICTURE_A: &str = "The picture shows a small green rectangle on a white background. It looks like a placeholder image. There is no visible text in it.";

/// #70: the clean-bubble scene's question and answer.
const CLEAN_Q: &str = "What this image is all about";
const CLEAN_A: &str = "It is a screenshot of the Codex start page in ChatGPT, with a task named \"Fix mobile app crashes\".";
/// #70: a long answer for the read-aloud pieces note.
const READ_ALOUD_LONG: &str = "The Transformer replaces recurrence with attention, which lets every token look at every other token at once, and that is why it trains so much faster on modern hardware than the older recurrent networks did. It was introduced in 2017.";

/// What the mock's transcription returns for any audio file.
/// #71: the slow answer, the side chat's answer, and the second chat's.
const SLOW_A: &str = "Virtual memory gives every program its own address space. The operating system splits memory into pages and keeps a page table for each process that maps virtual pages to physical frames. When a program touches a page that is not in memory, the hardware raises a page fault and the operating system loads the page from disk, evicting another if memory is full. The translation lookaside buffer caches recent translations so most accesses skip the page table walk. Together these let programs use more memory than the machine has, keep processes isolated from each other, and share code pages safely.";
const SIDE_A: &str = "TLB is the translation lookaside buffer: a small cache of recent page-table translations inside the CPU.";
const TLB_A: &str = "A TLB (translation lookaside buffer) caches recent virtual-to-physical page translations, so most memory accesses skip the page-table walk.";
/// #72: rich answers. A wide table with long cells (they wrap inside the
/// column) and a highlighted code block.
const RICH_TABLE_A: &str = "Here is how the three sorts compare:\n\n| Algorithm | Average time | Worst time | Extra memory | When to use it |\n|---|---|---|---|---|\n| Quicksort | O(n log n) | O(n²) | O(log n) stack | The usual default for arrays in memory: fast in practice thanks to cache-friendly partitioning, as long as the pivot is chosen well. |\n| Merge sort | O(n log n) | O(n log n) | O(n) buffer | When the sort must be stable or the data is a linked list or too large for memory, since it reads and writes sequentially. |\n| Heapsort | O(n log n) | O(n log n) | O(1) | When the worst case matters and no extra memory is allowed, at the cost of poorer cache behaviour. |\n\nA quicksort partition in Rust:\n\n```rust\nfn partition(v: &mut [i32]) -> usize {\n    let pivot = v[v.len() - 1];\n    let mut i = 0;\n    for j in 0..v.len() - 1 {\n        if v[j] <= pivot {\n            v.swap(i, j);\n            i += 1;\n        }\n    }\n    let last = v.len() - 1;\n    v.swap(i, last);\n    i // the pivot's final place\n}\n```";
/// #72: inline and display math, and a Mermaid flowchart.
const RICH_MATH_A: &str = "Scaled dot-product attention weighs each value by how well its key matches the query, $\\alpha_{ij} = \\mathrm{softmax}_j(q_i \\cdot k_j / \\sqrt{d_k})$, so the whole layer is\n\n$$\\mathrm{Attention}(Q, K, V) = \\mathrm{softmax}\\left(\\frac{QK^\\top}{\\sqrt{d_k}}\\right) V$$\n\nThe encoder block in order:\n\n```mermaid\nflowchart LR\n    A[Tokens] --> B[Embedding + position]\n    B --> C[Multi-head attention]\n    C --> D[Add and norm]\n    D --> E[Feed-forward]\n    E --> F[Add and norm]\n```\n\nThe paper's base model costs $3.3 \\times 10^{18}$ FLOPs to train.";
/// #72: the answer after one rate-limited attempt.
const RETRY_A: &str = "Answered on the second attempt: the provider asked Tern to wait, and Tern said so in the status line instead of retrying silently.";
/// #72: the first request asking this gets HTTP 429 with `Retry-After: 4`.
const RETRY_Q: &str = "Tour: answer after a rate limit";
const MEMO_TEXT: &str = "Remind me to send the FYP draft to Doctor Amina on Friday.";
const TOUR_NOTES: &str = "# Tour notes\n\nThe FYP demo is on 14 November in Lab 3.\n";
const TOUR_PLAN: &str =
    "# Plan\n\nWeek 1: retrieval baseline. Week 2: FLARE policy. Week 3: evaluation.\n";

/// The Urdu question the composer tour sends ("What is the difference
/// between Urdu and Arabic?").
const URDU_QUESTION: &str = "اردو اور عربی میں کیا فرق ہے؟";
/// The mock's Urdu answer: a right-to-left paragraph, an English line, and
/// a short mixed line.
const URDU_ANSWER: &str = "اردو ایک خوبصورت زبان ہے جو پاکستان کی قومی زبان ہے۔ اردو اور عربی میں بنیادی فرق یہ ہے کہ اردو ایک ہند آریائی زبان ہے جبکہ عربی ایک سامی زبان ہے، اگرچہ دونوں کا رسم الخط ملتا جلتا ہے۔\n\nIn English: Urdu is an Indo-Aryan language and Pakistan's national language; Arabic is a Semitic language.\n\nمختصر جملہ: سلام پاکستان، Tern 2026";
/// Typed into the composer by the tour (#67): Urdu ending in the Urdu
/// question mark, and a mixed Urdu and English line.
const URDU_TYPED: &str = "کیا آپ اردو بولتے ہیں؟";
/// "How are you?" without its question mark, typed key by key before the
/// tour types `??` at the caret (#68: they landed between the words).
const URDU_HOW_ARE_YOU: &str = "آپ کیسے ہیں";
const MIXED_TYPED: &str = "Tern میں Whisper small کتنا تیز ہے؟ (GPU)";
/// The same Urdu question ending in an ASCII `?` (an English keyboard
/// layout): a neutral character, which lands at the sentence's end (the
/// left) only when the line is laid out right to left (#67, Windows patch).
const URDU_ASCII_TYPED: &str = "کیا آپ اردو بولتے ہیں?";
/// What Wikipedia (the mock) finds for "SubQ Snap 1.1": the user's log
/// had six Japanese and medical pages, none about SubQ.
const WIKI_OFF_TOPIC: [(&str, &str); 4] = [
    (
        "Sabi (Japanese aesthetics)",
        "In definition and meaning, wabi-sabi is a Japanese world view",
    ),
    (
        "Medical model",
        "The medical model is the term coined by psychiatrist R. D. Laing",
    ),
    (
        "Snap (gene)",
        "SNAP proteins: definition of soluble NSF attachment protein",
    ),
    (
        "Kanji",
        "meaning of logographic Chinese characters adapted from Japanese",
    ),
];
/// What the mock's web search finds for any query.
const WEB_HITS: [(&str, &str, &str); 2] = [
    (
        "LangChain - Wikipedia",
        "https://en.wikipedia.org/wiki/LangChain",
        "LangChain was launched in October 2022 as an open-source project by Harrison Chase.",
    ),
    (
        "Arctic tern - Wikipedia",
        "https://en.wikipedia.org/wiki/Arctic_tern",
        "Arctic terns fly about 70,000 km a year between the Arctic and the Antarctic.",
    ),
];

/// The composer scenes.
fn composer_script() -> Vec<Beat> {
    vec![
        b(Act::Nothing, 4000),
        b(Act::CState("start"), 200),
        scene(
            "c01-attach-image",
            "tern-only",
            "Attach an image",
            Act::CAttach("photo.png"),
            2500,
        ),
        b(Act::CState("image attached"), 200),
        scene(
            "c02-remove-chip",
            "tern-only",
            "× removes the chip",
            Act::CRemoveChip,
            2000,
        ),
        b(Act::CState("chip removed"), 200),
        scene(
            "c03-no-model-toast",
            "tern-only",
            "No transcription model toast",
            Act::CNoModelToast,
            3000,
        ),
        b(Act::ToastsClear, 500),
        scene(
            "c04-attach-audio",
            "tern-only",
            "Attach audio: transcribing",
            Act::CSeedStt,
            300,
        ),
        b(Act::CAttach("memo.wav"), 600),
        b(Act::CState("audio transcribing"), 3500),
        b(Act::CState("audio transcribed"), 200),
        b(Act::Type("What did I ask for in the memo?"), 1200),
        scene(
            "c05-audio-answer",
            "tern-only",
            "Transcript sent and answered",
            Act::CSend,
            6000,
        ),
        b(Act::CState("audio answered"), 200),
        b(Act::CRemember, 200),
        b(Act::Type("Draft kept in the first chat"), 1500),
        scene(
            "c06-new-chat",
            "tern-only",
            "A new chat has its own composer",
            Act::CNewChat,
            2500,
        ),
        b(Act::CState("new chat"), 200),
        b(Act::Type("Second chat's draft"), 1500),
        scene(
            "c07-back",
            "tern-only",
            "Back to the first chat: its draft is back",
            Act::COpenRemembered,
            3000,
        ),
        b(Act::CState("first chat again"), 200),
        b(Act::ClearComposer, 300),
        scene(
            "c08-mention-recent",
            "tern-only",
            "@ menu: recent files",
            Act::CTypePath("Summarise @"),
            3000,
        ),
        b(Act::CState("mention recent"), 200),
        scene(
            "c09-mention-folder",
            "tern-only",
            "@ menu: a typed folder, filtered by what follows it",
            Act::CTypePath("Summarise @{dir}tour"),
            3000,
        ),
        b(Act::CState("mention folder"), 200),
        b(Act::CChooseMention("tour-notes.md"), 1500),
        b(Act::CState("mention chosen"), 200),
        scene(
            "c10-mention-answer",
            "tern-only",
            "Attached file answered",
            Act::CSend,
            6000,
        ),
        b(Act::CState("mention answered"), 200),
        b(Act::CNewChat, 1500),
        b(Act::CTypePath("What does {dir}tour-plan.md say?"), 1500),
        scene(
            "c11-typed-path",
            "tern-only",
            "Typed full path: file_read",
            Act::CSend,
            2500,
        ),
        b(Act::CApprove, 2500),
        b(Act::CApprove, 2500),
        b(Act::CApprove, 2000),
        b(Act::CState("typed path answered"), 200),
        b(Act::CNewChat, 1500),
        b(Act::Type("Who created LangChain?"), 1200),
        scene(
            "c13-web-native",
            "tern-only",
            "Web search on: the model calls web_search, the status says so",
            Act::CSend,
            1200,
        ),
        b(Act::CState("web searching"), 3800),
        b(Act::CState("web answered"), 300),
        scene(
            "c13b-citation-chip",
            "tern-only",
            "The landed answer's [1] is a citation chip; it opens the page in Tern's browser",
            Act::CState("citation chip"),
            2500,
        ),
        b(
            Act::Type("How far do Arctic terns migrate each year?"),
            1200,
        ),
        scene(
            "c14-safety-net",
            "tern-only",
            "A 'no internet access' reply is searched and answered again",
            Act::CSend,
            1500,
        ),
        b(Act::CState("safety net searching"), 4000),
        b(Act::CState("safety net answered"), 300),
        b(Act::Type("browse about that"), 1000),
        scene(
            "c15-browse-follow-up",
            "tern-only",
            "'browse about that' searches the previous question",
            Act::CSend,
            1500,
        ),
        b(Act::CState("follow-up searching"), 4000),
        b(Act::CState("follow-up answered"), 300),
        b(Act::CNewChat, 1500),
        b(Act::Type(URDU_QUESTION), 1000),
        scene(
            "c16-urdu-rtl",
            "tern-only",
            "Urdu question and answer render right to left",
            Act::CSend,
            5000,
        ),
        b(Act::CState("urdu answered"), 300),
        // #67: Wikipedia answers "SubQ Snap 1.1" with off-topic pages; Tern
        // drops them and says Wikipedia has nothing, with a Tavily button.
        b(Act::CSearch("wikipedia"), 1500),
        b(Act::CNewChat, 1500),
        b(Act::Type("What is SubQ Snap 1.1? Search the web."), 1200),
        scene(
            "c17-wiki-off-topic",
            "tern-only",
            "Off-topic Wikipedia results dropped; the Wikipedia notice offers Tavily",
            Act::CSend,
            1200,
        ),
        // The keyword ladder tries four queries (about 6 s on the runners).
        b(Act::CState("wikipedia searching"), 9000),
        b(Act::CState("wikipedia answered"), 2500),
        b(Act::CSearch("searxng"), 800),
        // #67: Settings › Speech: device and CPU threads, and the GPU probe
        // (the Windows runner has no GPU and no vulkan-1.dll: CPU).
        scene(
            "c18-settings-speech",
            "tern-only",
            "Settings › Speech: Auto/GPU/CPU and CPU threads",
            Act::Settings(Section::Speech),
            2500,
        ),
        b(Act::CSpeechState("opened"), 1500),
        scene(
            "c19-speech-choices",
            "tern-only",
            "Speech: GPU chosen, 4 threads; the probe falls back to the CPU",
            Act::CSpeech("gpu", Some(4)),
            2500,
        ),
        b(Act::CSpeechState("gpu and 4 threads chosen"), 1500),
        b(Act::CSpeech("auto", None), 1000),
        b(Act::CSpeechState("back to auto"), 300),
        b(Act::CloseSettings, 1000),
        // #67: typing Urdu with an Urdu question mark in the composer.
        b(Act::CNewChat, 1500),
        scene(
            "c20-urdu-typing",
            "tern-only",
            "Urdu typed in the composer (؟ at the end of the sentence)",
            Act::Type(URDU_TYPED),
            3000,
        ),
        b(Act::CState("urdu typed"), 300),
        b(Act::Type(MIXED_TYPED), 3000),
        b(Act::CState("mixed typed"), 300),
        b(Act::Type(URDU_ASCII_TYPED), 3000),
        b(Act::CState("urdu ascii typed"), 300),
        b(Act::ClearComposer, 500),
        // #68: "آپ کیسے ہیں" typed key by key, then "??" at the caret: the
        // marks belong at the left end (the text's end), not between words;
        // the line is right-aligned and the arrow keys follow the text.
        b(Act::CNewChat, 1500),
        scene(
            "c21-urdu-caret",
            "tern-only",
            "Urdu typed key by key, then ?? at the caret: ?? at the left end, line right-aligned",
            Act::CKeys(URDU_HOW_ARE_YOU),
            1500,
        ),
        b(Act::CKeys("??"), 2500),
        b(Act::CState("urdu ?? typed"), 300),
        scene(
            "c22-urdu-arrows",
            "tern-only",
            "Right arrow moves towards the start of Urdu text, Left towards its end",
            Act::CArrow(false),
            700,
        ),
        b(Act::CArrow(false), 1500),
        b(Act::CState("two Right presses (towards the start)"), 300),
        b(Act::CArrow(true), 700),
        b(Act::CArrow(true), 1500),
        b(Act::CState("two Left presses (back to the end)"), 300),
        scene(
            "c23-urdu-sent",
            "tern-only",
            "Sent: the bubble and the sidebar title keep ?? at the left end",
            Act::CSend,
            5000,
        ),
        b(Act::CState("urdu ?? answered"), 300),
        b(Act::CNewChat, 1500),
        scene(
            "c24-mention-colour",
            "tern-only",
            "@ references in the accent colour, the caret after the text",
            Act::CTypePath("Compare @tour-notes.md with @tour-plan.md and "),
            3000,
        ),
        b(Act::CState("mentions coloured"), 300),
        b(Act::ClearComposer, 500),
        // #69: local pictures, documents, read aloud, the local banner and
        // Auto dictation's first recording.
        b(Act::CNewChat, 1500),
        scene(
            "c25-local-picture",
            "tern-only",
            "A picture read on this computer: \"Looking at image…\" leads the working line",
            Act::CLocalPicture(true),
            2500,
        ),
        b(Act::CState("looking at image"), 300),
        b(Act::CLocalPicture(false), 1500),
        b(Act::CState("local description answered"), 300),
        scene(
            "c26-read-aloud",
            "tern-only",
            "Read aloud under the answer: speaker button, then Preparing, Reading 2 of 3, Paused",
            Act::CReadAloud(3),
            2000,
        ),
        b(Act::CReadAloud(0), 1800),
        b(Act::CReadAloud(1), 1800),
        b(Act::CReadAloud(2), 1800),
        b(Act::CReadAloud(3), 1000),
        scene(
            "c27-pdf-markdown",
            "tern-only",
            "A PDF attached: \"Converting report.pdf to Markdown…\", then the chip \"PDF text → Markdown · 2 pages\"",
            Act::CAttach("report.pdf"),
            2500,
        ),
        b(Act::CState("pdf converted"), 300),
        b(Act::CAttach("notes.docx"), 2500),
        b(Act::CState("docx converted"), 300),
        b(Act::CRemoveChip, 300),
        b(Act::CRemoveChip, 500),
        scene(
            "c28-local-banner",
            "tern-only",
            "A local model failed: the banner over the composer offers Use the alternative",
            Act::CLocalFailed(true),
            3000,
        ),
        b(Act::CState("local banner"), 300),
        b(Act::CLocalFailed(false), 800),
        scene(
            "c29-dictation-auto",
            "tern-only",
            "Auto dictation: live text at once from the fast model; one language check on the first seconds",
            Act::CDictationAuto,
            800,
        ),
        b(Act::DictationLanguage("auto"), 300),
        b(Act::DictateFrom("", 0), 600),
        b(Act::Dictate("Hello from the first recording."), 2000),
        b(Act::CState("first auto recording"), 300),
        b(Act::ClearComposer, 500),
        // #70: fixes from the Windows test of #69.
        b(Act::CNewChat, 1200),
        scene(
            "c31-scanned-pdf",
            "tern-only",
            "A scan whose only text is \"Scanned with CamScanner\": not read as text; with no OCR model the toast names what to download",
            Act::CAttach("CA-3 scan.pdf"),
            3000,
        ),
        b(Act::CState("scanned pdf attached"), 300),
        scene(
            "c32-read-as-scanned",
            "tern-only",
            "A PDF with a text page and a scanned page: the chip offers Read as scanned",
            Act::CAttach("lecture-mixed.pdf"),
            3000,
        ),
        b(Act::CState("mixed pdf attached"), 300),
        scene(
            "c33-old-office",
            "tern-only",
            "Old Office files: budget.xls as a table, deck.ppt slides' text; a damaged .ppt names Save As .pptx",
            Act::CAttach("budget.xls"),
            1500,
        ),
        b(Act::CAttach("deck.ppt"), 1500),
        b(Act::CAttach("file_example_PPT_250kB.ppt"), 2500),
        b(Act::CState("old office attached"), 300),
        b(Act::CRemoveChip, 200),
        b(Act::CRemoveChip, 200),
        b(Act::CRemoveChip, 200),
        b(Act::CRemoveChip, 500),
        b(Act::ToastsClear, 600),
        scene(
            "c34-clean-bubble",
            "tern-only",
            "After OCR the bubble shows only what was typed and the picture chip (no OCR text, no </untrusted-data>)",
            Act::CCleanBubble,
            3000,
        ),
        b(Act::CPrefetchCheck, 300),
        b(Act::ToastsClear, 400),
        scene(
            "c35-ollama-not-running",
            "tern-only",
            "Model picker on Ollama with nothing running: \"Not running — start Ollama, then Retry.\"",
            Act::COllamaDown,
            4500,
        ),
        b(Act::CloseMenu, 400),
        b(Act::CReadAloudPieces, 300),
        // #71: fixes from the Windows test of #70.
        b(Act::CNewChat, 1200),
        scene(
            "c36-preview-cards",
            "tern-only",
            "Attachments as preview cards: picture thumbnail, first page of a PDF and documents, name, pages and size, a file-type logo beside the name",
            Act::CAttach("photo.png"),
            700,
        ),
        b(Act::CAttach("report.pdf"), 700),
        b(Act::CAttach("notes.docx"), 700),
        b(Act::CAttach("budget.xls"), 700),
        b(Act::CAttach("deck.ppt"), 700),
        b(Act::CAttach("tour-notes.md"), 500),
        b(Act::CAttach("train.py"), 500),
        b(Act::CAttach("lab.ipynb"), 500),
        b(Act::CAttach("data.csv"), 2500),
        b(Act::CState("preview cards"), 300),
        b(Act::CTypePath("What do these files say?"), 300),
        scene(
            "c37-sent-cards",
            "tern-only",
            "Sent: the message's attachments as small cards with the picture and file-type logos",
            Act::CSend,
            4000,
        ),
        b(Act::CState("sent cards"), 300),
        // #72: the file-type logos on eight kinds of attachment, dark and
        // light.
        b(Act::CNewChat, 1200),
        b(Act::CAttach("notes.docx"), 500),
        b(Act::CAttach("marks.xlsx"), 500),
        b(Act::CAttach("report.pdf"), 500),
        b(Act::CAttach("train.py"), 500),
        b(Act::CAttach("main.cpp"), 500),
        b(Act::CAttach("photo.png"), 500),
        b(Act::CAttach("archive.zip"), 500),
        b(Act::CAttach("mystery.qqq"), 2500),
        scene(
            "c37b-file-logos",
            "tern-only",
            "Eight attachments, each with its file-type logo: Word, Excel, PDF, Python, C++, picture, zip and an unknown kind (blank page)",
            Act::CState("file logos"),
            2500,
        ),
        scene(
            "c37c-file-logos-light",
            "tern-only",
            "The same logos in the light theme",
            Act::Theme,
            2500,
        ),
        b(Act::Theme, 1500),
        b(Act::CNewChat, 1200),
        b(Act::CTypePath("Hello"), 200),
        b(Act::CSend, 2500),
        b(Act::CRemember, 200),
        scene(
            "c38-progress-chip",
            "tern-only",
            "Granite-Docling reading a scan: the converting chip shows page 3/6 · 33%",
            Act::CProgress("CA-3 scan.pdf", 2, 6),
            2500,
        ),
        b(Act::CState("progress chip"), 300),
        b(Act::CNewChat, 1500),
        scene(
            "c39-isolated-composer",
            "tern-only",
            "Another chat: its composer is its own, with no converting chip from the first chat",
            Act::CState("other chat composer"),
            2500,
        ),
        b(Act::COpenRemembered, 2500),
        b(Act::CState("first chat composer again"), 300),
        b(Act::CProgress("CA-3 scan.pdf", 0, 0), 800),
        b(Act::CNewChat, 1200),
        b(Act::CTypePath("What is paging?"), 200),
        b(Act::CSend, 2500),
        b(Act::CRemember, 200),
        b(Act::CTypePath("Explain virtual memory slowly"), 200),
        scene(
            "c40-concurrent-side-chat",
            "tern-only",
            "The chat is mid-answer; a side chat over it answers at once, not queued behind the run",
            Act::CSend,
            2500,
        ),
        b(Act::SideChat, 600),
        b(Act::CSideAsk("What does TLB mean here?"), 2500),
        b(
            Act::CState("side chat answered while the main chat runs"),
            300,
        ),
        scene(
            "c41-concurrent-new-chat",
            "tern-only",
            "A new chat while the first still runs: it answers at once; the first goes on in the background",
            Act::CNewChat,
            1500,
        ),
        b(Act::CTypePath("What is a TLB?"), 200),
        b(Act::CSend, 3000),
        b(
            Act::CState("second chat answered while the first runs"),
            300,
        ),
        b(Act::CloseSide, 300),
        b(Act::COpenRemembered, 2500),
        b(Act::CState("first chat back, still answering"), 300),
        b(Act::Nothing, 14000),
        b(Act::CState("first chat finished"), 300),
        b(Act::CNewChat, 1200),
        b(Act::CTypePath("Explain caches slowly"), 200),
        b(Act::CSend, 3500),
        scene(
            "c42-stopped-reply",
            "tern-only",
            "Stop mid-answer: the text streamed so far stays, marked Stopped; nothing more is written",
            Act::CStop,
            3000,
        ),
        b(Act::CState("stopped reply"), 300),
        b(Act::ToastsClear, 400),
        // #72: rich answers.
        b(Act::CNewChat, 1200),
        b(
            Act::CTypePath("Compare quicksort, merge sort and heapsort"),
            200,
        ),
        scene(
            "c43-rich-table-code",
            "tern-only",
            "A table whose long cells wrap inside their columns, and a Rust block with syntax colours",
            Act::CSend,
            4000,
        ),
        b(Act::CState("rich table and code"), 300),
        b(Act::Nothing, 6000),
        scene(
            "c43b-code-colours",
            "tern-only",
            "The revealed Rust block in syntax colours",
            Act::CState("code colours"),
            2500,
        ),
        b(Act::CNewChat, 1200),
        b(
            Act::CTypePath("Explain attention with the formula and a diagram"),
            200,
        ),
        scene(
            "c44-rich-math-mermaid",
            "tern-only",
            "Inline and display math typeset by RaTeX, and a Mermaid flowchart drawn by merman",
            Act::CSend,
            4500,
        ),
        b(Act::CState("rich math and mermaid"), 300),
        scene(
            "c44b-light-theme",
            "tern-only",
            "Light theme: the formulas and the diagram in the light page's ink",
            Act::Theme,
            3000,
        ),
        b(Act::Theme, 1500),
        // #73: Math and Mermaid blocks, block export, the @ menu's keys,
        // folder logos.
        b(Act::CNewChat, 1200),
        b(
            Act::CTypePath(
                "Write the attention formula in LaTeX, inline and as a display equation",
            ),
            200,
        ),
        scene(
            "c46-math-block",
            "tern-only",
            "gpt-oss's \\( \\) and \\[ \\begin{align*} \\] answer typed as Math blocks: Rendered | LaTeX, Copy, Export",
            Act::CSend,
            4500,
        ),
        b(Act::CState("math block"), 300),
        scene(
            "c46b-math-latex",
            "tern-only",
            "The align Math block toggled to LaTeX: its source",
            Act::CClick("math-", "-b"),
            2500,
        ),
        b(Act::CClick("math-", "-a"), 1200),
        scene(
            "c46c-math-export-menu",
            "tern-only",
            "Export ▾ on the formula: LaTeX, SVG, PNG and Save as…",
            Act::CClick("math-", "-export"),
            2500,
        ),
        scene(
            "c46d-math-png-saved",
            "tern-only",
            "Saved the formula as PNG into the folder: toast with Open folder",
            Act::CClick("math-", "-save-2"),
            3000,
        ),
        b(Act::CCollectExports, 300),
        b(Act::ToastsClear, 400),
        b(Act::CNewChat, 1200),
        b(
            Act::CTypePath(
                "Draw the transformer encoder as a Mermaid flowchart with subgraphs, left to right",
            ),
            200,
        ),
        scene(
            "c47-mermaid-block",
            "tern-only",
            "gpt-oss's Transformer diagram (unquoted labels, %% comments) drawn in a Mermaid block",
            Act::CSend,
            5000,
        ),
        b(Act::CState("mermaid block"), 300),
        scene(
            "c47b-mermaid-code",
            "tern-only",
            "The Mermaid block toggled to Code",
            Act::CClick("mermaid-", "-b"),
            2500,
        ),
        b(Act::CClick("mermaid-", "-a"), 1500),
        b(Act::CClick("mermaid-", "-export"), 800),
        scene(
            "c47c-mermaid-png-saved",
            "tern-only",
            "Saved the diagram as PNG",
            Act::CClick("mermaid-", "-save-2"),
            3000,
        ),
        b(Act::CCollectExports, 300),
        b(Act::ToastsClear, 400),
        b(Act::CNewChat, 1200),
        b(
            Act::CTypePath("Compare quicksort, merge sort and heapsort"),
            200,
        ),
        b(Act::CSend, 4000),
        b(Act::Nothing, 6000),
        scene(
            "c48-table-export-menu",
            "tern-only",
            "Export ▾ open under the table: CSV, Markdown, Excel, Save as…",
            Act::CClick("table-", "-export"),
            2500,
        ),
        scene(
            "c48b-table-xlsx-saved",
            "tern-only",
            "Saved the table as an Excel workbook: toast with Open folder",
            Act::CClick("table-", "-save-2"),
            3000,
        ),
        scene(
            "c48c-code-export-menu",
            "tern-only",
            "Export ▾ open in the code block's corner, over the code",
            Act::CClick("code-", "-export"),
            2500,
        ),
        b(Act::CClick("code-", "-save-0"), 2000),
        b(Act::CCollectExports, 300),
        b(Act::ToastsClear, 400),
        b(Act::CNewChat, 1200),
        b(Act::CTypePath("Summarise @{dir}project/"), 1500),
        b(Act::CState("mention rows"), 200),
        b(Act::CDown, 500),
        scene(
            "c49-mention-keys",
            "tern-only",
            "@ menu driven by the keys: Down twice lights the third row; folders show their logos",
            Act::CDown,
            2500,
        ),
        b(Act::ClearComposer, 400),
        b(Act::CAttach("project/src"), 500),
        b(Act::CAttach("project/docs"), 500),
        b(Act::CAttach("project/tests"), 500),
        b(Act::CAttach("project/assets"), 500),
        b(Act::CAttach("project/server.go"), 400),
        b(Act::CAttach("project/report.pl"), 400),
        b(Act::CAttach("project/CMakeLists.txt"), 400),
        b(Act::CAttach("project/lexer.ml"), 400),
        b(Act::CAttach("project/chat.erl"), 2500),
        scene(
            "c50-folder-logos",
            "tern-only",
            "Folder chips with their logos (src, docs, tests, assets) and Go, Perl, CMake, OCaml, Erlang logos without white squares, dark theme",
            Act::CState("folder logos"),
            2500,
        ),
        scene(
            "c50b-folder-logos-light",
            "tern-only",
            "The same chips in the light theme",
            Act::Theme,
            2500,
        ),
        b(Act::Theme, 1500),
        b(Act::CNewChat, 1200),
        b(Act::CTypePath(RETRY_Q), 200),
        scene(
            "c45-retry-status",
            "tern-only",
            "The provider answers 429 with Retry-After: the status line says Retrying in N s (attempt k/n)",
            Act::CSend,
            1800,
        ),
        b(Act::CState("retry status"), 4500),
        b(Act::CState("answered after retry"), 300),
        scene(
            "c30-settings-read-aloud",
            "tern-only",
            "Settings › Speech › Read aloud: voice, speed and the voice model",
            Act::Settings(Section::Speech),
            3500,
        ),
        b(Act::CloseSettings, 800),
        scene(
            "c12-toasts",
            "tern-only",
            "Info and error toasts",
            Act::Toasts,
            3500,
        ),
        b(Act::CState("toasts"), 200),
        b(Act::ToastsClear, 600),
        b(Act::Quit, 0),
    ]
}

/// Writes the composer tour's files (a note, a plan, a picture, a short
/// tone as a voice memo) and starts the mock model as the chat's
/// provider, with its `whisper-1` for audio files.
fn seed_composer(prefs: &mut tern_agent::Preferences) {
    let dir = tour_files();
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(dir.join("drafts"));
    let _ = std::fs::write(dir.join("tour-notes.md"), TOUR_NOTES);
    let _ = std::fs::write(dir.join("tour-plan.md"), TOUR_PLAN);
    let _ = std::fs::write(dir.join("drafts").join("chapter-2.md"), "# Chapter 2\n");
    let _ = std::fs::write(dir.join("photo.png"), mock::png(96, 64));
    let _ = std::fs::write(dir.join("memo.wav"), mock::wav(16_000, 3));
    // #69: a two-page PDF with a text layer and a small Word file.
    let _ = std::fs::write(
        dir.join("report.pdf"),
        tern_agent::docs::sample_pdf(&[
            "Quarterly report: revenue grew in Lahore and Islamabad this quarter.",
            "Next steps: hire two engineers and ship the local models release.",
        ]),
    );
    let _ = std::fs::write(dir.join("notes.docx"), tern_agent::docs::sample_docx());
    // #71: files for the preview cards' kind badges.
    let _ = std::fs::write(
        dir.join("train.py"),
        "import torch\n\n# Train the tern classifier\nmodel = torch.nn.Linear(4, 2)\n",
    );
    let _ = std::fs::write(
        dir.join("lab.ipynb"),
        r##"{"cells": [{"cell_type": "markdown", "source": ["# Lab 4: caches"]}], "nbformat": 4, "nbformat_minor": 5}"##,
    );
    let _ = std::fs::write(
        dir.join("data.csv"),
        "city,students\nLahore,120\nIslamabad,80\n",
    );
    // #72: files for the file-type logos (a workbook, C++, an archive and
    // a kind Tern has no logo for).
    let _ = std::fs::write(dir.join("marks.xlsx"), tern_agent::docs::sample_xlsx());
    let _ = std::fs::write(
        dir.join("main.cpp"),
        "#include <iostream>\n\nint main() {\n    std::cout << \"tern\\n\";\n}\n",
    );
    let _ = std::fs::write(dir.join("archive.zip"), tern_agent::docs::sample_docx());
    let _ = std::fs::write(dir.join("mystery.qqq"), "no one knows what this is\n");
    // #73: folders with their own logos, and logos whose white squares
    // were cleared.
    for (folder, file, text) in [
        ("src", "main.rs", "fn main() {}\n"),
        ("docs", "guide.md", "# Guide\n"),
        ("tests", "smoke.py", "def test_smoke():\n    assert True\n"),
        ("assets", "readme.txt", "logos\n"),
    ] {
        let at = dir.join("project").join(folder);
        let _ = std::fs::create_dir_all(&at);
        let _ = std::fs::write(at.join(file), text);
    }
    for (file, text) in [
        ("server.go", "package main\n\nfunc main() {}\n"),
        ("report.pl", "print \"tern\\n\";\n"),
        ("CMakeLists.txt", "project(tern)\n"),
        ("lexer.ml", "let () = print_endline \"tern\"\n"),
        ("chat.erl", "-module(chat).\n"),
    ] {
        let _ = std::fs::write(dir.join("project").join(file), text);
    }
    // #70: a scan with only a watermark, a text page plus a scan, and old
    // Office files (one damaged).
    let _ = std::fs::write(
        dir.join("CA-3 scan.pdf"),
        tern_agent::docs::sample_scanned_pdf(3, "Scanned with CamScanner"),
    );
    let _ = std::fs::write(
        dir.join("lecture-mixed.pdf"),
        tern_agent::docs::sample_mixed_pdf(
            "Lecture 3: the memory hierarchy, registers, caches, main memory and disk, each larger and slower than the one before it.",
        ),
    );
    let _ = std::fs::write(
        dir.join("budget.xls"),
        tern_agent::office_legacy::sample_xls(),
    );
    let _ = std::fs::write(
        dir.join("deck.ppt"),
        tern_agent::office_legacy::sample_ppt(),
    );
    let _ = std::fs::write(
        dir.join("file_example_PPT_250kB.ppt"),
        b"this is not a PowerPoint file".as_slice(),
    );
    prefs.note_recent_file(&dir.join("tour-notes.md").display().to_string());
    match mock::start() {
        Ok(port) => {
            log(&format!("TOUR mock model on 127.0.0.1:{port}"));
            // The same mock answers web searches the SearXNG way, and as
            // Wikipedia for the #67 scene.
            tern_agent::search::set_wikipedia_base(Some(format!("http://127.0.0.1:{port}")));
            prefs.search = Some("searxng".into());
            prefs.search_url = Some(format!("http://127.0.0.1:{port}"));
            prefs.custom_providers = vec![tern_agent::providers::CustomProvider {
                id: "tourmock".into(),
                name: "Tour mock".into(),
                base_url: format!("http://127.0.0.1:{port}/v1"),
                needs_key: false,
                models: vec!["tour-chat".into(), "whisper-1".into()],
                ..Default::default()
            }];
            prefs.provider = Some("tourmock".into());
            prefs.model = Some("tour-chat".into());
            prefs.default_stt_model = Some("tourmock/whisper-1".into());
        }
        Err(e) => log(&format!("TOUR error mock model did not start: {e}")),
    }
}

/// A tiny OpenAI-compatible server for the composer tour: lists two
/// models, transcribes any audio as [`MEMO_TEXT`], and answers chats. When
/// the user's message names a file by its full path and nothing was read
/// yet this turn, it calls `file_read` on it; after a tool result it
/// answers from the result; otherwise it answers from what was attached.
mod mock {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};

    use serde_json::{Value, json};

    pub(super) fn start() -> std::io::Result<u16> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    if let Err(e) = serve(stream) {
                        super::log(&format!("TOUR mock error {e}"));
                    }
                });
            }
        });
        Ok(port)
    }

    fn serve(stream: TcpStream) -> std::io::Result<()> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_owned();
        let path = parts.next().unwrap_or_default().to_owned();
        let (mut len, mut chunked) = (0usize, false);
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
                break;
            }
            let lower = h.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:") {
                len = v.trim().parse().unwrap_or(0);
            }
            if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
                chunked = true;
            }
        }
        let body = if chunked {
            let mut out = Vec::new();
            loop {
                let mut size = String::new();
                reader.read_line(&mut size)?;
                let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
                if n == 0 {
                    let mut end = String::new();
                    let _ = reader.read_line(&mut end);
                    break;
                }
                let mut chunk = vec![0; n + 2];
                reader.read_exact(&mut chunk)?;
                chunk.truncate(n);
                out.extend(chunk);
            }
            out
        } else {
            let mut b = vec![0; len];
            reader.read_exact(&mut b)?;
            b
        };
        // #72: the first chat request for the retry scene is rate limited.
        if rate_limit_once(&body) {
            super::log("TOUR mock 429 Retry-After 4");
            let reply = json!({"error": {"message": "Rate limit reached for tour-chat. Please try again in 4s.", "type": "rate_limit_error"}}).to_string();
            let mut out = stream;
            write!(
                out,
                "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nRetry-After: 4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.len()
            )?;
            out.write_all(reply.as_bytes())?;
            return out.flush();
        }
        let (ctype, reply) = route(&method, &path, &body);
        // #71: a slow answer streams a word every 250 ms, so a scene can
        // stop it or open another chat while it runs.
        if ctype == "text/event-stream" && slow(&body) {
            let mut out = stream;
            write!(
                out,
                "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nConnection: close\r\n\r\n"
            )?;
            for event in reply.split_inclusive("\n\n") {
                out.write_all(event.as_bytes())?;
                out.flush()?;
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            return Ok(());
        }
        super::log(&format!(
            "TOUR mock {method} {path} -> {} bytes",
            reply.len()
        ));
        let mut out = stream;
        write!(
            out,
            "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            reply.len()
        )?;
        out.write_all(reply.as_bytes())?;
        out.flush()
    }

    fn route(method: &str, path: &str, body: &[u8]) -> (&'static str, String) {
        if method == "GET" && path.ends_with("/models") {
            let data = ["tour-chat", "whisper-1"]
                .map(|id| json!({"id": id, "object": "model", "owned_by": "tour"}));
            return (
                "application/json",
                json!({"object": "list", "data": data}).to_string(),
            );
        }
        if method == "GET" && (path == "/search" || path.starts_with("/search?")) {
            // Slow enough for the recording to show the status line.
            std::thread::sleep(std::time::Duration::from_millis(2500));
            let q = query_param(path, "q").unwrap_or_default();
            super::log(&format!("TOUR mock search q={q:?}"));
            let results: Vec<Value> = super::WEB_HITS
                .iter()
                .map(
                    |(title, url, content)| json!({"title": title, "url": url, "content": content}),
                )
                .collect();
            return (
                "application/json",
                json!({"query": q, "results": results}).to_string(),
            );
        }
        if method == "GET" && path.starts_with("/w/api.php") {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let q = query_param(path, "srsearch").unwrap_or_default();
            super::log(&format!("TOUR mock wikipedia srsearch={q:?}"));
            let rows: Vec<Value> = super::WIKI_OFF_TOPIC
                .iter()
                .enumerate()
                .map(|(i, (title, snippet))| json!({"ns": 0, "title": title, "pageid": i + 1, "snippet": snippet}))
                .collect();
            return (
                "application/json",
                json!({"batchcomplete": true, "query": {"search": rows}}).to_string(),
            );
        }
        if path.ends_with("/audio/transcriptions") {
            return (
                "application/json",
                json!({"text": super::MEMO_TEXT}).to_string(),
            );
        }
        let req: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let stream = req["stream"].as_bool().unwrap_or(false);
        let (text, call) = decide(&req);
        if stream {
            ("text/event-stream", sse(&text, call.as_ref()))
        } else {
            let mut message = json!({"role": "assistant", "content": text});
            if let Some((name, args)) = &call {
                message["tool_calls"] = json!([{"id": "call_tour", "type": "function",
                    "function": {"name": name, "arguments": args.to_string()}}]);
            }
            let finish = if call.is_some() { "tool_calls" } else { "stop" };
            (
                "application/json",
                json!({"id": "tour", "object": "chat.completion", "model": "tour-chat",
                    "choices": [{"index": 0, "message": message, "finish_reason": finish}],
                    "usage": {"prompt_tokens": 120, "completion_tokens": 40, "total_tokens": 160}})
                .to_string(),
            )
        }
    }

    /// Whether this is the first chat request whose last user message is
    /// the retry scene's question (#72).
    fn rate_limit_once(body: &[u8]) -> bool {
        static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let req: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let has_tools = req["tools"].as_array().is_some_and(|t| !t.is_empty());
        let asks = has_tools
            && req["messages"]
                .as_array()
                .and_then(|m| m.iter().rev().find(|m| m["role"] == "user"))
                .is_some_and(|m| text_of(m).contains(super::RETRY_Q));
        asks && !DONE.swap(true, std::sync::atomic::Ordering::SeqCst)
    }

    /// Whether the request's last user message asks for a slow answer.
    fn slow(body: &[u8]) -> bool {
        let req: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let has_tools = req["tools"].as_array().is_some_and(|t| !t.is_empty());
        has_tools
            && req["messages"]
                .as_array()
                .and_then(|m| m.iter().rev().find(|m| m["role"] == "user"))
                .is_some_and(|m| text_of(m).contains("slowly"))
    }

    /// A query parameter of a request path, decoded.
    pub(super) fn query_param(path: &str, key: &str) -> Option<String> {
        let query = path.split_once('?')?.1;
        query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == key).then(|| decode(v))
        })
    }

    fn decode(v: &str) -> String {
        let bytes = v.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&v[i + 1..i + 3], 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                },
                b => out.push(b),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// The text of a message's content (a string, or text parts).
    fn text_of(m: &Value) -> String {
        match &m["content"] {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    }

    /// The reply: text, or a tool call (name, arguments).
    fn decide(req: &Value) -> (String, Option<(String, Value)>) {
        let msgs = req["messages"].as_array().cloned().unwrap_or_default();
        let has_tools = req["tools"].as_array().is_some_and(|t| !t.is_empty());
        let Some(last_user) = msgs.iter().rposition(|m| m["role"] == "user") else {
            return ("Tour chat".into(), None);
        };
        let after = &msgs[last_user + 1..];
        if let Some(tool) = after.iter().rev().find(|m| m["role"] == "tool") {
            let result = text_of(tool);
            let line = first_line(&result);
            return (
                format!("I read the file with file_read. It says: “{line}”"),
                None,
            );
        }
        let user = text_of(&msgs[last_user]);
        // #71: the side chat's question, and slow answers for the
        // concurrent-chats and Stop scenes.
        if user.starts_with("About this part of the conversation:") {
            return (super::SIDE_A.into(), None);
        }
        if has_tools && user.contains("slowly") {
            return (super::SLOW_A.into(), None);
        }
        if has_tools && user.contains("What is a TLB?") {
            return (super::TLB_A.into(), None);
        }
        // #72: rich answers.
        if has_tools && user.contains("Compare quicksort") {
            return (super::RICH_TABLE_A.into(), None);
        }
        if has_tools && user.contains("Explain attention with the formula") {
            return (super::RICH_MATH_A.into(), None);
        }
        // #73: gpt-oss's answers that showed raw LaTeX and failed to draw.
        if has_tools && user.contains("attention formula in LaTeX") {
            return (crate::mathsrc::SAMPLE.into(), None);
        }
        if has_tools && user.contains("as a Mermaid flowchart") {
            return (
                format!(
                    "Here is the encoder:\n\n```mermaid\n{}\n```\n",
                    crate::mermaidsrc::SAMPLE.trim_end()
                ),
                None,
            );
        }
        if has_tools && user.contains(super::RETRY_Q) {
            return (super::RETRY_A.into(), None);
        }
        // Web results Tern sent back (its own search, or the model's call):
        // a cited answer. The follow-up says it searched again.
        if user.starts_with("Result of `web_search`") && user.contains("Wikipedia has nothing") {
            return (
                "Wikipedia has nothing on SubQ Snap 1.1, and I won't guess. A Tavily or Brave key under Settings › Search & Crawlers searches the whole web.".into(),
                None,
            );
        }
        if user.starts_with("Result of `web_search`") {
            let asked = msgs[..last_user]
                .iter()
                .rev()
                .filter(|m| m["role"] == "user")
                .map(text_of)
                .find(|t| !t.starts_with("Result of `"))
                .unwrap_or_default();
            let (title, url, content) = if asked.contains("tern") || asked.contains("browse") {
                super::WEB_HITS[1]
            } else {
                super::WEB_HITS[0]
            };
            let lead = if asked.contains("browse") {
                "Searched again for your earlier question:"
            } else {
                "From the web:"
            };
            return (format!("{lead} {content} [1]\n\n[1] {title} — {url}"), None);
        }
        if has_tools && user.contains("SubQ Snap 1.1") {
            return (
                String::new(),
                Some((
                    "web_search".into(),
                    json!({"query": "SubQ Snap 1.1 AI model definition or meaning"}),
                )),
            );
        }
        if has_tools && user.contains("Who created LangChain") {
            return (
                String::new(),
                Some(("web_search".into(), json!({"query": "LangChain creator"}))),
            );
        }
        if has_tools && user.contains("Arctic terns migrate") {
            return (
                "I'm not sure. I don't have real-time access to the internet, so I can't look that up.".into(),
                None,
            );
        }
        if has_tools && user.contains(super::URDU_QUESTION) {
            return (super::URDU_ANSWER.into(), None);
        }
        if !has_tools {
            // A title or other side request.
            return ("Tour chat".into(), None);
        }
        if user.contains(super::MEMO_TEXT) || user.contains("FYP draft") {
            return (
                "Your memo asks to send the FYP draft to Doctor Amina on Friday.".into(),
                None,
            );
        }
        if user.contains("FYP demo is on 14 November") {
            return (
                "From the attached notes: the FYP demo is on 14 November in Lab 3.".into(),
                None,
            );
        }
        if user.contains("Week 1: retrieval baseline") {
            return (
                "The plan (read before answering): Week 1 retrieval baseline, Week 2 FLARE, Week 3 evaluation.".into(),
                None,
            );
        }
        if let Some(path) = user
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| "\"'`?,.;:()".contains(c)))
            .find(|w| w.ends_with(".md") && (w.starts_with('/') || w.contains(":\\")))
        {
            return (
                String::new(),
                Some(("file_read".into(), json!({"path": path}))),
            );
        }
        ("Got it.".into(), None)
    }

    fn first_line(text: &str) -> String {
        text.lines()
            .map(|l| l.trim_start_matches(['#', ' ']).trim())
            .find(|l| !l.is_empty() && !l.starts_with('<') && !l.starts_with('{'))
            .unwrap_or("(empty)")
            .chars()
            .take(160)
            .collect()
    }

    fn sse(text: &str, call: Option<&(String, Value)>) -> String {
        let mut out = String::new();
        let mut push = |v: Value| {
            out.push_str("data: ");
            out.push_str(&v.to_string());
            out.push_str("\n\n");
        };
        push(json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}}]}));
        for piece in text.split_inclusive(' ') {
            push(json!({"choices": [{"index": 0, "delta": {"content": piece}}]}));
        }
        let finish = match call {
            Some((name, args)) => {
                push(
                    json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0,
                    "id": "call_tour", "type": "function",
                    "function": {"name": name, "arguments": args.to_string()}}]}}]}),
                );
                "tool_calls"
            }
            None => "stop",
        };
        push(
            json!({"choices": [{"index": 0, "delta": {}, "finish_reason": finish}],
            "usage": {"prompt_tokens": 120, "completion_tokens": 40, "total_tokens": 160}}),
        );
        out.push_str("data: [DONE]\n\n");
        out
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }

    /// A small gradient picture as an uncompressed PNG.
    pub(super) fn png(w: u32, h: u32) -> Vec<u8> {
        let mut raw = Vec::new();
        for y in 0..h {
            raw.push(0u8);
            for x in 0..w {
                raw.extend([(x * 255 / w) as u8, (y * 255 / h) as u8, 200u8]);
            }
        }
        // zlib with stored deflate blocks.
        let mut z = vec![0x78, 0x01];
        let chunks: Vec<&[u8]> = raw.chunks(65_535).collect();
        for (i, c) in chunks.iter().enumerate() {
            z.push(u8::from(i + 1 == chunks.len()));
            let n = c.len() as u16;
            z.extend(n.to_le_bytes());
            z.extend((!n).to_le_bytes());
            z.extend_from_slice(c);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &x in &raw {
            a = (a + u32::from(x)) % 65_521;
            b = (b + a) % 65_521;
        }
        z.extend(((b << 16) | a).to_be_bytes());
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut chunk = |kind: &[u8], data: &[u8]| {
            out.extend((data.len() as u32).to_be_bytes());
            let mut c = kind.to_vec();
            c.extend_from_slice(data);
            out.extend_from_slice(&c);
            out.extend(crc32(&c).to_be_bytes());
        };
        let mut ihdr = Vec::new();
        ihdr.extend(w.to_be_bytes());
        ihdr.extend(h.to_be_bytes());
        ihdr.extend([8, 2, 0, 0, 0]);
        chunk(b"IHDR", &ihdr);
        chunk(b"IDAT", &z);
        chunk(b"IEND", &[]);
        out
    }

    /// `secs` of a quiet tone as a 16-bit mono WAV.
    pub(super) fn wav(rate: u32, secs: u32) -> Vec<u8> {
        let n = rate * secs;
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((36 + n * 2).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(rate.to_le_bytes());
        out.extend((rate * 2).to_le_bytes());
        out.extend(2u16.to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend((n * 2).to_le_bytes());
        for i in 0..n {
            let s =
                ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 3000.0) as i16;
            out.extend(s.to_le_bytes());
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_mock_reads_a_typed_path_then_answers_from_the_result() {
            let req = json!({"stream": true, "tools": [{"type": "function", "function": {"name": "file_read"}}],
                "messages": [{"role": "user", "content": "What does /tmp/tern-tour-files/tour-plan.md say?"}]});
            let (_, call) = decide(&req);
            let (name, args) = call.unwrap();
            assert_eq!(name, "file_read");
            assert_eq!(args["path"], "/tmp/tern-tour-files/tour-plan.md");
            let sse = sse("", Some(&(name, args)));
            assert!(
                sse.contains("\"finish_reason\":\"tool_calls\"")
                    && sse.ends_with("data: [DONE]\n\n")
            );
            let req = json!({"stream": true, "tools": [{"type": "function", "function": {"name": "file_read"}}],
                "messages": [{"role": "user", "content": "x"}, {"role": "assistant", "content": ""},
                    {"role": "tool", "content": "# Plan\n\nWeek 1"}]});
            assert!(decide(&req).0.contains("“Plan”"));
        }

        #[test]
        fn the_mock_searches_refuses_and_answers_from_web_results() {
            assert_eq!(
                query_param("/search?q=Who+created%20LangChain%3F&format=json", "q").as_deref(),
                Some("Who created LangChain?")
            );
            let tools = json!([{"type": "function", "function": {"name": "web_search"}}]);
            let req = json!({"tools": tools, "messages": [{"role": "user", "content": "Who created LangChain?"}]});
            assert_eq!(decide(&req).1.unwrap().0, "web_search");
            let req = json!({"tools": tools, "messages": [{"role": "user", "content": "How far do Arctic terns migrate each year?"}]});
            assert!(decide(&req).0.contains("real-time access"));
            let req = json!({"tools": tools, "messages": [
                {"role": "user", "content": "browse about that"},
                {"role": "assistant", "content": "```tool\n{}\n```"},
                {"role": "user", "content": "Result of `web_search`:\nWeb search results"}]});
            let (text, call) = decide(&req);
            assert!(call.is_none() && text.starts_with("Searched again") && text.contains("[1]"));
        }

        #[test]
        fn the_tour_picture_and_memo_are_well_formed() {
            let p = png(4, 3);
            assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(&p[p.len() - 8..p.len() - 4], b"IEND");
            let w = wav(16_000, 1);
            assert_eq!(w.len(), 44 + 32_000);
        }
    }
}
