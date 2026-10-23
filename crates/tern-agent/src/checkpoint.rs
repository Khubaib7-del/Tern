//! Run checkpoints: a research run's state, saved at safe points so an
//! interrupted run (a crash, a forced quit, a stop) can be resumed without
//! repeating paid work.
//!
//! Not to be confused with the Git screen's checkpoints ([`crate::git`]),
//! which are commits of the vault. A run checkpoint is one JSON file per
//! chat, `<data>/checkpoints/<chat>.json`, holding the chat's latest
//! unfinished run. It is written atomically (a temporary file, then a
//! rename) and removed when the run answers.
//!
//! | Run | Saved | A resume skips |
//! | --- | --- | --- |
//! | Quick turn | At the start and after every tool result | Every model reply and tool result so far (the conversation is kept) |
//! | Deep research | Plan approved; each worker's reads and notes; each worker's end; before the answer | The plan and its approval, finished workers, and pages already read (their text is kept, cut to what a worker is shown) |
//!
//! The format is versioned ([`CHECKPOINT_VERSION`]); a newer file than this
//! build knows is refused with a plain message rather than misread.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tern_core::TurnState;
use tern_platform::AppPaths;
use tern_providers::ChatMessage;
use tern_research::deep::WorkerStatus;
use tern_research::plan::{ResearchPlan, WorkerBudget};

use crate::sources::PageRef;

/// The checkpoint format's version.
pub const CHECKPOINT_VERSION: u32 = 1;

/// Which loop the run was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Quick,
    Deep,
}

/// The last safe point the run reached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Started; for a quick turn, it may have tool results already.
    #[default]
    Started,
    /// Deep research: the user approved the plan; no worker has finished.
    PlanApproved,
    /// Deep research: workers are under way.
    Workers,
    /// Deep research: every worker finished; the answer is next.
    Synthesis,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::PlanApproved => "plan approved",
            Self::Workers => "workers running",
            Self::Synthesis => "writing the answer",
        }
    }
}

/// A page a worker read, with as much text as a worker is ever shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPage {
    pub page: PageRef,
    pub text: String,
}

/// One deep-research worker's progress.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerCheckpoint {
    pub n: usize,
    pub question: String,
    pub budget: WorkerBudget,
    /// How it ended; `None` while it still has work to do.
    #[serde(default)]
    pub finished: Option<WorkerStatus>,
    #[serde(default)]
    pub note: String,
    /// Rounds whose notes are written.
    #[serde(default)]
    pub round: usize,
    /// This round's pages are read; only its notes are left.
    #[serde(default)]
    pub pending_notes: bool,
    /// Searches still to run.
    #[serde(default)]
    pub queue: Vec<String>,
    #[serde(default)]
    pub searches: usize,
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pages: Vec<SavedPage>,
    /// URLs it has picked to read (read or not).
    #[serde(default)]
    pub seen: Vec<String>,
    /// Time it has run so far.
    #[serde(default)]
    pub took_ms: u64,
    /// Redirects from steering it has taken in.
    #[serde(default)]
    pub steered: Vec<String>,
    /// Extra rounds steering gave it.
    #[serde(default)]
    pub extra_rounds: usize,
}

/// The note a worker gets when a steer drops it.
pub const DROPPED_BY_STEER: &str = "dropped by your steer";

impl WorkerCheckpoint {
    pub fn has_progress(&self) -> bool {
        self.round > 0 || self.pending_notes || !self.pages.is_empty() || self.searches > 0
    }

    /// Ended without notes worth keeping (failed, stopped by you, or out
    /// of time with nothing written), so a resume runs it again from the
    /// start. Done workers, and ones a steer dropped, are kept as they are.
    pub fn retryable(&self) -> bool {
        match self.finished {
            None | Some(WorkerStatus::Done) => false,
            Some(WorkerStatus::Cancelled) => self.note != DROPPED_BY_STEER,
            Some(WorkerStatus::TimedOut | WorkerStatus::OverBudget) => self.notes.trim().is_empty(),
            Some(WorkerStatus::Failed | WorkerStatus::Queued | WorkerStatus::Running) => true,
        }
    }

    /// Back to a fresh start on the same sub-question and budget.
    pub fn reset(&mut self) {
        *self = Self {
            n: self.n,
            question: std::mem::take(&mut self.question),
            budget: self.budget,
            ..Self::default()
        };
    }
}

/// A deep run's workers by how they stand.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkerCounts {
    pub all: usize,
    /// Finished with notes that are kept.
    pub done: usize,
    pub failed: usize,
    /// Stopped by you or by their time or budget, with nothing kept.
    pub stopped: usize,
    /// Not finished yet.
    pub left: usize,
}

