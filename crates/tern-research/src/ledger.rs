//! A run's ledgers: every URL it considered and what became of it, and
//! what each step and each worker used.
//!
//! | Ledger | One row per | Holds |
//! | --- | --- | --- |
//! | Source | URL (per worker) | Read live, read from the cache, skipped (robots, budget, duplicate, not picked, policy, cancelled), or failed |
//! | Budget | step | Tokens in and out, pages, searches, and time since the lane's previous step |
//!
//! [`LedgerBuilder`] takes the facts in the order a run logged them and
//! settles each URL's fate; the engine in `tern-agent` feeds it from the
//! event log, so any past run's ledgers can be rebuilt.

use serde::{Deserialize, Serialize};

use crate::plan::WorkerBudget;

/// What became of a URL the run came across.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFate {
    Read,
    #[default]
    Skipped,
    Failed,
}

impl SourceFate {
    pub fn label(self) -> &'static str {
        match self {
            Self::Read => "Read",
            Self::Skipped => "Skipped",
            Self::Failed => "Failed",
        }
    }
}

/// Why, in one word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerReason {
    /// Read from Tern's page cache.
    CacheHit,
    /// Read from the site.
    Live,
    /// The site's robots.txt asks automated readers to stay out.
    Robots,
    /// The page, token, or time budget had run out.
    Budget,
    /// Already read in this run (or by another worker).
    Duplicate,
    /// A search result that was not chosen for reading.
    #[default]
    NotPicked,
    /// The policy gate refused it or it needed an approval.
    Policy,
    /// The run or its worker was stopped first.
    Cancelled,
    /// Brought back from a checkpoint after a resume.
    Restored,
    /// The read failed for another reason.
    Error,
}

impl LedgerReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::CacheHit => "cache hit",
            Self::Live => "read live",
            Self::Robots => "robots.txt",
            Self::Budget => "budget",
            Self::Duplicate => "duplicate",
            Self::NotPicked => "not picked",
            Self::Policy => "policy",
            Self::Cancelled => "cancelled",
            Self::Restored => "from checkpoint",
            Self::Error => "error",
        }
    }
}

/// One URL in the source ledger.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEntry {
    pub url: String,
    #[serde(default)]
    pub site: String,
    #[serde(default)]
    pub title: String,
    /// The deep-research worker that came across it; `None` for a quick turn.
    #[serde(default)]
    pub worker: Option<usize>,
    pub fate: SourceFate,
    pub reason: LedgerReason,
    /// The error or the search it came from.
    #[serde(default)]
    pub detail: String,
    /// Milliseconds since the run started.
    #[serde(default)]
    pub at_ms: u64,
    /// The paper this source is, when a scholarly search found it (#82):
    /// authors, year, venue, DOI, citations, open-access PDF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paper: Option<crate::scholar::Paper>,
}

/// What kind of step a budget row is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepUse {
    #[default]
    Model,
    Search,
    Read,
    /// A worker's whole run, its totals.
    Worker,
}

/// One step in the budget ledger.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetEntry {
    /// "plan", "notes", "answer", a query, or a URL.
    pub step: String,
    pub kind: StepUse,
    #[serde(default)]
    pub worker: Option<usize>,
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub pages: u64,
    #[serde(default)]
    pub searches: u64,
    /// Time since the previous step in the same lane (the worker, or the
    /// run itself), in milliseconds.
    #[serde(default)]
    pub ms: u64,
    #[serde(default)]
    pub at_ms: u64,
}

/// One lane's totals: the run's own steps (`worker: None`) or a worker's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneTotal {
    #[serde(default)]
    pub worker: Option<usize>,
    pub label: String,
    pub model_calls: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub pages: u64,
    pub searches: u64,
    pub ms: u64,
    /// The worker's slice of the run's budget.
    #[serde(default)]
    pub budget: Option<WorkerBudget>,
}

