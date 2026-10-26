//! How an answer unfolds in the transcript, kept apart from GPUI so it can
//! be tested without a display: the Thinking section, the working rollup,
//! the paced reveal, hover times, grouped message actions, whether Explain
//! in 3D fits, watch toggles with undo, and stacked notices.

use std::time::{Duration, Instant};

use tern_agent::Run;

/// The answer's reasoning, once the answer started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thought {
    pub text: String,
    pub took: Duration,
}

impl Thought {
    /// "Thought for 6s".
    pub fn label(&self) -> String {
        format!("Thought for {}", seconds(self.took))
    }
}

/// Reasoning streaming in for the running turn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveThinking {
    pub text: String,
    /// The model call it belongs to; a new call starts over.
    pub call: u32,
    pub started: Option<Instant>,
    /// Set when the answer's first text arrived: the section folds.
    pub took: Option<Duration>,
    /// The user opened it while folded (or closed it while live).
    pub toggled: bool,
}

impl LiveThinking {
    pub fn push(&mut self, call: u32, text: &str) {
        if call != self.call {
            *self = Self {
                call,
                ..Self::default()
            };
        }
        self.started.get_or_insert_with(Instant::now);
        self.text.push_str(text);
    }

    /// Answer text arrived for `call`: thinking is over.
    pub fn answer_started(&mut self, call: u32) {
        if call == self.call && self.took.is_none() {
            if let Some(s) = self.started {
                self.took = Some(s.elapsed());
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Whether the section shows its text: open while thinking, folded
    /// once the answer starts, the other way round when toggled.
    pub fn is_open(&self) -> bool {
        let live = self.took.is_none();
        live != self.toggled
    }

    /// "Thinking…" while live, then "Thought for Ns".
    pub fn label(&self) -> String {
        match self.took {
            None => "Thinking…".into(),
            Some(t) => format!("Thought for {}", seconds(t)),
        }
    }

    /// What the finished answer keeps.
    pub fn finish(&mut self) -> Option<Thought> {
        if self.is_empty() {
            *self = Self::default();
            return None;
        }
        let took = self
            .took
            .or_else(|| self.started.map(|s| s.elapsed()))
            .unwrap_or_default();
        let t = Thought {
            text: std::mem::take(&mut self.text),
            took,
        };
        *self = Self::default();
        Some(t)
    }
}

/// "6s", "1m 05s".
pub fn seconds(d: Duration) -> String {
    let s = d.as_secs().max(1);
    if s < 60 {
        format!("{s}s")
    } else {
        format!("{}m {:02}s", s / 60, s % 60)
    }
}

/// The working rollup's one line: "Searched 3 · read 6 · 3 verified".
/// `None` when the run did no tool work.
pub fn rollup(run: &Run) -> Option<String> {
    // Deep workers announce a search with `tool.started`; a chat turn's
    // engine records only `tool.executed`. Count whichever the run has
    // more of, so a search logged as both started and executed counts once.
    let web = |event: &str| {
        run.steps
            .iter()
            .filter(|s| s.event == event && s.payload["tool"] == "web_search")
            .count()
    };
    let searched = web("tool.started").max(web("tool.executed"));
    let read = run
        .steps
        .iter()
        .filter(|s| s.event == "tool.executed" && s.payload["tool"] == "read_url")
        .count();
    let files = run
        .steps
        .iter()
        .filter(|s| {
            s.event == "tool.executed"
                && s.payload["tool"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("file") || t.starts_with("vault"))
        })
        .count();
    let verified = run
        .checks
        .iter()
        .filter(|c| c.verdict == tern_agent::Verdict::Supported)
        .count();
    // Which service answered, and how many off-topic pages it dropped.
    let searches = run
        .steps
        .iter()
        .filter(|s| s.event == "tool.executed" && s.payload["tool"] == "web_search");
    let mut service: Option<&str> = None;
    let mut dropped = 0;
    for s in searches {
        if let Some(n) = s.payload["meta"]["service"].as_str() {
            service = Some(n);
        }
        dropped += s.payload["meta"]["filtered"].as_u64().unwrap_or(0);
    }
    let mut parts = Vec::new();
    if searched > 0 {
        parts.push(match service {
            Some(n) => format!("Searched {searched} on {n}"),
            None => format!("Searched {searched}"),
        });
        if dropped > 0 {
            parts.push(format!("{dropped} off-topic dropped"));
        }
    }
    if read > 0 {
        parts.push(format!("read {read}"));
    }
    if files > 0 {
        parts.push(format!(
            "{files} file step{}",
            if files == 1 { "" } else { "s" }
        ));
    }
    if !run.checks.is_empty() {
        parts.push(format!("{verified} verified"));
    }
    if parts.is_empty() {
        return None;
    }
    let mut line = parts.join(" · ");
    // Capitalise a line that starts with a lower-case part.
    if let Some(first) = line.get(0..1) {
        line = first.to_uppercase() + &line[1..];
    }
    Some(line)
}

/// Which view the rollup opens on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RollupView {
    #[default]
    Summary,
    Detailed,
    Raw,
}

impl RollupView {
    pub const ALL: [RollupView; 3] = [Self::Summary, Self::Detailed, Self::Raw];
    pub fn label(self) -> &'static str {
        match self {
            Self::Summary => "Summary",
            Self::Detailed => "Detailed",
            Self::Raw => "Raw",
        }
    }
}

/// Reveals streamed text at a reading pace, block by block: headings and
/// list items land whole, paragraphs word by word.
#[derive(Clone, Debug, PartialEq)]
pub struct Pacer {
    /// Bytes of the text shown so far.
    pub shown: usize,
    /// Words allowed so far (grows with time).
    budget: f32,
    pub words_per_sec: f32,
}

impl Default for Pacer {
    fn default() -> Self {
        Self {
            shown: 0,
            budget: 0.,
            words_per_sec: 50.,
        }
    }
}

/// Byte offsets where a reveal may stop, each with the words before it.
fn stops(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut words = 0usize;
    let mut at = 0usize;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        let whole = t.starts_with('#')
            || t.starts_with("- ")
            || t.starts_with("* ")
            || t.starts_with('|')
            || t.starts_with("```")
            || t.split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        let complete = line.ends_with('\n');
        if whole {
            // A heading or list item shows once its line is complete. Its
            // marker (`#`, `-`, `1.`, `|`) is not a word to read.
            let marker = |w: &str| {
                w.chars().all(|c| "#-*|`".contains(c))
                    || w.strip_suffix('.')
                        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            };
            words += line
                .split_whitespace()
                .filter(|w| !marker(w))
                .count()
                .max(1);
            at += line.len();
            if complete {
                out.push((at, words));
            }
            continue;
        }
        let mut pos = at;
        for (i, w) in line.split_inclusive(char::is_whitespace).enumerate() {
            pos += w.len();
            let _ = i;
            if w.trim().is_empty() {
                continue;
            }
            words += 1;
            // A word is only safe to show once whitespace follows it.
            if w.ends_with(char::is_whitespace) {
                out.push((pos, words));
            }
        }
        at += line.len();
    }
    out
}

