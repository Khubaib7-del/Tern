//! An evaluation run's results: metrics with intervals, every failure,
//! Markdown and JSON, comparison between runs, and the baseline check.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::stats::{mean, t_interval, wilson};
use crate::suite::{Level, SuiteKind};

/// The report format this build writes.
pub const REPORT_FORMAT: u32 = 1;

/// How a metric is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// 0 to 1, shown as a percentage.
    #[default]
    Ratio,
    /// Per 100 runs.
    PerHundred,
    Tokens,
    Usd,
    Ms,
    /// A plain number (a Brier score, a SUS score).
    Number,
}

/// Which way is good.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Better {
    #[default]
    Higher,
    Lower,
}

/// One metric: a value per trial, their mean, and a 95% interval.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub key: String,
    pub label: String,
    pub unit: Unit,
    pub better: Better,
    /// One value per trial.
    pub trials: Vec<f64>,
    pub mean: f64,
    /// The 95% interval: for a proportion, the wider of the Wilson interval
    /// over every case and trial and the t interval over trial values; for
    /// other metrics, the t interval over trial values.
    pub low: f64,
    pub high: f64,
    /// Successes and attempts across every trial, for proportions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl Metric {
    /// A proportion from per-trial (successes, attempts). Trials with no
    /// attempts are left out; with none at all the metric is `None`.
    pub fn ratio(key: &str, label: &str, better: Better, per_trial: &[(u64, u64)]) -> Option<Self> {
        let used: Vec<(u64, u64)> = per_trial.iter().copied().filter(|(_, n)| *n > 0).collect();
        if used.is_empty() {
            return None;
        }
        let trials: Vec<f64> = used.iter().map(|(k, n)| *k as f64 / *n as f64).collect();
        let k: u64 = used.iter().map(|x| x.0).sum();
        let n: u64 = used.iter().map(|x| x.1).sum();
        let (wl, wh) = wilson(k, n);
        let (tl, th) = t_interval(&trials);
        Some(Self {
            key: key.into(),
            label: label.into(),
            unit: Unit::Ratio,
            better,
            mean: k as f64 / n as f64,
            low: wl.min(tl).max(0.0),
            high: wh.max(th).min(1.0),
            trials,
            k: Some(k),
            n: Some(n),
            note: String::new(),
        })
    }

    /// A value per trial (tokens, error, a rate per 100 runs).
    pub fn values(key: &str, label: &str, unit: Unit, better: Better, trials: Vec<f64>) -> Option<Self> {
        if trials.is_empty() || trials.iter().any(|x| !x.is_finite()) {
            return None;
        }
        let (low, high) = t_interval(&trials);
        Some(Self {
            key: key.into(),
            label: label.into(),
            unit,
            better,
            mean: mean(&trials),
            low,
            high,
            trials,
            k: None,
            n: None,
            note: String::new(),
        })
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }

    /// The value as a reader sees it: `86.0%`, `12.5 per 100 runs`.
    pub fn show(&self, x: f64) -> String {
        match self.unit {
            Unit::Ratio => format!("{:.1}%", x * 100.0),
            Unit::PerHundred => format!("{x:.1} per 100 runs"),
            Unit::Tokens => format!("{} tokens", x.round() as i64),
            Unit::Usd => format!("${x:.4}"),
            Unit::Ms => format!("{} ms", x.round() as i64),
            Unit::Number => format!("{x:.3}"),
        }
    }

    /// `86.0% (95% CI 78.4%–91.2%; 129/150)`.
    pub fn line(&self) -> String {
        let mut s = self.show(self.mean);
        if (self.high - self.low).abs() > 1e-12 {
            s.push_str(&format!(
                " (95% CI {}–{})",
                self.show(self.low),
                self.show(self.high)
            ));
        }
        if let (Some(k), Some(n)) = (self.k, self.n) {
            s.push_str(&format!(" · {k}/{n}"));
        }
        s
    }
}

