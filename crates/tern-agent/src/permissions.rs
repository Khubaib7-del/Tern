//! Approval modes and permission rules (#75).
//!
//! The approval mode is the broad setting, in the vocabulary Codex uses:
//!
//! | Mode | What runs without asking |
//! | --- | --- |
//! | Read-only | Reads, searches and analysis. Writes and commands are refused |
//! | Ask (default) | Reads, searches, analysis and vault notes; file writes and commands ask |
//! | Workspace write | Also file writes and sandboxed commands inside the chat's folder |
//! | Full | Everything, except what a rule or a tool's own setting refuses |
//!
//! Permission rules are the fine setting: allow, ask or deny by web
//! domain (`url`), by file path glob inside or outside the folder
//! (`path`), or by tool name and its query or command (`tool`). A deny
//! rule beats an ask rule, which beats an allow rule. Rules are stored in
//! `preferences.json`; an approval card's "Always allow this" adds one.
//!
//! Nothing here weakens the web provenance checks in [`crate::policy`]:
//! an address that carries a secret, or a domain only outside content
//! mentioned, still asks (or is refused) in every mode.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tern_core::EffectClass;

use crate::policy::Mode;

/// The broad approval setting (Settings › Permissions, and the composer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Reads and searches only: anything that writes or runs is refused.
    ReadOnly,
    /// Asks before file writes, commands and anything that acts outside.
    #[default]
    Ask,
    /// File writes and sandboxed commands inside the chat's folder run
    /// without asking; the rest asks.
    WorkspaceWrite,
    /// Runs everything without asking, except what rules refuse.
    Full,
}

impl ApprovalMode {
    pub const ALL: [ApprovalMode; 4] =
        [Self::ReadOnly, Self::Ask, Self::WorkspaceWrite, Self::Full];

    pub fn id(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Ask => "ask",
            Self::WorkspaceWrite => "workspace_write",
            Self::Full => "full",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.id() == s.trim().to_ascii_lowercase().replace(['-', ' '], "_"))
    }

    /// The name in Settings and on the composer chip.
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read-only",
            Self::Ask => "Ask",
            Self::WorkspaceWrite => "Workspace write",
            Self::Full => "Full access",
        }
    }

    /// One line under the name.
    pub fn line(self) -> &'static str {
        match self {
            Self::ReadOnly => "Reads and searches only. Writes and commands are refused",
            Self::Ask => "Asks before it writes a file or runs a command",
            Self::WorkspaceWrite => {
                "Writes files and runs sandboxed commands in the chat's folder without asking"
            }
            Self::Full => "Runs everything without asking, except what your rules refuse",
        }
    }
}

/// What a rule matches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// A web domain: `arxiv.org` matches it and its subdomains; `*.edu`
    /// matches any domain ending in `.edu`.
    Url,
    /// A file path glob. `./` starts it at the chat's folder; `~/` at the
    /// home folder; `*` stays inside one folder, `**` crosses folders.
    Path,
    /// A tool name, then optionally its query or command:
    /// `web_search`, `mcp__github__*`, `run_command pandoc *`.
    Tool,
}

impl RuleKind {
    pub const ALL: [RuleKind; 3] = [Self::Url, Self::Path, Self::Tool];

    pub fn label(self) -> &'static str {
        match self {
            Self::Url => "Web domain",
            Self::Path => "Path",
            Self::Tool => "Tool or command",
        }
    }
}

/// One permission rule.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PermissionRule {
    pub kind: RuleKind,
    pub pattern: String,
    pub action: Mode,
}

impl PermissionRule {
    pub fn new(kind: RuleKind, pattern: impl Into<String>, action: Mode) -> Self {
        Self {
            kind,
            pattern: pattern.into(),
            action,
        }
    }

    /// The rule as one line: "Allow · Web domain · arxiv.org".
    pub fn line(&self) -> String {
        format!(
            "{} · {} · {}",
            match self.action {
                Mode::Allow => "Allow",
                Mode::Ask => "Ask",
                Mode::Deny => "Deny",
            },
            self.kind.label(),
            self.pattern
        )
    }

    /// Whether this rule covers `call`.
    pub fn matches(&self, call: &CallFacts<'_>) -> bool {
        let pat = self.pattern.trim();
        if pat.is_empty() {
            return false;
        }
        match self.kind {
            RuleKind::Url => call.host.is_some_and(|h| domain_matches(pat, h)),
            RuleKind::Path => call
                .paths
                .iter()
                .any(|p| path_matches(pat, p, call.folder, call.home.as_deref())),
            RuleKind::Tool => {
                let (name, rest) = match pat.split_once(char::is_whitespace) {
                    Some((n, r)) => (n, Some(r.trim())),
                    None => (pat, None),
                };
                glob(name, call.tool, false)
                    && rest.is_none_or(|r| glob(r, call.detail.trim(), false))
            }
        }
    }