impl Pacer {
    /// Moves on by `dt`; returns how much of `text` to show. When `done`
    /// (the stream ended), everything is shown once the budget reaches it,
    /// and a long backlog catches up faster.
    pub fn advance(&mut self, text: &str, dt: Duration, done: bool) -> usize {
        let st = stops(text);
        let total_words = text.split_whitespace().count() as f32;
        let backlog = total_words - self.budget;
        let speed = if backlog > 300. {
            self.words_per_sec * 3.
        } else {
            self.words_per_sec
        };
        self.budget += speed * dt.as_secs_f32();
        let allowed = st
            .iter()
            .take_while(|(_, w)| (*w as f32) <= self.budget)
            .last()
            .map(|(at, _)| *at)
            .unwrap_or(0);
        if done && self.budget >= total_words {
            self.shown = text.len();
        } else {
            self.shown = self.shown.max(allowed).min(text.len());
        }
        self.shown
    }

    /// Shows everything now (the user clicked Show all, or reduced motion).
    pub fn finish(&mut self, text: &str) {
        self.shown = text.len();
        self.budget = f32::MAX;
    }

    pub fn caught_up(&self, text: &str) -> bool {
        self.shown >= text.len()
    }
}

/// When a message was sent and how long its answer took, for the hover
/// line. Times are RFC 3339 (UTC); labels use the local clock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub at: Option<String>,
    pub took: Option<Duration>,
}

