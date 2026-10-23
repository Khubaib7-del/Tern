//! `tern eval`: the test ladder. List suites and rungs, run them, read
//! reports, compare runs, and run a usability session.

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;
use std::time::Instant;

use clap::Subcommand;
use tern_agent::eval::{
    self as ev, EvalRequest, EvalRun, EvalStore, Level, LiveModel, TaskResult, TernSubject,
};

use crate::ChatArgs;

// Parsed once per process; the size of `Run` does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub(crate) enum EvalCmd {
    /// The test ladder's rungs and every suite, bundled and your own.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Run one rung: unit, synthetic (the default, offline), pinned
    /// (--live), or public (--scifact).
    Run {
        #[arg(long, short, default_value = "synthetic")]
        level: String,
        /// Only these suites (by id), comma-separated.
        #[arg(long, value_delimiter = ',')]
        suite: Vec<String>,
        /// Trials per case (at least 3 for a published result).
        #[arg(long, short, default_value_t = ev::MIN_TRIALS)]
        trials: usize,
        /// Ask your model instead of replaying recorded answers.
        #[arg(long)]
        live: bool,
        /// The SciFact folder (claims_dev.jsonl and corpus.jsonl).
        #[arg(long, value_name = "FOLDER")]
        scifact: Option<PathBuf>,
        /// Keep the first N public cases.
        #[arg(long)]
        limit: Option<usize>,
        /// US dollars per million input and output tokens, as `IN,OUT`,
        /// for the cost per verified report.
        #[arg(long, value_delimiter = ',', num_args = 2)]
        price: Vec<f64>,
        /// Fail (exit 1) when a bundled baseline line is broken.
        #[arg(long)]
        check_baseline: bool,
        /// The source checkout for the unit rung (default: here).
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        chat: ChatArgs,
    },
    /// A saved run's report (`last` by default), or the list of runs.
    Report {
        #[arg(default_value = "last")]
        run: String,
        #[arg(long)]
        list: bool,
        #[arg(long)]
        json: bool,
    },
    /// Compare two runs metric by metric (default: the previous run of the
    /// same rung against the last). Exits 1 on a regression with --strict.
    Compare {
        old: Option<String>,
        new: Option<String>,
        #[arg(long)]
        strict: bool,
        #[arg(long)]
        json: bool,
    },
    /// A usability session: scripted tasks, timed, then the System
    /// Usability Scale. Saved as a run on the human rung.
    Usability {
        /// A label for the participant (not their name).
        #[arg(long, default_value = "P1")]
        participant: String,
    },
}

pub(crate) async fn run(cmd: EvalCmd) -> Result<(), String> {
    let store = EvalStore::discover()?;
    match cmd {
        EvalCmd::List { json } => list(&store, json),
        EvalCmd::Run {
            level,
            suite,
            trials,
            live,
            scifact,
            limit,
            price,
            check_baseline,
            root,
            json,
            chat,
        } => {
            let level = Level::parse(&level).ok_or_else(|| {
                format!("No rung `{level}`: unit, synthetic, pinned, public, or human.")
            })?;
            let (run, path) = match level {
                Level::Unit => {
                    let root = match root {
                        Some(r) => r,
                        None => std::env::current_dir().map_err(|e| e.to_string())?,
                    };
                    eprintln!("Running cargo test --workspace in {} …", root.display());
                    ev::run_unit(&store, &root)?
                }
                Level::Human => {
                    return Err(
                        "The human rung is a usability session: `tern eval usability`.".into(),
                    );
                }
                _ => {
                    let subject = if live {
                        let (opts, _) = crate::runs::options(chat);
                        TernSubject::live(LiveModel::open(&opts).await?)
                    } else {
                        TernSubject::offline()
                    };
                    let subject = subject.with_price(match price.as_slice() {
                        [i, o] => Some((*i, *o)),
                        _ => None,
                    });
                    let req = EvalRequest {
                        level: Some(level),
                        only: suite,
                        trials: Some(trials),
                        scifact,
                        limit,
                    };
                    Box::pin(ev::run(&store, &req, &subject)).await?
                }
            };
            if json {
                println!("{}", run.to_json());
            } else {
                print_run(&run);
                if let Some(prev) = store.previous(&run) {
                    let cmp = ev::compare(&prev, &run);
                    println!(
                        "\nAgainst {}: {} regression{}{}",
                        prev.id,
                        cmp.regressions(),
                        if cmp.regressions() == 1 { "" } else { "s" },
                        if cmp.regressions() > 0 {
                            " (tern eval compare)"
                        } else {
                            ""
                        }
                    );
                }
                println!(
                    "\nReport: {}\n        {}",
                    path.display(),
                    path.with_extension("md").display()
                );
            }
            if check_baseline {
                let broken = ev::check_baseline(&run, &ev::baseline());
                if !broken.is_empty() {
                    return Err(format!("Below the baseline:\n  {}", broken.join("\n  ")));
                }
                eprintln!("Baseline holds.");
            }
            Ok(())
        }
        EvalCmd::Report { run, list, json } => {
            if list {
                let rows = store.summaries();
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?
                    );
                } else if rows.is_empty() {
                    println!("No evaluation runs yet. Start one with `tern eval run`.");
                } else {
                    for r in rows {
                        let heads: Vec<String> = r
                            .headlines
                            .iter()
                            .map(|(s, _, v)| format!("{s} {v}"))
                            .collect();
                        println!(
                            "{}  {:<9} {:<7} {}/{} passed  {}",
                            r.id,
                            r.level.label(),
                            r.mode,
                            r.passed,
                            r.attempted,
                            heads.join(" · ")
                        );
                    }
                }
                return Ok(());
            }
            let run = store.find(&run)?;
            if json {
                println!("{}", run.to_json());
            } else {
                println!("{}", run.to_markdown());
            }
            Ok(())
        }
        EvalCmd::Compare {
            old,
            new,
            strict,
            json,
        } => {
            let new = store.find(new.as_deref().unwrap_or("last"))?;
            let old = match old {
                Some(o) => store.find(&o)?,
                None => store
                    .previous(&new)
                    .ok_or("There is no earlier run of this rung to compare with.")?,
            };
            let cmp = ev::compare(&old, &new);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&cmp).map_err(|e| e.to_string())?
                );
            } else {
                print!("{}", cmp.to_markdown());
            }
            if strict && cmp.regressions() > 0 {
                return Err(format!("{} regression(s)", cmp.regressions()));
            }
            Ok(())
        }
        EvalCmd::Usability { participant } => usability(&store, &participant),
    }
}

