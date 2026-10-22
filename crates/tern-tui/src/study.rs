//! `tern study`, `tern tutor`, `tern quiz`, and `tern viva`: the Study
//! harness in the terminal.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Instant;

use tern_agent::study::{
    self, Answered, Difficulty, QuestionKind, QuizAttempt, StudyEngine, StudyLibrary, StudyStore,
    TutorMode, TutorTurn,
};

use crate::ChatArgs;

#[derive(Debug, clap::Subcommand)]
pub enum StudyCmd {
    /// Add study materials: files, folders, or `.zip`s (PDFs are read page
    /// by page), vault notes, a research report, or web pages.
    Add {
        /// Files, folders, or archives.
        files: Vec<PathBuf>,
        /// A vault note by name (repeat for more).
        #[arg(long)]
        note: Vec<String>,
        /// A chat's latest research report: `last`, its id, or the start of it.
        #[arg(long)]
        report: Option<String>,
        /// A web page or online PDF (repeat for more).
        #[arg(long)]
        link: Vec<String>,
    },
    /// List the materials.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Remove a material (its number, id, or name).
    Rm { which: String },
    /// Show the topics, their learning objectives, and your mastery.
    Topics {
        /// Ask the model to regroup the materials into topics.
        #[arg(long)]
        rebuild: bool,
        /// With --rebuild, group by headings instead of asking the model.
        #[arg(long)]
        offline: bool,
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        chat: ChatArgs,
    },
    /// Past quizzes and vivas.
    History,
}

fn dim(line: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("\x1b[2m• {line}\x1b[0m");
    } else {
        eprintln!("• {line}");
    }
}

fn open() -> Result<(StudyStore, StudyLibrary), String> {
    let store = StudyStore::discover()?;
    let lib = store.load()?;
    Ok((store, lib))
}

/// The user's model, unless they asked for offline study or none connects.
async fn engine(chat: ChatArgs, offline: bool) -> Option<StudyEngine> {
    let (opts, prefs) = crate::runs::options(chat);
    if offline || prefs.study_offline {
        dim("Offline study: no model; passages come straight from your materials.");
        return None;
    }
    match StudyEngine::open(opts).await {
        Ok(e) => {
            dim(&format!(
                "Passages from your materials go to {} ({}).",
                e.provider_name(),
                e.model_name()
            ));
            Some(e)
        }
        Err(err) => {
            dim(&format!("No model ({err}). Studying offline."));
            None
        }
    }
}

fn read_line(prompt: &str) -> Option<String> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_owned()),
    }
}

fn ring(rating: f32) -> &'static str {
    match (rating * 4.0).round() as u32 {
        0 => "○",
        1 => "◔",
        2 => "◑",
        3 => "◕",
        _ => "●",
    }
}

