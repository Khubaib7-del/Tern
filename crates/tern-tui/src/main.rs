//! The `tern` command.
//!
//! `tern` (or `tern chat`) opens the TUI. `tern doctor` and `tern provider`
//! are plain commands. `tern ask` and `tern run --json` are the next slices.

#![forbid(unsafe_code)]

mod browse;
mod cards;
mod continuity;
mod decide;
mod eval;
mod extend;
mod research;
mod runs;
mod study;
mod tui;

use std::io::{BufRead, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tern_agent::doctor::Status;
use tern_agent::providers::{self, SecretStore};
use tern_core::SecretString;

#[derive(Debug, Parser)]
#[command(
    name = "tern",
    version,
    about = "Tern: research and study, with sources you can check."
)]
struct Cli {
    /// With no command, Tern opens the chat TUI.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open the chat TUI (the same as running `tern` alone).
    Chat(ChatArgs),
    /// Check that this machine can run Tern: keychain, data directory, disk, database.
    Doctor {
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Open a page (or search for words) the way the built-in browser
    /// does: the page as Tern's reader shows it plus Tern's explanation
    /// with numbered citations, or the search results for words.
    Browse {
        /// A web address, or words to search for.
        #[arg(required = true, num_args = 1..)]
        input: Vec<String>,
        #[command(flatten)]
        chat: ChatArgs,
        /// Skip Tern's explanation; print the page only.
        #[arg(long)]
        no_explain: bool,
        /// Also show the real page in a browser window (the tern-browser helper).
        #[arg(long)]
        open: bool,
        /// Most characters of the page to print.
        #[arg(long, default_value_t = 12_000)]
        max_chars: usize,
    },
    /// Ask a question and get a short answer with numbered, checked citations.
    Ask {
        /// The question. Quote it, or pass several words.
        #[arg(required = true, num_args = 1..)]
        question: Vec<String>,
        #[command(flatten)]
        chat: ChatArgs,
        /// Pages to read from the search results.
        #[arg(long, default_value_t = 4)]
        reads: usize,
        /// Print the full report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Deep research: Tern drafts a plan for you to run, edit, or skip,
    /// then workers research its parts in parallel and one cited answer
    /// is written from their notes.
    Research {
        /// The question. Quote it, or pass several words.
        #[arg(required = true, num_args = 1..)]
        question: Vec<String>,
        /// Plan and run workers (required for now; `tern ask` is the quick path).
        #[arg(long)]
        deep: bool,
        #[command(flatten)]
        chat: ChatArgs,
        /// Workers at once (1 to 6). Defaults to your preference, or 3.
        #[arg(long, short)]
        workers: Option<usize>,
        /// Run the plan as drafted without asking.
        #[arg(long, short)]
        yes: bool,
        /// Print the run report as JSON instead of the answer.
        #[arg(long)]
        json: bool,
    },
    /// Print or export a run report: the question, steps, sources read,
    /// each claim's verdict with its evidence, and what the run used.
    Report {
        /// The chat: `last` (the default), its id, or the start of its id.
        #[arg(default_value = "last")]
        chat: String,
        /// Which turn, 1 for the first. Defaults to the latest answered one.
        #[arg(long, short)]
        turn: Option<usize>,
        /// Print the report as JSON instead of Markdown.
        #[arg(long)]
        json: bool,
        /// Also write the report to this file.
        #[arg(long, short)]
        out: Option<std::path::PathBuf>,
        /// List recent chats and their ids instead.
        #[arg(long)]
        list: bool,
    },
    /// Resume a research run that stopped (a crash, a forced quit, a stop)
    /// from its last checkpoint, without paying again for finished work.
    Resume {
        /// The chat: `last` (the default), its id, or the start of its id.
        #[arg(default_value = "last")]
        chat: String,
        /// List every run waiting to be resumed instead.
        #[arg(long)]
        list: bool,
        /// Forget the chat's unfinished run instead of resuming it.
        #[arg(long)]
        discard: bool,
        /// Print the run report as JSON instead of the answer.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        args: ChatArgs,
    },
    /// Steer a run going in another window or terminal: `focus on 2024
    /// sources`, `drop sub-question 3`, `add: cost comparison`,
    /// `redirect 2: open-source options`.
    Steer {
        /// The chat: `last`, its id, or the start of its id.
        chat: String,
        /// The note.
        #[arg(required = true, num_args = 1..)]
        note: Vec<String>,
    },
    /// Put research questions on watch and see what changed since the last run.
    #[command(subcommand)]
    Watch(runs::WatchCmd),
    /// Print a run's source ledger (every URL considered, read, or skipped,
    /// and why) and budget ledger (tokens, pages, and time per step and
    /// per worker).
    Ledger {
        /// The chat: `last` (the default), its id, or the start of its id.
        #[arg(default_value = "last")]
        chat: String,
        #[arg(long, short)]
        turn: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// Study materials: add files, notes, reports, and links; list topics
    /// and mastery; see past quizzes and vivas.
    #[command(subcommand)]
    Study(study::StudyCmd),
    /// A tutor for one topic of your materials (the weakest when none is
    /// named), citing pages as it teaches.
    Tutor {
        /// A topic: its number, id, or title.
        topic: Option<String>,
        /// explain, socratic, or new (explain like I'm new).
        #[arg(long, default_value = "explain")]
        mode: String,
        /// No model: quote passages from your materials instead.
        #[arg(long)]
        offline: bool,
        #[command(flatten)]
        chat: ChatArgs,
    },
    /// A quiz on one topic, or on all your materials, graded as you go.
    Quiz {
        /// A topic: its number, id, or title. Leave out for all materials.
        topic: Option<String>,
        /// How many questions (1 to 30). Defaults to your choice, or 5.
        #[arg(long, short)]
        n: Option<usize>,
        /// easy, medium, or hard.
        #[arg(long, short)]
        difficulty: Option<String>,
        /// Question types to mix, comma-separated: mcq, short, tf, blank.
        #[arg(long, short)]
        kinds: Option<String>,
        /// No model: questions written from your materials' sentences.
        #[arg(long)]
        offline: bool,
        /// Print the quiz and its key as JSON instead of taking it.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        chat: ChatArgs,
    },
    /// Viva practice: an examiner asks progressively harder questions on
    /// your project or materials, then scores you on a rubric.
    Viva {
        /// What is being examined. Defaults to your research report or first material.
        #[arg(long, short)]
        title: Option<String>,
        /// How many questions (2 to 15). Defaults to your choice, or 6.
        #[arg(long, short)]
        n: Option<usize>,
        #[arg(long)]
        offline: bool,
        #[command(flatten)]
        chat: ChatArgs,
    },
    /// Flashcards from your materials with spaced repetition (SM-2):
    /// write, review, edit, and export to Anki (.apkg), CSV, or TSV.
    #[command(subcommand)]
    Cards(cards::CardsCmd),
    /// Explain a concept as an isometric machine with cited, step-by-step
    /// captions; saved to your study library, optionally as an SVG.
    #[command(name = "explain3d")]
    Explain3d {
        /// The concept. Leave out to explain a topic (--topic) or let the
        /// model pick the key idea in your materials.
        concept: Vec<String>,
        /// Draw the passages from this topic (its number, id, or title).
        #[arg(long, short)]
        topic: Option<String>,
        /// Write an SVG snapshot here.
        #[arg(long, value_name = "FILE")]
        svg: Option<PathBuf>,
        /// With --svg, the step to light (1-based); the whole machine otherwise.
        #[arg(long)]
        step: Option<usize>,
        /// Also save it to the vault as Explain/<concept>.md with the SVG.
        #[arg(long)]
        vault: bool,
        /// No model: build the scene from your materials' key terms.
        #[arg(long)]
        offline: bool,
        /// Print the scene as JSON.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        chat: ChatArgs,
    },
    /// Clean a chat by message ID: remove `Q3-Q5` to the chat's trash, or
    /// move messages with their context to a new session or sub-session
    /// (the originals stay until you confirm).
    #[command(subcommand)]
    Range(continuity::RangeCmd),
    /// Sub-sessions hung under a message (`Q12` → `Q12.1`, `Q12.2`):
    /// make, list, and run them; a run reports back to the parent.
    #[command(subcommand)]
    Sub(continuity::SubCmd),
    /// Print one message by its ID (`Q12`, `Q12.3`) or `tern://` link.
    Show {
        /// `Q12`, `Q12.3`, or `tern://chat/<id>/Q12.3`.
        id: String,
        #[arg(long, short, default_value = "last")]
        chat: String,
        #[arg(long)]
        json: bool,
    },
    /// Write a handoff packet for a chat: goal, decisions, open questions,
    /// key sources and claims, a ledger snapshot, and next steps.
    Handoff {
        /// The chat: `last` (the default), its id, or the start of its id.
        #[arg(default_value = "last")]
        chat: String,
        /// Write `FILE.md` and `FILE.json` instead of printing Markdown.
        #[arg(long, short, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Print JSON instead of Markdown.
        #[arg(long)]
        json: bool,
    },
    /// Start a new session from a handoff packet alone, and optionally ask
    /// its first question.
    New {
        /// The packet (`.md` or `.json` from `tern handoff`).
        #[arg(long, value_name = "FILE")]
        from_handoff: PathBuf,
        question: Vec<String>,
        #[command(flatten)]
        chat: ChatArgs,
    },
    /// The Library of everything: list, search, bulk delete to the trash,
    /// restore, and empty the trash.
    #[command(subcommand)]
    Library(continuity::LibraryCmd),
    /// A throwaway side question about a chat: it reads the thread and
    /// writes nothing back.
    Side {
        #[arg(required = true, num_args = 1..)]
        question: Vec<String>,
        #[arg(long, short, default_value = "last")]
        chat: String,
        #[command(flatten)]
        args: ChatArgs,
    },
    /// Ask the decision model a fixed-choice question, set it up (off,
    /// local, or a remote provider with your own key), or benchmark it.
    Decide(decide::DecideArgs),
    /// The evaluation ladder: list suites, run a rung (offline by
    /// default), read reports, compare runs, run a usability session.
    #[command(subcommand)]
    Eval(eval::EvalCmd),
    /// MCP servers: add, review (trust preview), switch on and off, check
    /// health, remove.
    #[command(subcommand)]
    Mcp(extend::McpCmd),
    /// Run Tern as an agent inside an editor that speaks the Agent Client
    /// Protocol (Zed and others), over stdin and stdout.
    Acp(ChatArgs),
    /// Skills: SKILL.md folders loaded on demand. List, install, switch,
    /// show, remove.
    #[command(subcommand)]
    Skills(extend::SkillsCmd),
    /// Exactly what the model is told: Tern's prompt, TERN.md rules,
    /// skills, tools, and MCP tools, piece by piece.
    Instructions {
        /// A project by name (its linked folder's rules and skills).
        #[arg(long)]
        project: Option<String>,
        /// A folder to read TERN.md and .tern/skills from (default: here).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Leave MCP servers out (they are started to list their tools).
        #[arg(long)]
        no_mcp: bool,
        #[arg(long)]
        json: bool,
        /// Print the system prompt exactly as sent, without headings.
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        chat: ChatArgs,
    },
    /// Project folders: new, list, mv (file a chat), rename, root, rm.
    #[command(subcommand)]
    Project(extend::ProjectCmd),
    /// Add, test, and remove model providers (bring your own key).
    #[command(subcommand)]
    Provider(ProviderCmd),
    /// Add, test, and remove web search services (bring your own key).
    #[command(subcommand)]
    Search(SearchCmd),
}

#[derive(Debug, Subcommand)]
enum SearchCmd {
    /// Show every search service and whether a key is saved.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Paste a key. Tern runs one search with it and saves it only if it works.
    Add {
        /// Service id: `tavily` or `brave`.
        id: String,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Run a test search with the saved key, or against your SearXNG.
    Test {
        id: String,
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Delete the saved key from the keychain.
    Remove { id: String },
}

#[derive(Debug, Default, clap::Args)]
pub(crate) struct ChatArgs {
    /// Provider id. Defaults to the first provider with a saved key.
    #[arg(long, short)]
    provider: Option<String>,
    /// Model id. Defaults to the provider's first chat model.
    #[arg(long, short)]
    model: Option<String>,
    /// Use a different endpoint (a local server on another port, a proxy).
    #[arg(long)]
    base_url: Option<String>,
    /// Search service id. Defaults to the first with a saved key.
    #[arg(long, short)]
    search: Option<String>,
    /// Your SearXNG address, or another search endpoint.
    #[arg(long)]
    search_url: Option<String>,
}

#[derive(Debug, Subcommand)]
enum ProviderCmd {
    /// Show every provider Tern can use and whether a key is saved.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Paste a key. Tern tests it first and saves it to the OS keychain only if it works.
    Add {
        /// Provider id, for example `openai` or `anthropic`.
        id: String,
        /// Read the key from standard input instead of a hidden prompt.
        #[arg(long)]
        stdin: bool,
        /// Use a different endpoint (a proxy or a compatible server).
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Check the saved key, or a local server, by listing its models.
    Test {
        id: String,
        #[arg(long)]
        base_url: Option<String>,
    },
    /// List every model this key can use, grouped by category (chat,
    /// vision, speech, OCR, image, embeddings, safety, other).
    Models {
        id: String,
        /// Also list deprecated and retired models.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Delete the saved key from the keychain.
    Remove { id: String },
}

fn main() -> ExitCode {
    // Every command's future is laid out in one frame, and in a debug
    // build that outgrows Windows' 1 MB main-thread stack. Run on a thread
    // with room to spare.
    std::thread::Builder::new()
        .name("tern".into())
        .stack_size(16 << 20)
        .spawn(real_main)
        .expect("start the main thread")
        .join()
        .unwrap_or(ExitCode::FAILURE)
}

fn real_main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Chat(ChatArgs::default())) {
        Command::Doctor { json } => run_doctor(json),
        Command::Chat(args) => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            let opts = tern_agent::ChatOptions {
                provider: args.provider,
                model: args.model,
                base_url: args.base_url,
                search: args.search,
                search_url: args.search_url,
                ..Default::default()
            };
            let result = tui::run(&rt, opts);
            rt.shutdown_background();
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(msg) => {
                    eprintln!("{msg}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Browse {
            input,
            chat,
            no_explain,
            open,
            max_chars,
        } => block_on(browse::run(
            input.join(" "),
            chat,
            !no_explain,
            open,
            max_chars,
        )),
        Command::Ask {
            question,
            chat,
            reads,
            json,
        } => block_on(run_ask(question.join(" "), chat, reads, json)),
        Command::Research {
            question,
            deep,
            chat,
            workers,
            yes,
            json,
        } => block_on(research::run(
            question.join(" "),
            deep,
            chat,
            workers,
            yes,
            json,
        )),
        Command::Search(cmd) => block_on(run_search(cmd)),
        Command::Mcp(cmd) => block_on(Box::pin(extend::mcp(cmd))),
        Command::Acp(args) => block_on(extend::acp(args)),
        Command::Skills(cmd) => plain(extend::skills_cmd(cmd)),
        Command::Instructions {
            project,
            root,
            no_mcp,
            json,
            raw,
            chat,
        } => block_on(extend::instructions(project, root, no_mcp, json, raw, chat)),
        Command::Project(cmd) => plain(extend::project(cmd)),
        Command::Resume {
            chat,
            list,
            discard,
            json,
            args,
        } => block_on(runs::resume(chat, args, list, discard, json)),
        Command::Steer { chat, note } => match runs::steer(chat, note) {
            Ok(()) => ExitCode::SUCCESS,
            Err(msg) => {
                eprintln!("{msg}");
                ExitCode::FAILURE
            }
        },
        Command::Watch(cmd) => block_on(runs::watch(cmd)),
        Command::Study(cmd) => block_on(study::study(cmd)),
        Command::Tutor {
            topic,
            mode,
            offline,
            chat,
        } => block_on(study::tutor(topic, mode, offline, chat)),
        Command::Quiz {
            topic,
            n,
            difficulty,
            kinds,
            offline,
            json,
            chat,
        } => block_on(study::quiz(
            topic, n, difficulty, kinds, offline, json, chat,
        )),
        Command::Viva {
            title,
            n,
            offline,
            chat,
        } => block_on(study::viva(title, n, offline, chat)),
        Command::Cards(cmd) => block_on(cards::run(cmd)),
        Command::Decide(args) => block_on(decide::run(args)),
        Command::Eval(cmd) => block_on(Box::pin(eval::run(cmd))),
        Command::Range(cmd) => sync(continuity::range_cmd(cmd)),
        Command::Sub(cmd) => block_on(continuity::sub_cmd(cmd)),
        Command::Show { id, chat, json } => sync(continuity::show(id, chat, json)),
        Command::Handoff { chat, out, json } => sync(continuity::handoff_cmd(chat, out, json)),
        Command::New {
            from_handoff,
            question,
            chat,
        } => block_on(continuity::new_cmd(from_handoff, question, chat)),
        Command::Library(cmd) => sync(continuity::library_cmd(cmd)),
        Command::Side {
            question,
            chat,
            args,
        } => block_on(continuity::side_cmd(question, chat, args)),
        Command::Explain3d {
            concept,
            topic,
            svg,
            step,
            vault,
            offline,
            json,
            chat,
        } => block_on(cards::explain3d(
            concept, topic, svg, step, vault, offline, json, chat,
        )),
        Command::Ledger { chat, turn, json } => block_on(runs::ledger(chat, turn, json)),
        Command::Report {
            chat,
            turn,
            json,
            out,
            list,
        } => block_on(run_report(chat, turn, json, out, list)),
        Command::Provider(cmd) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            match rt.block_on(run_provider(cmd)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(msg) => {
                    eprintln!("{msg}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn sync(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

fn plain(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

fn block_on(f: impl std::future::Future<Output = Result<(), String>>) -> ExitCode {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    match rt.block_on(f) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

async fn run_ask(question: String, chat: ChatArgs, reads: usize, json: bool) -> Result<(), String> {
    let opts = tern_agent::AskOptions {
        provider: chat.provider,
        model: chat.model,
        base_url: chat.base_url,
        search: chat.search,
        search_url: chat.search_url,
        reads: reads.clamp(1, 8),
        ..Default::default()
    };
    let tty = std::io::stderr().is_terminal();
    let progress = move |line: &str| {
        if tty {
            eprintln!("\x1b[2m• {line}\x1b[0m");
        } else {
            eprintln!("• {line}");
        }
    };
    let r = tern_agent::ask::ask(&question, opts, &progress).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&r).expect("report serialises")
        );
        return Ok(());
    }
    let links = std::io::stdout().is_terminal();
    println!();
    println!("{}", r.answer);
    println!();
    println!("Sources");
    for s in &r.sources {
        let mark = if s.cited { " " } else { "·" };
        let title = if links {
            format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", s.url, s.title)
        } else {
            s.title.clone()
        };
        println!(" {mark}[{}] {title} · {}", s.n, s.site);
        println!("      {}", s.url);
    }
    println!();
    let mut tail = vec![format!(
        "Read {} of {} pages via {}",
        r.sources.len(),
        r.sources.len() + r.unread.len(),
        r.search_service
    )];
    let uncited = r.sources.iter().filter(|s| !s.cited).count();
    if uncited > 0 {
        tail.push(format!("{uncited} read but not cited (·)"));
    }
    if !r.invalid_citations.is_empty() {
        let nums: Vec<String> = r
            .invalid_citations
            .iter()
            .map(|n| format!("[{n}]"))
            .collect();
        tail.push(format!("{} match no source", nums.join(" ")));
    }
    let est = if r.usage.measure == tern_core::Measure::Estimated {
        "~"
    } else {
        ""
    };
    tail.push(format!("{} · {est}{} tokens", r.model, r.usage.total()));
    tail.push(format!("{:.1} s", r.elapsed_ms as f64 / 1000.0));
    println!("{}", tail.join(" · "));
    Ok(())
}

async fn run_report(
    chat: String,
    turn: Option<usize>,
    json: bool,
    out: Option<std::path::PathBuf>,
    list: bool,
) -> Result<(), String> {
    if list {
        let recent = tern_agent::recent_sessions(20)?;
        if recent.is_empty() {
            println!("There are no chats yet.");
        }
        for s in recent {
            println!("{}  {}  {}", s.id, s.created_at, s.title);
        }
        return Ok(());
    }
    let (report, files) = tern_agent::report::report_for(&chat, turn).await?;
    let text = if json {
        report.to_json()
    } else {
        report.to_markdown()
    };
    if let Some(path) = &out {
        std::fs::write(path, &text)
            .map_err(|e| format!("Tern could not write {}: {e}", path.display()))?;
        eprintln!("Wrote {}", path.display());
    } else {
        println!("{text}");
    }
    eprintln!(
        "Saved as {} and {}",
        files.markdown.display(),
        files.json.display()
    );
    Ok(())
}

async fn run_search(cmd: SearchCmd) -> Result<(), String> {
    use tern_agent::search;
    let keychain = providers::keychain();
    let store: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
    let need_store = || {
        keychain
            .as_ref()
            .map(|b| b.as_ref())
            .map_err(|e| format!("{e}. Keys cannot be saved here yet."))
    };
    match cmd {
        SearchCmd::List { json } => {
            let rows = search::list(store);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).expect("rows serialise")
                );
                return Ok(());
            }
            for r in rows {
                let state = if r.self_hosted {
                    "self-hosted, no key"
                } else if r.key_saved {
                    "key saved"
                } else {
                    "no key"
                };
                println!("  {:<8}  {:<12}  {state}", r.id, r.name);
            }
            Ok(())
        }
        SearchCmd::Add {
            id,
            stdin,
            base_url,
        } => {
            let store = need_store()?;
            if let Some(spec) = tern_crawl_spec(&id)
                && !spec.is_empty()
            {
                eprintln!("Create a key at {spec}");
            }
            let key = read_key(&id, stdin)?;
            let n = search::add(&id, key, store, base_url.as_deref()).await?;
            println!("Saved. The test search returned {n} result(s).");
            Ok(())
        }
        SearchCmd::Test { id, base_url } => {
            let n = search::test(&id, store, base_url.as_deref()).await?;
            println!("Search works: {n} result(s) for a test query.");
            Ok(())
        }
        SearchCmd::Remove { id } => {
            let existed = search::remove(&id, need_store()?)?;
            println!(
                "{}",
                if existed {
                    "Key deleted from the keychain."
                } else {
                    "No key was saved for that service."
                }
            );
            Ok(())
        }
    }
}

fn tern_crawl_spec(id: &str) -> Option<&'static str> {
    tern_agent::search::list(None)
        .into_iter()
        .find(|r| r.id == id)
        .map(|r| r.key_url)
}

async fn run_provider(cmd: ProviderCmd) -> Result<(), String> {
    let keychain = providers::keychain();
    let store: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
    let need_store = || {
        keychain
            .as_ref()
            .map(|b| b.as_ref())
            .map_err(|e| format!("{e}. Keys cannot be saved here yet."))
    };
    match cmd {
        ProviderCmd::List { json } => {
            let rows = providers::list(store);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).expect("rows serialise")
                );
                return Ok(());
            }
            let width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
            for r in rows {
                let state = if r.local {
                    "local, no key"
                } else if r.key_saved {
                    "key saved"
                } else {
                    "no key"
                };
                println!("  {:<10}  {:<width$}  {state}", r.id, r.name);
            }
            if let Err(e) = &keychain {
                println!();
                println!("{}.", capitalise(e));
            }
            Ok(())
        }
        ProviderCmd::Add {
            id,
            stdin,
            base_url,
        } => {
            let spec = providers::spec_for(&id).map_err(|e| e.to_string())?;
            let store = need_store()?;
            if !spec.key_url.is_empty() {
                eprintln!("Create a key at {}", spec.key_url);
            }
            let key = read_key(spec.name, stdin)?;
            let r = providers::add(&id, key, store, base_url.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            println!(
                "Saved. {} accepted the key: {} models, {} for chat, {} ms.",
                r.provider, r.models, r.chat_models, r.latency_ms
            );
            Ok(())
        }
        ProviderCmd::Test { id, base_url } => {
            let r = providers::test(&id, store, base_url.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            println!(
                "{} works at {}: {} models, {} for chat, {} ms.",
                r.provider, r.base_url, r.models, r.chat_models, r.latency_ms
            );
            Ok(())
        }
        ProviderCmd::Models {
            id,
            all,
            base_url,
            json,
        } => {
            let mut models = providers::models(&id, store, base_url.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            let retired = models.iter().filter(|m| m.is_retired()).count();
            if !all {
                models.retain(|m| !m.is_retired());
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&models).expect("models serialise")
                );
                return Ok(());
            }
            for cat in providers::Category::ALL {
                let group: Vec<_> = models.iter().filter(|m| m.category() == cat).collect();
                if group.is_empty() {
                    continue;
                }
                println!("{} ({})", cat.label(), group.len());
                for m in group {
                    let ctx = m
                        .context_window
                        .filter(|_| m.can_chat())
                        .map(|c| format!("  {}k context", c / 1000))
                        .unwrap_or_default();
                    let name = Some(m.label())
                        .filter(|n| *n != m.id)
                        .map(|n| format!("  ({n})"))
                        .unwrap_or_default();
                    let badges: Vec<&str> = m.capabilities.iter().map(|c| c.badge()).collect();
                    let aliases = if m.aliases.is_empty() {
                        String::new()
                    } else {
                        format!("  also {}", m.aliases.join(", "))
                    };
                    let gone = if m.is_retired() { "  [retired]" } else { "" };
                    println!(
                        "  {}{name}{ctx}  [{}]{aliases}{gone}",
                        m.id,
                        badges.join(", ")
                    );
                }
            }
            println!(
                "{} models.{}",
                models.len(),
                if all || retired == 0 {
                    String::new()
                } else {
                    format!(" {retired} deprecated or retired hidden (use --all).")
                }
            );
            Ok(())
        }
        ProviderCmd::Remove { id } => {
            let store = need_store()?;
            let existed = providers::remove(&id, store).map_err(|e| e.to_string())?;
            println!(
                "{}",
                if existed {
                    "Key deleted from the keychain."
                } else {
                    "No key was saved for that provider."
                }
            );
            Ok(())
        }
    }
}

fn read_key(name: &str, from_stdin: bool) -> Result<SecretString, String> {
    if from_stdin || !std::io::stdin().is_terminal() {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| format!("Could not read the key: {e}"))?;
        return Ok(SecretString::new(line));
    }
    rpassword::prompt_password(format!("Paste your {name} API key (hidden): "))
        .map(SecretString::new)
        .map_err(|e| format!("Could not read the key: {e}"))
}

fn run_doctor(json: bool) -> ExitCode {
    let report = tern_agent::doctor::run_on_this_machine();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("report serialises")
        );
    } else {
        println!("Tern {} on {}", report.version, report.os);
        println!();
        let width = report
            .checks
            .iter()
            .map(|c| c.name.len())
            .max()
            .unwrap_or(0);
        for c in &report.checks {
            println!("  {:<4}  {:<width$}  {}", mark(c.status), c.name, c.detail);
        }
        println!();
        println!(
            "{}",
            match report.overall() {
                Status::Ok => "Everything Tern needs is in place.",
                Status::Warn => "Tern can run, with the warnings above.",
                Status::Fail => "Tern cannot run yet. Fix the failed checks above.",
            }
        );
    }
    match report.overall() {
        Status::Fail => ExitCode::FAILURE,
        _ => ExitCode::SUCCESS,
    }
}

fn mark(s: Status) -> &'static str {
    match s {
        Status::Ok => "ok",
        Status::Warn => "warn",
        Status::Fail => "FAIL",
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}
