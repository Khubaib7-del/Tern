//! The watcher: research questions put on watch, re-run on an interval,
//! with what changed since the last run.
//!
//! Watches live in `<data>/watches.json`. A watch is due when its interval
//! has passed since its last run. The desktop app runs due watches in the
//! background while it is open; `tern watch run` runs them from the
//! terminal. There is no operating-system scheduler: nothing runs while
//! Tern is closed.
//!
//! Each run is a new chat (a quick turn, or deep research with the plan run
//! as drafted) followed by the claim checks and run report. The new report
//! is diffed against the last one ([`tern_research::watch`]): new and gone
//! sources, verdicts that changed, and claims that are new or dropped.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tern_platform::AppPaths;
pub use tern_research::watch::{ChangedSource, VerdictChange, WhatChanged, diff};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::chat::{ChatOptions, ChatSession};
use crate::deep::{DeepControl, DeepOptions, RunAsDrafted};
use crate::engine::{CancelFlag, DenyAll};
use crate::report::{ReportFiles, RunReport};

/// The watch list's format version.
pub const WATCHES_VERSION: u32 = 1;

/// Shortest interval a watch may have, in hours.
pub const MIN_HOURS: u32 = 1;

/// One watched question.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watch {
    /// A short id (`tern watch rm <id>`).
    pub id: String,
    pub question: String,
    /// Hours between runs: 24 is daily, 168 weekly.
    pub every_hours: u32,
    /// Run as deep research rather than a quick turn.
    #[serde(default)]
    pub deep: bool,
    /// RFC 3339.
    pub created: String,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub last_run: Option<String>,
    /// The chat the last run made.
    #[serde(default)]
    pub last_session: Option<String>,
    /// The last run's report.
    #[serde(default)]
    pub last_report: Option<ReportFiles>,
    /// What the last run changed, against the run before it.
    #[serde(default)]
    pub last_changes: Option<WhatChanged>,
    /// The last run's verdict tally line ("3 supported · 1 partly").
    #[serde(default)]
    pub last_tally: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub runs: u32,
}

impl Watch {
    /// "daily", "weekly", "every 12 h", "every 3 days".
    pub fn every_label(&self) -> String {
        every_label(self.every_hours)
    }

    /// When it is next due; `None` when it has never run (due now).
    pub fn next_due(&self) -> Option<OffsetDateTime> {
        let last = OffsetDateTime::parse(self.last_run.as_deref()?, &Rfc3339).ok()?;
        Some(last + time::Duration::hours(i64::from(self.every_hours.max(MIN_HOURS))))
    }

    /// "next 2026-10-23 09:00 UTC", or "due now".
    pub fn next_due_text(&self) -> String {
        self.due_label(OffsetDateTime::now_utc())
    }

    /// "paused", "due now", or "next 2026-10-23 09:00 UTC".
    pub fn due_label(&self, now: OffsetDateTime) -> String {
        if self.paused {
            return "paused".into();
        }
        match self.next_due() {
            Some(d) if d > now => {
                let f =
                    time::macros::format_description!("[year]-[month]-[day] [hour]:[minute] UTC");
                format!("next {}", d.format(&f).unwrap_or_default())
            }
            _ => "due now".into(),
        }
    }

    pub fn is_due(&self, now: OffsetDateTime) -> bool {
        !self.paused && self.next_due().is_none_or(|d| d <= now)
    }

    /// One line for a list: what changed last time, or why it failed.
    pub fn status_line(&self) -> String {
        if let Some(e) = &self.last_error {
            return format!("Last run failed: {e}");
        }
        match (&self.last_run, &self.last_changes) {
            (None, _) => "Not run yet".to_owned(),
            (Some(_), None) => "First run done; changes show from the next run".to_owned(),
            (Some(_), Some(c)) => c.line(),
        }
    }
}

/// "daily", "weekly", "every 12 h".
pub fn every_label(hours: u32) -> String {
    match hours {
        1 => "hourly".into(),
        24 => "daily".into(),
        168 => "weekly".into(),
        h if h % 168 == 0 => format!("every {} weeks", h / 168),
        h if h % 24 == 0 => format!("every {} days", h / 24),
        h => format!("every {h} h"),
    }
}

