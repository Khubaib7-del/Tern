//! Continuity commands: `tern range`, `tern sub`, `tern show`,
//! `tern handoff`, `tern new --from-handoff`, `tern library`, and
//! `tern side`.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use clap::Subcommand;
use tern_agent::library::{self, ItemState, LibraryKind, LibraryQuery, LibrarySort};
use tern_agent::range::{self, MoveTarget, clip, label};
use tern_agent::{
    CancelFlag, ChatSession, DenyAll, PastTurn, Store, handoff, open_store, refs, side, sub,
};
use tern_core::{MessageRef, SessionId};

use crate::ChatArgs;
use crate::runs::options;

fn dim(line: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("\x1b[2m• {line}\x1b[0m");
    } else {
        eprintln!("• {line}");
    }
}

fn chat_of(store: &Store, which: &str) -> Result<SessionId, String> {
    if let Some((s, _)) = refs::parse_link(which) {
        return Ok(s);
    }
    tern_agent::report::find_session(store, which)
}

#[derive(Debug, Subcommand)]
pub(crate) enum RangeCmd {
    /// Remove messages to the chat's trash: `Q3`, `Q3-Q5`, `Q3,Q7`.
    Rm {
        ids: String,
        /// The chat: `last` (the default), its id, or the start of its id.
        #[arg(long, short, default_value = "last")]
        chat: String,
    },
    /// Copy messages, with the context before them, to a new session (or a
    /// sub-session with --sub). The originals stay until you confirm.
    Mv {
        ids: String,
        #[arg(long, short, default_value = "last")]
        chat: String,
        /// Move into a sub-session of this chat instead of a new session.
        #[arg(long)]
        sub: bool,
        /// With --sub, the message it hangs under (defaults to the one before).
        #[arg(long)]
        anchor: Option<String>,
        #[arg(long, short)]
        title: Option<String>,
        /// Confirm at once: the originals go to the chat's trash.
        #[arg(long, short)]
        yes: bool,
    },
    /// Confirm the waiting move: the originals go to the chat's trash.
    Confirm {
        #[arg(long, short, default_value = "last")]
        chat: String,
    },
    /// Undo the waiting move: the copy is archived.
    Undo {
        #[arg(long, short, default_value = "last")]
        chat: String,
    },
    /// Bring removed messages back (all of them when no IDs are given).
    Restore {
        ids: Option<String>,
        #[arg(long, short, default_value = "last")]
        chat: String,
    },
    /// List the chat's removed messages.
    Trash {
        #[arg(long, short, default_value = "last")]
        chat: String,
    },
}

pub(crate) fn range_cmd(cmd: RangeCmd) -> Result<(), String> {
    let mut store = open_store()?;
    match cmd {
        RangeCmd::Rm { ids, chat } => {
            let s = chat_of(&store, &chat)?;
            let nums = range::numbers_of(&refs::parse_selection(&ids)?)?;
            let done = range::remove(&mut store, s, &nums)?;
            println!(
                "Removed {} to the chat's trash. `tern range restore {}` brings {} back.",
                label(&done),
                ids,
                if done.len() == 1 { "it" } else { "them" }
            );
        }
        RangeCmd::Mv {
            ids,
            chat,
            sub,
            anchor,
            title,
            yes,
        } => {
            let s = chat_of(&store, &chat)?;
            let nums = range::numbers_of(&refs::parse_selection(&ids)?)?;
            let target = if sub {
                let anchor = match anchor {
                    Some(a) => Some(
                        a.trim_start_matches(['Q', 'q'])
                            .parse::<u32>()
                            .map_err(|_| format!("`{a}` is not a message like Q2."))?,
                    ),
                    None => None,
                };
                MoveTarget::SubSession { anchor, title }
            } else {
                MoveTarget::NewSession { title }
            };
            let p = range::move_to(&mut store, s, &nums, target)?;
            println!("Copied {} with its context to {}.", label(&p.numbers), p.to);
            if yes {
                range::confirm(&mut store, s)?;
                println!(
                    "Confirmed: the originals are in the chat's trash (`tern range restore`)."
                );
            } else {
                println!(
                    "The originals stay until you confirm: `tern range confirm`, or `tern range undo`."
                );
            }
        }
        RangeCmd::Confirm { chat } => {
            let s = chat_of(&store, &chat)?;
            let p = range::confirm(&mut store, s)?;
            println!(
                "Moved {} to {}. The originals are in the chat's trash.",
                label(&p.numbers),
                p.to
            );
        }
        RangeCmd::Undo { chat } => {
            let s = chat_of(&store, &chat)?;
            let p = range::undo(&mut store, s)?;
            println!(
                "Undid the move of {}; the copy {} is archived.",
                label(&p.numbers),
                p.to
            );
        }
        RangeCmd::Restore { ids, chat } => {
            let s = chat_of(&store, &chat)?;
            let nums = match ids {
                Some(i) => Some(range::numbers_of(&refs::parse_selection(&i)?)?),
                None => None,
            };
            let back = range::restore(&mut store, s, nums.as_deref())?;
            println!("Restored {}.", label(&back));
        }
        RangeCmd::Trash { chat } => {
            let s = chat_of(&store, &chat)?;
            let t = range::trashed(&store, s)?;
            if t.is_empty() {
                println!("This chat's trash is empty.");
            }
            for turn in t {
                println!("Q{}  {}", turn.number, one_line(&turn));
            }
        }
    }
    Ok(())
}