/// A run's source and budget ledgers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    pub sources: Vec<SourceEntry>,
    pub steps: Vec<BudgetEntry>,
    pub lanes: Vec<LaneTotal>,
}

/// Counts for a one-line summary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCounts {
    pub considered: usize,
    pub read: usize,
    pub cache_hits: usize,
    pub skipped: usize,
    pub failed: usize,
}

impl SourceCounts {
    /// "14 considered · 6 read (2 from cache) · 7 skipped · 1 failed".
    pub fn line(&self) -> String {
        let mut parts = vec![format!("{} considered", self.considered)];
        let mut read = format!("{} read", self.read);
        if self.cache_hits > 0 {
            read.push_str(&format!(" ({} from cache)", self.cache_hits));
        }
        parts.push(read);
        if self.skipped > 0 {
            parts.push(format!("{} skipped", self.skipped));
        }
        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        parts.join(" · ")
    }
}

impl Ledger {
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.steps.is_empty()
    }

    pub fn counts(&self) -> SourceCounts {
        let mut c = SourceCounts {
            considered: self.sources.len(),
            ..SourceCounts::default()
        };
        for s in &self.sources {
            match s.fate {
                SourceFate::Read => {
                    c.read += 1;
                    if s.reason == LedgerReason::CacheHit {
                        c.cache_hits += 1;
                    }
                }
                SourceFate::Skipped => c.skipped += 1,
                SourceFate::Failed => c.failed += 1,
            }
        }
        c
    }

    /// The two ledgers as Markdown sections.
    pub fn to_markdown(&self) -> String {
        let mut md = String::from("\n## Source ledger\n\n");
        if self.sources.is_empty() {
            md.push_str("No URLs were considered.\n");
        } else {
            md.push_str(&format!("{}\n\n", self.counts().line()));
            md.push_str(
                "| URL | Worker | Fate | Why | Detail |\n| --- | --- | --- | --- | --- |\n",
            );
            for s in &self.sources {
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    cell(&s.url),
                    s.worker
                        .map(|w| w.to_string())
                        .unwrap_or_else(|| "–".into()),
                    s.fate.label(),
                    s.reason.label(),
                    cell(&s.detail)
                ));
            }
        }
        md.push_str("\n## Budget ledger\n\n");
        if self.lanes.is_empty() {
            md.push_str("Nothing was spent.\n");
            return md;
        }
        md.push_str("| Lane | Model calls | Tokens in | Tokens out | Pages | Searches | Time |\n| --- | --- | --- | --- | --- | --- | --- |\n");
        for l in &self.lanes {
            let of = l
                .budget
                .map(|b| format!(" (of {} tokens, {} pages)", b.tokens, b.pages))
                .unwrap_or_default();
            md.push_str(&format!(
                "| {}{} | {} | {} | {} | {} | {} | {:.1}s |\n",
                cell(&l.label),
                of,
                l.model_calls,
                l.tokens_in,
                l.tokens_out,
                l.pages,
                l.searches,
                l.ms as f64 / 1000.0
            ));
        }
        md.push_str("\n| Time | Lane | Step | Tokens | Pages | Took |\n| --- | --- | --- | --- | --- | --- |\n");
        for s in self.steps.iter().filter(|s| s.kind != StepUse::Worker) {
            md.push_str(&format!(
                "| {:.1}s | {} | {} | {} | {} | {:.1}s |\n",
                s.at_ms as f64 / 1000.0,
                s.worker
                    .map(|w| format!("worker {w}"))
                    .unwrap_or_else(|| "run".into()),
                cell(&s.step),
                s.tokens_in + s.tokens_out,
                s.pages,
                s.ms as f64 / 1000.0
            ));
        }
        md
    }
}

/// Settles each URL's fate from the facts a run logged, in order.
#[derive(Clone, Debug, Default)]
pub struct LedgerBuilder {
    sources: Vec<SourceEntry>,
    steps: Vec<BudgetEntry>,
    /// Each lane's latest step time.
    lane_at: Vec<(Option<usize>, u64)>,
    budgets: Vec<(usize, WorkerBudget, String, u64)>,
}