    /// The rule an approval card's "Always allow this" adds for a call:
    /// the command's program for `run_command`, the domain for a network
    /// call, the folder for a file write, else the tool.
    pub fn suggest(tool: &str, effect: EffectClass, target: Option<&str>, args: &Value) -> Self {
        if tool == "run_command"
            && let Some(prog) = args["command"]
                .as_str()
                .and_then(|c| c.split_whitespace().next())
        {
            return Self::new(RuleKind::Tool, format!("run_command {prog} *"), Mode::Allow);
        }
        if effect == EffectClass::Network
            && let Some(host) = args["url"]
                .as_str()
                .and_then(host_of)
                .or_else(|| target.map(|t| bare_host(t).to_owned()))
                .filter(|h| h.contains('.'))
        {
            return Self::new(RuleKind::Url, host, Mode::Allow);
        }
        if tool == "file_write"
            && let Some(p) = args["path"].as_str()
        {
            let p = p.replace('\\', "/");
            let dir = match p.rsplit_once('/') {
                Some((d, _)) if !d.is_empty() => format!("./{}/**", d.trim_start_matches("./")),
                _ => "./**".to_owned(),
            };
            return Self::new(RuleKind::Path, dir, Mode::Allow);
        }
        Self::new(RuleKind::Tool, tool, Mode::Allow)
    }
}

/// What the rules look at in one call.
#[derive(Clone, Debug, Default)]
pub struct CallFacts<'a> {
    pub tool: &'a str,
    /// The web domain a network call goes to.
    pub host: Option<&'a str>,
    /// Full paths the call reads, writes or runs in.
    pub paths: Vec<PathBuf>,
    /// The call's query or command line, for tool rules.
    pub detail: String,
    /// The chat's folder, for `./` rules.
    pub folder: Option<&'a Path>,
    /// The home folder, for `~/` rules.
    pub home: Option<PathBuf>,
}

/// The rules' verdict for a call: deny beats ask beats allow, and the
/// rule that decided it.
pub fn rule_verdict<'r>(
    rules: &'r [PermissionRule],
    call: &CallFacts<'_>,
) -> Option<(Mode, &'r PermissionRule)> {
    let hit = |m: Mode| rules.iter().find(|r| r.action == m && r.matches(call));
    hit(Mode::Deny)
        .map(|r| (Mode::Deny, r))
        .or_else(|| hit(Mode::Ask).map(|r| (Mode::Ask, r)))
        .or_else(|| hit(Mode::Allow).map(|r| (Mode::Allow, r)))
}

/// The mode the approval setting gives a call that no rule decided.
/// `confined` says the call stays inside the chat's folder under the
/// sandbox (a file write, a sandboxed command). `None` leaves the call to
/// the class settings (the Ask behaviour).
pub fn mode_verdict(mode: ApprovalMode, effect: EffectClass, confined: bool) -> Option<Mode> {
    let changes = matches!(
        effect,
        EffectClass::Write
            | EffectClass::ExternalSideEffect
            | EffectClass::Destructive
            | EffectClass::Credentials
    );
    match mode {
        ApprovalMode::ReadOnly if changes => Some(Mode::Deny),
        ApprovalMode::ReadOnly | ApprovalMode::Ask => None,
        ApprovalMode::WorkspaceWrite if confined => Some(Mode::Allow),
        ApprovalMode::WorkspaceWrite => None,
        ApprovalMode::Full => Some(Mode::Allow),
    }
}

/// The host of a web address, without `www.`.
pub fn host_of(raw: &str) -> Option<String> {
    url::Url::parse(raw.trim())
        .ok()
        .and_then(|u| u.host_str().map(|h| bare_host(h).to_owned()))
}

fn bare_host(h: &str) -> &str {
    let h = h.trim().trim_end_matches('.');
    h.strip_prefix("www.").unwrap_or(h)
}