/// `tern study …`.
pub async fn study(cmd: StudyCmd) -> Result<(), String> {
    let (store, mut lib) = open()?;
    match cmd {
        StudyCmd::Add {
            files,
            note,
            report,
            link,
        } => {
            if files.is_empty() && note.is_empty() && report.is_none() && link.is_empty() {
                return Err(
                    "Name something to add: files, `--note`, `--report last`, or `--link URL`."
                        .into(),
                );
            }
            let mut added = Vec::new();
            let mut failed = 0;
            for f in &files {
                match study::add_path(&store, &mut lib, f) {
                    Ok(ms) => added.extend(ms),
                    Err(e) => {
                        eprintln!("{e}");
                        failed += 1;
                    }
                }
            }
            if !note.is_empty() {
                let vault = tern_agent::vault::Vault::open_default()?;
                for n in &note {
                    match study::add_note(&store, &mut lib, &vault, n) {
                        Ok(m) => added.push(m),
                        Err(e) => {
                            eprintln!("{e}");
                            failed += 1;
                        }
                    }
                }
            }
            if let Some(r) = &report {
                match study::add_report(&store, &mut lib, r) {
                    Ok(m) => added.push(m),
                    Err(e) => {
                        eprintln!("{e}");
                        failed += 1;
                    }
                }
            }
            for l in &link {
                match study::add_link(&store, &mut lib, l).await {
                    Ok(m) => added.push(m),
                    Err(e) => {
                        eprintln!("{e}");
                        failed += 1;
                    }
                }
            }
            study::ensure_topics(&store, &mut lib)?;
            for m in &added {
                println!("Added [{}] {} · {}", m.id, m.name, meta(m));
            }
            println!(
                "{} materials, {} topics. `tern study topics --rebuild` asks the model to regroup them.",
                lib.materials.len(),
                lib.topics.len()
            );
            if failed > 0 && added.is_empty() {
                return Err("Nothing was added.".into());
            }
            Ok(())
        }
        StudyCmd::List { json } => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&lib.materials).unwrap_or_default()
                );
                return Ok(());
            }
            if lib.materials.is_empty() {
                println!("No materials yet. `tern study add notes.pdf` adds one.");
            }
            for (i, m) in lib.materials.iter().enumerate() {
                println!("{}. [{}] {} · {}", i + 1, m.id, m.name, meta(m));
            }
            Ok(())
        }
        StudyCmd::Rm { which } => {
            let m = study::remove(&store, &mut lib, &which)?;
            study::ensure_topics(&store, &mut lib)?;
            println!("Removed {}. {} topics now.", m.name, lib.topics.len());
            Ok(())
        }
        StudyCmd::Topics {
            rebuild,
            offline,
            json,
            chat,
        } => {
            if rebuild {
                let e = engine(chat, offline).await;
                let (by, note) = study::build_topics(&store, &mut lib, e.as_ref()).await?;
                if let Some(n) = note {
                    dim(&n);
                }
                dim(&format!(
                    "Topics by {}.",
                    if by == "model" {
                        "the model"
                    } else {
                        "headings and key words"
                    }
                ));
            } else {
                study::ensure_topics(&store, &mut lib)?;
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&lib.topics).unwrap_or_default()
                );
                return Ok(());
            }
            if lib.topics.is_empty() {
                println!("No topics yet. Add materials with `tern study add <files>`.");
            }
            let now = study::now();
            for (i, t) in lib.topics.iter().enumerate() {
                let m = lib.mastery_of(&t.id);
                println!(
                    "{} {}. {}  ({})",
                    ring(m.rating),
                    i + 1,
                    t.title,
                    t.where_label()
                );
                println!("     {}", m.explain(now));
                for o in &t.objectives {
                    println!("     - {o}");
                }
            }
            Ok(())
        }
        StudyCmd::History => {
            if lib.quizzes.is_empty() && lib.vivas.is_empty() {
                println!("No quizzes or vivas yet.");
            }
            let now = study::now();
            for q in lib.quizzes.iter().rev().take(20) {
                println!(
                    "Quiz [{}] {} · {:.1} / {} · {}",
                    q.id,
                    q.title,
                    q.score,
                    q.total,
                    study::ago(now - q.taken_at)
                );
            }
            for v in lib.vivas.iter().rev().take(20) {
                println!(
                    "Viva [{}] {} · {} / 100 · {} questions · {}",
                    v.id,
                    v.title,
                    v.overall,
                    v.questions,
                    study::ago(now - v.taken_at)
                );
            }
            Ok(())
        }
    }
}

fn meta(m: &study::Material) -> String {
    let mut s = m.kind.label().to_owned();
    if m.pages > 0 {
        s.push_str(&format!(" · {} pages", m.pages));
    }
    s.push_str(&format!(" · {} chunks", m.chunks));
    if m.cut {
        s.push_str(" · cut to fit");
    }
    s
}

fn need_materials(lib: &StudyLibrary) -> Result<(), String> {
    if lib.materials.is_empty() {
        return Err("Add materials first: `tern study add notes.pdf` (or `--report last` for a research report).".into());
    }
    Ok(())
}

