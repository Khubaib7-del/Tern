//! Git for the vault (`docs/02-ui-definition.md`, Integrations › Git/GitHub):
//! the vault folder as a repository, so a research project gets
//! checkpoints (commits), branches, push, and a pull request on GitHub.
//!
//! Tern runs the `git` the user already has, so their sign-in (Git
//! Credential Manager on Windows) is the one used. Prompts on the terminal
//! are off: a push that needs a password fails with a message instead of
//! hanging.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One changed file, from `git status --porcelain`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    /// The two-letter code (`M `, ` M`, `??`, `A `, `D `, `R `).
    pub code: String,
}

impl Change {
    /// What happened, in a word.
    pub fn kind(&self) -> &'static str {
        let c = self.code.trim();
        match c.chars().next().unwrap_or(' ') {
            '?' => "New",
            'A' => "Added",
            'D' => "Deleted",
            'R' => "Renamed",
            'U' => "Conflict",
            _ if c.contains('D') => "Deleted",
            _ => "Changed",
        }
    }
}

/// One commit in the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub author: String,
    /// "2 hours ago".
    pub when: String,
}

/// Where the repository stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<Change>,
    pub remote: Option<String>,
}

impl Status {
    /// The GitHub `owner/repo` of the `origin` remote, if it is on GitHub.
    pub fn github(&self) -> Option<(String, String)> {
        github_repo(self.remote.as_deref()?)
    }
}

/// Who commits, and whether Tern is credited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: Option<String>,
    pub email: Option<String>,
    /// Adds a `Co-authored-by: Tern` trailer.
    pub credit_tern: bool,
}

pub const TERN_TRAILER: &str = "Co-authored-by: Tern <tern@users.noreply.github.com>";

/// A folder Tern runs git in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    dir: PathBuf,
}

impl Repo {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether a `git` program can be run at all.
    pub fn git_available() -> bool {
        git_cmd(Path::new("."))
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = git_cmd(&self.dir)
            .args(args)
            .output()
            .map_err(|e| format!("Could not run git: {e}. Install Git from git-scm.com."))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            let msg = err
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with("hint:"))
                .collect::<Vec<_>>()
                .join(" ");
            Err(if msg.is_empty() {
                format!("git {} failed.", args.first().unwrap_or(&""))
            } else {
                msg
            })
        }
    }

    pub fn is_repo(&self) -> bool {
        self.run(&["rev-parse", "--is-inside-work-tree"])
            .is_ok_and(|o| o.trim() == "true")
    }

    /// Makes the folder a repository on `main`.
    pub fn init(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        self.run(&["init", "-b", "main"]).map(|_| ())
    }

    pub fn status(&self) -> Status {
        if !self.is_repo() {
            return Status::default();
        }
        let mut s = self
            .run(&["status", "--porcelain=v1", "-b", "-uall"])
            .map(|o| parse_status(&o))
            .unwrap_or_default();
        s.is_repo = true;
        s.remote = self
            .run(&["remote", "get-url", "origin"])
            .ok()
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty());
        s
    }

    /// The last `n` commits, newest first. Empty for a new repository.
    pub fn log(&self, n: usize) -> Vec<Commit> {
        let n = format!("-n{n}");
        self.run(&["log", &n, "--pretty=format:%h%x1f%s%x1f%an%x1f%ar"])
            .map(|o| parse_log(&o))
            .unwrap_or_default()
    }

    /// Stages everything and commits: a checkpoint. Returns the short hash.
    pub fn checkpoint(&self, message: &str, who: &Identity) -> Result<String, String> {
        let message = message.trim();
        if message.is_empty() {
            return Err("A checkpoint needs a message.".into());
        }
        self.run(&["add", "-A"])?;
        if self.run(&["diff", "--cached", "--quiet"]).is_ok() {
            return Err("Nothing has changed since the last checkpoint.".into());
        }
        let full = commit_message(message, who.credit_tern);
        let mut args: Vec<String> = Vec::new();
        if let Some(n) = who.name.as_deref().filter(|s| !s.trim().is_empty()) {
            args.extend(["-c".into(), format!("user.name={n}")]);
        }
        if let Some(e) = who.email.as_deref().filter(|s| !s.trim().is_empty()) {
            args.extend(["-c".into(), format!("user.email={e}")]);
        }
        args.extend(["commit".into(), "-q".into(), "-m".into(), full]);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs).map_err(|e| {
            if e.contains("user.email") || e.contains("Please tell me who you are") {
                "Git needs your name and email first (Git settings).".into()
            } else {
                e
            }
        })?;
        self.run(&["rev-parse", "--short", "HEAD"])
            .map(|h| h.trim().to_owned())
    }

    pub fn branches(&self) -> Vec<String> {
        self.run(&["branch", "--format=%(refname:short)"])
            .map(|o| o.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// Switches to `name`, creating it from here when it does not exist.
    pub fn switch(&self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if !valid_branch(name) {
            return Err(format!("\"{name}\" is not a branch name git accepts."));
        }
        if self.branches().iter().any(|b| b == name) {
            self.run(&["switch", name]).map(|_| ())
        } else {
            self.run(&["switch", "-c", name]).map(|_| ())
        }
    }

    /// Sets (or replaces) the `origin` remote.
    pub fn set_remote(&self, url: &str) -> Result<(), String> {
        let url = url.trim();
        if url.is_empty() {
            return Err("Paste the repository's address.".into());
        }
        if self.run(&["remote", "get-url", "origin"]).is_ok() {
            self.run(&["remote", "set-url", "origin", url]).map(|_| ())
        } else {
            self.run(&["remote", "add", "origin", url]).map(|_| ())
        }
    }

    /// Pushes the current branch, setting its upstream the first time.
    pub fn push(&self) -> Result<(), String> {
        self.run(&["push", "-u", "origin", "HEAD"]).map(|_| ())
    }

    pub fn pull(&self) -> Result<(), String> {
        self.run(&["pull", "--ff-only"]).map(|_| ())
    }

    /// Writes the baseline files that are missing. Returns the ones made.
    pub fn baselines(&self, owner: &str, year: i32) -> Result<Vec<&'static str>, String> {
        let name = self
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Research".into());
        let mut made = Vec::new();
        for (file, text) in baseline_files(&name, owner, year) {
            let path = self.dir.join(file);
            if path.exists() {
                continue;
            }
            std::fs::write(&path, text).map_err(|e| format!("{file}: {e}"))?;
            made.push(file);
        }
        Ok(made)
    }
}

