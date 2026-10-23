//! Customize and project-folder state for the window, kept apart from
//! GPUI so it can be tested without a display: the Customize page's tabs
//! (Skills, MCP servers, Providers, Instructions), MCP health and trust
//! previews, skill installs, the effective instructions, and the sidebar's
//! project tree with its collapse state.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use tern_agent::mcp::{Health, HealthState, McpConfig, TrustPreview};
use tern_agent::projects::Project;
use tern_agent::rules::Effective;
use tern_agent::skills::{SkillPreview, SkillSet};
use tern_agent::{ProjectSummary, SessionSummary};
use tern_core::{ProjectId, SessionId};

/// The Customize page's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CustomizeTab {
    #[default]
    Skills,
    Mcp,
    Providers,
    Instructions,
}

impl CustomizeTab {
    pub const ALL: [Self; 4] = [Self::Skills, Self::Mcp, Self::Providers, Self::Instructions];

    pub fn label(self) -> &'static str {
        match self {
            Self::Skills => "Skills",
            Self::Mcp => "MCP servers",
            Self::Providers => "Providers",
            Self::Instructions => "Instructions",
        }
    }
}

/// A line to show at the top of the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    Working(String),
    Ok(String),
    Failed(String),
}

/// The Customize page.
#[derive(Clone, Debug, Default)]
pub struct CustomizeState {
    pub tab: CustomizeTab,
    pub loaded: bool,
    pub mcp: McpConfig,
    /// The last health check per server.
    pub health: HashMap<String, Health>,
    /// Servers being checked now.
    pub checking: BTreeSet<String>,
    /// The trust preview sheet, and the server it is being read for.
    pub preview: Option<TrustPreview>,
    pub previewing: Option<String>,
    /// The last preview per server, for its tool switches.
    pub previews: HashMap<String, TrustPreview>,
    /// The add-server form is open.
    pub adding: bool,
    pub skills: SkillSet,
    /// What installing from a folder would add, and the folder.
    pub skill_preview: Option<(PathBuf, Vec<SkillPreview>)>,
    /// The effective instructions, and whether they are for the open chat.
    pub instructions: Option<Effective>,
    pub instructions_for_chat: bool,
    pub loading_instructions: bool,
    /// Instruction pieces expanded, by index.
    pub open_pieces: BTreeSet<usize>,
    pub note: Option<Note>,
}

impl CustomizeState {
    /// The dot for a server: its last check, or what the config says.
    pub fn state_of(&self, name: &str) -> HealthState {
        let Some(s) = self.mcp.servers.get(name) else {
            return HealthState::Off;
        };
        if s.tern.trusted.is_none() {
            return HealthState::NeedsReview;
        }
        if !s.active() {
            return HealthState::Off;
        }
        self.health
            .get(name)
            .map(|h| h.state)
            .unwrap_or(HealthState::Off)
    }

    /// A health check finished.
    pub fn checked(&mut self, h: Health) {
        self.checking.remove(&h.server);
        self.health.insert(h.server.clone(), h);
    }

    /// Splits a typed command line into the program and its arguments,
    /// keeping quoted parts together.
    pub fn split_command(line: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut quote: Option<char> = None;
        let mut any = false;
        for c in line.chars() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), c) => cur.push(c),
                (None, '"' | '\'') => {
                    quote = Some(c);
                    any = true;
                }
                (None, c) if c.is_whitespace() => {
                    if !cur.is_empty() || any {
                        out.push(std::mem::take(&mut cur));
                        any = false;
                    }
                }
                (None, c) => cur.push(c),
            }
        }
        if !cur.is_empty() || any {
            out.push(cur);
        }
        out
    }
}

/// A glyph for a server row: a known service's logo when the name says
/// which, else the transport's Lucide icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerGlyph {
    Service(&'static str),
    Terminal,
    Globe,
}

/// Service logos Tern ships, matched against a server's name or address.
const KNOWN: [&str; 9] = [
    "github",
    "zotero",
    "arxiv",
    "pubmed",
    "orcid",
    "wikipedia",
    "semanticscholar",
    "googlescholar",
    "brave",
];