/// One case that did not do what it should, in one trial.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub case: String,
    /// 1-based.
    pub trial: usize,
    pub expected: String,
    pub got: String,
    #[serde(default)]
    pub why: String,
}

/// One suite's results across every trial.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SuiteResult {
    pub id: String,
    pub version: u32,
    pub title: String,
    pub kind: SuiteKind,
    pub level: Level,
    pub origin: String,
    #[serde(default)]
    pub split: Option<String>,
    pub cases: usize,
    pub trials: usize,
    /// Case-trials that passed, out of `attempted`.
    pub passed: usize,
    pub attempted: usize,
    /// Case-trials not run, with why (a canary case offline).
    pub skipped: usize,
    #[serde(default)]
    pub skip_reason: String,
    /// The suite's headline metric key.
    pub headline: String,
    pub metrics: Vec<Metric>,
    pub failures: Vec<Failure>,
    /// Model calls made, with measured or estimated tokens.
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub latency_ms: u64,
}

impl SuiteResult {
    pub fn metric(&self, key: &str) -> Option<&Metric> {
        self.metrics.iter().find(|m| m.key == key)
    }

    pub fn headline(&self) -> Option<&Metric> {
        self.metric(&self.headline)
    }

    /// `research@1`.
    pub fn tag(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }
}

/// An evaluation run: one rung of the ladder, one subject, N trials.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvalRun {
    pub format: u32,
    pub id: String,
    /// RFC 3339, UTC.
    pub started: String,
    #[serde(default)]
    pub seconds: f64,
    pub level: Level,
    /// `offline` (recorded replies and Tern's offline checks) or `live`.
    pub mode: String,
    /// Who answered: `offline` or `provider · model`.
    pub subject: String,
    /// The provider id, for its logo; `None` offline.
    #[serde(default)]
    pub provider: Option<String>,
    pub trials: usize,
    pub tern_version: String,
    pub suites: Vec<SuiteResult>,
    /// Plain notes: fewer than three trials, a page that drifted.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl EvalRun {
    pub fn suite(&self, id: &str) -> Option<&SuiteResult> {
        self.suites.iter().find(|s| s.id == id)
    }

    pub fn passed(&self) -> (usize, usize) {
        self.suites
            .iter()
            .fold((0, 0), |(p, a), s| (p + s.passed, a + s.attempted))
    }

    pub fn failures(&self) -> usize {
        self.suites.iter().map(|s| s.failures.len()).sum()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("an eval run serialises")
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let run: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if run.format != REPORT_FORMAT {
            return Err(format!(
                "report format {} (this Tern reads {REPORT_FORMAT})",
                run.format
            ));
        }
        Ok(run)
    }

    /// The report a person reads: every metric with its interval, and every
    /// failure, published in full.
    pub fn to_markdown(&self) -> String {
        let (p, a) = self.passed();
        let mut out = format!(
            "# Tern evaluation {}\n\n| | |\n| --- | --- |\n| Level | {} |\n| Mode | {} |\n| Subject | {} |\n| Trials | {} |\n| Started | {} |\n| Took | {:.1} s |\n| Tern | {} |\n| Passed | {p}/{a} case-trials |\n",
            self.id,
            self.level.label(),
            self.mode,
            self.subject,
            self.trials,
            self.started,
            self.seconds,
            self.tern_version,
        );
        for n in &self.notes {
            out.push_str(&format!("\n> {n}\n"));
        }
        for s in &self.suites {
            out.push_str(&format!(
                "\n## {} (`{}`)\n\n{} cases × {} trials · {}/{} passed",
                s.title,
                s.tag(),
                s.cases,
                s.trials,
                s.passed,
                s.attempted
            ));
            if s.skipped > 0 {
                out.push_str(&format!(" · {} skipped ({})", s.skipped, s.skip_reason));
            }
            out.push_str(&format!("\n\nOrigin: {}\n", s.origin));
            if let Some(split) = &s.split {
                out.push_str(&format!("Split: {split}\n"));
            }
            out.push_str("\n| Metric | Value | Per trial |\n| --- | --- | --- |\n");
            for m in &s.metrics {
                let per: Vec<String> = m.trials.iter().map(|x| m.show(*x)).collect();
                let label = if m.note.is_empty() {
                    m.label.clone()
                } else {
                    format!("{} ({})", m.label, m.note)
                };
                out.push_str(&format!("| {label} | {} | {} |\n", m.line(), per.join(", ")));
            }
            if s.input_tokens + s.output_tokens > 0 {
                out.push_str(&format!(
                    "\nModel use: {} input and {} output tokens, {} ms.\n",
                    s.input_tokens, s.output_tokens, s.latency_ms
                ));
            }
            if s.failures.is_empty() {
                out.push_str("\nNo failures.\n");
            } else {
                out.push_str(&format!(
                    "\n### Failures ({})\n\n| Case | Trial | Expected | Got | Why |\n| --- | --- | --- | --- | --- |\n",
                    s.failures.len()
                ));
                for f in &s.failures {
                    out.push_str(&format!(
                        "| {} | {} | {} | {} | {} |\n",
                        f.case,
                        f.trial,
                        cell(&f.expected),
                        cell(&f.got),
                        cell(&f.why)
                    ));
                }
            }
        }
        out
    }
}