fn git_cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up for each call.
        c.creation_flags(0x0800_0000);
    }
    c
}

/// The current year, for the LICENSE line.
pub fn this_year() -> i32 {
    time::OffsetDateTime::now_utc().year()
}

/// The message with Tern's trailer when asked for.
pub fn commit_message(message: &str, credit_tern: bool) -> String {
    if credit_tern {
        format!("{}\n\n{TERN_TRAILER}", message.trim())
    } else {
        message.trim().to_owned()
    }
}

/// Branch names git accepts (a practical subset of `check-ref-format`).
pub fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !name.contains("@{")
        && !name.contains("//")
        && !name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
}

/// Parses `git status --porcelain=v1 -b`.
pub fn parse_status(out: &str) -> Status {
    let mut s = Status::default();
    for line in out.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            let (names, counts) = match head.split_once(" [") {
                Some((n, c)) => (n, c.trim_end_matches(']')),
                None => (head, ""),
            };
            let names = names.strip_prefix("No commits yet on ").unwrap_or(names);
            match names.split_once("...") {
                Some((b, u)) => {
                    s.branch = Some(b.to_owned());
                    s.upstream = Some(u.to_owned());
                }
                None => s.branch = Some(names.to_owned()),
            }
            for part in counts.split(", ") {
                if let Some(n) = part.strip_prefix("ahead ") {
                    s.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("behind ") {
                    s.behind = n.parse().unwrap_or(0);
                }
            }
        } else if line.len() > 3 {
            let path = &line[3..];
            let path = path.rsplit(" -> ").next().unwrap_or(path);
            s.changes.push(Change {
                path: path.trim_matches('"').to_owned(),
                code: line[..2].to_owned(),
            });
        }
    }
    s
}

fn parse_log(out: &str) -> Vec<Commit> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.split('\u{1f}');
            Some(Commit {
                hash: f.next()?.to_owned(),
                subject: f.next()?.to_owned(),
                author: f.next()?.to_owned(),
                when: f.next()?.to_owned(),
            })
        })
        .collect()
}