impl Stamp {
    /// "Sent 9:31 PM".
    pub fn sent(&self, now: &str, offset: Option<i64>) -> Option<String> {
        let at = self.at.as_deref()?;
        let l = tern_agent::checkpoint::clock_label(at, now, offset);
        (!l.is_empty()).then(|| format!("Sent {l}"))
    }

    /// "9:31 PM · Worked for 32s".
    pub fn answered(&self, now: &str, offset: Option<i64>) -> Option<String> {
        let took = self.took.map(|t| format!("Worked for {}", seconds(t)));
        let at = self
            .at
            .as_deref()
            .map(|a| tern_agent::checkpoint::clock_label(a, now, offset))
            .filter(|l| !l.is_empty());
        match (at, took) {
            (Some(a), Some(t)) => Some(format!("{a} · {t}")),
            (Some(a), None) => Some(a),
            (None, Some(t)) => Some(t),
            (None, None) => None,
        }
    }
}

/// The grouped message actions: always-visible ones, then two menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionMenu {
    /// Sub-session, background branch, move, branch a copy, side chat.
    Branch,
    /// Watch, save to vault. (Explain in 3D sits in the row itself.)
    More,
}

/// What the More menu holds for an answer.
pub fn more_items(answer_text: &str, watched: bool) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![("save", "Save to vault")];
    v.push(if watched {
        ("watch", "Stop watching this question")
    } else {
        ("watch", "Watch this question (daily)")
    });
    // Explain in 3D is no longer here: it is in the visible row under
    // every answer.
    let _ = answer_text;
    v
}

/// A removal or unwatch that can still be undone, for a few seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Undo {
    pub label: String,
    pub what: UndoKind,
    pub until: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UndoKind {
    /// A watch removed: its question and how often it ran, to put back.
    Watch {
        question: String,
        every_hours: u32,
        deep: bool,
    },
    /// A chat moved to the trash.
    Chat(tern_core::SessionId),
}

pub const UNDO_FOR: Duration = Duration::from_secs(6);

impl Undo {
    pub fn new(label: impl Into<String>, what: UndoKind) -> Self {
        Self {
            label: label.into(),
            what,
            until: Instant::now() + UNDO_FOR,
        }
    }

    pub fn expired(&self, now: Instant) -> bool {
        now >= self.until
    }
}

/// A banner over the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Notice {
    /// The chat's web search did not answer, so deep research stopped
    /// before any worker started.
    SearchDown,
    /// Wikipedia, the keyless search, had nothing on the question.
    WikiEmpty,
    /// A run stopped and can be resumed or retried.
    Resume,
    /// The answer was cut off at the length limit.
    Continue,
    /// An MCP server wrote the composer text.
    Inserted,
}

/// The nudge in Settings › Search & Crawlers and the first-run tour
/// while only Wikipedia is set up (#67).
pub const TAVILY_NUDGE: &str = "Recommended: add a free Tavily key (1,000 searches a month). Wikipedia only finds encyclopedia articles, so new models, products and niche topics come back empty.";

/// The Wikipedia banner's line (#67).
pub const WIKI_EMPTY_LINE: &str = "Wikipedia has nothing on this. Add a free Tavily key (1,000 searches/month) or Brave under Settings › Search & Crawlers for full-web results.";

/// Whether a run's stop reason says web search is not working (the deep
/// run's preflight, or every worker's search failing).
pub fn is_search_failure(reason: &str) -> bool {
    reason.starts_with("Web search isn't working")
        || reason.starts_with("No search service answered")
}

