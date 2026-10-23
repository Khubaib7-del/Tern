//! Extensibility commands: `tern mcp`, `tern acp`, `tern skills`,
//! `tern instructions`, and `tern project`.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use clap::Subcommand;
use tern_agent::mcp::{self, HealthState, McpConfig, ServerConfig, Severity, TrustPreview};
use tern_agent::policy::Mode;
use tern_agent::{open_store, projects, skills};

use crate::ChatArgs;
use crate::runs::options;

fn data_dir() -> Result<PathBuf, String> {
    mcp::config::data_dir()
}

fn dim(line: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("\x1b[2m• {line}\x1b[0m");
    } else {
        eprintln!("• {line}");
    }
}

fn confirm(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn pairs(items: &[String], what: &str) -> Result<Vec<(String, String)>, String> {
    items
        .iter()
        .map(|kv| {
            kv.split_once('=')
                .map(|(k, v)| (k.trim().to_owned(), v.to_owned()))
                .filter(|(k, _)| !k.is_empty())
                .ok_or_else(|| format!("{what} must look like NAME=value, not “{kv}”."))
        })
        .collect()
}

fn parse_mode(s: &str) -> Result<Mode, String> {
    match s.to_ascii_lowercase().as_str() {
        "ask" => Ok(Mode::Ask),
        "allow" => Ok(Mode::Allow),
        "deny" | "refuse" => Ok(Mode::Deny),
        _ => Err("Approval is ask, allow, or deny.".into()),
    }
}

fn mode_label(m: Mode) -> &'static str {
    match m {
        Mode::Ask => "asks",
        Mode::Allow => "allowed",
        Mode::Deny => "refused",
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum McpCmd {
    /// Add a server, switched off until you review it with `tern mcp preview`.
    /// stdio: `tern mcp add notes -- npx -y notes-mcp`. HTTP: `--url`.
    Add {
        name: String,
        /// A Streamable HTTP server's address.
        #[arg(long)]
        url: Option<String>,
        /// An environment variable for a stdio server (repeatable).
        #[arg(long, short, value_name = "NAME=VALUE")]
        env: Vec<String>,
        /// A header for an HTTP server (repeatable).
        #[arg(long, value_name = "NAME=VALUE")]
        header: Vec<String>,
        /// The folder a stdio server starts in.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// The command and its arguments, after `--`.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Add every server from another app's config (Claude Desktop's shape).
    Import { file: PathBuf },
    /// Every server, whether it is on, and its tools' settings.
    List {
        #[arg(long)]
        json: bool,
    },
    /// The trust preview: the command, its environment, every tool and its
    /// description, and anything suspicious. `--trust` switches it on.
    Preview {
        name: String,
        /// Trust what was shown and switch the server on.
        #[arg(long, short = 'y')]
        trust: bool,
        #[arg(long)]
        json: bool,
    },
    /// Switch a reviewed server on, or one of its tools (with its approval).
    Enable {
        name: String,
        #[arg(long)]
        tool: Option<String>,
        /// For `--tool`: ask (the default), allow, or deny.
        #[arg(long)]
        approval: Option<String>,
    },
    /// Switch a server off, or one of its tools.
    Disable {
        name: String,
        #[arg(long)]
        tool: Option<String>,
    },
    /// Start, ping, and list each server, timing each step.
    Health {
        name: Option<String>,
        /// Stop and start the server first.
        #[arg(long)]
        restart: bool,
        #[arg(long)]
        json: bool,
    },
    /// Remove a server.
    Rm { name: String },
}

fn print_preview(p: &TrustPreview) {
    println!("{} · {} · {}", p.server, p.transport, p.target);
    if let Some(c) = &p.cwd {
        println!("  starts in {c}");
    }
    for (k, v) in &p.env {
        println!("  env {k}={v}");
    }
    for (k, v) in &p.headers {
        println!("  header {k}: {v}");
    }
    if let Some(i) = &p.info {
        println!(
            "  says it is {} {} (protocol {})",
            if i.name.is_empty() {
                "(unnamed)"
            } else {
                &i.name
            },
            i.version,
            i.protocol
        );
        if let Some(text) = &i.instructions {
            println!(
                "  its instructions (never sent to the model): {}",
                text.trim()
            );
        }
    }
    let mark = |s: Severity| match s {
        Severity::Danger => "⚠",
        Severity::Note => "·",
    };
    for f in &p.flags {
        println!(
            "  {} {}{}",
            mark(f.severity),
            f.reason,
            if f.excerpt.is_empty() {
                String::new()
            } else {
                format!(": {}", f.excerpt)
            }
        );
    }
    if let Some(e) = &p.error {
        println!("\nCould not read its tools: {e}");
        return;
    }
    println!(
        "\n{} tool{}:",
        p.tools.len(),
        if p.tools.len() == 1 { "" } else { "s" }
    );
    for t in &p.tools {
        println!(
            "\n  {} ({}){}",
            t.name,
            t.effect,
            if t.params.is_empty() {
                String::new()
            } else {
                format!(" — {}", t.params.join(", "))
            }
        );
        for line in t.description.lines().filter(|l| !l.trim().is_empty()) {
            println!("    {}", line.trim());
        }
        for f in &t.flags {
            println!(
                "    {} {}{}",
                mark(f.severity),
                f.reason,
                if f.excerpt.is_empty() {
                    String::new()
                } else {
                    format!(": “{}”", f.excerpt)
                }
            );
        }
    }
    if p.changed {
        println!("\nIts tools changed since you last trusted it.");
    }
    let d = p.dangers();
    if d > 0 {
        println!(
            "\n{d} warning{} above. Tools with a warning start switched off if you trust this server.",
            if d == 1 { "" } else { "s" }
        );
    }
}

pub(crate) async fn mcp(cmd: McpCmd) -> Result<(), String> {
    match cmd {
        McpCmd::Add {
            name,
            url,
            env,
            header,
            cwd,
            command,
        } => {
            let mut cfg = McpConfig::load()?;
            let mut s = ServerConfig::default();
            match (url, command.split_first()) {
                (Some(u), None) => {
                    s.kind = Some("http".into());
                    s.url = Some(u);
                }
                (None, Some((c, args))) => {
                    s.command = Some(c.clone());
                    s.args = args.to_vec();
                }
                (Some(_), Some(_)) => return Err("Give a --url or a command, not both.".into()),
                (None, None) => {
                    return Err(
                        "Give the server's command after `--` (tern mcp add notes -- npx -y notes-mcp), or --url.".into(),
                    );
                }
            }
            s.env = pairs(&env, "--env")?.into_iter().collect();
            s.headers = pairs(&header, "--header")?.into_iter().collect();
            s.cwd = cwd.map(|c| c.to_string_lossy().into_owned());
            cfg.add(&name, s)?;
            cfg.save()?;
            println!("Added “{name}”. It is off until you review it: tern mcp preview {name}");
            Ok(())
        }
        McpCmd::Import { file } => {
            let text =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let other: McpConfig = serde_json::from_str(&text)
                .map_err(|e| format!("{} is not an MCP config: {e}", file.display()))?;
            let mut cfg = McpConfig::load()?;
            let mut added = Vec::new();
            for (name, s) in other.servers {
                match cfg.add(&name, s) {
                    Ok(()) => added.push(name),
                    Err(e) => dim(&e),
                }
            }
            cfg.save()?;
            println!(
                "Added {} server{}: {}. Each is off until you review it with `tern mcp preview <name>`.",
                added.len(),
                if added.len() == 1 { "" } else { "s" },
                added.join(", ")
            );
            Ok(())
        }
        McpCmd::List { json } => {
            let cfg = McpConfig::load()?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?
                );
                return Ok(());
            }
            if cfg.servers.is_empty() {
                println!("No MCP servers yet. Add one: tern mcp add <name> -- <command> [args…]");
                return Ok(());
            }
            for (name, s) in &cfg.servers {
                let state = if s.active() {
                    "on"
                } else if s.tern.trusted.is_none() {
                    "not reviewed"
                } else {
                    "off"
                };
                println!("{name:<20} {state:<13} {}", s.target());
                for (t, set) in &s.tern.tools {
                    println!(
                        "    {t}: {}{}",
                        if set.enabled { "on" } else { "off" },
                        if set.enabled {
                            format!(", {}", mode_label(set.approval))
                        } else {
                            String::new()
                        }
                    );
                }
            }
            Ok(())
        }
        McpCmd::Preview { name, trust, json } => {
            let mut cfg = McpConfig::load()?;
            let server = cfg.get(&name)?.clone();
            dim(&format!("Starting “{name}” to read its tools…"));
            let p = mcp::trust::preview(&name, &server).await;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?
                );
            } else {
                print_preview(&p);
            }
            if p.error.is_some() {
                return Ok(());
            }
            let yes = trust || (!json && confirm("\nTrust these tools and switch the server on?"));
            if yes {
                mcp::trust::trust(&mut cfg, &name, &p)?;
                cfg.save()?;
                println!(
                    "Trusted “{name}”. Its tools ask before each call; change that with `tern mcp enable {name} --tool <tool> --approval allow`."
                );
            } else if !json {
                println!("Nothing changed. Run `tern mcp preview {name} --trust` to trust it.");
            }
            Ok(())
        }
        McpCmd::Enable {
            name,
            tool,
            approval,
        } => {
            let mut cfg = McpConfig::load()?;
            let s = cfg.get_mut(&name)?;
            match tool {
                None => {
                    if s.tern.trusted.is_none() {
                        return Err(format!("Review “{name}” first: tern mcp preview {name}"));
                    }
                    s.tern.enabled = true;
                    s.disabled = false;
                    println!("“{name}” is on for new chats.");
                }
                Some(t) => {
                    let mut set = s.tool(&t);
                    set.enabled = true;
                    if let Some(a) = approval {
                        set.approval = parse_mode(&a)?;
                    }
                    println!("{name} · {t}: on, {}.", mode_label(set.approval));
                    s.tern.tools.insert(t, set);
                }
            }
            cfg.save()
        }
        McpCmd::Disable { name, tool } => {
            let mut cfg = McpConfig::load()?;
            let s = cfg.get_mut(&name)?;
            match tool {
                None => {
                    s.tern.enabled = false;
                    println!("“{name}” is off.");
                }
                Some(t) => {
                    let mut set = s.tool(&t);
                    set.enabled = false;
                    s.tern.tools.insert(t.clone(), set);
                    println!("{name} · {t}: off.");
                }
            }
            cfg.save()?;
            mcp::hub().stop(&name).await;
            Ok(())
        }
        McpCmd::Health {
            name,
            restart,
            json,
        } => {
            let cfg = McpConfig::load()?;
            let names: Vec<String> = match name {
                Some(n) => {
                    cfg.get(&n)?;
                    vec![n]
                }
                None => cfg.servers.keys().cloned().collect(),
            };
            if names.is_empty() {
                println!("No MCP servers yet.");
                return Ok(());
            }
            let mut all = Vec::new();
            for n in names {
                let h = mcp::check(&n, cfg.get(&n)?, restart).await;
                if !json {
                    let dot = match h.state {
                        HealthState::Ok => "●",
                        HealthState::Slow => "◐",
                        HealthState::Down => "✕",
                        HealthState::Off | HealthState::NeedsReview => "○",
                    };
                    let ms =
                        |v: Option<u64>| v.map(|v| format!("{v} ms")).unwrap_or_else(|| "—".into());
                    println!(
                        "{dot} {:<20} {:<13} start {} · ping {} · tools/list {} · {} tools{}",
                        h.server,
                        h.state.label(),
                        ms(h.start_ms),
                        ms(h.ping_ms),
                        ms(h.list_ms),
                        h.tools,
                        h.protocol
                            .as_deref()
                            .map(|p| format!(" · {p}"))
                            .unwrap_or_default()
                    );
                    if let Some(e) = &h.error {
                        println!("    {e}");
                    }
                }
                all.push(h);
            }
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?
                );
            }
            mcp::hub().stop_all().await;
            Ok(())
        }
        McpCmd::Rm { name } => {
            let mut cfg = McpConfig::load()?;
            cfg.get(&name)?;
            cfg.servers.remove(&name);
            cfg.save()?;
            println!("Removed “{name}”.");
            Ok(())
        }
    }
}