fn one_line(t: &PastTurn) -> String {
    clip(
        t.message
            .as_deref()
            .map(|m| tern_agent::attach::split(m).0)
            .unwrap_or("(message not kept)"),
        90,
    )
}

#[derive(Debug, Subcommand)]
pub(crate) enum SubCmd {
    /// Make a sub-session hung under a message (`Q12`), optionally planned
    /// with the question it will run.
    New {
        /// The message it branches from: `Q12`.
        from: String,
        /// The planned question. Leave out for an empty sub-session.
        question: Vec<String>,
        #[arg(long, short, default_value = "last")]
        chat: String,
        #[arg(long, short)]
        title: Option<String>,
    },
    /// List a chat's sub-sessions and their message IDs.
    List {
        #[arg(long, short, default_value = "last")]
        chat: String,
        #[arg(long)]
        json: bool,
    },
    /// Run a sub-session's planned question (or the one given) and post a
    /// summary back to the parent chat.
    Run {
        /// `Q12.3`, `Q12` (its newest sub-session), or a sub-session id.
        which: String,
        /// A question to run instead of the planned one.
        question: Vec<String>,
        #[arg(long, short, default_value = "last")]
        chat: String,
        #[command(flatten)]
        args: ChatArgs,
    },
}

pub(crate) async fn sub_cmd(cmd: SubCmd) -> Result<(), String> {
    match cmd {
        SubCmd::New {
            from,
            question,
            chat,
            title,
        } => {
            let mut store = open_store()?;
            let s = chat_of(&store, &chat)?;
            let r: MessageRef = refs::parse_selection(&from)?
                .first()
                .cloned()
                .ok_or("Name the message to branch from, like Q12.")?;
            if r.sub.is_some() {
                return Err("Sub-sessions hang under a top-level message, like Q12.".into());
            }
            let plan = question.join(" ");
            let made = sub::create(
                &mut store,
                s,
                r.number,
                title.as_deref(),
                (!plan.trim().is_empty()).then_some(plan.as_str()),
            )?;
            println!("↳ {}  {}", made.id, made.title);
            if made.plan.is_some() {
                println!(
                    "Planned. `tern sub run {} ` runs it and reports back to Q{}.",
                    made.id, r.number
                );
            }
        }
        SubCmd::List { chat, json } => {
            let store = open_store()?;
            let s = chat_of(&store, &chat)?;
            let rows = sub::list(&store, s)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?
                );
                return Ok(());
            }
            if rows.is_empty() {
                println!(
                    "This chat has no sub-sessions. `tern sub new Q3 \"question\"` makes one."
                );
            }
            for r in rows {
                let state = if r.planned {
                    "planned"
                } else if r.reported {
                    "reported back"
                } else {
                    "open"
                };
                println!(
                    "↳ {}  {}  {}  ({state})",
                    r.anchor, r.session.title, r.session.id
                );
                if !r.refs.is_empty() {
                    println!("    {}", r.refs.join(" "));
                }
            }
        }
        SubCmd::Run {
            which,
            question,
            chat,
            args,
        } => {
            let id = {
                let store = open_store()?;
                let s = chat_of(&store, &chat)?;
                sub::find(&store, s, &which)?
            };
            let (opts, _) = options(args);
            let q = question.join(" ");
            dim("Running the sub-session; the parent chat is free meanwhile.");
            let run = sub::run(
                opts,
                id,
                (!q.trim().is_empty()).then_some(q.as_str()),
                &DenyAll,
                CancelFlag::default(),
                &|e| {
                    if e.kind == "tool.executed" {
                        dim(&e.user_summary);
                    }
                },
            )
            .await?;
            println!();
            println!(
                "{}",
                run.outcome.answer.clone().unwrap_or_else(|| run
                    .outcome
                    .reason
                    .clone()
                    .unwrap_or_default())
            );
            if let Some(s) = run.summary {
                println!();
                dim(&format!("Reported back to the parent: {}", clip(&s, 120)));
            }
        }
    }
    Ok(())
}

