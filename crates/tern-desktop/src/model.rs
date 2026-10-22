//! What the desktop window shows, kept apart from GPUI so it can be tested
//! without a display. The view reads this; the bridge feeds it.

use std::time::{Duration, Instant};

use tern_agent::{ApprovalRequest, Harness, PastTurn, SessionSummary, TurnOutcome, Usage};
use tern_core::TurnState;

use crate::providers::ProvidersState;

/// What the centre of the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Chat,
    Providers,
}

/// One block of the transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    User(String),
    /// A finished answer, with the steps that led to it and how long it took.
    Assistant {
        text: String,
        steps: Vec<String>,
        took: Duration,
    },
    /// A plain note from Tern (a stop reason, a phase that is not built yet).
    Notice(String),
}

/// Whether the window can take a message.
#[derive(Debug)]
pub enum Phase {
    Connecting,
    /// No provider could be opened. The text says what to do next.
    Offline(String),
    Ready,
    Working {
        since: Instant,
        label: String,
        steps: Vec<String>,
        /// Reply text streaming in, and which model call of the turn it is.
        draft: String,
        draft_call: u32,
    },
    Approval {
        since: Instant,
        steps: Vec<String>,
        request: ApprovalRequest,
    },
}

/// The connected model, shown under the composer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Connection {
    pub provider: String,
    pub model: String,
    pub search: Option<String>,
    pub context_window: Option<u64>,
}