/// What the chat offers for a run that did not answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfferKind {
    /// Pick up where it stopped.
    Resume,
    /// Run the failed parts again; finished work is kept.
    Retry,
}

/// The banner for a stopped or failed run: one line that says what
/// happened, one with where it stands, and when it was saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOffer {
    pub kind: OfferKind,
    /// "Run failed: …", "Stopped by you: …", "This run was interrupted: …".
    pub headline: String,
    /// "Deep research · 3 of 5 done · 2 failed".
    pub status: String,
    /// When it was last saved, RFC 3339 (UTC).
    pub saved_at: String,
    pub question: String,
}

impl RunOffer {
    /// The button's label.
    pub fn action(&self) -> &'static str {
        match self.kind {
            OfferKind::Resume => "Resume",
            OfferKind::Retry => "Retry failed workers",
        }
    }

    /// Whether the cause points at web search, so the banner offers to fix it.
    pub fn about_search(&self) -> bool {
        let h = self.headline.to_lowercase();
        h.contains("search")
    }
}

/// A deep run's plan and workers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepState {
    /// The approved plan, with steering's additions and redirects.
    pub plan: ResearchPlan,
    pub concurrency: usize,
    pub workers: Vec<WorkerCheckpoint>,
}

/// A quick turn's conversation so far.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuickState {
    /// The turn's messages after the chat's history: the user's message,
    /// then each model reply and tool result.
    pub messages: Vec<ChatMessage>,
    /// Tool calls asked for so far, for loop detection.
    #[serde(default)]
    pub seen: Vec<(String, u32)>,
}

/// One chat's latest unfinished run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunCheckpoint {
    pub version: u32,
    /// The chat's session id.
    pub session: String,
    /// The interrupted turn's id.
    pub turn: String,
    pub mode: RunMode,
    /// What the user sent.
    pub message: String,
    pub model: String,
    /// When it was last saved, RFC 3339.
    pub saved_at: String,
    pub stage: Stage,
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub model_calls: u32,
    #[serde(default)]
    pub tool_calls: u32,
    /// Steering guidance received so far, which a resumed run keeps.
    #[serde(default)]
    pub guidance: Vec<String>,
    /// What each applied steer did, in order.
    #[serde(default)]
    pub steers: Vec<String>,
    /// Times this run has been resumed.
    #[serde(default)]
    pub resumes: u32,
    #[serde(default)]
    pub quick: Option<QuickState>,
    #[serde(default)]
    pub deep: Option<DeepState>,
}

impl RunCheckpoint {
    /// The question, without attachments or a quoted reply.
    pub fn question(&self) -> String {
        crate::deep::question_of(&self.message)
    }

    /// Workers finished, of all, for a deep run.
    pub fn workers_done(&self) -> Option<(usize, usize)> {
        let d = self.deep.as_ref()?;
        let done = d.workers.iter().filter(|w| w.finished.is_some()).count();
        Some((done, d.workers.len()))
    }

    /// Workers by how they stand, from their saved states.
    pub fn worker_counts(&self) -> Option<WorkerCounts> {
        let d = self.deep.as_ref()?;
        let mut c = WorkerCounts {
            all: d.workers.len(),
            ..WorkerCounts::default()
        };
        for w in &d.workers {
            match w.finished {
                None => c.left += 1,
                Some(WorkerStatus::Failed) => c.failed += 1,
                Some(_) if w.retryable() => c.stopped += 1,
                Some(_) => c.done += 1,
            }
        }
        Some(c)
    }

    /// Whether a resume has anything to do: a worker unfinished or worth
    /// running again, or only the answer left to write.
    pub fn can_resume(&self) -> bool {
        match self.mode {
            RunMode::Quick => true,
            RunMode::Deep => match &self.deep {
                Some(d) if !d.workers.is_empty() => {
                    self.stage == Stage::Synthesis
                        || d.workers
                            .iter()
                            .any(|w| w.finished.is_none() || w.retryable())
                }
                // Approved, but no worker had started.
                Some(_) => true,
                None => false,
            },
        }
    }