/// `tern tutor [topic]`.
pub async fn tutor(
    topic: Option<String>,
    mode: String,
    offline: bool,
    chat: ChatArgs,
) -> Result<(), String> {
    let (store, mut lib) = open()?;
    need_materials(&lib)?;
    study::ensure_topics(&store, &mut lib)?;
    let mut mode = TutorMode::parse(&mode).ok_or("Modes: explain, socratic, new.")?;
    let topic = match topic {
        Some(t) => lib
            .topic(&t)
            .cloned()
            .ok_or_else(|| format!("No topic matches \"{t}\". `tern study topics` lists them."))?,
        None => lib.weakest(study::now()).cloned().ok_or("No topics yet.")?,
    };
    let e = engine(chat, offline).await;
    println!("Tutor · {} · {}", topic.title, mode.label());
    dim(
        "Type your answer or question. `/mode socratic|explain|new` switches mode; an empty line or `/quit` ends.",
    );
    let mut history: Vec<TutorTurn> = Vec::new();
    let print = |t: &str| {
        print!("{t}");
        let _ = std::io::stdout().flush();
        true
    };
    loop {
        let reply = study::tutor_reply(
            &store,
            &mut lib,
            e.as_ref(),
            &topic.id,
            mode,
            &history,
            Some(&print),
        )
        .await?;
        println!();
        if let Some(n) = &reply.note {
            dim(n);
        }
        if !reply.citations.is_empty() {
            dim(&format!("Cited: {}", reply.citations.join(" · ")));
        }
        if history.is_empty() {
            history.push(TutorTurn::user(study::tutor_opening(mode, &topic)));
        }
        history.push(TutorTurn::tutor(reply.text));
        let Some(line) = read_line("\n› ") else {
            break;
        };
        let line = line.trim();
        if line.is_empty() || line == "/quit" || line == "/q" {
            break;
        }
        if let Some(m) = line.strip_prefix("/mode") {
            match TutorMode::parse(m) {
                Some(m) => {
                    mode = m;
                    dim(&format!("Mode: {}", mode.label()));
                    history.push(TutorTurn::user(format!(
                        "Switch to {} mode and continue.",
                        mode.label()
                    )));
                }
                None => {
                    dim("Modes: explain, socratic, new.");
                    continue;
                }
            }
        } else {
            history.push(TutorTurn::user(line));
        }
    }
    dim(&lib.mastery_of(&topic.id).explain(study::now()));
    Ok(())
}

/// `tern quiz [topic]`.
#[allow(clippy::too_many_arguments)]
pub async fn quiz(
    topic: Option<String>,
    n: Option<usize>,
    difficulty: Option<String>,
    kinds: Option<String>,
    offline: bool,
    json: bool,
    chat: ChatArgs,
) -> Result<(), String> {
    let (store, mut lib) = open()?;
    need_materials(&lib)?;
    let prefs = tern_agent::Preferences::load();
    let mut spec = prefs.quiz_spec(topic);
    if let Some(n) = n {
        spec.count = n.clamp(1, 30);
    }
    if let Some(d) = difficulty {
        spec.difficulty = Difficulty::parse(&d).ok_or("Difficulty: easy, medium, or hard.")?;
    }
    if let Some(k) = kinds {
        spec.kinds = k
            .split(',')
            .map(|x| {
                QuestionKind::parse(x).ok_or_else(|| {
                    format!("Unknown question type \"{x}\" (mcq, short, tf, blank).")
                })
            })
            .collect::<Result<_, _>>()?;
    }
    let e = engine(chat, offline).await;
    let (quiz, note) = study::make_quiz(&store, &mut lib, e.as_ref(), &spec).await?;
    if let Some(n) = note {
        dim(&n);
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&quiz).unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "Quiz · {} · {} · {} questions",
        quiz.title,
        quiz.difficulty.label(),
        quiz.questions.len()
    );
    let mut attempt = QuizAttempt {
        started_at: study::now(),
        ..QuizAttempt::default()
    };
    for q in &quiz.questions {
        println!("\n{}/{} · {}", q.n, quiz.questions.len(), q.kind.label());
        println!("{}", q.prompt);
        for (i, c) in q.choices.iter().enumerate() {
            println!("  {}. {c}", study::letter(i));
        }
        let hint = match q.kind {
            QuestionKind::MultipleChoice => "Letter",
            QuestionKind::TrueFalse => "True or false",
            QuestionKind::FillBlank => "The missing word",
            QuestionKind::ShortAnswer => "Your answer",
        };
        let Some(given) = read_line(&format!("{hint} › ")) else {
            break;
        };
        let g = study::grade(e.as_ref(), q, &given).await;
        println!("{} {} — {}", g.mark.glyph(), g.mark.label(), g.feedback);
        attempt.answers.push(Answered {
            n: q.n,
            given,
            grade: g,
        });
    }
    let s = study::finish_quiz(&store, &mut lib, &quiz, attempt.clone())?;
    println!("\nScore: {}", attempt.score_label(s.total));
    for t in quiz
        .questions
        .iter()
        .filter_map(|q| q.topic.clone())
        .collect::<std::collections::BTreeSet<_>>()
    {
        if let Some(topic) = lib.topic(&t) {
            println!(
                "  {} {}: {}",
                ring(lib.mastery_of(&t).rating),
                topic.title,
                lib.mastery_of(&t).explain(study::now())
            );
        }
    }
    dim(&format!(
        "Saved as quiz {} in `tern study history`.",
        quiz.id
    ));
    Ok(())
}