/// Reads an interval: `hourly`, `daily`, `weekly`, `12h`, `3d`, `2w`.
pub fn parse_every(text: &str) -> Result<u32, String> {
    let t = text.trim().to_lowercase();
    let hours = match t.as_str() {
        "hourly" => 1,
        "daily" | "day" | "1d" => 24,
        "weekly" | "week" | "1w" => 168,
        _ => {
            let (num, unit) = t.split_at(t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len()));
            let n: u32 = num.parse().map_err(|_| {
                format!("`{text}` is not an interval: try daily, weekly, 12h, or 3d")
            })?;
            match unit.trim() {
                "h" | "hr" | "hrs" | "hour" | "hours" => n,
                "d" | "day" | "days" => n * 24,
                "w" | "wk" | "week" | "weeks" => n * 168,
                _ => {
                    return Err(format!(
                        "`{text}` is not an interval: try daily, weekly, 12h, or 3d"
                    ));
                }
            }
        }
    };
    if hours < MIN_HOURS {
        return Err("A watch runs at most once an hour.".into());
    }
    Ok(hours)
}

/// Every watch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchList {
    pub version: u32,
    pub watches: Vec<Watch>,
}

/// Where the watch list is kept.
pub fn watches_path(paths: &AppPaths) -> PathBuf {
    paths.data_dir.join("watches.json")
}

impl WatchList {
    /// Reads the list; a missing file is an empty list.
    pub fn load_from(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let list: Self = serde_json::from_str(&text)
                    .map_err(|e| format!("{} is not a Tern watch list: {e}", path.display()))?;
                if list.version > WATCHES_VERSION {
                    return Err("The watch list was saved by a newer Tern.".into());
                }
                Ok(list)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("Tern could not read {}: {e}", path.display())),
        }
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut list = self.clone();
        list.version = WATCHES_VERSION;
        let text = serde_json::to_string_pretty(&list).unwrap_or_default();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    /// The app's list.
    pub fn load() -> Result<Self, String> {
        let paths = AppPaths::discover().map_err(|e| e.to_string())?;
        Self::load_from(&watches_path(&paths))
    }

    pub fn save(&self) -> Result<(), String> {
        let paths = AppPaths::discover().map_err(|e| e.to_string())?;
        self.save_to(&watches_path(&paths))
    }

    /// Puts a question on watch. The same question twice is refused.
    pub fn add(&mut self, question: &str, every_hours: u32, deep: bool) -> Result<Watch, String> {
        let question = question.split_whitespace().collect::<Vec<_>>().join(" ");
        if question.is_empty() {
            return Err("A watch needs a question.".into());
        }
        if self
            .watches
            .iter()
            .any(|w| w.question.eq_ignore_ascii_case(&question))
        {
            return Err("That question is already on watch.".into());
        }
        let id = loop {
            let raw = tern_core::TurnId::new().to_string().to_lowercase();
            let id: String = raw.chars().rev().take(6).collect();
            if !self.watches.iter().any(|w| w.id == id) {
                break id;
            }
        };
        let w = Watch {
            id,
            question,
            every_hours: every_hours.max(MIN_HOURS),
            deep,
            created: now(),
            ..Watch::default()
        };
        self.watches.push(w.clone());
        Ok(w)
    }

    /// The watch `which` names: its id, the start of it, or its number in
    /// the list (from 1).
    pub fn position(&self, which: &str) -> Result<usize, String> {
        let which = which.trim().to_lowercase();
        if let Ok(n) = which.parse::<usize>()
            && n >= 1
            && n <= self.watches.len()
            && !self.watches.iter().any(|w| w.id == which)
        {
            return Ok(n - 1);
        }
        let hits: Vec<usize> = (0..self.watches.len())
            .filter(|&i| self.watches[i].id.starts_with(&which))
            .collect();
        match hits.as_slice() {
            [i] => Ok(*i),
            [] => Err(format!(
                "No watch matches `{which}`. `tern watch list` shows them."
            )),
            _ => Err(format!(
                "`{which}` matches {} watches; give more of the id.",
                hits.len()
            )),
        }
    }

    pub fn remove(&mut self, which: &str) -> Result<Watch, String> {
        let i = self.position(which)?;
        Ok(self.watches.remove(i))
    }

    /// Ids of the watches due at `now`.
    pub fn due(&self, now: OffsetDateTime) -> Vec<String> {
        self.watches
            .iter()
            .filter(|w| w.is_due(now))
            .map(|w| w.id.clone())
            .collect()
    }
}

