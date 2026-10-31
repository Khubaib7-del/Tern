//! The run-state event contract (#85, item A0).
//!
//! Every front end shows a turn in one of seven [`RunState`]s, and every
//! failure Tern can meet falls into one [`ErrorClass`]. A turn that stops
//! before its answer leaves a [`TurnReceipt`] saying what was kept
//! ([`KeptSummary`]), so the user never loses what the run already did.
//! Model switches, context trims and the one-line digest of an interrupted
//! turn are events too ([`ModelSwitched`], [`ContextTrimmed`],
//! [`TurnDigest`]).
//!
//! These are types and fixtures only: `tern-agent` emits them and the
//! front ends read them. The fixtures in `tests/fixtures/run_contract/`
//! are the wire form the front ends build against.

use serde::{Deserialize, Serialize};

use crate::TurnState;

/// `turn.receipt`: a turn stopped before its answer, with what was kept.
pub const TURN_RECEIPT: &str = "turn.receipt";
/// `model.switched`: this turn continues on another model.
pub const MODEL_SWITCHED: &str = "model.switched";
/// `context.trimmed`: Tern trimmed what the model reads, and how.
pub const CONTEXT_TRIMMED: &str = "context.trimmed";
/// `turn.digest`: the one-line record of an interrupted turn that joins
/// the model's history.
pub const TURN_DIGEST: &str = "turn.digest";

/// What a turn is doing, as every front end shows it.
///
/// When several turns or steps are summarised in one place (a chat row, a
/// tab), the most urgent state wins: `needs_you > failed > waiting >
/// working > streaming > paused > done` ([`RunState::most_urgent`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Waiting on the user: an approval or a question.
    NeedsYou,
    /// Stopped by an error Tern could not get past.
    Failed,
    /// Waiting on a provider (a rate-limit countdown), then continuing.
    Waiting,
    /// Running tools, reading, checking.
    Working,
    /// The model's answer is arriving.
    Streaming,
    /// Stopped with its work kept; Continue picks it up.
    Paused,
    /// Finished.
    Done,
}

impl RunState {
    /// Every state, most urgent first.
    pub const ALL: [RunState; 7] = [
        RunState::NeedsYou,
        RunState::Failed,
        RunState::Waiting,
        RunState::Working,
        RunState::Streaming,
        RunState::Paused,
        RunState::Done,
    ];

    /// Higher is more urgent.
    pub fn priority(self) -> u8 {
        match self {
            RunState::NeedsYou => 7,
            RunState::Failed => 6,
            RunState::Waiting => 5,
            RunState::Working => 4,
            RunState::Streaming => 3,
            RunState::Paused => 2,
            RunState::Done => 1,
        }
    }

    /// The most urgent of `states`, or `None` when there are none.
    pub fn most_urgent(states: impl IntoIterator<Item = RunState>) -> Option<RunState> {
        states.into_iter().max_by_key(|s| s.priority())
    }

    /// The state a turn's engine state shows as. A rate-limit wait is
    /// [`RunState::Waiting`], which the engine reports on its own (it is
    /// not a [`TurnState`]).
    pub fn of_turn(state: TurnState) -> RunState {
        use TurnState as T;
        match state {
            T::ApprovalRequested => RunState::NeedsYou,
            T::Failed | T::Blocked => RunState::Failed,
            T::ModelStreaming => RunState::Streaming,
            T::Created
            | T::Preparing
            | T::ToolRequested
            | T::PolicyCheck
            | T::Executing
            | T::RecordResult
            | T::Verifying => RunState::Working,
            // A stop keeps the work (#85): stopped by the user, by the
            // budget, or by a pause, the turn can be continued.
            T::Paused | T::Cancelled | T::BudgetExceeded => RunState::Paused,
            T::Completed | T::CompletedWithGaps => RunState::Done,
        }
    }

    /// The word a front end shows.
    pub fn label(self) -> &'static str {
        match self {
            RunState::NeedsYou => "Needs you",
            RunState::Failed => "Failed",
            RunState::Waiting => "Waiting",
            RunState::Working => "Working",
            RunState::Streaming => "Writing",
            RunState::Paused => "Paused",
            RunState::Done => "Done",
        }
    }
}

/// Why a paused turn paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PausedBy {
    /// The user pressed Stop.
    User,
    /// The turn's token or cost budget ran out.
    Budget,
    /// The key's daily quota is used up; it resets at a known time.
    DailyQuota,
    /// The connection dropped.
    Network,
    /// Tern closed while the turn was running.
    Closed,
}

impl PausedBy {
    /// "Stopped here · …", the receipt's first line.
    pub fn line(self) -> &'static str {
        match self {
            PausedBy::User => "Stopped here · you stopped the run",
            PausedBy::Budget => "Stopped here · the turn's budget ran out",
            PausedBy::DailyQuota => "Stopped here · this key's daily limit is used up",
            PausedBy::Network => "Stopped here · connection lost",
            PausedBy::Closed => "Tern closed while this turn was running",
        }
    }
}