/// `arxiv.org` covers `arxiv.org` and `export.arxiv.org`; `*.edu` covers
/// any `.edu` domain; a full address is reduced to its domain.
pub fn domain_matches(pattern: &str, host: &str) -> bool {
    let pat = host_of(pattern).unwrap_or_else(|| pattern.trim().to_ascii_lowercase());
    let pat = bare_host(&pat).to_ascii_lowercase();
    let host = bare_host(host).to_ascii_lowercase();
    if pat.contains('*') {
        return glob(&pat, &host, false);
    }
    host == pat || host.ends_with(&format!(".{pat}"))
}

/// Matches a path glob against a full path. Separators are `/` on every
/// system; on Windows the match ignores case.
pub fn path_matches(
    pattern: &str,
    path: &Path,
    folder: Option<&Path>,
    home: Option<&Path>,
) -> bool {
    let pat = pattern.trim().replace('\\', "/");
    let base = |p: &Path, rest: &str| {
        let mut b = norm(p);
        if !b.ends_with('/') {
            b.push('/');
        }
        b.push_str(rest);
        b
    };
    let pat = if let Some(rest) = pat.strip_prefix("./") {
        match folder {
            Some(f) => base(f, rest),
            None => return false,
        }
    } else if pat == "." {
        match folder {
            Some(f) => base(f, "**"),
            None => return false,
        }
    } else if let Some(rest) = pat.strip_prefix("~/") {
        match home {
            Some(h) => base(h, rest),
            None => return false,
        }
    } else if !pat.starts_with('/') && !has_drive(&pat) && !pat.starts_with("**") {
        // A bare `*.md` or `notes/**` matches anywhere below a folder.
        format!("**/{pat}")
    } else {
        pat
    };
    glob(&pat, &norm(path), cfg!(windows))
}

fn has_drive(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// A path as the matcher sees it: `/` separators, no Windows `\\?\`.
pub fn norm(p: &Path) -> String {
    let s = p.display().to_string().replace('\\', "/");
    s.strip_prefix("//?/").map(str::to_owned).unwrap_or(s)
}

/// A small glob: `*` (not across `/`), `**` (across `/`), `?` (one
/// character). `fold` ignores ASCII case.
pub fn glob(pattern: &str, text: &str, fold: bool) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = if fold {
        (
            pattern.to_ascii_lowercase().chars().collect(),
            text.to_ascii_lowercase().chars().collect(),
        )
    } else {
        (pattern.chars().collect(), text.chars().collect())
    };
    glob_at(&p, &t)
}