/// The whole window's state.
#[derive(Debug)]
pub struct Model {
    pub entries: Vec<Entry>,
    pub phase: Phase,
    pub connection: Option<Connection>,
    pub usage: Usage,
    pub harness: Harness,
    pub sessions: Vec<SessionSummary>,
    pub sidebar_open: bool,
    pub run_panel_open: bool,
    pub dark: bool,
    pub stopping: bool,
    pub screen: Screen,
    pub providers: ProvidersState,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Model {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            phase: Phase::Connecting,
            connection: None,
            usage: Usage::measured(0, 0),
            harness: Harness::Research,
            sessions: Vec::new(),
            sidebar_open: true,
            run_panel_open: false,
            dark: true,
            stopping: false,
            screen: Screen::Chat,
            providers: ProvidersState::default(),
        }
    }

    pub fn connected(&mut self, connection: Connection) {
        self.connection = Some(connection);
        self.phase = Phase::Ready;
    }

    pub fn connect_failed(&mut self, reason: String) {
        self.phase = Phase::Offline(reason);
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.phase, Phase::Working { .. } | Phase::Approval { .. })
    }

    pub fn can_send(&self) -> bool {
        matches!(self.phase, Phase::Ready)
    }

    /// Starts a turn. Returns the trimmed text to send, or `None` when the
    /// window cannot send now or the text is empty.
    pub fn start_turn(&mut self, text: &str) -> Option<String> {
        let text = text.trim();
        if text.is_empty() || !self.can_send() {
            return None;
        }
        self.entries.push(Entry::User(text.to_owned()));
        self.phase = Phase::Working {
            since: Instant::now(),
            label: "Thinking".into(),
            steps: Vec::new(),
            draft: String::new(),
            draft_call: 0,
        };
        self.stopping = false;
        Some(text.to_owned())
    }

    /// An event from the run, by type and its one-line summary.
    pub fn on_event(&mut self, kind: &str, summary: &str) {
        let label = match kind {
            "turn.started" | "tool.executed" | "tool.failed" | "tool.rejected" => Some("Thinking"),
            "model.replied" => Some("Reading the reply"),
            "policy.decided" => Some("Checking permissions"),
            "approval.answered" => Some("Working"),
            "tool.started" => Some("Using a tool"),
            _ => None,
        };
        let is_step = is_step(kind, summary);
        match &mut self.phase {
            Phase::Working {
                label: l, steps, ..
            } => {
                if let Some(new) = label {
                    *l = new.to_owned();
                }
                if is_step {
                    steps.push(summary.to_owned());
                }
            }
            Phase::Approval { steps, .. } if is_step => steps.push(summary.to_owned()),
            _ => {}
        }
    }

    /// Reply text from the stream. A new call number starts the draft over,
    /// since the previous reply was a tool request, not the answer.
    pub fn on_text(&mut self, call: u32, text: &str) {
        if let Phase::Working {
            draft,
            draft_call,
            label,
            ..
        } = &mut self.phase
        {
            if *draft_call != call {
                draft.clear();
                *draft_call = call;
            }
            draft.push_str(text);
            if draft_visible(draft).is_some() {
                "Writing".clone_into(label);
            }
        }
    }

    /// The streaming answer to show, without any tool request it turns into.
    pub fn draft(&self) -> Option<&str> {
        match &self.phase {
            Phase::Working { draft, .. } => draft_visible(draft),
            _ => None,
        }
    }

    pub fn ask_approval(&mut self, request: ApprovalRequest) {
        let (since, steps) = match std::mem::replace(&mut self.phase, Phase::Ready) {
            Phase::Working { since, steps, .. } | Phase::Approval { since, steps, .. } => {
                (since, steps)
            }
            _ => (Instant::now(), Vec::new()),
        };
        self.phase = Phase::Approval {
            since,
            steps,
            request,
        };
    }

    pub fn approval_answered(&mut self) {
        if let Phase::Approval { since, steps, .. } =
            std::mem::replace(&mut self.phase, Phase::Ready)
        {
            self.phase = Phase::Working {
                since,
                label: "Working".into(),
                steps,
                draft: String::new(),
                draft_call: 0,
            };
        }
    }

    pub fn request_stop(&mut self) {
        if self.is_busy() {
            self.stopping = true;
        }
    }

    pub fn finish_turn(&mut self, out: &TurnOutcome, usage: Usage) {
        let (took, steps) = match std::mem::replace(&mut self.phase, Phase::Ready) {
            Phase::Working { since, steps, .. } | Phase::Approval { since, steps, .. } => {
                (since.elapsed(), steps)
            }
            _ => (Duration::ZERO, Vec::new()),
        };
        self.usage = usage;
        self.stopping = false;
        match &out.answer {
            Some(a) if !a.is_empty() => self.entries.push(Entry::Assistant {
                text: a.clone(),
                steps,
                took,
            }),
            _ => {}
        }
        if out.state != TurnState::Completed
            && let Some(r) = &out.reason
        {
            self.entries.push(Entry::Notice(r.clone()));
        }
    }

    /// Switching harness. Study is not built yet, so the switch explains that
    /// and leaves the chat in Research.
    pub fn choose_harness(&mut self, harness: Harness) {
        match harness {
            Harness::Research => self.harness = Harness::Research,
            Harness::Study => self.entries.push(Entry::Notice(
                "The Study harness (tutor, flashcards, quizzes, viva practice) arrives in phase 7. This chat stays in Research."
                    .into(),
            )),
        }
    }

    /// Fills the transcript from a reopened chat's past turns.
    pub fn restore(&mut self, turns: &[PastTurn]) {
        self.entries.clear();
        let mut missing = 0;
        for t in turns {
            match &t.message {
                Some(m) => self.entries.push(Entry::User(m.clone())),
                None => missing += 1,
            }
            if let Some(a) = &t.answer {
                let steps = t
                    .events
                    .iter()
                    .filter(|(k, s)| is_step(k, s))
                    .map(|(_, s)| s.clone())
                    .collect();
                self.entries.push(Entry::Assistant {
                    text: a.clone(),
                    steps,
                    took: t.took,
                });
            }
            if let Some(why) = &t.stop {
                self.entries.push(Entry::Notice(why.clone()));
            }
        }
        if missing > 0 {
            let note = format!(
                "{missing} earlier {} from before Tern kept chat text, so {} not shown.",
                if missing == 1 { "turn is" } else { "turns are" },
                if missing == 1 { "it is" } else { "they are" },
            );
            self.entries.insert(0, Entry::Notice(note));
        }
    }

    /// Shows a new title in the list at once; the bridge saves it.
    pub fn rename(&mut self, id: tern_core::SessionId, title: &str) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.title = title.to_owned();
        }
    }

    /// Clears the transcript for a new chat. The caller opens a new session.
    pub fn new_chat(&mut self) {
        if self.is_busy() {
            return;
        }
        self.entries.clear();
        self.usage = Usage::measured(0, 0);
        if self.connection.is_some() {
            self.phase = Phase::Ready;
        } else {
            self.phase = Phase::Connecting;
        }
    }

    /// The working line: label and whole seconds, while a turn runs.
    pub fn working_line(&self) -> Option<String> {
        match &self.phase {
            Phase::Working { since, label, .. } => {
                let label = if self.stopping { "Stopping" } else { label };
                Some(format!("{label}… {}s", since.elapsed().as_secs()))
            }
            Phase::Approval { since, .. } => {
                Some(format!("Waiting for you… {}s", since.elapsed().as_secs()))
            }
            _ => None,
        }
    }

    /// Steps of the run in progress, or of the last answer, for the run panel.
    pub fn run_steps(&self) -> &[String] {
        match &self.phase {
            Phase::Working { steps, .. } | Phase::Approval { steps, .. } => steps,
            _ => self
                .entries
                .iter()
                .rev()
                .find_map(|e| match e {
                    Entry::Assistant { steps, .. } => Some(steps.as_slice()),
                    _ => None,
                })
                .unwrap_or(&[]),
        }
    }

    /// Context used, from 0.0 to 1.0, when the model's window is known.
    pub fn context_fraction(&self) -> Option<f32> {
        let window = self.connection.as_ref()?.context_window?;
        if window == 0 {
            return None;
        }
        Some((self.usage.total() as f32 / window as f32).clamp(0.0, 1.0))
    }

    /// "1.2k of 128k tokens", or just the count when the window is unknown.
    pub fn context_label(&self) -> String {
        let used = compact(self.usage.total());
        match self.connection.as_ref().and_then(|c| c.context_window) {
            Some(w) => format!("{used} of {} tokens", compact(w)),
            None => format!("{used} tokens"),
        }
    }
}