/// A URL's identity in the ledger: no fragment, no trailing slash.
pub fn url_key(url: &str) -> String {
    let u = url.split('#').next().unwrap_or(url).trim();
    u.trim_end_matches('/').to_lowercase()
}

impl LedgerBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn find(&mut self, url: &str, worker: Option<usize>) -> Option<&mut SourceEntry> {
        let key = url_key(url);
        self.sources
            .iter_mut()
            .find(|s| s.worker == worker && url_key(&s.url) == key)
    }

    fn read_elsewhere(&self, url: &str, worker: Option<usize>) -> bool {
        let key = url_key(url);
        self.sources
            .iter()
            .any(|s| s.worker != worker && s.fate == SourceFate::Read && url_key(&s.url) == key)
    }

    /// A search result: considered, not read yet.
    pub fn considered(
        &mut self,
        url: &str,
        site: &str,
        title: &str,
        worker: Option<usize>,
        query: &str,
        at_ms: u64,
    ) {
        if url.is_empty() || self.find(url, worker).is_some() {
            return;
        }
        self.sources.push(SourceEntry {
            url: url.to_owned(),
            site: site.to_owned(),
            title: title.to_owned(),
            worker,
            fate: SourceFate::Skipped,
            reason: LedgerReason::NotPicked,
            detail: if query.is_empty() {
                String::new()
            } else {
                format!("from “{query}”")
            },
            at_ms,
            paper: None,
        });
    }

    /// Attaches a scholarly paper's metadata to the source at `url`.
    pub fn paper(&mut self, url: &str, worker: Option<usize>, paper: crate::scholar::Paper) {
        if let Some(e) = self.find(url, worker) {
            match &mut e.paper {
                Some(p) => p.absorb(paper),
                None => e.paper = Some(paper),
            }
        }
    }

    /// A paper read through its PDF: the row its search listed (the same
    /// DOI, arXiv id or title, under another URL) becomes the read, so one
    /// paper is one row; else the read gets a row of its own.
    #[allow(clippy::too_many_arguments)]
    pub fn read_paper(
        &mut self,
        url: &str,
        site: &str,
        title: &str,
        worker: Option<usize>,
        cache_hit: bool,
        at_ms: u64,
        paper: crate::scholar::Paper,
    ) {
        if !url.is_empty() && self.find(url, worker).is_none() {
            let keys = paper.keys();
            if let Some(e) = self.sources.iter_mut().find(|s| {
                s.worker == worker
                    && s.fate != SourceFate::Read
                    && s.paper
                        .as_ref()
                        .is_some_and(|p| p.keys().iter().any(|k| keys.contains(k)))
            }) {
                e.url = url.to_owned();
            }
        }
        self.read(url, site, title, worker, cache_hit, at_ms);
        self.paper(url, worker, paper);
    }

    /// A page read, from the cache or live.
    pub fn read(
        &mut self,
        url: &str,
        site: &str,
        title: &str,
        worker: Option<usize>,
        cache_hit: bool,
        at_ms: u64,
    ) {
        let reason = if cache_hit {
            LedgerReason::CacheHit
        } else {
            LedgerReason::Live
        };
        self.settle(
            url,
            site,
            title,
            worker,
            SourceFate::Read,
            reason,
            None,
            at_ms,
        );
    }

    /// A read that failed; robots.txt refusals are told apart.
    pub fn failed(&mut self, url: &str, worker: Option<usize>, error: &str, at_ms: u64) {
        let reason = if error.contains("robots.txt") {
            LedgerReason::Robots
        } else if error.to_lowercase().contains("budget") {
            LedgerReason::Budget
        } else {
            LedgerReason::Error
        };
        let fate = if reason == LedgerReason::Robots {
            SourceFate::Skipped
        } else {
            SourceFate::Failed
        };
        let site = site_of(url);
        self.settle(url, &site, "", worker, fate, reason, Some(error), at_ms);
    }

    /// A URL left unread on purpose.
    pub fn skipped(
        &mut self,
        url: &str,
        site: &str,
        title: &str,
        worker: Option<usize>,
        reason: LedgerReason,
        at_ms: u64,
    ) {
        if let Some(e) = self.find(url, worker)
            && e.fate != SourceFate::Skipped
        {
            return;
        }
        self.settle(
            url,
            site,
            title,
            worker,
            SourceFate::Skipped,
            reason,
            None,
            at_ms,
        );
    }

    /// A page a resumed run brought back from its checkpoint.
    pub fn restored(&mut self, url: &str, site: &str, title: &str, worker: Option<usize>) {
        self.settle(
            url,
            site,
            title,
            worker,
            SourceFate::Read,
            LedgerReason::Restored,
            None,
            0,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn settle(
        &mut self,
        url: &str,
        site: &str,
        title: &str,
        worker: Option<usize>,
        fate: SourceFate,
        reason: LedgerReason,
        detail: Option<&str>,
        at_ms: u64,
    ) {
        if url.is_empty() {
            return;
        }
        match self.find(url, worker) {
            Some(e) => {
                // A read outranks a skip; nothing outranks a read.
                if e.fate == SourceFate::Read && fate != SourceFate::Read {
                    return;
                }
                e.fate = fate;
                e.reason = reason;
                if !site.is_empty() {
                    e.site = site.to_owned();
                }
                if !title.is_empty() {
                    e.title = title.to_owned();
                }
                if let Some(d) = detail {
                    e.detail = d.to_owned();
                }
                e.at_ms = at_ms.max(e.at_ms);
            }
            None => self.sources.push(SourceEntry {
                url: url.to_owned(),
                site: site.to_owned(),
                title: title.to_owned(),
                worker,
                fate,
                reason,
                detail: detail.unwrap_or_default().to_owned(),
                at_ms,
                paper: None,
            }),
        }
    }

    /// One step's use. `ms` is worked out from the lane's previous step.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        step: &str,
        kind: StepUse,
        worker: Option<usize>,
        tokens: (u64, u64),
        pages: u64,
        searches: u64,
        at_ms: u64,
    ) {
        let prev = match self.lane_at.iter_mut().find(|(w, _)| *w == worker) {
            Some((_, at)) => std::mem::replace(at, at_ms),
            None => {
                self.lane_at.push((worker, at_ms));
                // A worker's first step is timed from when it started.
                self.budgets
                    .iter()
                    .find(|b| Some(b.0) == worker)
                    .map(|b| b.3)
                    .unwrap_or(0)
            }
        };
        self.steps.push(BudgetEntry {
            step: step.to_owned(),
            kind,
            worker,
            tokens_in: tokens.0,
            tokens_out: tokens.1,
            pages,
            searches,
            ms: at_ms.saturating_sub(prev),
            at_ms,
        });
    }

    /// A worker started with its slice of the budget.
    pub fn worker(&mut self, n: usize, question: &str, budget: Option<WorkerBudget>, at_ms: u64) {
        if self.budgets.iter().any(|b| b.0 == n) {
            return;
        }
        self.budgets
            .push((n, budget.unwrap_or_default(), question.to_owned(), at_ms));
    }

    /// The ledgers, with duplicates (a URL another worker had read) and
    /// lane totals settled. `took_ms` is each worker's own time, when its
    /// end was logged.
    pub fn finish(mut self, run_ms: u64, took: &[(usize, u64)]) -> Ledger {
        let unread: Vec<usize> = (0..self.sources.len())
            .filter(|&i| {
                self.sources[i].fate == SourceFate::Skipped
                    && self.sources[i].reason == LedgerReason::NotPicked
            })
            .collect();
        for i in unread {
            let (url, worker) = (self.sources[i].url.clone(), self.sources[i].worker);
            if self.read_elsewhere(&url, worker) {
                self.sources[i].reason = LedgerReason::Duplicate;
            }
        }
        let mut lanes: Vec<LaneTotal> = Vec::new();
        let mut workers: Vec<Option<usize>> = vec![None];
        let mut ns: Vec<usize> = self.budgets.iter().map(|b| b.0).collect();
        ns.extend(self.steps.iter().filter_map(|s| s.worker));
        ns.sort_unstable();
        ns.dedup();
        workers.extend(ns.into_iter().map(Some));
        for w in workers {
            let mut lane = LaneTotal {
                worker: w,
                label: match w {
                    None => "Run".to_owned(),
                    Some(n) => {
                        let q = self
                            .budgets
                            .iter()
                            .find(|b| b.0 == n)
                            .map(|b| b.2.clone())
                            .unwrap_or_default();
                        if q.is_empty() {
                            format!("Worker {n}")
                        } else {
                            format!("Worker {n}: {q}")
                        }
                    }
                },
                budget: w.and_then(|n| {
                    self.budgets
                        .iter()
                        .find(|b| b.0 == n)
                        .map(|b| b.1)
                        .filter(|b| b.tokens > 0)
                }),
                ..LaneTotal::default()
            };
            for s in self.steps.iter().filter(|s| s.worker == w) {
                if s.kind == StepUse::Model {
                    lane.model_calls += 1;
                }
                lane.tokens_in += s.tokens_in;
                lane.tokens_out += s.tokens_out;
                lane.pages += s.pages;
                lane.searches += s.searches;
            }
            lane.ms = match w {
                None => run_ms,
                Some(n) => took
                    .iter()
                    .find(|t| t.0 == n)
                    .map(|t| t.1)
                    .unwrap_or_else(|| {
                        self.steps
                            .iter()
                            .filter(|s| s.worker == w)
                            .map(|s| s.ms)
                            .sum()
                    }),
            };
            if w.is_none()
                || lane.model_calls + lane.pages + lane.searches > 0
                || lane.budget.is_some()
            {
                lanes.push(lane);
            }
        }
        Ledger {
            sources: self.sources,
            steps: self.steps,
            lanes,
        }
    }
}