/// `tern show Q12.3 [--chat]` or `tern show tern://chat/<id>/Q12.3`.
pub(crate) fn show(which: String, chat: String, json: bool) -> Result<(), String> {
    let store = open_store()?;
    let (session, r) = match refs::parse_link(&which) {
        Some(x) => x,
        None => (
            chat_of(&store, &chat)?,
            which
                .parse::<MessageRef>()
                .map_err(|_| format!("`{which}` is not a message ID like Q12 or Q12.3."))?,
        ),
    };
    let found = refs::resolve(&store, session, &r)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": r.to_string(),
                "session": found.session.to_string(),
                "turn": found.number,
                "removed": found.turn.removed,
                "message": found.turn.message,
                "answer": found.turn.answer,
                "link": refs::link(found.session, &r),
            }))
            .map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    println!(
        "{r}{}  ({})",
        if found.turn.removed {
            " · in the trash"
        } else {
            ""
        },
        refs::link(found.session, &r)
    );
    if let Some(m) = &found.turn.message {
        println!("\n> {}", tern_agent::attach::split(m).0);
    }
    if let Some(a) = &found.turn.answer {
        println!("\n{a}");
    }
    Ok(())
}

/// `tern handoff [chat] [--out FILE] [--json]`.
pub(crate) fn handoff_cmd(chat: String, out: Option<PathBuf>, json: bool) -> Result<(), String> {
    let store = open_store()?;
    let s = chat_of(&store, &chat)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let packet = handoff::for_session(&store, s, now)?;
    let md = handoff::to_markdown(&packet);
    let js = handoff::to_json(&packet);
    match out {
        Some(path) => {
            let (md_path, js_path) = match path.extension().and_then(|x| x.to_str()) {
                Some("json") => (path.with_extension("md"), path.clone()),
                _ => (path.with_extension("md"), path.with_extension("json")),
            };
            if let Some(d) = md_path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::write(&md_path, &md).map_err(|e| e.to_string())?;
            std::fs::write(&js_path, &js).map_err(|e| e.to_string())?;
            println!("Wrote {} and {}.", md_path.display(), js_path.display());
            println!(
                "Start fresh from it: `tern new --from-handoff {}`",
                md_path.display()
            );
        }
        None if json => println!("{js}"),
        None => println!("{md}"),
    }
    Ok(())
}

/// `tern new --from-handoff FILE [question]`.
pub(crate) async fn new_cmd(
    from_handoff: PathBuf,
    question: Vec<String>,
    args: ChatArgs,
) -> Result<(), String> {
    let text = std::fs::read_to_string(&from_handoff)
        .map_err(|e| format!("{}: {e}", from_handoff.display()))?;
    let packet = handoff::parse(&text)?;
    let id = {
        let mut store = open_store()?;
        handoff::start_from(&mut store, &packet)?
    };
    println!(
        "New session {id} started from the packet “{}”.",
        packet.title
    );
    let q = question.join(" ");
    if q.trim().is_empty() {
        println!("Continue it with `tern sub` commands, the TUI, or the desktop app.");
        return Ok(());
    }
    let (opts, _) = options(args);
    let (mut chat, _) = ChatSession::resume(opts, id).await?;
    let out = chat
        .send(&q, &DenyAll, CancelFlag::default(), &|e| {
            if e.kind == "tool.executed" {
                dim(&e.user_summary);
            }
        })
        .await;
    println!();
    println!(
        "{}",
        out.answer.unwrap_or_else(|| out.reason.unwrap_or_default())
    );
    Ok(())
}

#[derive(Debug, Subcommand)]
pub(crate) enum LibraryCmd {
    /// List everything: reports, sources, notes, decks, files, Explain in
    /// 3D scenes, and sessions.
    List {
        /// report, source, note, deck, file, scene, or session (repeat or comma-separate).
        #[arg(long, short, value_delimiter = ',')]
        kind: Vec<String>,
        /// newest, oldest, title, kind, or largest.
        #[arg(long, short, default_value = "newest")]
        sort: String,
        /// Page number, from 1 (50 a page).
        #[arg(long, short, default_value_t = 1)]
        page: usize,
        /// List the trash instead.
        #[arg(long)]
        trash: bool,
        #[arg(long)]
        json: bool,
    },
    /// Search titles, details, and message IDs (`Q12.3`).
    Search {
        #[arg(required = true, num_args = 1..)]
        words: Vec<String>,
        #[arg(long, short, value_delimiter = ',')]
        kind: Vec<String>,
        #[arg(long, short, default_value = "newest")]
        sort: String,
        #[arg(long)]
        json: bool,
    },
    /// Move items to the trash by id (from `tern library list`).
    Rm {
        #[arg(required = true, num_args = 1..)]
        ids: Vec<String>,
    },
    /// Bring items back from the trash (all of them with --all).
    Restore {
        ids: Vec<String>,
        #[arg(long)]
        all: bool,
    },
    /// Delete what is in the trash for good.
    #[command(name = "empty-trash")]
    EmptyTrash {
        /// Skip the question.
        #[arg(long, short)]
        yes: bool,
    },
}