fn glob_at(p: &[char], t: &[char]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some('*') if p.get(1) == Some(&'*') => {
            // `**/` also matches no folder at all.
            let rest = &p[2..];
            let rest_slash = rest.strip_prefix(&['/']).unwrap_or(rest);
            if glob_at(rest_slash, t) {
                return true;
            }
            (0..t.len()).any(|i| glob_at(rest, &t[i + 1..]) || glob_at(rest_slash, &t[i + 1..]))
        }
        Some('*') => {
            let rest = &p[1..];
            if glob_at(rest, t) {
                return true;
            }
            for i in 0..t.len() {
                if t[i] == '/' {
                    return false;
                }
                if glob_at(rest, &t[i + 1..]) {
                    return true;
                }
            }
            false
        }
        Some('?') => !t.is_empty() && t[0] != '/' && glob_at(&p[1..], &t[1..]),
        Some(c) => t.first() == Some(c) && glob_at(&p[1..], &t[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modes_round_trip_and_read_only_refuses_changes() {
        for m in ApprovalMode::ALL {
            assert_eq!(ApprovalMode::parse(m.id()), Some(m));
        }
        assert_eq!(
            ApprovalMode::parse("Workspace-Write"),
            Some(ApprovalMode::WorkspaceWrite)
        );
        assert_eq!(ApprovalMode::default(), ApprovalMode::Ask);
        assert_eq!(
            mode_verdict(
                ApprovalMode::ReadOnly,
                EffectClass::ExternalSideEffect,
                true
            ),
            Some(Mode::Deny)
        );
        assert_eq!(
            mode_verdict(ApprovalMode::ReadOnly, EffectClass::Read, false),
            None
        );
        assert_eq!(
            mode_verdict(ApprovalMode::Ask, EffectClass::Write, true),
            None
        );
        assert_eq!(
            mode_verdict(
                ApprovalMode::WorkspaceWrite,
                EffectClass::ExternalSideEffect,
                true
            ),
            Some(Mode::Allow)
        );
        assert_eq!(
            mode_verdict(
                ApprovalMode::WorkspaceWrite,
                EffectClass::ExternalSideEffect,
                false
            ),
            None
        );
        assert_eq!(
            mode_verdict(ApprovalMode::Full, EffectClass::Destructive, false),
            Some(Mode::Allow)
        );
    }

    #[test]
    fn globs() {
        assert!(glob("*.md", "notes.md", false));
        assert!(!glob("*.md", "a/notes.md", false));
        assert!(glob("**/*.md", "a/b/notes.md", false));
        assert!(glob("**/*.md", "notes.md", false));
        assert!(glob("a/**", "a/b/c", false));
        assert!(glob("a/**/c", "a/c", false));
        assert!(glob("n?tes", "notes", false));
        assert!(!glob("NOTES", "notes", false));
        assert!(glob("NOTES", "notes", true));
    }

    #[test]
    fn domains_cover_subdomains_and_wildcards() {
        assert!(domain_matches("arxiv.org", "export.arxiv.org"));
        assert!(domain_matches("arxiv.org", "www.arxiv.org"));
        assert!(!domain_matches("arxiv.org", "notarxiv.org"));
        assert!(domain_matches("*.edu", "cs.stanford.edu"));
        assert!(domain_matches("https://arxiv.org/abs/1", "arxiv.org"));
    }

    #[test]
    fn paths_inside_and_outside_the_folder() {
        let folder = Path::new("/home/u/thesis");
        let home = Path::new("/home/u");
        let inside = Path::new("/home/u/thesis/out/report.md");
        assert!(path_matches("./**", inside, Some(folder), Some(home)));
        assert!(path_matches("./out/*.md", inside, Some(folder), Some(home)));
        assert!(!path_matches("./*.md", inside, Some(folder), Some(home)));
        assert!(path_matches("*.md", inside, Some(folder), Some(home)));
        assert!(path_matches(
            "~/.ssh/**",
            Path::new("/home/u/.ssh/id_ed25519"),
            None,
            Some(home)
        ));
        assert!(!path_matches(
            "./**",
            Path::new("/etc/passwd"),
            Some(folder),
            Some(home)
        ));
        assert!(!path_matches("./**", inside, None, Some(home)));
    }

    #[test]
    fn deny_beats_ask_beats_allow() {
        let rules = vec![
            PermissionRule::new(RuleKind::Tool, "run_command *", Mode::Allow),
            PermissionRule::new(RuleKind::Tool, "run_command git push*", Mode::Deny),
            PermissionRule::new(RuleKind::Tool, "run_command git *", Mode::Ask),
        ];
        let call = |cmd: &str| CallFacts {
            tool: "run_command",
            detail: cmd.to_owned(),
            ..CallFacts::default()
        };
        assert_eq!(
            rule_verdict(&rules, &call("pandoc a.md")).map(|v| v.0),
            Some(Mode::Allow)
        );
        assert_eq!(
            rule_verdict(&rules, &call("git status")).map(|v| v.0),
            Some(Mode::Ask)
        );
        assert_eq!(
            rule_verdict(&rules, &call("git push origin")).map(|v| v.0),
            Some(Mode::Deny)
        );
        let web = CallFacts {
            tool: "read_url",
            host: Some("export.arxiv.org"),
            ..CallFacts::default()
        };
        let r = [PermissionRule::new(RuleKind::Url, "arxiv.org", Mode::Allow)];
        assert_eq!(rule_verdict(&r, &web).map(|v| v.0), Some(Mode::Allow));
        assert!(rule_verdict(&r, &call("x")).is_none());
    }

    #[test]
    fn always_allow_suggestions() {
        let s = PermissionRule::suggest(
            "run_command",
            EffectClass::ExternalSideEffect,
            None,
            &json!({"command": "pandoc in.md -o out.docx"}),
        );
        assert_eq!(
            s,
            PermissionRule::new(RuleKind::Tool, "run_command pandoc *", Mode::Allow)
        );
        let s = PermissionRule::suggest(
            "read_url",
            EffectClass::Network,
            None,
            &json!({"url": "https://www.nature.com/articles/x"}),
        );
        assert_eq!(s.kind, RuleKind::Url);
        assert_eq!(s.pattern, "nature.com");
        let s = PermissionRule::suggest(
            "file_write",
            EffectClass::ExternalSideEffect,
            None,
            &json!({"path": "out/report.md", "text": ""}),
        );
        assert_eq!(s.pattern, "./out/**");
        let s = PermissionRule::suggest(
            "mcp__x__y",
            EffectClass::ExternalSideEffect,
            None,
            &json!({}),
        );
        assert_eq!(s.pattern, "mcp__x__y");
        assert!(s.line().starts_with("Allow · Tool"));
    }
}
