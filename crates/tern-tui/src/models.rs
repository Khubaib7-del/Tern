//! `tern models`: list, download, remove, and choose local models
//! (`docs/local-models.md`). Nothing is bundled; a download fetches the
//! files from Hugging Face into `<data>/models/<id>/` and checks them.

use std::io::Write;

use clap::Subcommand;
use tern_agent::Preferences;
use tern_agent::local_models::{
    self, CancelDownload, DownloadEvent, LocalModels, State, Task, human_size,
};

#[derive(Debug, Subcommand)]
pub(crate) enum ModelsCmd {
    /// Every model Tern can download, by task, with what is installed.
    List {
        /// Only this task: ocr, doc_parse, vision, dictation, stt, tts,
        /// embedding.
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Download a model (resumes an interrupted download) and check its files.
    Download {
        /// The model's id, as `tern models list` shows it.
        id: String,
        /// Also make it the default for its task.
        #[arg(long)]
        default: bool,
    },
    /// Delete a downloaded model's files.
    Remove { id: String },
    /// Show the defaults, or set one: `tern models default ocr <id>`
    /// (`none` clears it).
    Default {
        task: Option<String>,
        id: Option<String>,
    },
    /// Read the text in a picture with a local OCR model (the one in use
    /// for OCR unless `--model` names another).
    Read {
        /// A PNG, JPEG, GIF, or WebP file.
        image: std::path::PathBuf,
        #[arg(long)]
        model: Option<String>,
    },
    /// Transcribe an audio file on this computer with a local Whisper model
    /// (the one in use for transcription unless `--model` names another).
    Transcribe {
        /// WAV, MP3, M4A/AAC, Ogg Vorbis, or FLAC.
        file: std::path::PathBuf,
        #[arg(long)]
        model: Option<String>,
        /// The spoken language as a code (`en`, `ur`); detected when left
        /// out.
        #[arg(long)]
        language: Option<String>,
    },
    /// Describe a picture on this computer with a local vision model
    /// (SmolVLM; through the `tern-vlm` helper shipped beside Tern).
    Describe {
        /// A PNG, JPEG, GIF, or WebP file.
        image: std::path::PathBuf,
        #[arg(long)]
        model: Option<String>,
        /// What to ask about it.
        #[arg(
            long,
            default_value = "Describe this picture in detail. Transcribe any visible text exactly."
        )]
        prompt: String,
    },
    /// Speak English text with the local voice model (Kokoro) into a WAV
    /// file (24 kHz mono).
    Speak {
        text: String,
        /// Where to write the WAV.
        #[arg(long)]
        out: std::path::PathBuf,
        #[arg(long)]
        model: Option<String>,
        /// af_heart, am_michael, bf_emma, bm_george.
        #[arg(long, default_value = "af_heart")]
        voice: String,
        #[arg(long, default_value_t = 1.0)]
        speed: f32,
    },
    /// Convert a document (PDF, DOCX, PPTX, XLSX, HTML) to Markdown, as
    /// the app does for attachments.
    Convert { file: std::path::PathBuf },
    /// Switch an installed model on or off without deleting it.
    Enable {
        id: String,
        /// `off` switches it off.
        #[arg(default_value = "on")]
        state: String,
    },
}

fn open() -> Result<LocalModels, String> {
    let (lm, warn) = LocalModels::discover()?;
    if let Some(w) = warn {
        eprintln!("warning: {w}");
    }
    Ok(lm)
}

fn parse_task(s: &str) -> Result<Task, String> {
    Task::parse(s).ok_or_else(|| {
        format!(
            "`{s}` is not a task; use one of: {}",
            Task::ALL.map(|t| t.key()).join(", ")
        )
    })
}