fn kinds(list: &[String]) -> Result<Vec<LibraryKind>, String> {
    list.iter()
        .filter(|k| !k.trim().is_empty() && k.trim() != "all")
        .map(|k| {
            LibraryKind::parse(k).ok_or_else(|| {
                format!("`{k}` is not a kind: report, source, note, deck, file, scene, or session.")
            })
        })
        .collect()
}

fn print_page(page: &library::LibraryPage, json: bool, per: usize) -> Result<(), String> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "total": page.total,
                "offset": page.offset,
                "items": page.items,
            }))
            .map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if page.items.is_empty() {
        println!("Nothing here.");
        return Ok(());
    }
    for it in &page.items {
        println!(
            "{:<8} {}  ·  {}  ·  {}",
            it.kind().as_str(),
            clip(&it.title, 60),
            clip(&it.detail, 50),
            library::size_label(it)
        );
        println!("         {}", it.id);
    }
    let pages = page.total.div_ceil(per).max(1);
    println!(
        "\n{} items · page {} of {pages}",
        page.total,
        page.offset / per + 1
    );
    Ok(())
}

pub(crate) fn library_cmd(cmd: LibraryCmd) -> Result<(), String> {
    const PER: usize = 50;
    let (mut store, src) = library::open()?;
    match cmd {
        LibraryCmd::List {
            kind,
            sort,
            page,
            trash,
            json,
        } => {
            let q = LibraryQuery {
                kinds: kinds(&kind)?,
                state: if trash {
                    ItemState::Trash
                } else {
                    ItemState::Live
                },
                sort: LibrarySort::parse(&sort)
                    .ok_or("Sort by newest, oldest, title, kind, or largest.")?,
                offset: page.saturating_sub(1) * PER,
                limit: PER,
                ..LibraryQuery::default()
            };
            let p = store.library_query(&q).map_err(|e| e.to_string())?;
            print_page(&p, json, PER)?;
        }
        LibraryCmd::Search {
            words,
            kind,
            sort,
            json,
        } => {
            let q = LibraryQuery {
                kinds: kinds(&kind)?,
                text: words.join(" "),
                sort: LibrarySort::parse(&sort)
                    .ok_or("Sort by newest, oldest, title, kind, or largest.")?,
                limit: PER,
                ..LibraryQuery::default()
            };
            let p = store.library_query(&q).map_err(|e| e.to_string())?;
            print_page(&p, json, PER)?;
        }
        LibraryCmd::Rm { ids } => {
            let n = library::trash(&mut store, &src, &ids)?;
            println!("Moved {n} to the trash. `tern library restore <id>` brings items back.");
        }
        LibraryCmd::Restore { ids, all } => {
            let ids = if all {
                store
                    .library_trash()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|i| i.id)
                    .collect()
            } else {
                ids
            };
            if ids.is_empty() {
                return Err("Name the items to restore, or pass --all.".into());
            }
            let n = library::restore(&mut store, &src, &ids)?;
            println!("Restored {n}.");
        }
        LibraryCmd::EmptyTrash { yes } => {
            let n = store.library_trash().map_err(|e| e.to_string())?.len();
            if n == 0 {
                println!("The trash is empty.");
                return Ok(());
            }
            if !yes {
                eprint!("Delete {n} items in the trash for good? [y/N] ");
                let _ = std::io::stderr().flush();
                let mut answer = String::new();
                let _ = std::io::stdin().read_line(&mut answer);
                if !answer.trim().eq_ignore_ascii_case("y") {
                    println!("Kept the trash.");
                    return Ok(());
                }
            }
            let n = library::empty_trash(&mut store, &src)?;
            println!("Deleted {n} items for good. Sessions stay archived; their logs are kept.");
        }
    }
    Ok(())
}

/// `tern side "question" [--chat]`: a throwaway question about a chat.
pub(crate) async fn side_cmd(
    question: Vec<String>,
    chat: String,
    args: ChatArgs,
) -> Result<(), String> {
    let s = {
        let store = open_store()?;
        chat_of(&store, &chat)?
    };
    let (opts, _) = options(args);
    let mut side = side::SideChat::open_default(opts, s).await?;
    dim(&format!(
        "Side chat with {} · read {} messages of the chat · nothing is saved",
        side.model_name(),
        side.thread_len()
    ));
    let answer = side.ask(&question.join(" "), None).await?;
    println!("{answer}");
    Ok(())
}