fn cell(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let one = one.replace('|', "\\|");
    if one.chars().count() > 160 {
        format!("{}…", one.chars().take(160).collect::<String>())
    } else {
        one
    }
}

/// A short line per run, for lists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub started: String,
    pub level: Level,
    pub mode: String,
    pub subject: String,
    pub provider: Option<String>,
    pub trials: usize,
    pub passed: usize,
    pub attempted: usize,
    pub failures: usize,
    /// Each suite's headline: (suite id, metric label, value line).
    pub headlines: Vec<(String, String, String)>,
}

impl From<&EvalRun> for RunSummary {
    fn from(r: &EvalRun) -> Self {
        let (passed, attempted) = r.passed();
        Self {
            id: r.id.clone(),
            started: r.started.clone(),
            level: r.level,
            mode: r.mode.clone(),
            subject: r.subject.clone(),
            provider: r.provider.clone(),
            trials: r.trials,
            passed,
            attempted,
            failures: r.failures(),
            headlines: r
                .suites
                .iter()
                .filter_map(|s| {
                    s.headline()
                        .map(|m| (s.id.clone(), m.label.clone(), m.show(m.mean)))
                })
                .collect(),
        }
    }
}

/// How one metric moved between two runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Improved,
    Same,
    Regressed,
    /// Only in the newer run.
    New,
    /// Only in the older run.
    Gone,
}

impl Change {
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Improved => "▲",
            Self::Same => "=",
            Self::Regressed => "▼",
            Self::New => "+",
            Self::Gone => "–",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    pub suite: String,
    pub metric: String,
    pub label: String,
    pub old: Option<String>,
    pub new: Option<String>,
    /// New minus old, in the metric's unit.
    pub delta: f64,
    pub change: Change,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub old: String,
    pub new: String,
    pub rows: Vec<Delta>,
}

impl Comparison {
    pub fn regressions(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.change == Change::Regressed)
            .count()
    }

    pub fn to_markdown(&self) -> String {
        let mut out = format!(
            "# {} → {}\n\n{} regression{}\n\n| Suite | Metric | Before | After | Change |\n| --- | --- | --- | --- | --- |\n",
            self.old,
            self.new,
            self.regressions(),
            if self.regressions() == 1 { "" } else { "s" }
        );
        for r in &self.rows {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                r.suite,
                r.label,
                r.old.as_deref().unwrap_or("–"),
                r.new.as_deref().unwrap_or("–"),
                r.change.glyph()
            ));
        }
        out
    }
}