    /// "Deep research · 3 of 5 done · 2 failed" or "Quick turn · 3 tool results",
    /// worked out from the workers as they stand, never from a stale stage.
    pub fn progress(&self) -> String {
        match self.mode {
            RunMode::Deep => match self.worker_counts() {
                Some(c) if c.all > 0 => {
                    let mut s = format!("Deep research · {} of {} done", c.done, c.all);
                    if c.failed > 0 {
                        s.push_str(&format!(" · {} failed", c.failed));
                    }
                    if c.stopped > 0 {
                        s.push_str(&format!(" · {} stopped", c.stopped));
                    }
                    if c.left > 0 {
                        s.push_str(&format!(" · {} not finished", c.left));
                    } else if c.failed == 0 && c.stopped == 0 {
                        s.push_str(" · the answer is left to write");
                    }
                    s
                }
                _ => format!("Deep research · {}", self.stage.label()),
            },
            RunMode::Quick => {
                let results = self
                    .quick
                    .as_ref()
                    .map(|q| q.messages.len().saturating_sub(1) / 2)
                    .unwrap_or_default();
                format!(
                    "Quick turn · {results} tool result{}",
                    if results == 1 { "" } else { "s" }
                )
            }
        }
    }

    /// What to offer for this run, given how it ended (`None` when the
    /// app found it on reopening a chat, so it was interrupted). `None`
    /// back when nothing can be picked up.
    pub fn offer(&self, ended: Option<(TurnState, Option<&str>)>) -> Option<RunOffer> {
        if !self.can_resume() {
            return None;
        }
        let question = self.question();
        let retry = self
            .worker_counts()
            .is_some_and(|c| c.failed + c.stopped > 0);
        let (kind, headline) = match ended {
            Some((TurnState::Failed | TurnState::Blocked, why)) => (
                OfferKind::Retry,
                format!(
                    "Run failed: {}",
                    why.map(str::trim)
                        .filter(|w| !w.is_empty())
                        .unwrap_or("a step could not finish")
                ),
            ),
            Some((TurnState::Cancelled, _)) => {
                (OfferKind::Resume, format!("Stopped by you: {question}"))
            }
            Some((TurnState::BudgetExceeded | TurnState::Paused, why)) => (
                OfferKind::Resume,
                format!("Run paused: {}", why.unwrap_or("it reached a limit")),
            ),
            Some((_, why)) => (
                OfferKind::Resume,
                format!("Run stopped: {}", why.unwrap_or(&question)),
            ),
            None => (
                if retry {
                    OfferKind::Retry
                } else {
                    OfferKind::Resume
                },
                format!("This run was interrupted: {question}"),
            ),
        };
        let kind = if kind == OfferKind::Retry && !retry && self.mode == RunMode::Deep {
            OfferKind::Resume
        } else {
            kind
        };
        Some(RunOffer {
            kind,
            headline,
            status: self.progress(),
            saved_at: self.saved_at.clone(),
            question,
        })
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Reads a checkpoint, refusing one from a newer Tern.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("not a Tern run checkpoint: {e}"))?;
        let version = v["version"].as_u64().unwrap_or(0);
        if version > u64::from(CHECKPOINT_VERSION) {
            return Err(format!(
                "this checkpoint was saved by a newer Tern (format {version}; this build reads {CHECKPOINT_VERSION})"
            ));
        }
        serde_json::from_value(v).map_err(|e| format!("not a Tern run checkpoint: {e}"))
    }
}

/// Where a run saves its checkpoints. A failed save never stops the run.
pub trait CheckpointSink: Send + Sync {
    fn save(&self, checkpoint: &RunCheckpoint) -> Result<(), String>;
    /// Forgets the chat's checkpoint (the run answered).
    fn clear(&self, session: &str) -> Result<(), String>;
}

/// Checkpoints as files in a folder, one per chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointDir(pub PathBuf);

/// The app's checkpoint folder.
pub fn checkpoints_dir(paths: &AppPaths) -> PathBuf {
    paths.checkpoints()
}

impl CheckpointDir {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self(dir.into())
    }

    /// The app's own folder (`<data>/checkpoints`).
    pub fn discover() -> Result<Self, String> {
        let paths = AppPaths::discover().map_err(|e| e.to_string())?;
        Ok(Self(checkpoints_dir(&paths)))
    }

    pub fn path(&self, session: &str) -> PathBuf {
        self.0.join(format!("{session}.json"))
    }

    /// The chat's checkpoint, if it has one.
    pub fn load(&self, session: &str) -> Result<Option<RunCheckpoint>, String> {
        let path = self.path(session);
        match std::fs::read_to_string(&path) {
            Ok(text) => RunCheckpoint::from_json(&text).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("Tern could not read {}: {e}", path.display())),
        }
    }

    /// Every chat's checkpoint, newest first. Unreadable files are skipped.
    pub fn list(&self) -> Vec<RunCheckpoint> {
        let Ok(dir) = std::fs::read_dir(&self.0) else {
            return Vec::new();
        };
        let mut out: Vec<RunCheckpoint> = dir
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .filter_map(|t| RunCheckpoint::from_json(&t).ok())
            .collect();
        out.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
        out
    }
}

