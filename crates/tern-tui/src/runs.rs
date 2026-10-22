//! `tern resume`, `tern steer`, `tern watch`, and `tern ledger`.

use std::io::IsTerminal;

use tern_agent::checkpoint::CheckpointDir;
use tern_agent::deep::DeepControl;
use tern_agent::watch::{self, WatchList};
use tern_agent::{ChatOptions, ChatSession, DenyAll, Preferences, RunReport};
use tern_core::EventEnvelope;

use crate::ChatArgs;

pub(crate) fn options(chat: ChatArgs) -> (ChatOptions, Preferences) {
    let prefs = Preferences::load();
    let saved = prefs.chat_options();
    let opts = ChatOptions {
        provider: chat.provider.or(saved.provider),
        model: chat.model.or(saved.model),
        base_url: chat.base_url,
        search: chat.search.or(saved.search),
        search_url: chat.search_url,
        ..saved
    };
    (opts, prefs)
}

fn dim(line: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("\x1b[2m• {line}\x1b[0m");
    } else {
        eprintln!("• {line}");
    }
}

/// `tern resume [chat] [--list] [--discard]`.
pub async fn resume(
    chat: String,
    args: ChatArgs,
    list: bool,
    discard: bool,
    json: bool,
) -> Result<(), String> {
    let dir = CheckpointDir::discover()?;
    if list {
        let all = dir.list();
        if all.is_empty() {
            println!("No run is waiting to be resumed.");
        }
        for c in all {
            println!("{}  {}  {}", c.session, c.saved_at, c.progress());
            println!("    {}", c.question());
        }
        return Ok(());
    }
    let session = tern_agent::report::find_chat(&chat)?;
    let Some(ck) = dir.load(&session.to_string())? else {
        return Err(format!(
            "Chat {session} has no unfinished run. `tern resume --list` shows the ones that do."
        ));
    };
    if discard {
        use tern_agent::CheckpointSink;
        dir.clear(&session.to_string())?;
        eprintln!("Forgot the unfinished run of chat {session}.");
        return Ok(());
    }
    eprintln!("Resuming: {}", ck.question());
    eprintln!("{} · saved {}", ck.progress(), ck.saved_at);
    let (opts, prefs) = options(args);
    let (mut s, _) = ChatSession::resume(opts, session).await?;
    let steer = s.steering();
    crate::research::read_steering(steer, "Type a note and press Enter to steer the run.");
    let observer = |e: &EventEnvelope| {
        if let Some(line) = crate::research::progress_line(e) {
            dim(&line);
        }
    };
    let out = s
        .resume_run(
            &prefs.deep_options(),
            &DenyAll,
            &DeepControl::default(),
            &observer,
            None,
        )
        .await?;
    let Some(answer) = out.answer.clone() else {
        return Err(out
            .reason
            .unwrap_or_else(|| "The run stopped again; resume it once more.".into()));
    };
    let files = s.check_last_turn(&observer).await;
    if json {
        let report = files
            .as_ref()
            .and_then(|f| std::fs::read_to_string(&f.json).ok())
            .and_then(|t| RunReport::from_json(&t).ok())
            .ok_or("The run report could not be written.")?;
        println!("{}", report.to_json());
        return Ok(());
    }
    println!();
    println!("{answer}");
    if let Some(f) = &files {
        eprintln!("Report: {}", f.markdown.display());
    }
    Ok(())
}

/// `tern steer [chat] <note>`: hands a note to a run going in another
/// window or terminal.
pub fn steer(chat: String, note: Vec<String>) -> Result<(), String> {
    let note = note.join(" ");
    let session = tern_agent::report::find_chat(&chat)?;
    tern_agent::steer::send_to_chat(&session.to_string(), &note)?;
    let action = tern_agent::steer::parse_steer(&note);
    eprintln!(
        "Sent to chat {session}: {}. It lands at the run's next safe point.",
        action.describe()
    );
    Ok(())
}