pub(crate) async fn run(cmd: ModelsCmd) -> Result<(), String> {
    let lm = open()?;
    let mut prefs = Preferences::load();
    match cmd {
        ModelsCmd::List { task, json } => {
            let only = task.as_deref().map(parse_task).transpose()?;
            let st: Vec<_> = lm
                .statuses(&prefs.local_models)
                .into_iter()
                .filter(|s| only.is_none_or(|t| s.entry.task == t))
                .collect();
            if json {
                let rows: Vec<_> = st
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "id": s.entry.id,
                            "task": s.entry.task,
                            "name": s.entry.name,
                            "role": s.entry.role,
                            "size": s.entry.size(),
                            "licence": s.entry.licence,
                            "runtime": s.entry.runtime,
                            "placeholder": s.entry.placeholder,
                            "installed": s.installed(),
                            "partial_bytes": match s.state { State::Partial(n) => n, _ => 0 },
                            "enabled": s.enabled,
                            "default": s.is_default,
                            "runnable": local_models::can_run(&s.entry),
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&rows).unwrap_or_default()
                );
                return Ok(());
            }
            for t in Task::ALL {
                if only.is_some_and(|o| o != t) {
                    continue;
                }
                let rows: Vec<_> = st.iter().filter(|s| s.entry.task == t).collect();
                println!("{}", t.label());
                if rows.is_empty() {
                    println!("  (none listed yet)");
                }
                for s in rows {
                    let state = match s.state {
                        State::Installed if !s.enabled => "installed, off".to_owned(),
                        State::Installed => "installed".to_owned(),
                        State::Partial(n) => format!("partial {}", human_size(n)),
                        State::NotInstalled => "-".to_owned(),
                    };
                    println!(
                        "  {} {:<28} {:<11} {:>8}  {:<11} {:<14} {}{}",
                        if s.is_default { '*' } else { ' ' },
                        s.entry.id,
                        s.entry.role.badge(),
                        human_size(s.entry.size()),
                        s.entry.licence,
                        state,
                        s.entry.name,
                        if local_models::can_run(&s.entry) {
                            String::new()
                        } else {
                            format!("  [cannot run yet: {}]", s.entry.runtime.label())
                        }
                    );
                }
            }
            println!(
                "\n* your default for its task. Default and Alternative are Tern's picks; \
                 Optional parts download only when wanted.\nModels are stored in {}",
                lm.root().display()
            );
            Ok(())
        }
        ModelsCmd::Download { id, default } => {
            let entry = lm
                .manifest()
                .find(&id)
                .ok_or_else(|| format!("no local model is called `{id}`; see `tern models list`"))?
                .clone();
            eprintln!(
                "Downloading {} ({}, {}) from {}",
                entry.name,
                human_size(entry.size()),
                entry.licence,
                entry.page()
            );
            let cancel = CancelDownload::new();
            let mut last = 0u64;
            let mut on = |e: DownloadEvent| match e {
                DownloadEvent::Progress { done, total } => {
                    // Redraw at most every ~1% to keep the terminal quiet.
                    if total == 0 || done == total || done.saturating_sub(last) * 100 >= total {
                        last = done;
                        let pct = (done * 100).checked_div(total).unwrap_or(100);
                        eprint!(
                            "\r  {pct:>3}%  {} / {}   ",
                            human_size(done),
                            human_size(total)
                        );
                        let _ = std::io::stderr().flush();
                    }
                }
                DownloadEvent::Verifying(f) => {
                    eprint!("\r  checking {f}                    ");
                    let _ = std::io::stderr().flush();
                }
            };
            lm.download(&id, &cancel, &mut on)
                .await
                .map_err(|e| format!("\n{e}"))?;
            eprintln!(
                "\rInstalled {} in {}            ",
                entry.id,
                lm.dir(&id).display()
            );
            if default {
                prefs.local_models.set_default(entry.task, Some(&id));
                prefs.save()?;
                eprintln!("It is now the default for {}.", entry.task.label());
            }
            if !local_models::can_run(&entry) {
                eprintln!(
                    "Note: this build cannot run it yet ({} is not linked for this task); the files are ready for when it can.",
                    entry.runtime.label()
                );
            }
            Ok(())
        }
        ModelsCmd::Remove { id } => {
            lm.remove(&id).map_err(|e| e.to_string())?;
            prefs.local_models.forget(&id);
            prefs.save()?;
            println!("Removed {id}.");
            Ok(())
        }
        ModelsCmd::Default { task: None, .. } => {
            for t in Task::ALL {
                let d = prefs.local_models.default_for(t).unwrap_or("-");
                let using = lm
                    .usable(&prefs.local_models, t)
                    .map(|e| e.id.as_str())
                    .unwrap_or("-");
                println!("{:<18} default {d:<26} in use {using}", t.label());
            }
            Ok(())
        }
        ModelsCmd::Default {
            task: Some(task),
            id,
        } => {
            let t = parse_task(&task)?;
            match id.as_deref() {
                None => {
                    println!("{}", prefs.local_models.default_for(t).unwrap_or("-"));
                    return Ok(());
                }
                Some("none" | "-") => prefs.local_models.set_default(t, None),
                Some(id) => {
                    let e = lm
                        .manifest()
                        .find(id)
                        .ok_or_else(|| format!("no local model is called `{id}`"))?;
                    if e.task != t {
                        return Err(format!(
                            "{id} is a {} model, not {}",
                            e.task.label(),
                            t.label()
                        ));
                    }
                    if lm.installed(id).is_none() {
                        eprintln!(
                            "Note: {id} is not downloaded yet (`tern models download {id}`)."
                        );
                    }
                    prefs.local_models.set_default(t, Some(id));
                }
            }
            prefs.save()?;
            println!("Saved.");
            Ok(())
        }
        ModelsCmd::Read { image, model } => {
            let id = match model {
                Some(id) => id,
                None => lm
                    .usable(&prefs.local_models, Task::Ocr)
                    .map(|e| e.id.clone())
                    .ok_or("no local OCR model is installed and on; try `tern models download pp-ocrv6-small`")?,
            };
            let bytes = std::fs::read(&image)
                .map_err(|e| format!("cannot read {}: {e}", image.display()))?;
            println!("{}", lm.ocr(&id, &bytes)?);
            Ok(())
        }
        ModelsCmd::Transcribe {
            file,
            model,
            language,
        } => {
            let id = match model {
                Some(id) => id,
                None => lm
                    .usable(&prefs.local_models, Task::SpeechToText)
                    .map(|e| e.id.clone())
                    .ok_or("no local transcription model is installed and on; try `tern models download whisper-small-q5_1`")?,
            };
            let bytes =
                std::fs::read(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let name = file.to_string_lossy().into_owned();
            let started = std::time::Instant::now();
            let ctl = tern_agent::local_models::TranscribeCtl::new();
            let t = lm.transcribe_file_with(&id, &bytes, &name, language.as_deref(), &ctl)?;
            println!("{}", t.text.trim());
            // Where the time went, so a slow run can be told apart: the
            // file (decode), the resampler, or the model.
            eprintln!(
                "({id}, {:.1} s on this computer for {:.1} s of audio: decode {} ms, resample {} ms, model {} ms; {})",
                started.elapsed().as_secs_f32(),
                t.audio_secs,
                t.decode_ms,
                t.resample_ms,
                t.model_ms,
                t.engine
            );
            Ok(())
        }
        ModelsCmd::Describe {
            image,
            model,
            prompt,
        } => {
            let id = match model {
                Some(id) => id,
                None => lm
                    .usable(&prefs.local_models, Task::Vision)
                    .map(|e| e.id.clone())
                    .ok_or("no local vision model is installed and on; try `tern models download smolvlm-500m`")?,
            };
            let bytes = std::fs::read(&image)
                .map_err(|e| format!("cannot read {}: {e}", image.display()))?;
            let started = std::time::Instant::now();
            println!("{}", lm.describe(&id, &bytes, &prompt)?);
            eprintln!(
                "({id}, {:.1} s on this computer)",
                started.elapsed().as_secs_f32()
            );
            Ok(())
        }
        ModelsCmd::Speak {
            text,
            out,
            model,
            voice,
            speed,
        } => {
            let id = match model {
                Some(id) => id,
                None => lm
                    .usable(&prefs.local_models, Task::TextToSpeech)
                    .map(|e| e.id.clone())
                    .ok_or("no local voice model is installed and on; try `tern models download kokoro-82m-int8`")?,
            };
            let started = std::time::Instant::now();
            let audio = lm.speak(&id, &text, &voice, speed)?;
            let wav = tern_agent::platform::audio::wav(&audio, 24_000);
            std::fs::write(&out, wav)
                .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
            eprintln!(
                "({id}, {:.1} s of speech in {:.1} s)",
                audio.len() as f32 / 24_000.0,
                started.elapsed().as_secs_f32()
            );
            Ok(())
        }
        ModelsCmd::Convert { file } => {
            let bytes =
                std::fs::read(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let c = tern_agent::docs::convert(&name, &bytes)?;
            println!("{}", c.with_header(&name));
            Ok(())
        }
        ModelsCmd::Enable { id, state } => {
            if lm.manifest().find(&id).is_none() {
                return Err(format!("no local model is called `{id}`"));
            }
            let on = !matches!(state.as_str(), "off" | "false" | "no" | "0");
            prefs.local_models.set_enabled(&id, on);
            prefs.save()?;
            println!("{id} is {}.", if on { "on" } else { "off" });
            Ok(())
        }
    }
}
