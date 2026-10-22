//! The `tern` command.
//!
//! `tern` (or `tern chat`) opens the TUI. `tern doctor` and `tern provider`
//! are plain commands. `tern ask` and `tern run --json` are the next slices.

#![forbid(unsafe_code)]

mod tui;

use std::io::{BufRead, IsTerminal};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tern_agent::doctor::Status;
use tern_agent::providers::{self, ModelKind, SecretStore};
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
struct ChatArgs {
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
    /// List the models this key can use.
    Models {
        id: String,
        /// Include embedding, audio, image, and moderation models.
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
        Command::Ask {
            question,
            chat,
            reads,
            json,
        } => block_on(run_ask(question.join(" "), chat, reads, json)),
        Command::Search(cmd) => block_on(run_search(cmd)),
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
            if !all {
                models.retain(|m| m.kind == ModelKind::Chat);
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&models).expect("models serialise")
                );
                return Ok(());
            }
            for m in &models {
                let ctx = m
                    .context_window
                    .map(|c| format!("  {}k context", c / 1000))
                    .unwrap_or_default();
                let name = m
                    .display_name
                    .as_deref()
                    .map(|n| format!("  ({n})"))
                    .unwrap_or_default();
                println!("  {}{name}{ctx}", m.id);
            }
            println!(
                "{} models{}.",
                models.len(),
                if all {
                    ""
                } else {
                    " for chat (use --all for every kind)"
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