/// `tern ledger [chat] [--turn N] [--json]`.
pub async fn ledger(chat: String, turn: Option<usize>, json: bool) -> Result<(), String> {
    let (report, files) = tern_agent::report::report_for(&chat, turn).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report.ledger).unwrap_or_default()
        );
    } else {
        println!("# Ledgers: turn {} of chat {}", report.turn, report.session);
        print!("{}", report.ledger.to_markdown());
    }
    eprintln!("In the run report: {}", files.markdown.display());
    Ok(())
}

#[derive(Debug, clap::Subcommand)]
pub enum WatchCmd {
    /// Put a question on watch.
    Add {
        #[arg(required = true, num_args = 1..)]
        question: Vec<String>,
        /// How often: hourly, daily (the default), weekly, 12h, 3d, 2w.
        #[arg(long, short, default_value = "daily")]
        every: String,
        /// Run it as deep research (the plan runs as drafted).
        #[arg(long)]
        deep: bool,
    },
    /// Show every watch, when it is due, and what changed last time.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Take a watch off (its id, the start of it, or its number).
    Rm { which: String },
    /// Run watches now: one by id, or every due one.
    Run {
        /// A watch's id; leave out to run every due watch.
        which: Option<String>,
        /// With no id, run every watch, due or not.
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        chat: ChatArgs,
    },
}

/// `tern watch …`.
pub async fn watch(cmd: WatchCmd) -> Result<(), String> {
    match cmd {
        WatchCmd::Add {
            question,
            every,
            deep,
        } => {
            let hours = watch::parse_every(&every)?;
            let mut list = WatchList::load()?;
            let w = list.add(&question.join(" "), hours, deep)?;
            list.save()?;
            println!(
                "Watching [{}] {} ({}{}). `tern watch run {}` runs it now; the desktop app runs it while open.",
                w.id,
                w.question,
                w.every_label(),
                if w.deep { ", deep research" } else { "" },
                w.id
            );
            Ok(())
        }
        WatchCmd::List { json } => {
            let list = WatchList::load()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&list).unwrap_or_default()
                );
                return Ok(());
            }
            if list.watches.is_empty() {
                println!("Nothing is on watch. Add a question with `tern watch add \"…\"`.");
            }
            let due = watch::due_now()?;
            for (i, w) in list.watches.iter().enumerate() {
                let when = if w.paused {
                    "paused".to_owned()
                } else if due.contains(&w.id) {
                    "due now".to_owned()
                } else {
                    w.next_due_text()
                };
                println!(
                    "{}. [{}] {} · {}{} · {}",
                    i + 1,
                    w.id,
                    w.question,
                    w.every_label(),
                    if w.deep { " · deep" } else { "" },
                    when
                );
                println!("     {}", w.status_line());
            }
            Ok(())
        }
        WatchCmd::Rm { which } => {
            let mut list = WatchList::load()?;
            let w = list.remove(&which)?;
            list.save()?;
            println!("Stopped watching: {}", w.question);
            Ok(())
        }
        WatchCmd::Run { which, all, chat } => {
            let (opts, prefs) = options(chat);
            let deep = prefs.deep_options();
            let ids: Vec<String> = match which {
                Some(w) => vec![w],
                None => {
                    let list = WatchList::load()?;
                    if all {
                        list.watches.iter().map(|w| w.id.clone()).collect()
                    } else {
                        watch::due_now()?
                    }
                }
            };
            if ids.is_empty() {
                println!("No watch is due. `tern watch run --all` runs them anyway.");
                return Ok(());
            }
            let mut failed = 0;
            for id in ids {
                dim(&format!("Running watch {id}"));
                match watch::run_watch(&id, opts.clone(), deep).await {
                    Ok(r) => {
                        println!("[{}] {}", r.watch.id, r.watch.question);
                        match &r.watch.last_changes {
                            Some(c) => print!("{}", c.to_markdown()),
                            None => println!("   {}", r.watch.status_line()),
                        }
                        if let Some(f) = &r.watch.last_report {
                            eprintln!("   Report: {}", f.markdown.display());
                        }
                    }
                    Err(e) => {
                        failed += 1;
                        eprintln!("[{id}] {e}");
                    }
                }
            }
            if failed > 0 {
                return Err(format!("{failed} watch run(s) failed."));
            }
            Ok(())
        }
    }
}
