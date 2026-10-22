//! The Git screen's state: the vault repository's status, log, and the
//! operation running. GPUI-free so it is unit tested; git itself runs in
//! `tern_agent::git` on the bridge.

use tern_agent::git::{self, Commit, Repo, Status};

/// Something the Git screen asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitOp {
    Refresh,
    Init,
    Checkpoint(String),
    Switch(String),
    SetRemote(String),
    Push,
    Pull,
    Baselines,
}

impl GitOp {
    /// What the screen says while it runs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Refresh => "Checking…",
            Self::Init => "Making the repository…",
            Self::Checkpoint(_) => "Saving a checkpoint…",
            Self::Switch(_) => "Switching branch…",
            Self::SetRemote(_) => "Saving the remote…",
            Self::Push => "Pushing…",
            Self::Pull => "Pulling…",
            Self::Baselines => "Adding baseline files…",
        }
    }
}

/// The repository as last read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitSnapshot {
    pub available: bool,
    pub status: Status,
    pub log: Vec<Commit>,
    pub branches: Vec<String>,
}

impl GitSnapshot {
    /// Reads everything the screen shows. Blocking; runs on the bridge.
    pub fn read(repo: &Repo) -> Self {
        if !Repo::git_available() {
            return Self::default();
        }
        let status = repo.status();
        let (log, branches) = if status.is_repo {
            (repo.log(15), repo.branches())
        } else {
            (Vec::new(), Vec::new())
        };
        Self {
            available: true,
            status,
            log,
            branches,
        }
    }
}

/// Runs one operation. Returns what to tell the user when it worked.
pub fn run(
    repo: &Repo,
    op: &GitOp,
    who: &git::Identity,
    owner: &str,
    year: i32,
) -> Result<Option<String>, String> {
    match op {
        GitOp::Refresh => Ok(None),
        GitOp::Init => repo
            .init()
            .map(|_| Some("The vault is now a git repository.".into())),
        GitOp::Checkpoint(m) => repo
            .checkpoint(m, who)
            .map(|h| Some(format!("Checkpoint {h} saved."))),
        GitOp::Switch(b) => repo
            .switch(b)
            .map(|_| Some(format!("On branch {}.", b.trim()))),
        GitOp::SetRemote(u) => repo.set_remote(u).map(|_| Some("Remote saved.".into())),
        GitOp::Push => repo.push().map(|_| Some("Pushed.".into())),
        GitOp::Pull => repo.pull().map(|_| Some("Up to date.".into())),
        GitOp::Baselines => repo.baselines(owner, year).map(|made| {
            Some(if made.is_empty() {
                "The baseline files are already there.".into()
            } else {
                format!("Added {}.", made.join(", "))
            })
        }),
    }
}

#[derive(Debug, Default)]
pub struct GitState {
    pub snapshot: Option<GitSnapshot>,
    /// The operation running, by its label.
    pub busy: Option<&'static str>,
    pub note: Option<String>,
    pub error: Option<String>,
}

impl GitState {
    /// Marks `op` as running. `false` when another one still is.
    pub fn start(&mut self, op: &GitOp) -> bool {
        if self.busy.is_some() {
            return false;
        }
        self.busy = Some(op.label());
        self.error = None;
        if *op != GitOp::Refresh {
            self.note = None;
        }
        true
    }

    pub fn finished(&mut self, snapshot: GitSnapshot, result: Result<Option<String>, String>) {
        self.busy = None;
        self.snapshot = Some(snapshot);
        match result {
            Ok(Some(n)) => self.note = Some(n),
            Ok(None) => {}
            Err(e) => self.error = Some(e),
        }
    }

    pub fn status(&self) -> Option<&Status> {
        self.snapshot
            .as_ref()
            .map(|s| &s.status)
            .filter(|s| s.is_repo)
    }

    /// The title-bar chip: branch and number of changes.
    pub fn chip(&self) -> Option<String> {
        let s = self.status()?;
        let branch = s.branch.clone().unwrap_or_else(|| "detached".into());
        Some(match s.changes.len() {
            0 => branch,
            n => format!("{branch} · {n}"),
        })
    }

    /// GitHub's page to open a pull request for this branch, when the
    /// branch is not the default one and the remote is on GitHub.
    pub fn pull_request_url(&self) -> Option<String> {
        let s = self.status()?;
        let (owner, repo) = s.github()?;
        let branch = s.branch.as_deref()?;
        let base = ["main", "master"]
            .into_iter()
            .find(|b| {
                self.snapshot
                    .as_ref()
                    .is_some_and(|sn| sn.branches.iter().any(|x| x == b))
            })
            .unwrap_or("main");
        (branch != base).then(|| git::pull_request_url(&owner, &repo, base, branch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_agent::git::{Change, parse_status};

    fn snap(status: &str, remote: Option<&str>, branches: &[&str]) -> GitSnapshot {
        let mut s = parse_status(status);
        s.is_repo = true;
        s.remote = remote.map(str::to_owned);
        GitSnapshot {
            available: true,
            status: s,
            log: Vec::new(),
            branches: branches.iter().map(|b| b.to_string()).collect(),
        }
    }

    #[test]
    fn one_operation_at_a_time() {
        let mut g = GitState::default();
        assert!(g.chip().is_none());
        assert!(g.start(&GitOp::Push));
        assert!(!g.start(&GitOp::Pull));
        assert_eq!(g.busy, Some("Pushing…"));
        g.finished(snap("## main\n", None, &["main"]), Err("denied".into()));
        assert_eq!(g.error.as_deref(), Some("denied"));
        assert!(g.start(&GitOp::Checkpoint("x".into())));
        assert!(g.error.is_none());
        g.finished(
            snap("## main\n M a.md\n?? b.md\n", None, &["main"]),
            Ok(Some("Checkpoint abc saved.".into())),
        );
        assert_eq!(g.chip().as_deref(), Some("main · 2"));
        assert_eq!(g.note.as_deref(), Some("Checkpoint abc saved."));
        assert_eq!(
            g.status().unwrap().changes[0],
            Change {
                path: "a.md".into(),
                code: " M".into()
            }
        );
    }

    #[test]
    fn pull_requests_only_from_a_side_branch_on_github() {
        let mut g = GitState::default();
        g.finished(
            snap("## main\n", Some("git@github.com:o/r.git"), &["main"]),
            Ok(None),
        );
        assert_eq!(g.pull_request_url(), None);
        g.finished(
            snap(
                "## draft\n",
                Some("git@github.com:o/r.git"),
                &["master", "draft"],
            ),
            Ok(None),
        );
        assert_eq!(
            g.pull_request_url().as_deref(),
            Some("https://github.com/o/r/compare/master...draft?expand=1")
        );
        g.finished(
            snap(
                "## draft\n",
                Some("https://gitlab.com/o/r"),
                &["main", "draft"],
            ),
            Ok(None),
        );
        assert_eq!(g.pull_request_url(), None);
    }
}