/// Every failure Tern meets, in ten classes. The class decides what
/// happens next: waited out inside the turn, paused with a receipt, or
/// shown as a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// A per-minute limit (RPM/TPM): wait, then the same turn continues.
    RateLimitShort,
    /// A per-day limit (RPD/TPD) or exhausted quota: never retried.
    RateLimitDaily,
    /// The provider is overloaded (HTTP 5xx, 529).
    Overloaded,
    /// The connection failed or dropped before a reply started.
    Network,
    /// The reply stream was cut part-way.
    StreamCut,
    /// The key was refused.
    Auth,
    /// The turn's own budget ran out.
    Budget,
    /// The request does not fit the model's window.
    ContextFull,
    /// A tool failed.
    ToolFailed,
    /// The policy gate or the user refused a step.
    Policy,
}

impl ErrorClass {
    /// Every class.
    pub const ALL: [ErrorClass; 10] = [
        ErrorClass::RateLimitShort,
        ErrorClass::RateLimitDaily,
        ErrorClass::Overloaded,
        ErrorClass::Network,
        ErrorClass::StreamCut,
        ErrorClass::Auth,
        ErrorClass::Budget,
        ErrorClass::ContextFull,
        ErrorClass::ToolFailed,
        ErrorClass::Policy,
    ];

    /// Whether waiting and trying again inside the same turn can fix it.
    pub fn waits_in_turn(self) -> bool {
        matches!(
            self,
            ErrorClass::RateLimitShort | ErrorClass::Overloaded | ErrorClass::Network
        )
    }

    /// Whether the turn pauses with its work kept (Continue resumes it),
    /// rather than failing.
    pub fn pauses(self) -> bool {
        matches!(
            self,
            ErrorClass::RateLimitDaily
                | ErrorClass::StreamCut
                | ErrorClass::Network
                | ErrorClass::Budget
        )
    }

    /// Whether the call that met it counts against the turn's budget.
    /// Failed calls never do (#85: "3 failed calls not counted").
    pub fn counts_against_budget(self) -> bool {
        false
    }
}

/// What a stopped turn kept, for its receipt.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptSummary {
    /// Characters of answer text kept (streamed before the stop).
    pub answer_chars: u64,
    /// Sources the turn read, by title or address, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// Tool results recorded (replayed on Continue, never re-run).
    pub tool_results: u32,
    /// Calls that failed and were not counted against the budget.
    #[serde(default)]
    pub failed_not_counted: u32,
}

impl KeptSummary {
    /// "Kept: 412 characters, 3 sources, 5 tool results", or "Nothing
    /// was kept" when there is nothing.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if self.answer_chars > 0 {
            parts.push(format!("{} characters", self.answer_chars));
        }
        let n = self.sources.len();
        if n > 0 {
            parts.push(format!("{n} source{}", if n == 1 { "" } else { "s" }));
        }
        if self.tool_results > 0 {
            let n = self.tool_results;
            parts.push(format!("{n} tool result{}", if n == 1 { "" } else { "s" }));
        }
        if parts.is_empty() {
            "Nothing was kept".to_owned()
        } else {
            format!("Kept: {}", parts.join(", "))
        }
    }
}

/// `turn.receipt`: how a turn stopped and what it kept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnReceipt {
    pub state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_by: Option<PausedBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorClass>,
    /// The receipt's first line ("Stopped here · connection lost").
    pub line: String,
    pub kept: KeptSummary,
    /// Whether Continue can pick the turn up.
    pub resumable: bool,
    /// When a daily quota resets, RFC 3339 in UTC; front ends show it in
    /// the user's own zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

impl TurnReceipt {
    /// A paused turn's receipt.
    pub fn paused(by: PausedBy, error: Option<ErrorClass>, kept: KeptSummary) -> Self {
        Self {
            state: RunState::Paused,
            paused_by: Some(by),
            error,
            line: by.line().to_owned(),
            kept,
            resumable: true,
            resets_at: None,
        }
    }
}

/// `model.switched`: the rest of this turn runs on another model. Never
/// sticky: the next turn and new chats use the chosen model again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSwitched {
    pub from_provider: String,
    pub from_model: String,
    pub to_provider: String,
    pub to_model: String,
    /// Why, as a class.
    pub reason: ErrorClass,
    /// The row's words ("Mistral's daily limit is used up; this turn
    /// continues on mistral-small").
    pub line: String,
    /// Whether the user chose it (true) or the within-provider fallback
    /// did (false).
    pub chosen_by_user: bool,
}

/// `context.trimmed`: a trim decision, recorded before the call it
/// shapes and replayed on resume, so a resumed chat sends the same bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextTrimmed {
    /// The cascade tier: `cap`, `strip_turn`, `strip_finished`, `fit`,
    /// `compact`, or `batch` (a scholarly batch split to fit).
    pub tier: String,
    pub before_tokens: u64,
    pub after_tokens: u64,
    /// What was trimmed: result ids, turn numbers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
    /// One plain line for the divider.
    pub line: String,
}

