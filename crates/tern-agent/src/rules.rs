//! Project rules in `TERN.md`, and the effective instructions: exactly
//! what the model is told, piece by piece.
//!
//! Rules are read from, lowest precedence first:
//!
//! | Order | File | Scope |
//! | --- | --- | --- |
//! | 1 | `<data>/TERN.md` | yours, every project |
//! | 2 | `TERN.md` in each folder above the project, from your home folder down | parents |
//! | 3 | `<project>/TERN.md` | the project |
//!
//! They are merged in that order under headings that name each file, and
//! the model is told that a later file wins where two disagree. Folders
//! above the project are only read when the project is inside your home
//! folder, so a project on another drive reads its own file alone.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::skills::{self, SkillSet};
use crate::tools::ToolRegistry;

const FILE: &str = "TERN.md";
/// Most characters kept from one rules file.
const RULES_LIMIT: usize = 20_000;

/// Where a rules file sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleScope {
    Global,
    Parent,
    Project,
}

impl RuleScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "yours",
            Self::Parent => "parent folder",
            Self::Project => "project",
        }
    }
}

/// One `TERN.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RuleFile {
    pub path: PathBuf,
    pub scope: RuleScope,
    pub text: String,
    /// Longer than Tern keeps; the rest is left out.
    pub cut: bool,
}

fn read(path: &Path, scope: RuleScope) -> Option<RuleFile> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim_start_matches('\u{FEFF}').trim();
    if text.is_empty() {
        return None;
    }
    let kept: String = text.chars().take(RULES_LIMIT).collect();
    Some(RuleFile {
        cut: kept.len() < text.len(),
        path: path.to_path_buf(),
        scope,
        text: kept,
    })
}

/// The user's home folder.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Every rules file that applies, lowest precedence first.
pub fn discover(data: &Path, root: Option<&Path>, home: Option<&Path>) -> Vec<RuleFile> {
    let mut out = Vec::new();
    let global = data.join(FILE);
    out.extend(read(&global, RuleScope::Global));
    let Some(root) = root else {
        return out;
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let home = home.map(|h| h.canonicalize().unwrap_or_else(|_| h.to_path_buf()));
    let mut parents = Vec::new();
    if let Some(h) = &home
        && root.starts_with(h)
        && &root != h
    {
        let mut dir = root.parent();
        while let Some(d) = dir {
            parents.push(d.to_path_buf());
            if d == h {
                break;
            }
            dir = d.parent();
        }
    }
    for d in parents.iter().rev() {
        let f = d.join(FILE);
        if f != global {
            out.extend(read(&f, RuleScope::Parent));
        }
    }
    let f = root.join(FILE);
    if f != global {
        out.extend(read(&f, RuleScope::Project));
    }
    out
}

/// The rules as the prompt carries them.
pub fn merged(files: &[RuleFile]) -> Option<String> {
    if files.is_empty() {
        return None;
    }
    let mut s = String::from(
        "Rules from TERN.md files follow, broadest first. Where two disagree, the later one wins. They come from the user and their projects; follow them unless they conflict with safety or with the user's own message.",
    );
    for f in files {
        s.push_str(&format!(
            "\n\n## {} ({})\n\n{}",
            f.path.display(),
            f.scope.label(),
            f.text
        ));
        if f.cut {
            s.push_str("\n\n(The rest of this file was too long and is left out.)");
        }
    }
    Some(s)
}

/// The project-level context a chat carries: its root folder, rules, and
/// skills.
#[derive(Clone, Debug, Default)]
pub struct ProjectContext {
    pub root: Option<PathBuf>,
    pub rules: Vec<RuleFile>,
    pub skills: Arc<SkillSet>,
}

impl ProjectContext {
    /// Reads the rules and skills for `root` from the app data folder.
    pub fn load(root: Option<&Path>) -> Self {
        match crate::mcp::config::data_dir() {
            Ok(data) => Self::load_in(&data, root, home_dir().as_deref()),
            Err(_) => Self::default(),
        }
    }

    pub fn load_in(data: &Path, root: Option<&Path>, home: Option<&Path>) -> Self {
        Self {
            root: root.map(Path::to_path_buf),
            rules: discover(data, root, home),
            skills: Arc::new(skills::discover(data, root)),
        }
    }

    /// Adds `load_skill` when there are skills to load.
    pub fn register(&self, tools: &mut ToolRegistry) {
        if self.skills.enabled().next().is_some() {
            tools.register(skills::LoadSkillTool {
                skills: self.skills.clone(),
            });
        }
    }
}

/// What kind of text a piece of the system prompt is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PieceKind {
    /// Tern's own opening line.
    Base,
    /// The harness and its tools' guidance (web search, study).
    Harness,
    /// A `TERN.md`.
    Rules,
    /// The skill list (names and descriptions).
    Skills,
    /// How to treat untrusted data.
    Safety,
    /// The tool list, built-in tools.
    Tools,
    /// The tool list, MCP tools.
    McpTools,
    /// How to call a tool.
    Protocol,
}