fn list(store: &EvalStore, json: bool) -> Result<(), String> {
    let (suites, bad) = ev::catalog(store);
    if json {
        let rows: Vec<serde_json::Value> = suites
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id, "version": s.version, "title": s.title,
                    "kind": s.kind.key(), "level": s.level.label(),
                    "cases": s.cases.len(), "origin": s.origin, "from": s.from,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    println!("The test ladder\n");
    for l in Level::ALL {
        println!(
            "  {:<10} {}\n             needs {}",
            l.label(),
            l.command(),
            l.needs()
        );
    }
    println!("\nSuites\n");
    for s in &suites {
        println!(
            "  {:<18} v{}  {:<9} {:>3} cases  {}",
            s.id,
            s.version,
            s.level.label(),
            s.cases.len(),
            s.title
        );
    }
    println!("\n  Your own suites go in {}", store.suites_dir().display());
    for b in bad {
        println!("  ! {b}");
    }
    Ok(())
}

fn print_run(run: &EvalRun) {
    let (p, a) = run.passed();
    println!(
        "{}  {} · {} · {} · {} trial{}  {p}/{a} passed in {:.1} s",
        run.id,
        run.level.label(),
        run.mode,
        run.subject,
        run.trials,
        if run.trials == 1 { "" } else { "s" },
        run.seconds
    );
    for n in &run.notes {
        println!("  {n}");
    }
    for s in &run.suites {
        println!(
            "\n{} ({})  {}/{} passed{}",
            s.title,
            s.tag(),
            s.passed,
            s.attempted,
            if s.skipped > 0 {
                format!(", {} skipped: {}", s.skipped, s.skip_reason)
            } else {
                String::new()
            }
        );
        for m in &s.metrics {
            println!("  {:<44} {}", m.label, m.line());
        }
        if !s.failures.is_empty() {
            println!(
                "  {} failure{} in the report",
                s.failures.len(),
                if s.failures.len() == 1 { "" } else { "s" }
            );
        }
    }
}

fn ask(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim().to_owned())
}

fn usability(store: &EvalStore, participant: &str) -> Result<(), String> {
    println!(
        "Usability session for {participant}. Open Tern in another window; do each task, then come back here.\n"
    );
    let mut tasks = Vec::new();
    for (k, (id, text)) in ev::TASKS.iter().enumerate() {
        println!("Task {} of {}: {text}", k + 1, ev::TASKS.len());
        ask("  Press Enter to start the clock… ")?;
        let t0 = Instant::now();
        let done = ask("  Press Enter when you finish (type n if you gave up): ")?;
        let seconds = t0.elapsed().as_secs_f64();
        let finished = !done.eq_ignore_ascii_case("n");
        let note = if finished {
            String::new()
        } else {
            ask("  What got in the way? ")?
        };
        tasks.push(TaskResult {
            id: (*id).to_owned(),
            done: finished,
            seconds,
            note,
        });
    }
    println!("\nThe System Usability Scale: 1 = strongly disagree, 5 = strongly agree.\n");
    let mut sus = [0u8; 10];
    for (i, s) in ev::SUS.iter().enumerate() {
        loop {
            let a = ask(&format!("{:>2}. {s}  [1-5] ", i + 1))?;
            if let Ok(n) = a.parse::<u8>()
                && (1..=5).contains(&n)
            {
                sus[i] = n;
                break;
            }
        }
    }
    let (run, path) = ev::save_session(store, participant, &tasks, &sus)?;
    print_run(&run);
    println!("\nSaved: {}", path.display());
    Ok(())
}
