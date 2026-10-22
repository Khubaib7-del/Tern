//! `tern decide`: ask the decision model, set it up, and benchmark it.

use std::path::PathBuf;

use clap::Subcommand;
use tern_agent::decide::{self, DecisionKind, Mode};
use tern_agent::providers::{self, SecretStore};

use crate::ChatArgs;

#[derive(Debug, clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct DecideArgs {
    #[command(subcommand)]
    cmd: Option<DecideCmd>,
    /// The question to decide.
    question: Vec<String>,
    /// The fixed choices, comma-separated.
    #[arg(long, short, value_delimiter = ',')]
    choices: Vec<String>,
    /// A file whose text the question is about.
    #[arg(long, value_name = "FILE")]
    context: Option<PathBuf>,
    /// What kind of decision: source, tool, claim, triage, or general.
    #[arg(long, default_value = "general")]
    kind: String,
    /// Print the decision as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum DecideCmd {
    /// Show the decision model in use and the remote providers.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Off, local, or remote (with --provider and an optional --model).
    Mode {
        mode: String,
        /// Remote provider id: typesafe, subq, or voyage.
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        /// Milliseconds a remote call may take before the local scorer answers.
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
    /// Remote decision-model keys: add (tested first), test, remove.
    #[command(subcommand)]
    Key(KeyCmd),
    /// Score the bundled fixtures: the fixed-order baseline and the local
    /// model, plus the reasoning model and the remote decision model when
    /// asked and set up.
    Bench {
        /// Also ask the chat model each fixture (uses your provider key).
        #[arg(long)]
        reasoning: bool,
        /// Also run the remote decision model chosen in preferences.
        #[arg(long)]
        remote: bool,
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        chat: ChatArgs,
    },
}

#[derive(Debug, Subcommand)]
enum KeyCmd {
    /// Paste a key; Tern asks one small decision with it and saves it only if it works.
    Add {
        id: String,
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Ask one small decision with the saved key.
    Test {
        id: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
    },
    /// Delete the saved key from the keychain.
    Remove { id: String },
}

pub(crate) async fn run(args: DecideArgs) -> Result<(), String> {
    match args.cmd {
        Some(cmd) => run_cmd(cmd).await,
        None => ask(args).await,
    }
}

fn print_decision(d: &decide::Decision, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(d).expect("decision serialises")
        );
        return;
    }
    for i in d.ranking() {
        let mark = if i == d.chosen { "▸" } else { " " };
        println!(
            "{mark} {:>5.1}%  {}",
            d.probabilities[i] * 100.0,
            d.choices[i]
        );
    }
    println!();
    println!("{}", d.rationale);
    let mut tail = vec![
        format!("{} ({})", d.model, d.provider),
        format!("confidence {:.2}", d.confidence),
        format!("{} ms", d.latency_ms),
    ];
    if d.cost.input_tokens > 0 {
        tail.push(format!(
            "{} input tokens, ${:.6}",
            d.cost.input_tokens, d.cost.usd
        ));
    }
    if d.cached {
        tail.push("cached".into());
    }
    println!("{}", tail.join(" · "));
    if let Some(f) = &d.fallback {
        println!("Fell back to the local scorer: {f}");
    }
}

async fn ask(args: DecideArgs) -> Result<(), String> {
    let question = args.question.join(" ");
    if question.trim().is_empty() {
        return Err(
            "Ask a question with its choices, for example: tern decide \"Which source first?\" --choices arxiv,blog,forum"
                .into(),
        );
    }
    let kind = DecisionKind::parse(&args.kind).ok_or_else(|| {
        format!(
            "`{}` is not a decision kind. Use source, tool, claim, triage, or general.",
            args.kind
        )
    })?;
    let context = match &args.context {
        Some(p) => std::fs::read_to_string(p)
            .map_err(|e| format!("Tern could not read {}: {e}", p.display()))?,
        None => String::new(),
    };
    let decider = decide::decider();
    let d = decide::ask(&decider, kind, &question, &args.choices, &context).await?;
    print_decision(&d, args.json);
    Ok(())
}