/// Events that show as a step under an answer.
fn is_step(kind: &str, summary: &str) -> bool {
    matches!(
        kind,
        "tool.executed" | "tool.failed" | "tool.rejected" | "approval.answered" | "loop.suspected"
    ) || (kind == "policy.decided" && !summary.contains("allowed"))
}

/// The part of a streaming reply worth showing: everything before a tool
/// fence, minus a fence that is still arriving. `None` when nothing is left.
pub fn draft_visible(draft: &str) -> Option<&str> {
    const FENCE: &str = "```tool";
    let mut text = match draft.find(FENCE) {
        Some(i) => &draft[..i],
        None => draft,
    };
    for k in (1..FENCE.len()).rev() {
        if let Some(cut) = text.strip_suffix(&FENCE[..k]) {
            text = cut;
            break;
        }
    }
    let text = text.trim_end();
    (!text.trim_start().is_empty()).then_some(text)
}

/// 950 → "950", 1_234 → "1.2k", 128_000 → "128k", 1_000_000 → "1M".
pub fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => trim(n as f64 / 1_000.0, "k"),
        _ => trim(n as f64 / 1_000_000.0, "M"),
    }
}

fn trim(v: f64, unit: &str) -> String {
    if v >= 100.0 || (v - v.round()).abs() < 0.05 {
        format!("{}{unit}", v.round() as u64)
    } else {
        format!("{v:.1}{unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tern_core::{EffectClass, TurnId};

    fn outcome(state: TurnState, answer: Option<&str>, reason: Option<&str>) -> TurnOutcome {
        TurnOutcome {
            turn_id: TurnId::new(),
            state,
            answer: answer.map(str::to_owned),
            reason: reason.map(str::to_owned),
            states: vec![],
            usage: Usage::measured(0, 0),
            model_calls: 1,
            tool_calls: 0,
        }
    }

    #[test]
    fn drafts_hide_tool_requests_and_restart_per_call() {
        let mut m = ready();
        m.start_turn("hi").unwrap();
        assert_eq!(m.draft(), None);
        m.on_text(1, "```to");
        assert_eq!(
            m.draft(),
            None,
            "a fence that is still arriving stays hidden"
        );
        m.on_text(1, "ol\n{\"name\":\"web_search\"}");
        assert_eq!(m.draft(), None);
        m.on_text(2, "Paris is the ");
        m.on_text(2, "capital.");
        assert_eq!(m.draft(), Some("Paris is the capital."));
        assert_eq!(m.working_line().unwrap().split('…').next(), Some("Writing"));
        m.on_text(3, "Let me check. `");
        assert_eq!(m.draft(), Some("Let me check."));
        assert_eq!(draft_visible("Answer\n```tool\n{}"), Some("Answer"));
        assert_eq!(draft_visible("   "), None);
    }

    #[test]
    fn rename_updates_the_list_row() {
        let mut m = ready();
        let id = tern_core::SessionId::new();
        m.sessions.push(SessionSummary {
            id,
            title: "Chat".into(),
            harness: Harness::Research,
            created_at: String::new(),
            events: 3,
        });
        m.rename(id, "Attention paper");
        assert_eq!(m.sessions[0].title, "Attention paper");
        m.rename(tern_core::SessionId::new(), "Other");
        assert_eq!(m.sessions[0].title, "Attention paper");
    }

    #[test]
    fn restore_rebuilds_the_transcript() {
        let mut m = ready();
        m.entries.push(Entry::Notice("old".into()));
        let turns = vec![
            PastTurn::default(),
            PastTurn {
                message: Some("What is RAG?".into()),
                answer: Some("Retrieval [1].".into()),
                events: vec![
                    ("model.replied".into(), "replied".into()),
                    ("tool.executed".into(), "Searched the web".into()),
                ],
                took: Duration::from_secs(3),
                ..PastTurn::default()
            },
            PastTurn {
                message: Some("More".into()),
                stop: Some("You stopped the run.".into()),
                ..PastTurn::default()
            },
        ];
        m.restore(&turns);
        assert_eq!(
            m.entries,
            [
                Entry::Notice(
                    "1 earlier turn is from before Tern kept chat text, so it is not shown.".into()
                ),
                Entry::User("What is RAG?".into()),
                Entry::Assistant {
                    text: "Retrieval [1].".into(),
                    steps: vec!["Searched the web".into()],
                    took: Duration::from_secs(3),
                },
                Entry::User("More".into()),
                Entry::Notice("You stopped the run.".into()),
            ]
        );
    }

    fn ready() -> Model {
        let mut m = Model::new();
        m.connected(Connection {
            provider: "Groq".into(),
            model: "llama-3.3-70b".into(),
            search: Some("Tavily".into()),
            context_window: Some(128_000),
        });
        m
    }

    #[test]
    fn nothing_is_sent_while_connecting_or_empty() {
        let mut m = Model::new();
        assert_eq!(m.start_turn("hi"), None);
        m.connect_failed("No provider".into());
        assert_eq!(m.start_turn("hi"), None);
        let mut m = ready();
        assert_eq!(m.start_turn("   "), None);
        assert_eq!(m.start_turn("  hi \n"), Some("hi".into()));
        assert!(m.is_busy());
        assert_eq!(m.start_turn("again"), None, "one turn at a time");
    }

    #[test]
    fn steps_follow_the_run_and_stay_with_the_answer() {
        let mut m = ready();
        m.start_turn("What is RAG?");
        m.on_event("turn.started", "Turn started");
        m.on_event("tool.executed", "web_search: 5 results");
        m.on_event("policy.decided", "read_url allowed");
        assert_eq!(m.run_steps(), ["web_search: 5 results"]);
        assert!(
            m.working_line()
                .unwrap()
                .starts_with("Checking permissions… ")
        );
        m.finish_turn(
            &outcome(TurnState::Completed, Some("RAG is…"), None),
            Usage::measured(1000, 234),
        );
        assert!(!m.is_busy());
        match m.entries.last().unwrap() {
            Entry::Assistant { text, steps, .. } => {
                assert_eq!(text, "RAG is…");
                assert_eq!(steps, &["web_search: 5 results".to_owned()]);
            }
            e => panic!("{e:?}"),
        }
        assert_eq!(m.run_steps(), ["web_search: 5 results"]);
        assert_eq!(m.context_label(), "1.2k of 128k tokens");
        assert!((m.context_fraction().unwrap() - 1234.0 / 128_000.0).abs() < 1e-6);
    }

    #[test]
    fn approvals_pause_and_resume_the_same_run() {
        let mut m = ready();
        m.start_turn("q");
        m.on_event("tool.executed", "one");
        m.ask_approval(ApprovalRequest {
            tool: "read_url".into(),
            summary: "Read example.com".into(),
            effect: EffectClass::Network,
            target: Some("example.com".into()),
            args: json!({}),
        });
        assert!(matches!(m.phase, Phase::Approval { .. }));
        assert!(m.working_line().unwrap().starts_with("Waiting for you"));
        m.approval_answered();
        assert!(matches!(m.phase, Phase::Working { .. }));
        assert_eq!(m.run_steps(), ["one"]);
    }

    #[test]
    fn stopped_turns_leave_a_notice() {
        let mut m = ready();
        m.start_turn("q");
        m.request_stop();
        assert!(m.working_line().unwrap().starts_with("Stopping"));
        m.finish_turn(
            &outcome(TurnState::Cancelled, None, Some("You stopped this turn.")),
            Usage::measured(0, 0),
        );
        assert_eq!(
            m.entries.last(),
            Some(&Entry::Notice("You stopped this turn.".into()))
        );
        assert!(m.can_send());
    }

    #[test]
    fn study_explains_itself_and_new_chat_clears() {
        let mut m = ready();
        m.choose_harness(Harness::Study);
        assert_eq!(m.harness, Harness::Research);
        assert!(matches!(m.entries.last(), Some(Entry::Notice(t)) if t.contains("phase 7")));
        m.new_chat();
        assert!(m.entries.is_empty());
        assert!(m.can_send());
    }

    #[test]
    fn compact_numbers() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(1_234), "1.2k");
        assert_eq!(compact(128_000), "128k");
        assert_eq!(compact(2_000), "2k");
        assert_eq!(compact(1_000_000), "1M");
    }
}