/// How far a metric may move before it counts: 2 points for a proportion,
/// 5% of the old value (and at least 0.005) otherwise.
pub fn tolerance(m: &Metric) -> f64 {
    match m.unit {
        Unit::Ratio => 0.02,
        _ => (m.mean.abs() * 0.05).max(0.005),
    }
}

/// Compares `new` with `old`, metric by metric. A worse value beyond the
/// tolerance, and outside the older run's interval, is a regression.
pub fn compare(old: &EvalRun, new: &EvalRun) -> Comparison {
    let mut rows = Vec::new();
    let key = |s: &SuiteResult, m: &Metric| (s.id.clone(), m.key.clone());
    let olds: BTreeMap<(String, String), &Metric> = old
        .suites
        .iter()
        .flat_map(|s| s.metrics.iter().map(move |m| (key(s, m), m)))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for s in &new.suites {
        for m in &s.metrics {
            let k = key(s, m);
            seen.insert(k.clone());
            let row = match olds.get(&k) {
                None => Delta {
                    suite: s.id.clone(),
                    metric: m.key.clone(),
                    label: m.label.clone(),
                    old: None,
                    new: Some(m.show(m.mean)),
                    delta: 0.0,
                    change: Change::New,
                },
                Some(o) => {
                    let d = m.mean - o.mean;
                    let worse = match m.better {
                        Better::Higher => d < 0.0,
                        Better::Lower => d > 0.0,
                    };
                    let outside = m.mean < o.low - 1e-12 || m.mean > o.high + 1e-12;
                    let moved = d.abs() > tolerance(o);
                    let change = if !moved {
                        Change::Same
                    } else if worse && (outside || (o.high - o.low).abs() < 1e-12) {
                        Change::Regressed
                    } else if worse {
                        Change::Same
                    } else {
                        Change::Improved
                    };
                    Delta {
                        suite: s.id.clone(),
                        metric: m.key.clone(),
                        label: m.label.clone(),
                        old: Some(o.show(o.mean)),
                        new: Some(m.show(m.mean)),
                        delta: d,
                        change,
                    }
                }
            };
            rows.push(row);
        }
    }
    for ((suite, metric), o) in &olds {
        if !seen.contains(&(suite.clone(), metric.clone())) {
            rows.push(Delta {
                suite: suite.clone(),
                metric: metric.clone(),
                label: o.label.clone(),
                old: Some(o.show(o.mean)),
                new: None,
                delta: 0.0,
                change: Change::Gone,
            });
        }
    }
    Comparison {
        old: old.id.clone(),
        new: new.id.clone(),
        rows,
    }
}

/// The floor the offline suites must stay above: `suite id → metric key →
/// value`, read in the metric's own direction (a floor for higher-is-better,
/// a ceiling for lower-is-better).
pub type Baseline = BTreeMap<String, BTreeMap<String, f64>>;

const BASELINE: &str = include_str!("../baseline.json");

/// The baseline bundled with this build, set from the first offline run.
pub fn baseline() -> Baseline {
    serde_json::from_str(BASELINE).expect("the bundled baseline parses")
}

/// Every baseline line `run` breaks, in plain words.
pub fn check_baseline(run: &EvalRun, base: &Baseline) -> Vec<String> {
    let mut out = Vec::new();
    for (suite, metrics) in base {
        let Some(s) = run.suite(suite) else {
            continue;
        };
        for (key, floor) in metrics {
            let Some(m) = s.metric(key) else {
                out.push(format!("{suite}: {key} is missing from the run"));
                continue;
            };
            let bad = match m.better {
                Better::Higher => m.mean + 1e-9 < *floor,
                Better::Lower => m.mean - 1e-9 > *floor,
            };
            if bad {
                out.push(format!(
                    "{suite}: {} is {}, the baseline is {}",
                    m.label,
                    m.show(m.mean),
                    m.show(*floor)
                ));
            }
        }
    }
    out
}