/// `owner/repo` from a GitHub remote (https or ssh form).
pub fn github_repo(remote: &str) -> Option<(String, String)> {
    let r = remote.trim();
    let rest = r
        .strip_prefix("https://github.com/")
        .or_else(|| r.strip_prefix("http://github.com/"))
        .or_else(|| r.strip_prefix("git@github.com:"))
        .or_else(|| r.strip_prefix("ssh://git@github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = rest.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/'))
        .then(|| (owner.to_owned(), repo.to_owned()))
}

/// GitHub's page to open a pull request from `branch` into `base`.
pub fn pull_request_url(owner: &str, repo: &str, base: &str, branch: &str) -> String {
    format!("https://github.com/{owner}/{repo}/compare/{base}...{branch}?expand=1")
}

/// The repository's page on GitHub.
pub fn repo_url(owner: &str, repo: &str) -> String {
    format!("https://github.com/{owner}/{repo}")
}

/// README, LICENSE (all rights reserved), SECURITY, and CONTRIBUTING.
pub fn baseline_files(project: &str, owner: &str, year: i32) -> [(&'static str, String); 4] {
    let owner = if owner.trim().is_empty() {
        "The author"
    } else {
        owner.trim()
    };
    [
        (
            "README.md",
            format!("# {project}\n\nResearch notes and sources, kept with Tern.\n\n## Layout\n\n- Notes are Markdown files; `[[links]]` connect them.\n- Each checkpoint is a commit, so any earlier state can be restored.\n"),
        ),
        (
            "LICENSE",
            format!("Copyright (c) {year} {owner}. All rights reserved.\n\nNo part of this repository may be copied, changed, or shared without written permission from the copyright holder.\n"),
        ),
        (
            "SECURITY.md",
            "# Security\n\nPlease report a security problem privately to the repository owner rather than in a public issue. Do not commit API keys, passwords, or personal data to this repository.\n".to_owned(),
        ),
        (
            "CONTRIBUTING.md",
            "# Contributing\n\n1. Make a branch for your change.\n2. Keep notes in Markdown and cite every source by its link.\n3. Open a pull request with a short description of what changed and why.\n".to_owned(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_status_output() {
        let s = parse_status(
            "## main...origin/main [ahead 2, behind 1]\n M notes/RAG.md\n?? New note.md\nR  old.md -> new.md\nD  gone.md\n",
        );
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        let kinds: Vec<_> = s
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.kind()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("notes/RAG.md", "Changed"),
                ("New note.md", "New"),
                ("new.md", "Renamed"),
                ("gone.md", "Deleted")
            ]
        );
        let fresh = parse_status("## No commits yet on main\n");
        assert_eq!(fresh.branch.as_deref(), Some("main"));
        assert_eq!(fresh.upstream, None);
    }

    #[test]
    fn github_addresses_and_branch_names() {
        for r in [
            "https://github.com/Khubaib7-del/Tern.git",
            "git@github.com:Khubaib7-del/Tern.git",
            "https://github.com/Khubaib7-del/Tern/",
        ] {
            assert_eq!(
                github_repo(r),
                Some(("Khubaib7-del".into(), "Tern".into())),
                "{r}"
            );
        }
        assert_eq!(github_repo("https://gitlab.com/a/b"), None);
        assert_eq!(
            pull_request_url("o", "r", "main", "draft"),
            "https://github.com/o/r/compare/main...draft?expand=1"
        );
        assert!(valid_branch("chapter-2/lit-review"));
        for bad in ["", "-x", "a b", "a..b", "a.lock", "x:", "a/", "~x"] {
            assert!(!valid_branch(bad), "{bad}");
        }
        assert!(commit_message(" Draft ", true).ends_with(TERN_TRAILER));
        assert_eq!(commit_message(" Draft ", false), "Draft");
    }

    #[test]
    fn a_vault_becomes_a_repository() {
        if !Repo::git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::at(dir.path().join("Thesis"));
        assert!(!repo.status().is_repo);
        repo.init().unwrap();
        let made = repo.baselines("Khubaib Nazeer", 2026).unwrap();
        assert_eq!(
            made,
            ["README.md", "LICENSE", "SECURITY.md", "CONTRIBUTING.md"]
        );
        assert!(repo.baselines("x", 2026).unwrap().is_empty());
        let s = repo.status();
        assert!(s.is_repo);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.changes.len(), 4);
        let who = Identity {
            name: Some("Test".into()),
            email: Some("test@example.org".into()),
            credit_tern: true,
        };
        assert!(repo.checkpoint("  ", &who).is_err());
        let hash = repo.checkpoint("Baselines", &who).unwrap();
        assert!(!hash.is_empty());
        assert!(
            repo.checkpoint("Again", &who)
                .unwrap_err()
                .contains("Nothing has changed")
        );
        let log = repo.log(5);
        assert_eq!(log.len(), 1);
        assert_eq!(
            (log[0].subject.as_str(), log[0].author.as_str()),
            ("Baselines", "Test")
        );
        repo.switch("draft").unwrap();
        assert_eq!(repo.status().branch.as_deref(), Some("draft"));
        assert!(repo.switch("bad name").is_err());
        repo.set_remote("https://github.com/o/r.git").unwrap();
        repo.set_remote("git@github.com:o/r2.git").unwrap();
        assert_eq!(repo.status().github(), Some(("o".into(), "r2".into())));
        assert!(repo.status().changes.is_empty());
    }
}
