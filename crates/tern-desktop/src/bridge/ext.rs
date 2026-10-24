//! The bridge's extensibility jobs: MCP servers (config, trust previews,
//! health checks), skills, the effective instructions, and project
//! folders. Everything that touches disk, the network, or a server process
//! runs here, off the UI thread.

use std::path::PathBuf;

use tern_agent::mcp::{self, Health, McpConfig, ServerConfig, ToolSetting, TrustPreview};
use tern_agent::projects::{self, Project};
use tern_agent::rules::Effective;
use tern_agent::skills::{self, SkillPreview, SkillSet};
use tern_agent::{Preferences, SessionSummary};
use tern_core::{ProjectId, SessionId};

use super::{Bridge, Msg, RECENT};

/// What the extensibility jobs tell the window.
pub enum ExtMsg {
    /// The MCP config and the skills, as saved, and a line for the user.
    Loaded(Result<(McpConfig, SkillSet), String>, Option<String>),
    Health(Health),
    Preview(Box<TrustPreview>),
    SkillPreview(Result<(PathBuf, Vec<SkillPreview>), String>),
    Instructions(Result<Effective, String>),
    /// The project list, the chat list, and a line for the user.
    Projects(
        Result<Vec<Project>, String>,
        Vec<SessionSummary>,
        Option<String>,
    ),
    /// The prompts and resources the reviewed servers offer.
    Menu(mcp::Menu),
    /// A prompt filled in by its server, for the composer.
    Prompt(Box<Result<mcp::Inserted, String>>),
    /// A resource read by its server, as an attachment.
    Resource(Box<Result<tern_agent::Attachment, String>>),
}

impl std::fmt::Debug for ExtMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loaded(r, _) => write!(f, "Loaded(ok: {})", r.is_ok()),
            Self::Health(h) => write!(f, "Health({}, {:?})", h.server, h.state),
            Self::Preview(p) => write!(f, "Preview({}, {} tools)", p.server, p.tools.len()),
            Self::SkillPreview(r) => write!(f, "SkillPreview(ok: {})", r.is_ok()),
            Self::Instructions(r) => write!(f, "Instructions(ok: {})", r.is_ok()),
            Self::Projects(r, s, _) => {
                write!(f, "Projects(ok: {}, {} chats)", r.is_ok(), s.len())
            }
            Self::Menu(m) => write!(f, "Menu({} items)", m.items.len()),
            Self::Prompt(r) => write!(f, "Prompt(ok: {})", r.is_ok()),
            Self::Resource(r) => write!(f, "Resource(ok: {})", r.is_ok()),
        }
    }
}

fn data() -> Result<PathBuf, String> {
    mcp::config::data_dir()
}

fn load_all() -> Result<(McpConfig, SkillSet), String> {
    Ok((McpConfig::load()?, skills::discover(&data()?, None)))
}

/// A change to one MCP server.
#[derive(Clone, Debug)]
pub enum McpChange {
    Add(String, Box<ServerConfig>),
    /// Switch the server on or off.
    Switch(String, bool),
    /// One tool's switch and approval.
    Tool(String, String, ToolSetting),
    /// Trust the reviewed tools and switch the server on.
    Trust(String, Box<TrustPreview>),
    Remove(String),
}

/// A change to the project folders.
#[derive(Clone, Debug)]
pub enum ProjectChange {
    New(String),
    Rename(ProjectId, String),
    Root(ProjectId, Option<PathBuf>),
    Delete(ProjectId),
    Move(SessionId, ProjectId),
    /// Only reload.
    Reload,
}