/// The search banner's line: SearXNG names its address; others their name.
pub fn search_down_line(service: &str, searxng_address: Option<&str>) -> String {
    if service.eq_ignore_ascii_case("searxng") {
        match searxng_address {
            Some(a) => format!("SearXNG isn't reachable at {a}"),
            None => "SearXNG isn't set up: it has no address yet".to_owned(),
        }
    } else {
        format!("{service} isn't answering web searches")
    }
}

/// Which banners to draw: the first by priority, plus the rest when the
/// stack is open. The second value is how many are folded away.
pub fn stack(mut active: Vec<Notice>, open: bool) -> (Vec<Notice>, usize) {
    active.sort();
    active.dedup();
    if active.len() <= 1 || open {
        return (active, 0);
    }
    let rest = active.len() - 1;
    active.truncate(1);
    (active, rest)
}

/// "1 more notice", "2 more notices".
pub fn more_notices(n: usize) -> String {
    format!("{n} more notice{}", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_failures_get_one_clear_banner() {
        assert!(is_search_failure(
            "Web search isn't working, so deep research did not start: connection refused."
        ));
        assert!(is_search_failure(
            "No search service answered: timed out. Fix web search"
        ));
        assert!(!is_search_failure(
            "No worker found a page to cite (1: failed)"
        ));
        assert_eq!(
            search_down_line("SearXNG", Some("http://localhost:8080")),
            "SearXNG isn't reachable at http://localhost:8080"
        );
        assert_eq!(
            search_down_line("SearXNG", None),
            "SearXNG isn't set up: it has no address yet"
        );
        assert_eq!(
            search_down_line("Tavily", None),
            "Tavily isn't answering web searches"
        );
        let (shown, more) = stack(vec![Notice::Resume, Notice::SearchDown], false);
        assert_eq!(
            shown,
            vec![Notice::SearchDown],
            "the search banner comes first"
        );
        assert_eq!(more, 1);
    }
    use tern_core::{EffectClass, EventEnvelope, SessionId};

    #[test]
    fn thinking_opens_live_then_folds() {
        let mut t = LiveThinking::default();
        t.push(1, "Hmm, ");
        t.push(1, "paging.");
        assert!(t.is_open());
        assert_eq!(t.label(), "Thinking…");
        t.answer_started(1);
        assert!(!t.is_open(), "folds when the answer starts");
        assert!(t.label().starts_with("Thought for "));
        t.toggled = true;
        assert!(t.is_open(), "can be reopened");
        let th = t.finish().unwrap();
        assert_eq!(th.text, "Hmm, paging.");
        assert!(th.label().starts_with("Thought for "));
        assert!(t.is_empty());
        // A new model call starts over; empty thinking leaves nothing.
        t.push(1, "a");
        t.push(2, "b");
        assert_eq!(t.text, "b");
        let mut e = LiveThinking::default();
        assert!(e.finish().is_none());
    }

    #[test]
    fn seconds_read_well() {
        assert_eq!(seconds(Duration::from_millis(200)), "1s");
        assert_eq!(seconds(Duration::from_secs(32)), "32s");
        assert_eq!(seconds(Duration::from_secs(65)), "1m 05s");
    }

    fn ev(kind: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope::new(SessionId::new(), kind, EffectClass::Network, kind, payload)
    }

    #[test]
    fn rollup_counts_work() {
        let mut run = Run::new();
        assert!(rollup(&run).is_none());
        run.push(&ev("tool.started", json!({"tool": "web_search"})));
        run.push(&ev("tool.executed", json!({"tool": "web_search"})));
        run.push(&ev("tool.started", json!({"tool": "web_search"})));
        run.push(&ev("tool.executed", json!({"tool": "read_url"})));
        run.push(&ev("tool.executed", json!({"tool": "file_read"})));
        assert_eq!(rollup(&run).unwrap(), "Searched 2 · read 1 · 1 file step");
        run.push(&ev(
            "tool.executed",
            json!({"tool": "web_search", "meta": {"service": "Wikipedia", "filtered": 4}}),
        ));
        assert_eq!(
            rollup(&run).unwrap(),
            "Searched 2 on Wikipedia · 4 off-topic dropped · read 1 · 1 file step"
        );
        // A chat turn records only `tool.executed` (no `tool.started`).
        let mut chat = Run::new();
        chat.push(&ev(
            "tool.executed",
            json!({"tool": "web_search", "meta": {"service": "Wikipedia", "found": 16, "filtered": 16, "results": 0}}),
        ));
        assert_eq!(
            rollup(&chat).unwrap(),
            "Searched 1 on Wikipedia · 16 off-topic dropped"
        );
        let mut deep = Run::new();
        deep.push(&ev(
            "tool.executed",
            json!({"tool": "read_url", "worker": 1}),
        ));
        assert_eq!(rollup(&deep).unwrap(), "Read 1");
    }

    #[test]
    fn pacer_reveals_by_block_at_reading_pace() {
        let text =
            "# GPUs\n\nA GPU runs many threads at once. It hides memory latency.\n- SMs\n- Warps\n";
        let mut p = Pacer::default();
        let n = p.advance(text, Duration::from_millis(20), false);
        assert_eq!(&text[..n], "# GPUs\n", "the heading lands whole first");
        let n = p.advance(text, Duration::from_millis(100), false);
        let shown = &text[..n];
        assert!(shown.starts_with("# GPUs\n\nA GPU"));
        assert!(
            !shown.contains("Warps"),
            "not everything at once: {shown:?}"
        );
        // A list item never shows half.
        for ms in [10, 30, 50, 70] {
            let n = p.advance(text, Duration::from_millis(ms), false);
            let s = &text[..n];
            assert!(!s.ends_with("- S") && !s.ends_with("- Wa"), "{s:?}");
        }
        let n = p.advance(text, Duration::from_secs(5), true);
        assert_eq!(n, text.len());
        assert!(p.caught_up(text));
        let mut q = Pacer::default();
        q.finish(text);
        assert!(q.caught_up(text));
    }

    #[test]
    fn stamps_say_sent_and_worked_for() {
        let s = Stamp {
            at: Some("2026-10-23T16:31:00Z".into()),
            took: Some(Duration::from_secs(32)),
        };
        let now = "2026-10-23T17:00:00Z";
        assert_eq!(s.sent(now, Some(5 * 3600)).unwrap(), "Sent 9:31 PM");
        assert_eq!(
            s.answered(now, Some(5 * 3600)).unwrap(),
            "9:31 PM · Worked for 32s"
        );
        assert!(Stamp::default().sent(now, None).is_none());
        assert_eq!(
            Stamp {
                at: None,
                took: Some(Duration::from_secs(3))
            }
            .answered(now, None)
            .unwrap(),
            "Worked for 3s"
        );
    }

    #[test]
    fn explain_in_3d_is_not_in_the_more_menu() {
        let gpu = "A GPU is built from many Streaming Multiprocessors (SMs).";
        // Explain in 3D moved out of More into the visible row.
        assert!(
            !more_items(gpu, false)
                .iter()
                .any(|(k, _)| *k == "explain3d")
        );
        assert!(
            more_items("hello", true)
                .iter()
                .any(|(_, l)| l.starts_with("Stop watching"))
        );
    }

    #[test]
    fn undo_expires() {
        let u = Undo::new("Stopped watching", UndoKind::Chat(SessionId::new()));
        assert!(!u.expired(Instant::now()));
        assert!(u.expired(Instant::now() + UNDO_FOR + Duration::from_millis(1)));
    }

    #[test]
    fn notices_collapse_to_one() {
        let (shown, folded) = stack(
            vec![Notice::Continue, Notice::Resume, Notice::Inserted],
            false,
        );
        assert_eq!(shown, [Notice::Resume]);
        assert_eq!(folded, 2);
        assert_eq!(more_notices(folded), "2 more notices");
        let (shown, folded) = stack(vec![Notice::Continue, Notice::Resume], true);
        assert_eq!(shown.len(), 2);
        assert_eq!(folded, 0);
        assert_eq!(
            stack(vec![Notice::Continue], false),
            (vec![Notice::Continue], 0)
        );
    }
}