pub fn server_glyph(name: &str, target: &str, http: bool) -> ServerGlyph {
    let hay = format!("{name} {target}").to_ascii_lowercase();
    if let Some(k) = KNOWN.iter().find(|k| hay.contains(*k)) {
        return ServerGlyph::Service(k);
    }
    if http {
        ServerGlyph::Globe
    } else {
        ServerGlyph::Terminal
    }
}

/// One row of the sidebar's project tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeRow {
    /// A project folder header.
    Project {
        id: ProjectId,
        name: String,
        inbox: bool,
        collapsed: bool,
        /// Chats in it that the sidebar lists.
        shown: usize,
        has_rules: bool,
    },
    /// A chat or sub-session, by index into the session list.
    Session(usize),
}

/// The sidebar's project folders.
#[derive(Clone, Debug, Default)]
pub struct ProjectTree {
    pub projects: Vec<Project>,
    pub loaded: bool,
    /// Collapsed project ids (saved in preferences).
    pub collapsed: BTreeSet<String>,
    /// The chat whose Move to… menu is open.
    pub move_menu: Option<SessionId>,
    /// The project whose name is being typed (`None` inside means a new
    /// project).
    pub editing: Option<Option<ProjectId>>,
    /// The project whose delete is waiting for a second click.
    pub confirm_delete: Option<ProjectId>,
    /// The project the next new chat goes into.
    pub new_in: Option<String>,
}

impl ProjectTree {
    pub fn inbox(&self) -> Option<&ProjectSummary> {
        self.projects.iter().map(|p| &p.row).find(|p| p.inbox)
    }

    pub fn find(&self, id: ProjectId) -> Option<&Project> {
        self.projects.iter().find(|p| p.row.id == id)
    }