impl Bridge {
    /// Loads the MCP config and the skills.
    pub fn ext_load(&self) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Loaded(load_all(), None))));
        });
    }

    /// Saves a change to the MCP config, then sends it again.
    pub fn mcp_change(&self, change: McpChange) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let stop = match &change {
                McpChange::Switch(n, false) | McpChange::Remove(n) => Some(n.clone()),
                McpChange::Tool(n, ..) => Some(n.clone()),
                _ => None,
            };
            let done = tokio::task::spawn_blocking(move || -> Result<String, String> {
                let mut cfg = McpConfig::load()?;
                let note = match change {
                    McpChange::Add(name, s) => {
                        cfg.add(&name, *s)?;
                        format!("Added “{name}”. Review it before it can run.")
                    }
                    McpChange::Switch(name, on) => {
                        let s = cfg.get_mut(&name)?;
                        if on && s.tern.trusted.is_none() {
                            return Err(format!("Review “{name}” before switching it on."));
                        }
                        s.tern.enabled = on;
                        s.disabled = false;
                        format!(
                            "“{name}” is {} for new chats.",
                            if on { "on" } else { "off" }
                        )
                    }
                    McpChange::Tool(name, tool, set) => {
                        cfg.get_mut(&name)?.tern.tools.insert(tool.clone(), set);
                        format!("Saved {name} · {tool}. New chats use it.")
                    }
                    McpChange::Trust(name, p) => {
                        mcp::trust::trust(&mut cfg, &name, &p)?;
                        format!("Trusted “{name}”. Its tools ask before each call.")
                    }
                    McpChange::Remove(name) => {
                        cfg.get(&name)?;
                        cfg.servers.remove(&name);
                        format!("Removed “{name}”.")
                    }
                };
                cfg.save()?;
                Ok(note)
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
            if let Some(n) = stop {
                mcp::hub().stop(&n).await;
            }
            let (loaded, note) = match done {
                Ok(note) => (load_all(), Some(note)),
                Err(e) => (load_all(), Some(format!("! {e}"))),
            };
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Loaded(loaded, note))));
        });
    }

    /// Starts the server, reads its tools, and sends the trust preview.
    pub fn mcp_preview(&self, name: String) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let server = match McpConfig::load().and_then(|c| c.get(&name).cloned()) {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Loaded(
                        load_all(),
                        Some(format!("! {e}")),
                    ))));
                    return;
                }
            };
            let p = mcp::trust::preview(&name, &server).await;
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Preview(Box::new(p)))));
        });
    }

    /// Checks a server's health (restarting it first when asked).
    pub fn mcp_health(&self, name: String, restart: bool) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let h = match McpConfig::load().and_then(|c| c.get(&name).cloned()) {
                Ok(s) => mcp::check(&name, &s, restart).await,
                Err(e) => Health {
                    server: name,
                    state: mcp::HealthState::Down,
                    start_ms: None,
                    ping_ms: None,
                    list_ms: None,
                    tools: 0,
                    prompts: 0,
                    resources: 0,
                    protocol: None,
                    error: Some(e),
                },
            };
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Health(h))));
        });
    }

    /// Reads the prompts and resources the reviewed servers offer.
    pub fn mcp_menu(&self) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let menu = match McpConfig::load() {
                Ok(cfg) => mcp::catalog::menu(&cfg, None).await,
                Err(e) => mcp::Menu {
                    items: Vec::new(),
                    notes: vec![e],
                },
            };
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Menu(menu))));
        });
    }

    /// Asks a server to fill in a prompt. Its text comes back for the
    /// composer; nothing is sent.
    pub fn mcp_prompt(
        &self,
        server: String,
        name: String,
        args: std::collections::BTreeMap<String, String>,
    ) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let r = match McpConfig::load() {
                Ok(cfg) => mcp::catalog::prompt(&cfg, &server, &name, &args).await,
                Err(e) => Err(e),
            };
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Prompt(Box::new(r)))));
        });
    }

    /// Reads a resource as a Context attachment (untrusted data).
    pub fn mcp_resource(&self, server: String, uri: String) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let r = match McpConfig::load() {
                Ok(cfg) => mcp::catalog::resource(&cfg, &server, &uri).await,
                Err(e) => Err(e),
            };
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Resource(Box::new(r)))));
        });
    }

    /// Reads what a folder would install.
    pub fn skills_preview(&self, from: PathBuf) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = skills::preview_folder(&from).map(|p| (from, p));
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::SkillPreview(r))));
        });
    }

    /// Installs the previewed folder, switches a skill, or removes one.
    pub fn skills_change(&self, change: SkillChange) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = data().and_then(|d| match change {
                SkillChange::Install(from) => skills::install_folder(&d, &from, false)
                    .map(|n| format!("Installed {}.", n.join(", "))),
                SkillChange::Switch(name, on) => skills::set_enabled(&d, &name, on)
                    .map(|_| format!("“{name}” is {}.", if on { "on" } else { "off" })),
                SkillChange::Remove(name) => {
                    skills::remove(&d, &name).map(|_| format!("Removed “{name}”."))
                }
                SkillChange::Create(name, description, body) => {
                    tern_agent::skill_author::create(&d, &name, &description, &body, false)
                        .map(|_| format!("Saved skill “{}”.", name.trim()))
                }
            });
            let note = match r {
                Ok(n) => n,
                Err(e) => format!("! {e}"),
            };
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Loaded(load_all(), Some(note)))));
        });
    }

    /// Downloads a skill folder from GitHub and installs it.
    pub fn skills_import_github(&self, r: tern_agent::skill_author::GithubRef) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let note = match data() {
                Err(e) => format!("! {e}"),
                Ok(d) => match tern_agent::skill_author::import_github(&d, &r, false).await {
                    Ok(names) => format!("Installed {} from {}.", names.join(", "), r.label()),
                    Err(e) => format!("! {e}"),
                },
            };
            let all = tokio::task::spawn_blocking(load_all)
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Loaded(all, Some(note)))));
        });
    }

    /// The effective instructions for a new chat with the saved choices
    /// (when no chat is open to ask).
    pub fn instructions(&self, project: Option<String>) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let prefs = tokio::task::spawn_blocking(Preferences::load)
                .await
                .unwrap_or_default();
            let mut opts = prefs.chat_options();
            opts.project = project;
            let r = tern_agent::chat::preview_instructions(&opts).await;
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Instructions(r))));
        });
    }

    /// Changes the project folders, then sends them and the chat list.
    pub fn projects(&self, change: ProjectChange) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let mut note = None;
            let r = tern_agent::open_store().and_then(|mut store| {
                let done = match change {
                    ProjectChange::New(name) => projects::create(&store, &name, None)
                        .map(|_| Some(format!("Made project “{}”.", name.trim()))),
                    ProjectChange::Rename(id, name) => {
                        projects::rename(&store, id, &name).map(|_| None)
                    }
                    ProjectChange::Root(id, root) => {
                        projects::set_root(&store, id, root.as_deref()).map(|_| {
                            Some(match root {
                                Some(r) => format!("Linked {}; its TERN.md and .tern/skills apply to the project's new chats.", r.display()),
                                None => "Unlinked the folder.".into(),
                            })
                        })
                    }
                    ProjectChange::Delete(id) => projects::delete(&mut store, id).map(|n| {
                        Some(format!(
                            "Deleted the project; {n} session{} moved to the Inbox.",
                            if n == 1 { "" } else { "s" }
                        ))
                    }),
                    ProjectChange::Move(s, to) => projects::move_session(&mut store, s, to)
                        .map(|_| None),
                    ProjectChange::Reload => Ok(None),
                };
                match done {
                    Ok(n) => note = n,
                    Err(e) => note = Some(format!("! {e}")),
                }
                projects::list(&store)
            });
            let recent = tern_agent::recent_sessions(RECENT).unwrap_or_default();
            let _ = tx.send(Msg::Ext(Box::new(ExtMsg::Projects(r, recent, note))));
        });
    }

    /// Opens a new chat filed under `project` (by name).
    pub fn connect_in(&self, project: Option<String>) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let prefs = tokio::task::spawn_blocking(Preferences::load)
                .await
                .unwrap_or_default();
            let mut opts = prefs.chat_options();
            opts.project = project;
            match tern_agent::ChatSession::open(opts).await {
                Ok(s) => {
                    let recent = s.recent_sessions(RECENT).unwrap_or_default();
                    let _ = tx.send(Msg::Connected(Box::new(s), recent));
                }
                Err(e) => {
                    let _ = tx.send(Msg::ConnectFailed(e));
                }
            }
        });
    }
}

/// A change to the skills.
#[derive(Clone, Debug)]
pub enum SkillChange {
    Install(PathBuf),
    Switch(String, bool),
    Remove(String),
    /// A new skill from the editor: name, description, instructions.
    Create(String, String, String),
}
