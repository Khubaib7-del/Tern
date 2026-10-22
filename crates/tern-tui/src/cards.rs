//! `tern cards` and `tern explain3d`: flashcards with spaced repetition,
//! Anki export, and Explain in 3D in the terminal.

use std::path::PathBuf;

use tern_agent::cards::{self as cards, Card, CardKind, CardSpec, Format, Rating};
use tern_agent::explain;
use tern_agent::study::{self, StudyStore};

use crate::ChatArgs;
use crate::study::{dim, engine, read_line};

#[derive(Debug, clap::Subcommand)]
pub enum CardsCmd {
    /// Write flashcards from your materials (one topic, or all), each with
    /// the passage it came from. Repeats of cards you have are skipped.
    Gen {
        /// A topic: its number, id, or title. Leave out for all materials.
        topic: Option<String>,
        /// How many cards (1 to 40).
        #[arg(long, short, default_value_t = 12)]
        n: usize,
        /// Card types to mix, comma-separated: basic, cloze, why.
        #[arg(long, short)]
        kinds: Option<String>,
        /// No model: cards written from your materials' sentences.
        #[arg(long)]
        offline: bool,
        #[command(flatten)]
        chat: ChatArgs,
    },
    /// Review today's cards: Enter flips, then 1 Again, 2 Hard, 3 Good,
    /// 4 Easy. q stops.
    Review {
        /// Only cards on this topic.
        topic: Option<String>,
    },
    /// The deck: every card with its number, state, and next due date.
    List {
        topic: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Counts: due today, new, learning, young, mature, recall rate.
    Stats,
    /// Rewrite a card's front and back (cloze cards keep their {{c1::…}} gaps).
    Edit {
        /// The card's number or id.
        card: String,
        #[arg(long)]
        front: Option<String>,
        #[arg(long)]
        back: Option<String>,
    },
    /// Suspend a card (it stops coming up), or `--off` to bring it back.
    Suspend {
        card: String,
        #[arg(long)]
        off: bool,
    },
    /// Delete a card.
    Rm { card: String },
    /// Export to Anki (`.apkg`), Tern's CSV, or Anki-ready TSV.
    Export {
        /// An Anki package: SQLite collection plus media, ready for File › Import.
        #[arg(long, value_name = "FILE", group = "out")]
        apkg: Option<PathBuf>,
        #[arg(long, value_name = "FILE", group = "out")]
        csv: Option<PathBuf>,
        #[arg(long, value_name = "FILE", group = "out")]
        tsv: Option<PathBuf>,
        /// Only cards on this topic.
        #[arg(long)]
        topic: Option<String>,
    },
    /// Import cards from CSV or TSV (Tern's own, Anki's plain-text export,
    /// or any file with front and back columns).
    Import { file: PathBuf },
}

fn open() -> Result<(StudyStore, study::StudyLibrary, cards::Deck), String> {
    let store = StudyStore::discover()?;
    let lib = store.load()?;
    let deck = cards::load_deck(&store)?;
    Ok((store, lib, deck))
}

fn topic_id(lib: &study::StudyLibrary, which: Option<String>) -> Result<Option<String>, String> {
    match which {
        None => Ok(None),
        Some(w) => lib
            .topic(&w)
            .map(|t| Some(t.id.clone()))
            .ok_or_else(|| format!("No topic matches \"{w}\". `tern study topics` lists them.")),
    }
}

fn due_label(c: &Card, now: i64) -> String {
    if c.suspended {
        return "suspended".into();
    }
    match c.sched.state {
        cards::CardState::New => "new".into(),
        _ if c.sched.due <= now => "due now".into(),
        _ => format!("in {}", cards::wait_label(c.sched.due - now)),
    }
}

/// `tern cards …`.
pub async fn run(cmd: CardsCmd) -> Result<(), String> {
    let (store, mut lib, mut deck) = open()?;
    let now = study::now();
    let offset = cards::local_offset();
    match cmd {
        CardsCmd::Gen {
            topic,
            n,
            kinds,
            offline,
            chat,
        } => {
            if lib.materials.is_empty() {
                return Err("Add materials first: `tern study add notes.pdf`.".into());
            }
            let mut spec = CardSpec {
                topic,
                count: n,
                kinds: Vec::new(),
            };
            if let Some(k) = kinds {
                spec.kinds = k
                    .split(',')
                    .map(|x| {
                        CardKind::parse(x).ok_or_else(|| {
                            format!("Unknown card type \"{x}\" (basic, cloze, why).")
                        })
                    })
                    .collect::<Result<_, _>>()?;
            }
            let e = engine(chat, offline).await;
            let g = cards::generate(&store, &mut lib, e.as_ref(), &spec).await?;
            if let Some(n) = &g.note {
                dim(n);
            }
            for c in &g.added {
                println!(
                    "+ {} · {} · {}",
                    c.id,
                    c.kind.label(),
                    study::clip(&c.question(), 90)
                );
                if let Some(a) = &c.anchor {
                    dim(&format!("  {}", a.label()));
                }
            }
            println!(
                "{} new card{} (written by {}){}.",
                g.added.len(),
                if g.added.len() == 1 { "" } else { "s" },
                g.written_by,
                if g.repeats > 0 {
                    format!(
                        ", {} repeat{} skipped",
                        g.repeats,
                        if g.repeats == 1 { "" } else { "s" }
                    )
                } else {
                    String::new()
                }
            );
            let deck = cards::load_deck(&store)?;
            dim(&deck.stats(now, offset).line());
        }
        CardsCmd::Review { topic } => {
            let tid = topic_id(&lib, topic)?;
            let total = deck.due_count(now, offset, tid.as_deref());
            if total == 0 && deck.next_card(now, offset, tid.as_deref()).is_none() {
                println!("Nothing due. {}", deck.stats(now, offset).line());
                return Ok(());
            }
            println!("{total} card{} due.", if total == 1 { "" } else { "s" });
            let mut done = 0usize;
            loop {
                let at = study::now();
                let Some(card) = deck.next_card(at, offset, tid.as_deref()).cloned() else {
                    break;
                };
                println!("\n{} · {}", card.kind.label(), due_label(&card, at));
                println!("{}", card.question());
                let Some(l) = read_line("Enter to show the answer (q stops) › ") else {
                    break;
                };
                if l.trim().eq_ignore_ascii_case("q") {
                    break;
                }
                println!("—\n{}", card.answer());
                if let Some(a) = &card.anchor {
                    dim(&a.label());
                }
                let hints = cards::button_hints(&card, at);
                let prompt = format!(
                    "1 Again {} · 2 Hard {} · 3 Good {} · 4 Easy {} › ",
                    hints[0], hints[1], hints[2], hints[3]
                );
                let rating = loop {
                    let Some(l) = read_line(&prompt) else {
                        break None;
                    };
                    if l.trim().eq_ignore_ascii_case("q") {
                        break None;
                    }
                    match Rating::parse(&l) {
                        Some(r) => break Some(r),
                        None => eprintln!("Type 1, 2, 3, or 4."),
                    }
                };
                let Some(r) = rating else {
                    break;
                };
                cards::review(&store, &mut lib, &mut deck, &card.id, r, study::now())?;
                done += 1;
            }
            println!(
                "\n{done} reviewed. {}",
                deck.stats(study::now(), offset).line()
            );
        }
        CardsCmd::List { topic, json } => {
            let tid = topic_id(&lib, topic)?;
            let shown: Vec<(usize, &Card)> = deck
                .cards
                .iter()
                .enumerate()
                .filter(|(_, c)| tid.is_none() || c.topic == tid)
                .collect();
            if json {
                let v: Vec<&Card> = shown.iter().map(|(_, c)| *c).collect();
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                return Ok(());
            }
            if shown.is_empty() {
                println!("No cards yet. `tern cards gen` writes some from your materials.");
            }
            for (i, c) in shown {
                println!(
                    "{:>3}. {} · {} · {} · {}",
                    i + 1,
                    c.id,
                    c.kind.label(),
                    due_label(c, now),
                    study::clip(&c.question(), 80)
                );
            }
        }
        CardsCmd::Stats => {
            let s = deck.stats(now, offset);
            println!("{}", s.line());
            println!(
                "Reviewed today: {} · new left today: {}",
                s.reviewed_today, s.new_left
            );
            let mut by_topic: Vec<(String, usize, usize)> = lib
                .topics
                .iter()
                .map(|t| {
                    (
                        t.title.clone(),
                        deck.for_topic(&t.id).count(),
                        deck.due_count(now, offset, Some(&t.id)),
                    )
                })
                .filter(|x| x.1 > 0)
                .collect();
            by_topic.sort_by_key(|a| std::cmp::Reverse(a.2));
            for (t, n, due) in by_topic {
                println!("  {t}: {n} cards, {due} due");
            }
        }
        CardsCmd::Edit { card, front, back } => {
            let i = deck.index(&card)?;
            let c = &deck.cards[i];
            let f = front.unwrap_or_else(|| c.front.clone());
            let b = back.unwrap_or_else(|| c.back.clone());
            let id = c.id.clone();
            deck.edit(&id, &f, &b)?;
            cards::save_deck(&store, &deck)?;
            println!("Updated {id}.");
        }
        CardsCmd::Suspend { card, off } => {
            let c = deck.suspend(&card, !off)?.id.clone();
            cards::save_deck(&store, &deck)?;
            println!(
                "{c} {}.",
                if off {
                    "is back in review"
                } else {
                    "suspended"
                }
            );
        }
        CardsCmd::Rm { card } => {
            let c = deck.delete(&card)?;
            cards::save_deck(&store, &deck)?;
            println!("Deleted {}: {}", c.id, study::clip(&c.question(), 80));
        }
        CardsCmd::Export {
            apkg,
            csv,
            tsv,
            topic,
        } => {
            let (path, format) = match (apkg, csv, tsv) {
                (Some(p), _, _) => (p, Format::Apkg),
                (_, Some(p), _) => (p, Format::Csv),
                (_, _, Some(p)) => (p, Format::Tsv),
                _ => {
                    return Err(
                        "Name a file: --apkg deck.apkg, --csv deck.csv, or --tsv deck.tsv.".into(),
                    );
                }
            };
            let tid = topic_id(&lib, topic)?;
            println!("{}", cards::export(&deck, tid.as_deref(), &path, format)?);
        }
        CardsCmd::Import { file } => {
            let (added, skipped) = cards::import(&store, &mut deck, &file)?;
            println!(
                "{added} card{} imported{}.",
                if added == 1 { "" } else { "s" },
                if skipped > 0 {
                    format!(", {skipped} already in the deck")
                } else {
                    String::new()
                }
            );
        }
    }
    Ok(())
}

/// `tern explain3d "concept"`.
#[allow(clippy::too_many_arguments)]
pub async fn explain3d(
    concept: Vec<String>,
    topic: Option<String>,
    svg: Option<PathBuf>,
    step: Option<usize>,
    vault: bool,
    offline: bool,
    json: bool,
    chat: ChatArgs,
) -> Result<(), String> {
    let store = StudyStore::discover()?;
    let mut lib = store.load()?;
    let e = engine(chat, offline).await;
    let ask = explain::Ask {
        concept: concept.join(" "),
        topic,
        context: None,
    };
    let (scene, note) = explain::build(&store, &mut lib, e.as_ref(), &ask).await?;
    if let Some(n) = note {
        dim(&n);
    }
    let (path, _) = explain::save(&store, &scene)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&scene).unwrap_or_default()
        );
    } else {
        println!("Explain in 3D · {}", scene.concept);
        if !scene.summary.is_empty() {
            println!("{}", scene.summary);
        }
        println!(
            "\n{} parts: {}",
            scene.parts.len(),
            scene
                .parts
                .iter()
                .map(|p| p.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        for (i, s) in scene.steps.iter().enumerate() {
            println!("\n{}. {}", i + 1, s.caption);
            if let Some(c) = s.cite() {
                dim(&c);
            }
        }
        dim(&format!(
            "Saved to your study library ({}); written by {}.",
            path.display(),
            scene.written_by
        ));
    }
    if let Some(p) = svg {
        let at = step.map(|s| s.saturating_sub(1).min(scene.steps.len().saturating_sub(1)));
        explain::export_svg(&scene, at, &p)?;
        dim(&format!("Snapshot → {}", p.display()));
    }
    if vault {
        let v = tern_agent::vault::Vault::open_default()?;
        let name = explain::save_to_vault(&v, &scene)?;
        dim(&format!("Saved to the vault as {name}."));
    }
    Ok(())
}