/// Records a finished run on its watch: what changed against the last
/// report, the new report's files, and the time.
pub fn record_run(
    w: &mut Watch,
    session: &str,
    files: Option<ReportFiles>,
    report: Option<&RunReport>,
    previous: Option<&RunReport>,
    at: &str,
) {
    w.runs += 1;
    w.last_run = Some(at.to_owned());
    w.last_session = Some(session.to_owned());
    match report {
        Some(r) => {
            w.last_error = None;
            w.last_changes = previous.map(|p| diff(p, r));
            w.last_tally = Some(r.tally().line());
            w.last_report = files;
        }
        None => {
            w.last_error = Some("the run gave no answer to report on".into());
        }
    }
}

/// One watch run's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchRun {
    pub watch: Watch,
    pub answer: Option<String>,
}

/// Runs watch `which` now: a new chat with its question, the claim checks
/// and report, and the diff against its last report. The list is saved
/// with the result (or the error).
pub async fn run_watch(
    which: &str,
    opts: ChatOptions,
    deep: DeepOptions,
) -> Result<WatchRun, String> {
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let path = watches_path(&paths);
    let list = WatchList::load_from(&path)?;
    let w = list.watches[list.position(which)?].clone();
    let result = run_once(&w, opts, deep).await;
    // Saved against a fresh read, so watches added meanwhile are kept.
    let mut list = WatchList::load_from(&path)?;
    let i = list
        .watches
        .iter()
        .position(|x| x.id == w.id)
        .ok_or("The watch was removed while it ran.")?;
    let at = now();
    let answer = match result {
        Ok((session, files, answer)) => {
            let report = files
                .as_ref()
                .and_then(|f| std::fs::read_to_string(&f.json).ok())
                .and_then(|t| RunReport::from_json(&t).ok());
            let previous = list.watches[i]
                .last_report
                .as_ref()
                .and_then(|f| std::fs::read_to_string(&f.json).ok())
                .and_then(|t| RunReport::from_json(&t).ok());
            record_run(
                &mut list.watches[i],
                &session,
                files,
                report.as_ref(),
                previous.as_ref(),
                &at,
            );
            if answer.is_none() {
                list.watches[i].last_error = Some("the run stopped without an answer".into());
            }
            answer
        }
        Err(e) => {
            let w = &mut list.watches[i];
            w.last_run = Some(at);
            w.last_error = Some(e);
            None
        }
    };
    list.save_to(&path)?;
    Ok(WatchRun {
        watch: list.watches[i].clone(),
        answer,
    })
}

async fn run_once(
    w: &Watch,
    opts: ChatOptions,
    deep: DeepOptions,
) -> Result<(String, Option<ReportFiles>, Option<String>), String> {
    let mut session = ChatSession::open(opts).await?;
    let quiet = |_: &tern_core::EventEnvelope| {};
    let out = if w.deep {
        session
            .deep_research(
                &w.question,
                &deep,
                &RunAsDrafted,
                &DeepControl::default(),
                &quiet,
                None,
            )
            .await
    } else {
        session
            .send(&w.question, &DenyAll, CancelFlag::default(), &quiet)
            .await
    };
    let files = session.check_last_turn(&quiet).await;
    let reason = out.reason.clone();
    match (out.answer, reason) {
        (None, Some(why)) => Err(why),
        (answer, _) => Ok((session.session_id().to_string(), files, answer)),
    }
}

/// The watches due now, from the app's list.
pub fn due_now() -> Result<Vec<String>, String> {
    Ok(WatchList::load()?.due(OffsetDateTime::now_utc()))
}

/// The due watches in the order to run them: the decision model weighs
/// each question and how overdue it is; with decisions off, list order.
pub async fn order_due(list: &WatchList, decider: &crate::decide::Decider) -> Vec<String> {
    let now = OffsetDateTime::now_utc();
    let due = list.due(now);
    let rows: Vec<(String, f32)> = due
        .iter()
        .filter_map(|id| list.watches.iter().find(|w| &w.id == id))
        .map(|w| {
            let late = w
                .next_due()
                .map(|d| (now - d).as_seconds_f32() / 3600.0)
                .unwrap_or(w.every_hours as f32)
                .max(0.0);
            (w.question.clone(), late)
        })
        .collect();
    match crate::decide::order_watches(decider, &rows).await {
        Some((order, _)) => order.into_iter().map(|i| due[i].clone()).collect(),
        None => due,
    }
}