async fn run_cmd(cmd: DecideCmd) -> Result<(), String> {
    let keychain = providers::keychain();
    let store: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
    let need_store = || {
        keychain
            .as_ref()
            .map(|b| b.as_ref())
            .map_err(|e| format!("{e}. Keys cannot be saved here yet."))
    };
    match cmd {
        DecideCmd::Status { json } => {
            let prefs = tern_agent::Preferences::load().decision;
            let rows = decide::list(store);
            if json {
                println!(
                    "{}",
                    serde_json::json!({"in_use": prefs.label(), "prefs": prefs, "providers": rows})
                );
                return Ok(());
            }
            println!("Decision model: {}", prefs.label());
            println!();
            for r in rows {
                println!(
                    "  {:<10} {:<13} {:<18} {}  {}",
                    r.id,
                    r.name,
                    r.product,
                    if r.key_saved { "key saved" } else { "no key   " },
                    r.price
                );
            }
            println!();
            println!("Local scorer: {} (offline, free)", decide::LOCAL_MODEL);
            Ok(())
        }
        DecideCmd::Mode {
            mode,
            provider,
            model,
            timeout_ms,
        } => {
            let m = Mode::parse(&mode)
                .ok_or_else(|| format!("`{mode}` is not a mode. Use off, local, or remote."))?;
            let mut prefs = tern_agent::Preferences::load();
            prefs.decision.mode = Some(m.label().to_lowercase());
            if let Some(p) = provider {
                let spec = decide::find_remote(&p)
                    .ok_or_else(|| format!("Tern does not know a decision model called `{p}`."))?;
                if prefs.decision.provider.as_deref() != Some(spec.id) {
                    prefs.decision.model = None;
                }
                prefs.decision.provider = Some(spec.id.to_owned());
            }
            if model.is_some() {
                prefs.decision.model = model;
            }
            if timeout_ms.is_some() {
                prefs.decision.timeout_ms = timeout_ms;
            }
            if m == Mode::Remote && prefs.decision.spec().is_none() {
                return Err("Choose a remote provider with --provider typesafe, subq, or voyage.".into());
            }
            prefs.save()?;
            println!("Decision model: {}", prefs.decision.label());
            Ok(())
        }
        DecideCmd::Key(KeyCmd::Add {
            id,
            stdin,
            model,
            base_url,
        }) => {
            let s = need_store()?;
            let spec = decide::find_remote(&id)
                .ok_or_else(|| format!("Tern does not know a decision model called `{id}`."))?;
            eprintln!("Create a key at {}", spec.key_url);
            let key = crate::read_key(spec.name, stdin)?;
            let d = decide::add(&id, key, s, model.as_deref(), base_url.as_deref()).await?;
            println!(
                "Saved. {} answered in {} ms ({} input tokens, ${:.6}).",
                d.model, d.latency_ms, d.cost.input_tokens, d.cost.usd
            );
            println!("Use it with `tern decide mode remote --provider {}`.", spec.id);
            Ok(())
        }
        DecideCmd::Key(KeyCmd::Test {
            id,
            model,
            base_url,
        }) => {
            let s = need_store()?;
            let d = decide::test_saved(&id, s, model.as_deref(), base_url.as_deref()).await?;
            println!(
                "Works. {} answered in {} ms ({} input tokens, ${:.6}).",
                d.model, d.latency_ms, d.cost.input_tokens, d.cost.usd
            );
            Ok(())
        }
        DecideCmd::Key(KeyCmd::Remove { id }) => {
            let s = need_store()?;
            if decide::remove(&id, s)? {
                println!("Removed the key.");
            } else {
                println!("No key was saved for `{id}`.");
            }
            Ok(())
        }
        DecideCmd::Bench {
            reasoning,
            remote,
            json,
            chat,
        } => {
            let prefs = tern_agent::Preferences::load();
            let remote_model = if remote {
                let spec = prefs.decision.spec().ok_or(
                    "No remote decision model is chosen. Run `tern decide mode remote --provider <id>` first.",
                )?;
                let key = store.and_then(|s| s.get(&decide::key_name(spec)).ok().flatten());
                Some(
                    decide::RemoteModel::new(
                        spec,
                        key,
                        prefs.decision.base_url.as_deref(),
                        prefs.decision.model.as_deref(),
                        prefs.decision.timeout(),
                    )
                    .map_err(|e| e.to_string())?,
                )
            } else {
                None
            };
            let model = if reasoning {
                let (opts, _) = crate::runs::options(chat);
                Some(tern_agent::chat::connect_chat_model(&opts, store).await?)
            } else {
                None
            };
            let r = decide::bench::run(
                model
                    .as_ref()
                    .map(|(p, name)| decide::bench::ReasoningModel {
                        model: p,
                        model_name: name,
                    }),
                remote_model
                    .as_ref()
                    .map(|m| m as &dyn decide::DecisionModel),
            )
            .await;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&r).expect("report serialises")
                );
            } else {
                print!("{}", r.table());
                if !reasoning || !remote {
                    println!();
                    println!(
                        "Offline run. Add --reasoning to compare the chat model and --remote for the remote decision model."
                    );
                }
            }
            Ok(())
        }
    }
}