/// `tern acp`: serve the Agent Client Protocol on stdin and stdout.
pub(crate) async fn acp(chat: ChatArgs) -> Result<(), String> {
    let (opts, _) = options(chat);
    let r = tern_agent::acp::serve_stdio(tern_agent::acp::AcpOptions {
        chat: opts,
        db: None,
    })
    .await;
    mcp::hub().stop_all().await;
    r
}

#[derive(Debug, Subcommand)]
pub(crate) enum SkillsCmd {
    /// Every skill: yours and the project's, with on or off.
    List {
        /// The project folder whose `.tern/skills` to include (default: here).
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Install skills from a folder or a git URL, after a preview.
    Install {
        from: String,
        /// Replace skills with the same name.
        #[arg(long)]
        replace: bool,
        /// Install without asking.
        #[arg(long, short)]
        yes: bool,
    },
    Enable {
        name: String,
    },
    Disable {
        name: String,
    },
    /// Print a skill's full instructions.
    Show {
        name: String,
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Remove one of your skills.
    Rm {
        name: String,
    },
}

fn here() -> Option<PathBuf> {
    std::env::current_dir().ok()
}

pub(crate) fn skills_cmd(cmd: SkillsCmd) -> Result<(), String> {
    let data = data_dir()?;
    match cmd {
        SkillsCmd::List { root, json } => {
            let set = skills::discover(&data, root.or_else(here).as_deref());
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&set).map_err(|e| e.to_string())?
                );
                return Ok(());
            }
            if set.skills.is_empty() {
                println!(
                    "No skills yet. Put a folder with a SKILL.md in {}, or `tern skills install <folder or git URL>`.",
                    skills::user_dir(&data).display()
                );
            }
            for s in &set.skills {
                println!(
                    "{} {:<28} {:<8} {}",
                    if s.enabled { "●" } else { "○" },
                    s.name,
                    s.source.label(),
                    s.description
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
            for p in &set.problems {
                dim(p);
            }
            Ok(())
        }
        SkillsCmd::Install { from, replace, yes } => {
            let temp;
            let src: PathBuf = if skills::is_git_url(&from) {
                dim(&format!("Cloning {from}…"));
                temp = skills::fetch_git(&from)?;
                temp.path().to_path_buf()
            } else {
                PathBuf::from(&from)
            };
            let previews = skills::preview_folder(&src)?;
            for p in &previews {
                println!("{} — {}", p.name, p.description);
                if !p.files.is_empty() {
                    println!("  files: {}", p.files.join(", "));
                }
                for f in &p.flags {
                    println!(
                        "  {} {}{}",
                        if f.severity == Severity::Danger {
                            "⚠"
                        } else {
                            "·"
                        },
                        f.reason,
                        if f.excerpt.is_empty() {
                            String::new()
                        } else {
                            format!(": “{}”", f.excerpt)
                        }
                    );
                }
            }
            if !yes && !confirm("Install?") {
                println!("Nothing installed.");
                return Ok(());
            }
            let names = skills::install_folder(&data, &src, replace)?;
            println!("Installed {}.", names.join(", "));
            Ok(())
        }
        SkillsCmd::Enable { name } => {
            skills::set_enabled(&data, &name, true)?;
            println!("“{name}” is on.");
            Ok(())
        }
        SkillsCmd::Disable { name } => {
            skills::set_enabled(&data, &name, false)?;
            println!("“{name}” is off.");
            Ok(())
        }
        SkillsCmd::Show { name, root } => {
            let set = skills::discover(&data, root.or_else(here).as_deref());
            let s = set
                .get(&name)
                .ok_or_else(|| format!("No skill called “{name}”. See `tern skills list`."))?;
            println!(
                "# {} ({}, {})\n\n{}\n",
                s.name,
                s.source.label(),
                if s.enabled { "on" } else { "off" },
                s.description
            );
            println!("{}", s.body()?);
            let files = s.files();
            if !files.is_empty() {
                println!("\nFiles: {}", files.join(", "));
            }
            Ok(())
        }
        SkillsCmd::Rm { name } => {
            skills::remove(&data, &name)?;
            println!("Removed “{name}”.");
            Ok(())
        }
    }
}

/// `tern instructions`: exactly what the model is told.
pub(crate) async fn instructions(
    project: Option<String>,
    root: Option<PathBuf>,
    no_mcp: bool,
    json: bool,
    raw: bool,
    chat: ChatArgs,
) -> Result<(), String> {
    let (mut opts, _) = options(chat);
    opts.no_mcp = no_mcp;
    opts.project = project.clone();
    opts.project_root = match (root, &project) {
        (Some(r), _) => Some(r),
        (None, Some(_)) => None,
        (None, None) => here(),
    };
    let e = tern_agent::chat::preview_instructions(&opts).await?;
    mcp::hub().stop_all().await;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&e).map_err(|e| e.to_string())?
        );
    } else if raw {
        println!("{}", e.prompt);
    } else {
        println!("{}", e.to_markdown());
    }
    Ok(())
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProjectCmd {
    /// Make a project folder, optionally linked to a folder on disk
    /// (its TERN.md and .tern/skills then apply to the project's chats).
    New {
        name: String,
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Every project and how many chats it holds.
    List {
        #[arg(long)]
        json: bool,
    },
    /// File a chat (and its sub-sessions) under a project.
    Mv {
        /// The chat: `last`, its id, or the start of its id.
        chat: String,
        /// The project's name or id (`Inbox` to unfile it).
        project: String,
    },
    Rename {
        project: String,
        name: String,
    },
    /// Link a folder to a project, or unlink it with --unlink.
    Root {
        project: String,
        dir: Option<PathBuf>,
        #[arg(long)]
        unlink: bool,
    },
    /// Delete a project; its chats move to the Inbox.
    Rm {
        project: String,
        #[arg(long, short)]
        yes: bool,
    },
}

pub(crate) fn project(cmd: ProjectCmd) -> Result<(), String> {
    let mut store = open_store()?;
    match cmd {
        ProjectCmd::New { name, root } => {
            let id = projects::create(&store, &name, root.as_deref())?;
            println!("Made project “{}” ({id}).", name.trim());
            Ok(())
        }
        ProjectCmd::List { json } => {
            let all = projects::list(&store)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?
                );
                return Ok(());
            }
            for p in all {
                println!(
                    "{:<28} {:>4} chat{}  {}{}",
                    p.row.name,
                    p.row.sessions,
                    if p.row.sessions == 1 { " " } else { "s" },
                    p.row
                        .root
                        .as_ref()
                        .map(|r| r.display().to_string())
                        .unwrap_or_default(),
                    if p.tern_md.is_some() {
                        "  · TERN.md"
                    } else {
                        ""
                    }
                );
            }
            Ok(())
        }
        ProjectCmd::Mv { chat, project } => {
            let session = if chat.trim().eq_ignore_ascii_case("last") {
                store
                    .recent_sessions(500)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|s| s.parent.is_none())
                    .map(|s| s.id)
                    .ok_or("There are no chats yet.")?
            } else {
                tern_agent::report::find_session(&store, &chat)?
            };
            let to = projects::find(&store, &project)?;
            let n = projects::move_session(&mut store, session, to.id)?;
            println!(
                "Moved {n} session{} to “{}”.",
                if n == 1 { "" } else { "s" },
                to.name
            );
            Ok(())
        }
        ProjectCmd::Rename { project, name } => {
            let p = projects::find(&store, &project)?;
            projects::rename(&store, p.id, &name)?;
            println!("Renamed “{}” to “{}”.", p.name, name.trim());
            Ok(())
        }
        ProjectCmd::Root {
            project,
            dir,
            unlink,
        } => {
            let p = projects::find(&store, &project)?;
            match (dir, unlink) {
                (_, true) => {
                    projects::set_root(&store, p.id, None)?;
                    println!("“{}” has no folder now.", p.name);
                }
                (Some(d), false) => {
                    projects::set_root(&store, p.id, Some(&d))?;
                    println!("“{}” works in {}.", p.name, d.display());
                }
                (None, false) => match &p.root {
                    Some(r) => println!("{}", r.display()),
                    None => println!(
                        "“{}” has no folder. Link one: tern project root \"{}\" <dir>",
                        p.name, p.name
                    ),
                },
            }
            Ok(())
        }
        ProjectCmd::Rm { project, yes } => {
            let p = projects::find(&store, &project)?;
            if !yes
                && !confirm(&format!(
                    "Delete “{}”? Its {} chat{} move to the Inbox.",
                    p.name,
                    p.sessions,
                    if p.sessions == 1 { "" } else { "s" }
                ))
            {
                println!("Nothing deleted.");
                return Ok(());
            }
            let n = projects::delete(&mut store, p.id)?;
            println!(
                "Deleted “{}”; {n} session{} moved to the Inbox.",
                p.name,
                if n == 1 { "" } else { "s" }
            );
            Ok(())
        }
    }
}