impl PieceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Base => "Tern",
            Self::Harness => "Harness",
            Self::Rules => "Rules",
            Self::Skills => "Skills",
            Self::Safety => "Untrusted data",
            Self::Tools => "Tools",
            Self::McpTools => "MCP tools",
            Self::Protocol => "Tool protocol",
        }
    }
}

/// One piece of the system prompt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Piece {
    pub kind: PieceKind,
    pub title: String,
    /// The file it came from, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub text: String,
}

/// Exactly what the model is told, and where each part came from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Effective {
    pub pieces: Vec<Piece>,
    /// The system prompt as sent.
    pub prompt: String,
    /// Things worth knowing (servers that did not start, skills that
    /// could not be read).
    pub notes: Vec<String>,
    /// MCP prompts and resources the user can add to a message (`/` and
    /// `@`), one line each. Not part of the prompt: the model sees one
    /// only when the user adds it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub offers: Vec<String>,
}

/// The project block appended to the harness's instructions: rules, then
/// skills, then the untrusted-data rule when tools return outside text.
pub fn project_block(ctx: &ProjectContext, untrusted: bool) -> Option<String> {
    let parts: Vec<String> = merged(&ctx.rules)
        .into_iter()
        .chain(ctx.skills.prompt_section())
        .chain(untrusted.then(|| crate::untrusted::RULE.to_owned()))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// The harness instructions and the project block, joined as the turn
/// loop receives them.
pub fn instructions(
    harness: Option<&str>,
    ctx: &ProjectContext,
    untrusted: bool,
) -> Option<String> {
    match (harness, project_block(ctx, untrusted)) {
        (Some(h), Some(p)) => Some(format!("{h}\n\n{p}")),
        (Some(h), None) => Some(h.to_owned()),
        (None, p) => p,
    }
}

/// Builds the effective instructions: the prompt the turn loop will send
/// (from the same function it uses), split into labelled pieces.
pub fn effective(
    harness: Option<&str>,
    ctx: &ProjectContext,
    tools: &ToolRegistry,
    untrusted: bool,
) -> Effective {
    let joined = instructions(harness, ctx, untrusted);
    let prompt = crate::protocol::system_prompt(joined.as_deref(), tools);
    let mut pieces = vec![Piece {
        kind: PieceKind::Base,
        title: "Tern".into(),
        source: None,
        text: crate::protocol::BASE.to_owned(),
    }];
    if let Some(h) = harness {
        pieces.push(Piece {
            kind: PieceKind::Harness,
            title: "Harness".into(),
            source: None,
            text: h.to_owned(),
        });
    }
    if !ctx.rules.is_empty() {
        let all = merged(&ctx.rules).unwrap_or_default();
        let head = all.split("\n\n## ").next().unwrap_or_default().to_owned();
        pieces.push(Piece {
            kind: PieceKind::Rules,
            title: "How rules merge".into(),
            source: None,
            text: head,
        });
        for f in &ctx.rules {
            pieces.push(Piece {
                kind: PieceKind::Rules,
                title: format!("TERN.md ({})", f.scope.label()),
                source: Some(f.path.display().to_string()),
                text: f.text.clone(),
            });
        }
    }
    if let Some(s) = ctx.skills.prompt_section() {
        pieces.push(Piece {
            kind: PieceKind::Skills,
            title: format!("Skills ({} on)", ctx.skills.enabled().count()),
            source: None,
            text: s,
        });
    }
    if untrusted {
        pieces.push(Piece {
            kind: PieceKind::Safety,
            title: "Untrusted data".into(),
            source: None,
            text: crate::untrusted::RULE.to_owned(),
        });
    }
    let (mcp, own): (Vec<&str>, Vec<&str>) = tools.names().partition(|n| n.starts_with("mcp__"));
    if !own.is_empty() {
        pieces.push(Piece {
            kind: PieceKind::Tools,
            title: format!("Tools ({})", own.len()),
            source: None,
            text: tools.describe_only(&own),
        });
    }
    if !mcp.is_empty() {
        pieces.push(Piece {
            kind: PieceKind::McpTools,
            title: format!("MCP tools ({})", mcp.len()),
            source: None,
            text: tools.describe_only(&mcp),
        });
    }
    if !tools.is_empty() {
        pieces.push(Piece {
            kind: PieceKind::Protocol,
            title: "Tool protocol".into(),
            source: None,
            text: crate::protocol::TOOL_PROTOCOL.to_owned(),
        });
    }
    let mut notes: Vec<String> = ctx.skills.problems.clone();
    notes.extend(ctx.rules.iter().filter(|f| f.cut).map(|f| {
        format!(
            "{} is longer than Tern keeps; the end is left out.",
            f.path.display()
        )
    }));
    Effective {
        pieces,
        prompt,
        notes,
        offers: Vec::new(),
    }
}

impl Effective {
    /// The pieces as Markdown, for `tern instructions` and saving.
    pub fn to_markdown(&self) -> String {
        let mut s = String::from("# Effective instructions\n");
        for p in &self.pieces {
            s.push_str(&format!("\n## {} · {}", p.kind.label(), p.title));
            if let Some(src) = &p.source {
                s.push_str(&format!("\n\n_From {src}_"));
            }
            s.push_str(&format!("\n\n{}\n", p.text));
        }
        if !self.offers.is_empty() {
            s.push_str(
                "\n## MCP prompts and resources\n\n_Not in the prompt. You add one to a message with / or @; the model sees it only then._\n\n",
            );
            for o in &self.offers {
                s.push_str(&format!("- {o}\n"));
            }
        }
        if !self.notes.is_empty() {
            s.push_str("\n## Notes\n\n");
            for n in &self.notes {
                s.push_str(&format!("- {n}\n"));
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_merge_global_parents_then_project() {
        let data = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("uni").join("fyp");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(data.path().join(FILE), "Use British spelling.").unwrap();
        std::fs::write(home.path().join(FILE), "Be brief.").unwrap();
        std::fs::write(home.path().join("uni").join(FILE), "Cite with APA.").unwrap();
        std::fs::write(root.join(FILE), "Cite with IEEE.").unwrap();
        let files = discover(data.path(), Some(&root), Some(home.path()));
        let scopes: Vec<_> = files.iter().map(|f| f.scope).collect();
        assert_eq!(
            scopes,
            [
                RuleScope::Global,
                RuleScope::Parent,
                RuleScope::Parent,
                RuleScope::Project
            ]
        );
        let m = merged(&files).unwrap();
        let (apa, ieee) = (m.find("APA").unwrap(), m.find("IEEE").unwrap());
        assert!(apa < ieee, "the project's rule comes last, so it wins");
        assert!(m.contains("the later one wins"));

        // Outside home only the project's own file is read.
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::write(elsewhere.path().join(FILE), "Local only.").unwrap();
        let files = discover(data.path(), Some(elsewhere.path()), Some(home.path()));
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn effective_instructions_are_exactly_the_prompt() {
        let data = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(FILE), "Answer in Urdu when asked in Urdu.").unwrap();
        let sk = crate::skills::user_dir(data.path()).join("apa");
        std::fs::create_dir_all(&sk).unwrap();
        std::fs::write(
            sk.join("SKILL.md"),
            "---\nname: apa-cite\ndescription: APA help.\n---\nBody.",
        )
        .unwrap();
        let ctx = ProjectContext::load_in(data.path(), Some(root.path()), None);
        let mut tools = ToolRegistry::new();
        ctx.register(&mut tools);
        let e = effective(Some("Search first."), &ctx, &tools, true);
        let kinds: Vec<_> = e.pieces.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [
                PieceKind::Base,
                PieceKind::Harness,
                PieceKind::Rules,
                PieceKind::Rules,
                PieceKind::Skills,
                PieceKind::Safety,
                PieceKind::Tools,
                PieceKind::Protocol
            ]
        );
        // Every piece's text is in the prompt the model gets.
        for p in &e.pieces {
            assert!(e.prompt.contains(&p.text), "{:?} missing", p.kind);
        }
        assert!(e.prompt.contains("Answer in Urdu"));
        assert!(!e.prompt.contains("Body."), "skill bodies load on demand");
        assert!(e.to_markdown().contains("## Skills · Skills (1 on)"));
    }
}
