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
    let pages: [(&'static str, &'static str, &'static str, Section); 12] = [
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
        self.tour.is_some()
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
        self.composer
            .update(cx, |c, cx| c.set_value(text.clone(), window, cx));
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