    /// Flips a project's collapse state; returns the new set to save.
    pub fn toggle(&mut self, id: ProjectId) -> Vec<String> {
        let key = id.to_string();
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key);
        }
        self.collapsed.iter().cloned().collect()
    }

    /// Rows for the sidebar: each project (Inbox last) with its chats in
    /// tree order (`order` is [`crate::continuity::tree`] over `sessions`).
    /// Chats with no known project go in the Inbox. Projects with no
    /// listed chats still show, so a new project is a drop target.
    pub fn rows(&self, sessions: &[SessionSummary], order: &[usize]) -> Vec<TreeRow> {
        let inbox = self.inbox().map(|p| p.id);
        let known = |p: &Option<ProjectId>| {
            p.is_some_and(|p| self.projects.iter().any(|x| x.row.id == p && !x.row.inbox))
        };
        let mut out = Vec::new();
        let mut folders: Vec<&Project> = self.projects.iter().filter(|p| !p.row.inbox).collect();
        // The Inbox goes last, and shows even before projects have loaded.
        let inbox_row = self.projects.iter().find(|p| p.row.inbox);
        folders.extend(inbox_row);
        for p in folders {
            let mine: Vec<usize> = order
                .iter()
                .copied()
                .filter(|i| {
                    let s = &sessions[*i];
                    if p.row.inbox {
                        !known(&s.project)
                    } else {
                        s.project == Some(p.row.id)
                    }
                })
                .collect();
            let collapsed = self.collapsed.contains(&p.row.id.to_string());
            out.push(TreeRow::Project {
                id: p.row.id,
                name: p.row.name.clone(),
                inbox: p.row.inbox,
                collapsed,
                shown: mine
                    .iter()
                    .filter(|i| sessions[**i].parent.is_none())
                    .count(),
                has_rules: p.tern_md.is_some(),
            });
            if !collapsed {
                out.extend(mine.into_iter().map(TreeRow::Session));
            }
        }
        if inbox.is_none() && self.projects.is_empty() {
            // Before the list loads: every chat, flat.
            out.extend(order.iter().copied().map(TreeRow::Session));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_agent::Harness;

    fn project(name: &str, inbox: bool) -> Project {
        Project {
            row: ProjectSummary {
                id: ProjectId::new(),
                name: name.into(),
                root: None,
                created_at: String::new(),
                sessions: 0,
                inbox,
            },
            tern_md: None,
        }
    }

    fn session(project: Option<ProjectId>, parent: Option<SessionId>, at: &str) -> SessionSummary {
        SessionSummary {
            id: SessionId::new(),
            title: at.into(),
            harness: Harness::Research,
            created_at: at.into(),
            events: 1,
            parent,
            anchor: parent.map(|_| 1),
            plan: None,
            project,
        }
    }

    #[test]
    fn chats_sit_in_their_project_with_sub_sessions_and_the_inbox_last() {
        let fyp = project("FYP", false);
        let inbox = project("Inbox", true);
        let empty = project("Later", false);
        let a = session(Some(fyp.row.id), None, "3");
        let a1 = session(Some(fyp.row.id), Some(a.id), "4");
        let loose = session(Some(inbox.row.id), None, "2");
        let orphan = session(None, None, "1");
        let sessions = vec![a.clone(), a1, loose, orphan];
        let order = crate::continuity::tree(&sessions);
        let mut t = ProjectTree {
            projects: vec![inbox.clone(), fyp.clone(), empty.clone()],
            ..Default::default()
        };
        let rows = t.rows(&sessions, &order);
        let shape: Vec<String> = rows
            .iter()
            .map(|r| match r {
                TreeRow::Project { name, shown, .. } => format!("[{name} {shown}]"),
                TreeRow::Session(i) => sessions[*i].title.clone(),
            })
            .collect();
        assert_eq!(
            shape,
            ["[FYP 1]", "3", "4", "[Later 0]", "[Inbox 2]", "2", "1"]
        );

        let saved = t.toggle(fyp.row.id);
        assert_eq!(saved, [fyp.row.id.to_string()]);
        let rows = t.rows(&sessions, &order);
        assert!(matches!(
            &rows[0],
            TreeRow::Project {
                collapsed: true,
                ..
            }
        ));
        assert!(matches!(&rows[1], TreeRow::Project { name, .. } if name == "Later"));
        t.toggle(fyp.row.id);
        assert!(t.collapsed.is_empty());
    }

    #[test]
    fn before_projects_load_every_chat_shows() {
        let s = vec![session(None, None, "1")];
        let rows = ProjectTree::default().rows(&s, &[0]);
        assert_eq!(rows, [TreeRow::Session(0)]);
    }

    #[test]
    fn command_lines_split_with_quotes() {
        assert_eq!(
            CustomizeState::split_command(r#"npx -y "@scope/notes mcp" --dir '' x"#),
            ["npx", "-y", "@scope/notes mcp", "--dir", "", "x"]
        );
    }

    #[test]
    fn servers_get_a_logo_or_a_glyph() {
        assert_eq!(
            server_glyph("github", "npx gh", false),
            ServerGlyph::Service("github")
        );
        assert_eq!(
            server_glyph("refs", "uvx zotero-mcp", false),
            ServerGlyph::Service("zotero")
        );
        assert_eq!(
            server_glyph("notes", "node x.js", false),
            ServerGlyph::Terminal
        );
        assert_eq!(
            server_glyph("docs", "https://x.dev/mcp", true),
            ServerGlyph::Globe
        );
    }

    #[test]
    fn dots_follow_review_switch_and_checks() {
        let mut c = CustomizeState::default();
        let s = tern_agent::mcp::ServerConfig {
            command: Some("x".into()),
            ..Default::default()
        };
        c.mcp.add("a", s).unwrap();
        assert_eq!(c.state_of("a"), HealthState::NeedsReview);
        let srv = c.mcp.get_mut("a").unwrap();
        srv.tern.trusted = Some("f".into());
        assert_eq!(c.state_of("a"), HealthState::Off);
        c.mcp.get_mut("a").unwrap().tern.enabled = true;
        c.checking.insert("a".into());
        c.checked(Health {
            server: "a".into(),
            state: HealthState::Slow,
            start_ms: Some(5),
            ping_ms: Some(1500),
            list_ms: Some(3),
            tools: 2,
            prompts: 0,
            resources: 0,
            protocol: None,
            error: None,
        });
        assert!(c.checking.is_empty());
        assert_eq!(c.state_of("a"), HealthState::Slow);
    }
}
