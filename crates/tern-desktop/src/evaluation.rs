//! The Evaluation page's state: saved runs, the one shown, the run before
//! it, and the diff between them. GPUI-free, so it is tested like the rest
//! of the model.

use tern_agent::eval::{Change, Comparison, EvalRun, RunSummary, compare};

/// The page's state.
#[derive(Clone, Debug, Default)]
pub struct EvalPage {
    pub loaded: bool,
    pub runs: Vec<RunSummary>,
    pub selected: Option<EvalRun>,
    /// The run before the selected one on the same rung and mode.
    pub previous: Option<EvalRun>,
    pub diff: Option<Comparison>,
    pub running: bool,
    /// The last run's result line, or why it failed.
    pub note: Option<Result<String, String>>,
    /// Suites whose failures are shown, by id.
    pub open: Vec<String>,
}

impl EvalPage {
    pub fn load(
        &mut self,
        runs: Vec<RunSummary>,
        selected: Option<EvalRun>,
        previous: Option<EvalRun>,
    ) {
        self.loaded = true;
        self.runs = runs;
        self.diff = match (&previous, &selected) {
            (Some(p), Some(s)) => Some(compare(p, s)),
            _ => None,
        };
        self.selected = selected;
        self.previous = previous;
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_ref().map(|r| r.id.as_str())
    }

    /// How one metric moved against the previous run.
    pub fn change(&self, suite: &str, metric: &str) -> Option<(Change, String)> {
        let d = self.diff.as_ref()?;
        let row = d
            .rows
            .iter()
            .find(|r| r.suite == suite && r.metric == metric)?;
        Some((row.change, row.old.clone().unwrap_or_default()))
    }

    pub fn regressions(&self) -> usize {
        self.diff.as_ref().map(Comparison::regressions).unwrap_or(0)
    }

    pub fn toggle(&mut self, suite: &str) {
        if let Some(i) = self.open.iter().position(|s| s == suite) {
            self.open.remove(i);
        } else {
            self.open.push(suite.to_owned());
        }
    }

    pub fn is_open(&self, suite: &str) -> bool {
        self.open.iter().any(|s| s == suite)
    }

    pub fn finished(&mut self, result: Result<String, String>) {
        self.running = false;
        self.note = Some(result);
    }
}

/// `23 Oct 14:30` from an RFC 3339 start time (UTC).
pub fn short_time(rfc3339: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = rfc3339.get(..10).unwrap_or_default();
    let time = rfc3339.get(11..16).unwrap_or_default();
    let mut parts = date.split('-');
    let (_, m, d) = (parts.next(), parts.next(), parts.next());
    match (m.and_then(|m| m.parse::<usize>().ok()), d) {
        (Some(m), Some(d)) if (1..=12).contains(&m) => {
            format!("{} {} {time} UTC", d.trim_start_matches('0'), MONTHS[m - 1])
        }
        _ => rfc3339.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_short() {
        assert_eq!(short_time("2026-10-23T14:30:05Z"), "23 Oct 14:30 UTC");
        assert_eq!(short_time("nonsense"), "nonsense");
    }

    #[test]
    fn toggling_opens_and_closes_failures() {
        let mut p = EvalPage::default();
        p.toggle("claims");
        assert!(p.is_open("claims"));
        p.toggle("claims");
        assert!(!p.is_open("claims"));
        assert_eq!(p.regressions(), 0);
        assert!(p.change("claims", "x").is_none());
    }
}