fn site_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    host.trim_start_matches("www.").to_lowercase()
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paper_read_settles_the_row_its_search_listed() {
        let mut b = LedgerBuilder::new();
        let listed = crate::scholar::Paper {
            title: "Active Retrieval Augmented Generation".into(),
            doi: Some("10.18653/v1/2023.emnlp-main.495".into()),
            arxiv: Some("2305.06983".into()),
            ..crate::scholar::Paper::default()
        };
        let url = "https://doi.org/10.18653/v1/2023.emnlp-main.495";
        b.considered(url, "doi.org", &listed.title, None, "q", 10);
        b.paper(url, None, listed.clone());
        let read = crate::scholar::Paper {
            arxiv: Some("2305.06983".into()),
            ..listed
        };
        b.read_paper(
            "https://arxiv.org/pdf/2305.06983",
            "arxiv.org",
            "Active Retrieval Augmented Generation",
            None,
            false,
            20,
            read,
        );
        let l = b.finish(30, &[]);
        assert_eq!(l.sources.len(), 1, "{:?}", l.sources);
        assert_eq!(l.sources[0].fate, SourceFate::Read);
        assert_eq!(l.sources[0].url, "https://arxiv.org/pdf/2305.06983");
        assert!(l.sources[0].paper.is_some());
    }

    #[test]
    fn each_url_settles_on_one_fate() {
        let mut b = LedgerBuilder::new();
        b.worker(1, "alpha", None, 0);
        b.worker(2, "beta", None, 0);
        for (url, w) in [
            ("https://a.test/1", 1),
            ("https://b.test/1", 1),
            ("https://c.test/1", 1),
            ("https://a.test/1/", 2),
            ("https://d.test/1", 2),
        ] {
            b.considered(url, "", "", Some(w), "q", 10);
        }
        b.read("https://a.test/1", "a.test", "A", Some(1), true, 20);
        b.failed(
            "https://b.test/1",
            Some(1),
            "b.test's robots.txt asks automated readers not to open this page",
            25,
        );
        b.failed("https://c.test/1#x", Some(1), "timed out", 30);
        b.skipped(
            "https://d.test/1",
            "d.test",
            "",
            Some(2),
            LedgerReason::Budget,
            40,
        );
        // A later skip does not undo a read.
        b.skipped(
            "https://a.test/1",
            "",
            "",
            Some(1),
            LedgerReason::Budget,
            50,
        );
        let l = b.finish(100, &[]);
        let fate = |url: &str, w: usize| {
            l.sources
                .iter()
                .find(|s| url_key(&s.url) == url_key(url) && s.worker == Some(w))
                .map(|s| (s.fate, s.reason))
                .unwrap()
        };
        assert_eq!(
            fate("https://a.test/1", 1),
            (SourceFate::Read, LedgerReason::CacheHit)
        );
        assert_eq!(
            fate("https://b.test/1", 1),
            (SourceFate::Skipped, LedgerReason::Robots)
        );
        assert_eq!(
            fate("https://c.test/1", 1),
            (SourceFate::Failed, LedgerReason::Error)
        );
        assert_eq!(
            fate("https://a.test/1", 2),
            (SourceFate::Skipped, LedgerReason::Duplicate),
            "worker 1 had read it"
        );
        assert_eq!(
            fate("https://d.test/1", 2),
            (SourceFate::Skipped, LedgerReason::Budget)
        );
        let c = l.counts();
        assert_eq!(
            (c.considered, c.read, c.cache_hits, c.skipped, c.failed),
            (5, 1, 1, 3, 1)
        );
        assert_eq!(
            c.line(),
            "5 considered · 1 read (1 from cache) · 3 skipped · 1 failed"
        );
    }

    #[test]
    fn steps_are_timed_per_lane_and_totalled() {
        let mut b = LedgerBuilder::new();
        let budget = WorkerBudget {
            tokens: 9_000,
            pages: 3,
            seconds: 60,
        };
        b.step("plan", StepUse::Model, None, (300, 100), 0, 0, 500);
        b.worker(1, "alpha", Some(budget), 600);
        b.step("alpha", StepUse::Search, Some(1), (0, 0), 0, 1, 900);
        b.step(
            "https://a.test",
            StepUse::Read,
            Some(1),
            (0, 0),
            1,
            0,
            1_400,
        );
        b.step("notes", StepUse::Model, Some(1), (1_000, 100), 0, 0, 3_400);
        b.step("answer", StepUse::Model, None, (2_000, 400), 0, 0, 5_000);
        let l = b.finish(5_200, &[(1, 2_900)]);
        let ms: Vec<u64> = l.steps.iter().map(|s| s.ms).collect();
        assert_eq!(ms, [500, 300, 500, 2_000, 4_500]);
        assert_eq!(l.lanes.len(), 2);
        let run = &l.lanes[0];
        assert_eq!((run.model_calls, run.tokens_in, run.ms), (2, 2_300, 5_200));
        let w = &l.lanes[1];
        assert_eq!(w.label, "Worker 1: alpha");
        assert_eq!((w.model_calls, w.pages, w.searches, w.ms), (1, 1, 1, 2_900));
        assert_eq!(w.budget, Some(budget));
        let md = l.to_markdown();
        assert!(md.contains("## Source ledger"));
        assert!(md.contains("## Budget ledger"));
        assert!(md.contains("Worker 1: alpha (of 9000 tokens, 3 pages)"));
    }

    #[test]
    fn ledgers_round_trip() {
        let mut b = LedgerBuilder::new();
        b.restored("https://a.test", "a.test", "A", Some(1));
        let l = b.finish(0, &[]);
        let back: Ledger = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
        assert_eq!(back.sources[0].reason, LedgerReason::Restored);
    }
}