/// `turn.digest`: what an interrupted turn leaves in the model's history.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnDigest {
    pub question: String,
    /// The answer text that streamed before the stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// The tools the turn ran, in order, with repeats folded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// How it stopped.
    pub stop: String,
}

impl TurnDigest {
    /// "Tools run: web_search ×2, read_url." or empty.
    pub fn tools_line(&self) -> String {
        if self.tools.is_empty() {
            return String::new();
        }
        let mut folded: Vec<(String, u32)> = Vec::new();
        for t in &self.tools {
            match folded.iter_mut().find(|(n, _)| n == t) {
                Some((_, c)) => *c += 1,
                None => folded.push((t.clone(), 1)),
            }
        }
        let list: Vec<String> = folded
            .into_iter()
            .map(|(n, c)| if c > 1 { format!("{n} ×{c}") } else { n })
            .collect();
        format!("Tools run: {}.", list.join(", "))
    }

    /// The assistant message an interrupted turn becomes in the model's
    /// history: the partial answer, then a note saying how it stopped,
    /// what it read and which tools it ran.
    pub fn history_text(&self) -> String {
        let mut out = String::new();
        if let Some(p) = self.partial.as_deref().filter(|p| !p.trim().is_empty()) {
            out.push_str(p.trim_end());
            out.push_str("\n\n");
        }
        out.push_str("(Tern note: this turn stopped before it finished: ");
        out.push_str(self.stop.trim().trim_end_matches('.'));
        out.push('.');
        if !self.sources.is_empty() {
            out.push_str(" Sources read: ");
            out.push_str(&self.sources.join("; "));
            out.push('.');
        }
        let tools = self.tools_line();
        if !tools.is_empty() {
            out.push(' ');
            out.push_str(&tools);
        }
        out.push(')');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_most_urgent_state_wins() {
        use RunState::*;
        assert_eq!(
            RunState::most_urgent([Done, Streaming, Waiting]),
            Some(Waiting)
        );
        assert_eq!(
            RunState::most_urgent([Paused, Failed, Working]),
            Some(Failed)
        );
        assert_eq!(RunState::most_urgent([Failed, NeedsYou]), Some(NeedsYou));
        assert_eq!(RunState::most_urgent(Vec::new()), None);
        let p: Vec<u8> = RunState::ALL.iter().map(|s| s.priority()).collect();
        assert!(
            p.windows(2).all(|w| w[0] > w[1]),
            "ALL is most urgent first"
        );
    }

    #[test]
    fn a_stop_keeps_the_work_and_reads_as_paused() {
        assert_eq!(RunState::of_turn(TurnState::Cancelled), RunState::Paused);
        assert_eq!(
            RunState::of_turn(TurnState::BudgetExceeded),
            RunState::Paused
        );
        assert_eq!(RunState::of_turn(TurnState::Failed), RunState::Failed);
        assert_eq!(
            RunState::of_turn(TurnState::ApprovalRequested),
            RunState::NeedsYou
        );
        assert_eq!(
            RunState::of_turn(TurnState::ModelStreaming),
            RunState::Streaming
        );
        assert_eq!(
            RunState::of_turn(TurnState::CompletedWithGaps),
            RunState::Done
        );
    }

    #[test]
    fn classes_say_what_happens_next() {
        assert!(ErrorClass::RateLimitShort.waits_in_turn());
        assert!(!ErrorClass::RateLimitDaily.waits_in_turn());
        assert!(ErrorClass::RateLimitDaily.pauses());
        assert!(ErrorClass::StreamCut.pauses());
        assert!(!ErrorClass::Auth.pauses());
        assert!(ErrorClass::ALL.iter().all(|c| !c.counts_against_budget()));
        let text = serde_json::to_string(&ErrorClass::RateLimitShort).unwrap();
        assert_eq!(text, "\"rate_limit_short\"");
    }

    #[test]
    fn kept_line_counts_what_was_kept() {
        let k = KeptSummary {
            answer_chars: 412,
            sources: vec!["a.org".into(), "b.org".into(), "c.org".into()],
            tool_results: 1,
            failed_not_counted: 0,
        };
        assert_eq!(k.line(), "Kept: 412 characters, 3 sources, 1 tool result");
        assert_eq!(KeptSummary::default().line(), "Nothing was kept");
    }

    #[test]
    fn a_digest_becomes_one_history_message() {
        let d = TurnDigest {
            question: "How does FLARE work?".into(),
            partial: Some("FLARE retrieves when".into()),
            sources: vec!["Jiang 2023".into()],
            tools: vec![
                "paper_search".into(),
                "paper_read".into(),
                "paper_read".into(),
            ],
            stop: "Connection lost.".into(),
        };
        assert_eq!(
            d.history_text(),
            "FLARE retrieves when\n\n(Tern note: this turn stopped before it finished: Connection lost. Sources read: Jiang 2023. Tools run: paper_search, paper_read ×2.)"
        );
    }
}