/// `tern viva`.
pub async fn viva(
    title: Option<String>,
    n: Option<usize>,
    offline: bool,
    chat: ChatArgs,
) -> Result<(), String> {
    let (store, mut lib) = open()?;
    need_materials(&lib)?;
    let prefs = tern_agent::Preferences::load();
    let planned = n.unwrap_or_else(|| prefs.viva_questions());
    let e = engine(chat, offline).await;
    let mut v = study::start_viva(&store, &mut lib, title.as_deref(), planned)?;
    println!("Viva practice · {} · {} questions", v.title, v.planned);
    dim("Answer in a paragraph and press Enter. `/end` finishes early and scores what you have.");
    let started = Instant::now();
    while !v.is_done() {
        if let Some(note) = study::viva_question(&store, &lib, e.as_ref(), &mut v).await? {
            dim(&note);
        }
        let turn = v.turns.last().expect("a question was just asked");
        println!(
            "\nQ{} · level {}{}  [{}]",
            v.turns.len(),
            turn.level,
            if turn.follow_up { " · follow-up" } else { "" },
            clock(started.elapsed().as_secs())
        );
        println!("{}", turn.question);
        let asked = Instant::now();
        let Some(answer) = read_line("› ") else {
            break;
        };
        if answer.trim() == "/end" {
            break;
        }
        v.answer(&answer, asked.elapsed().as_secs() as u32);
    }
    let (report, md, note) = study::finish_viva(&store, &mut lib, e.as_ref(), &mut v).await?;
    if let Some(n) = note {
        dim(&n);
    }
    println!(
        "\nOverall: {} / 100 ({})",
        report.overall(),
        clock(started.elapsed().as_secs())
    );
    for (name, s) in report.rubric.rows() {
        println!(
            "  {name:<15} {}{} {s}/5",
            "■".repeat(s as usize),
            "□".repeat(5 - s as usize)
        );
    }
    if !report.summary.is_empty() {
        println!("\n{}", report.summary);
    }
    for (head, items) in [
        ("Strengths", &report.strengths),
        ("Weaknesses", &report.weaknesses),
        ("Revise next", &report.suggested_topics),
    ] {
        if !items.is_empty() {
            println!("\n{head}:");
            for i in items {
                println!("  - {i}");
            }
        }
    }
    if let Some(p) = md {
        dim(&format!("Report saved: {}", p.display()));
    }
    Ok(())
}

fn clock(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}