impl CheckpointSink for CheckpointDir {
    fn save(&self, checkpoint: &RunCheckpoint) -> Result<(), String> {
        write_atomic(&self.path(&checkpoint.session), &checkpoint.to_json())
    }

    fn clear(&self, session: &str) -> Result<(), String> {
        match std::fs::remove_file(self.path(session)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Tern could not make {}: {e}", dir.display()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .map_err(|e| format!("Tern could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Tern could not save {}: {e}", path.display()))
}

/// Keeps checkpoints in memory, for tests: the latest one and every save.
#[derive(Debug, Default)]
pub struct MemoryCheckpoints {
    latest: Mutex<Option<RunCheckpoint>>,
    saves: Mutex<Vec<RunCheckpoint>>,
}

impl MemoryCheckpoints {
    pub fn latest(&self) -> Option<RunCheckpoint> {
        self.latest.lock().ok().and_then(|l| l.clone())
    }

    /// Every save, in order.
    pub fn saves(&self) -> Vec<RunCheckpoint> {
        self.saves.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl CheckpointSink for MemoryCheckpoints {
    fn save(&self, checkpoint: &RunCheckpoint) -> Result<(), String> {
        // Round-trip through JSON, as a file would.
        let back = RunCheckpoint::from_json(&checkpoint.to_json())?;
        if let Ok(mut l) = self.latest.lock() {
            *l = Some(back.clone());
        }
        if let Ok(mut s) = self.saves.lock() {
            s.push(back);
        }
        Ok(())
    }

    fn clear(&self, _session: &str) -> Result<(), String> {
        if let Ok(mut l) = self.latest.lock() {
            *l = None;
        }
        Ok(())
    }
}

/// Now, RFC 3339.
/// An RFC 3339 time on the local clock, like "Fri 23 Oct, 9:31 PM", given
/// the local offset from UTC in seconds. `None` offset (the platform cannot
/// say) keeps UTC and says so. The text as given when it is not RFC 3339.
pub fn local_label(rfc3339: &str, offset_secs: Option<i64>) -> String {
    use time::format_description::well_known::Rfc3339;
    let Ok(t) = time::OffsetDateTime::parse(rfc3339, &Rfc3339) else {
        return rfc3339.to_owned();
    };
    let fmt = time::macros::format_description!(
        "[weekday repr:short] [day padding:none] [month repr:short], [hour repr:12 padding:none]:[minute] [period]"
    );
    let (t, utc) = match offset_secs
        .and_then(|o| i32::try_from(o).ok())
        .and_then(|o| time::UtcOffset::from_whole_seconds(o).ok())
    {
        Some(o) => (t.to_offset(o), false),
        None => (t.to_offset(time::UtcOffset::UTC), true),
    };
    let s = t.format(&fmt).unwrap_or_else(|_| rfc3339.to_owned());
    if utc { format!("{s} UTC") } else { s }
}

/// The local clock's offset from UTC in seconds, when the platform says.
pub fn local_offset() -> Option<i64> {
    time::UtcOffset::current_local_offset()
        .ok()
        .map(|o| i64::from(o.whole_seconds()))
}

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RunCheckpoint {
        RunCheckpoint {
            version: CHECKPOINT_VERSION,
            session: "ses_01".into(),
            turn: "turn_01".into(),
            mode: RunMode::Deep,
            message: "Compare A and B".into(),
            saved_at: "2026-10-22T10:00:00Z".into(),
            stage: Stage::Workers,
            deep: Some(DeepState {
                concurrency: 2,
                workers: vec![
                    WorkerCheckpoint {
                        n: 1,
                        finished: Some(WorkerStatus::Done),
                        ..WorkerCheckpoint::default()
                    },
                    WorkerCheckpoint {
                        n: 2,
                        ..WorkerCheckpoint::default()
                    },
                ],
                ..DeepState::default()
            }),
            ..RunCheckpoint::default()
        }
    }

    #[test]
    fn files_round_trip_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointDir::new(dir.path().join("checkpoints"));
        assert_eq!(store.load("ses_01").unwrap(), None);
        let c = sample();
        store.save(&c).unwrap();
        assert_eq!(store.load("ses_01").unwrap(), Some(c.clone()));
        assert_eq!(store.list().len(), 1);
        assert_eq!(c.progress(), "Deep research · 1 of 2 done · 1 not finished");
        store.clear("ses_01").unwrap();
        store.clear("ses_01").unwrap();
        assert_eq!(store.load("ses_01").unwrap(), None);
    }

    fn worker(n: usize, finished: Option<WorkerStatus>, notes: &str) -> WorkerCheckpoint {
        WorkerCheckpoint {
            n,
            question: format!("q{n}"),
            finished,
            notes: notes.into(),
            searches: 2,
            ..WorkerCheckpoint::default()
        }
    }

    fn deep(workers: Vec<WorkerCheckpoint>) -> RunCheckpoint {
        RunCheckpoint {
            deep: Some(DeepState {
                concurrency: 3,
                workers,
                ..DeepState::default()
            }),
            saved_at: "2026-10-23T16:31:00Z".into(),
            ..sample()
        }
    }

    #[test]
    fn a_run_whose_workers_all_failed_offers_retry_not_resume() {
        let c = deep(
            (1..=5)
                .map(|n| worker(n, Some(WorkerStatus::Failed), ""))
                .collect(),
        );
        assert_eq!(c.progress(), "Deep research · 0 of 5 done · 5 failed");
        assert!(!c.progress().contains("running"));
        let o = c
            .offer(Some((
                TurnState::Failed,
                Some("No search service answered: SearXNG at http://localhost:8080 did not answer"),
            )))
            .unwrap();
        assert_eq!(o.kind, OfferKind::Retry);
        assert_eq!(o.action(), "Retry failed workers");
        assert!(
            o.headline
                .starts_with("Run failed: No search service answered")
        );
        assert!(o.about_search());
    }

    #[test]
    fn a_stopped_run_offers_resume_and_a_finished_one_nothing() {
        let c = deep(vec![
            worker(1, Some(WorkerStatus::Done), "notes [1]"),
            worker(2, None, ""),
        ]);
        let o = c.offer(Some((TurnState::Cancelled, None))).unwrap();
        assert_eq!(o.kind, OfferKind::Resume);
        assert_eq!(o.headline, "Stopped by you: Compare A and B");
        let done = deep(vec![
            worker(1, Some(WorkerStatus::Done), "a"),
            worker(2, Some(WorkerStatus::Done), "b"),
        ]);
        assert!(
            !done.can_resume(),
            "nothing left but a stage before the answer"
        );
        assert_eq!(done.offer(Some((TurnState::Failed, Some("x")))), None);
        let writing = RunCheckpoint {
            stage: Stage::Synthesis,
            ..done
        };
        assert!(writing.can_resume());
        assert_eq!(
            writing.progress(),
            "Deep research · 2 of 2 done · the answer is left to write"
        );
    }

    #[test]
    fn retryable_workers_are_the_ones_without_kept_notes() {
        assert!(worker(1, Some(WorkerStatus::Failed), "").retryable());
        assert!(worker(1, Some(WorkerStatus::Cancelled), "").retryable());
        let mut dropped = worker(1, Some(WorkerStatus::Cancelled), "");
        dropped.note = DROPPED_BY_STEER.into();
        assert!(!dropped.retryable());
        assert!(!worker(1, Some(WorkerStatus::TimedOut), "some notes").retryable());
        assert!(worker(1, Some(WorkerStatus::TimedOut), " ").retryable());
        assert!(!worker(1, Some(WorkerStatus::Done), "n").retryable());
        assert!(!worker(1, None, "").retryable());
        let mut w = worker(3, Some(WorkerStatus::Failed), "x");
        w.reset();
        assert_eq!(
            (w.n, w.question.as_str(), w.finished, w.searches),
            (3, "q3", None, 0)
        );
    }

    #[test]
    fn saved_times_read_on_the_local_clock() {
        let at = "2026-10-23T16:31:00Z";
        assert_eq!(local_label(at, Some(5 * 3600)), "Fri 23 Oct, 9:31 PM");
        assert_eq!(local_label(at, None), "Fri 23 Oct, 4:31 PM UTC");
        assert_eq!(local_label("yesterday", Some(0)), "yesterday");
    }

    #[test]
    fn a_newer_format_is_refused() {
        let mut v = serde_json::to_value(sample()).unwrap();
        v["version"] = serde_json::json!(CHECKPOINT_VERSION + 1);
        let err = RunCheckpoint::from_json(&v.to_string()).unwrap_err();
        assert!(err.contains("newer Tern"));
        assert!(RunCheckpoint::from_json("{").is_err());
    }
}