/// Runs every due watch, one after another, most pressing first.
pub async fn run_due(opts: ChatOptions, deep: DeepOptions) -> Vec<Result<WatchRun, String>> {
    let list = match WatchList::load() {
        Ok(l) => l,
        Err(e) => return vec![Err(e)],
    };
    let secrets = crate::providers::keychain().ok();
    let decider = crate::decide::decider_from(&opts.decision, secrets.as_deref());
    let due = order_due(&list, &decider).await;
    let mut out = Vec::new();
    for id in due {
        out.push(run_watch(&id, opts.clone(), deep).await);
    }
    out
}

fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_research::report::ReportSource;

    #[test]
    fn intervals_read_plainly() {
        assert_eq!(parse_every("daily"), Ok(24));
        assert_eq!(parse_every("Weekly"), Ok(168));
        assert_eq!(parse_every("12h"), Ok(12));
        assert_eq!(parse_every("3 days"), Ok(72));
        assert_eq!(parse_every("2w"), Ok(336));
        assert!(parse_every("0h").is_err());
        assert!(parse_every("soon").is_err());
        assert_eq!(every_label(24), "daily");
        assert_eq!(every_label(72), "every 3 days");
        assert_eq!(every_label(12), "every 12 h");
    }

    #[test]
    fn watches_are_added_found_due_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("watches.json");
        let mut list = WatchList::load_from(&path).unwrap();
        let a = list
            .add("  What changed in  EU AI rules? ", 24, false)
            .unwrap();
        assert_eq!(a.question, "What changed in EU AI rules?");
        assert!(list.add("what changed in eu ai rules?", 24, false).is_err());
        let b = list.add("Rust release notes", 168, true).unwrap();
        list.save_to(&path).unwrap();
        let mut list = WatchList::load_from(&path).unwrap();
        assert_eq!(list.watches.len(), 2);
        let now = OffsetDateTime::now_utc();
        assert_eq!(list.due(now).len(), 2, "never run means due");
        list.watches[0].last_run = Some(now.format(&Rfc3339).unwrap());
        assert_eq!(list.due(now), vec![b.id.clone()]);
        assert_eq!(
            list.due(now + time::Duration::hours(25)).len(),
            2,
            "a day later the daily one is due again"
        );
        list.watches[1].paused = true;
        assert_eq!(list.due(now + time::Duration::hours(25)).len(), 1);
        assert_eq!(list.position("2"), Ok(1));
        assert_eq!(list.position(&a.id[..4]), Ok(0));
        assert!(list.position("zzzzzz").is_err());
        assert_eq!(list.remove(&b.id).unwrap().question, "Rust release notes");
        assert_eq!(list.watches.len(), 1);
    }

    #[test]
    fn a_run_records_what_changed() {
        let source = |url: &str| ReportSource {
            url: url.into(),
            ..ReportSource::default()
        };
        let first = RunReport {
            sources: vec![source("https://a.test")],
            ..RunReport::default()
        };
        let second = RunReport {
            sources: vec![source("https://a.test"), source("https://b.test")],
            ..RunReport::default()
        };
        let mut w = Watch::default();
        assert_eq!(w.status_line(), "Not run yet");
        record_run(
            &mut w,
            "ses_1",
            None,
            Some(&first),
            None,
            "2026-10-01T00:00:00Z",
        );
        assert!(w.status_line().starts_with("First run done"));
        record_run(
            &mut w,
            "ses_2",
            None,
            Some(&second),
            Some(&first),
            "2026-10-02T00:00:00Z",
        );
        assert_eq!(w.runs, 2);
        assert_eq!(w.last_session.as_deref(), Some("ses_2"));
        assert_eq!(w.status_line(), "1 new source");
        record_run(
            &mut w,
            "ses_3",
            None,
            None,
            Some(&second),
            "2026-10-03T00:00:00Z",
        );
        assert!(w.status_line().starts_with("Last run failed"));
    }
}
