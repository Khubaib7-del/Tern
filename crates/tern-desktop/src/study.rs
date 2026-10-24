//! The Study harness's window state, free of GPUI so it can be tested:
//! the library as last loaded, which Study view is open, the tutor chat,
//! the quiz in progress, the viva with its timer, and the flashcard deck
//! with the review in progress.
//!
//! The bridge does the work (`tern_agent::study`); this only holds what
//! the screen shows and moves between steps.

use std::time::{Duration, Instant};

use tern_agent::cards::{Card, Deck, Rating};
use tern_agent::study::{
    Answered, Difficulty, Grade, Quiz, QuizAttempt, QuizSummary, StudyLibrary, Topic, TutorMode,
    TutorTurn, Viva, VivaReport,
};

/// Which Study view is open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StudyView {
    /// Materials, topics with mastery rings, and the three actions.
    #[default]
    Home,
    Tutor,
    Quiz,
    Viva,
    /// A flashcard review session.
    Cards,
    /// The deck: edit, suspend, delete, export, import.
    Deck,
}

/// The Study window's modes, as the tab bar at its top shows them (v2:
/// `Flashcards │ Quiz │ Viva`, with the chat and Explain in 3D beside
/// them). One window: the tab picks what the centre shows, and the
/// composer at the bottom stays the Study chat's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StudyTab {
    /// The Study chat; materials and topics when it is empty.
    #[default]
    Chat,
    Cards,
    Quiz,
    Viva,
    /// Explain in 3D.
    Explain,
}

impl StudyTab {
    pub const ALL: [StudyTab; 5] = [
        StudyTab::Chat,
        StudyTab::Cards,
        StudyTab::Quiz,
        StudyTab::Viva,
        StudyTab::Explain,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Cards => "Flashcards",
            Self::Quiz => "Quiz",
            Self::Viva => "Viva",
            Self::Explain => "Explain in 3D",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// Whether the mode works from the user's materials (an empty library
    /// shows the add-material empty state instead of the tool).
    pub fn needs_materials(self) -> bool {
        matches!(self, Self::Quiz | Self::Viva)
    }

    /// The tab a Study view belongs to.
    pub fn of(view: StudyView) -> Self {
        match view {
            StudyView::Home | StudyView::Tutor => Self::Chat,
            StudyView::Cards | StudyView::Deck => Self::Cards,
            StudyView::Quiz => Self::Quiz,
            StudyView::Viva => Self::Viva,
        }
    }
}

/// What a message typed in the Study composer asks for, when it names a
/// mode ("quiz me on paging", "flashcards for chapter 3", "explain TLBs
/// in 3D"): the tab to switch to, and what is left as its subject.
pub fn intent(text: &str) -> Option<(StudyTab, String)> {
    let t = text.trim();
    let low = t.to_lowercase();
    let rules: [(&[&str], StudyTab); 4] = [
        (
            &["in 3d", "3d model", "in three d", "explain in 3d"],
            StudyTab::Explain,
        ),
        (
            &["quiz me", "test me", "give me a quiz", "start a quiz"],
            StudyTab::Quiz,
        ),
        (
            &["viva", "examine me", "oral exam", "mock exam"],
            StudyTab::Viva,
        ),
        (
            &[
                "flashcards",
                "flashcard",
                "flash cards",
                "flash card",
                "review my cards",
                "make cards",
                "anki",
            ],
            StudyTab::Cards,
        ),
    ];
    for (keys, tab) in rules {
        if let Some(k) = keys.iter().find(|k| low.contains(*k)) {
            let at = low.find(k).unwrap_or(0);
            let mut rest = format!("{} {}", &t[..at], &t[at + k.len()..]);
            for filler in ["on ", "about ", "for ", "of "] {
                let trimmed = rest.trim_start();
                if trimmed.to_lowercase().starts_with(filler) {
                    rest = trimmed[filler.len()..].to_owned();
                }
            }
            let subject = rest
                .trim()
                .trim_start_matches(|c: char| !c.is_alphanumeric())
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != ')')
                .to_owned();
            let subject = subject
                .strip_prefix("explain ")
                .map(str::to_owned)
                .unwrap_or(subject);
            return Some((tab, subject));
        }
    }
    None
}

/// How long the card takes to turn over.
pub const FLIP: Duration = Duration::from_millis(320);

/// The flashcard review in progress.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewRun {
    /// Only cards on this topic (its id).
    pub topic: Option<String>,
    pub card: Option<Card>,
    pub flipped: bool,
    /// When the card was last turned over.
    pub flip_at: Option<Instant>,
    /// Cards answered this session.
    pub done: usize,
    /// Due when the session started, for the progress line.
    pub due_at_start: usize,
    /// The last answer, for Scout's reaction and the progress line.
    pub last: Option<Rating>,
}

impl ReviewRun {
    /// How far the card has turned: 0 front up, 1 back up. Without
    /// motion it turns at once.
    pub fn turn(&self, now: Instant, reduced: bool) -> f32 {
        let target = if self.flipped { 1.0 } else { 0.0 };
        match self.flip_at {
            Some(at) if !reduced => {
                let t = crate::motion::progress(at, FLIP, now);
                if self.flipped { t } else { 1.0 - t }
            }
            _ => target,
        }
    }

    /// The card's width as a share of its full width at turn `t`, and
    /// whether the back is the side showing. The card narrows to an edge
    /// and opens again on the other side.
    pub fn face(t: f32) -> (f32, bool) {
        ((1.0 - 2.0 * t).abs().max(0.02), t >= 0.5)
    }

    pub fn turning(&self, now: Instant, reduced: bool) -> bool {
        !reduced
            && self
                .flip_at
                .is_some_and(|t| now.saturating_duration_since(t) < FLIP)
    }
}

/// One tutor message with the passages it cited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TutorLine {
    pub turn: TutorTurn,
    pub citations: Vec<String>,
    pub beyond: bool,
    pub offline: bool,
}

/// The quiz in progress.
#[derive(Clone, Debug, PartialEq)]
pub struct QuizRun {
    pub quiz: Quiz,
    /// The question on screen (0-based).
    pub index: usize,
    pub attempt: QuizAttempt,
    /// The choice picked on a multiple-choice or true/false question.
    pub picked: Option<String>,
    /// Waiting for the grade of the question on screen.
    pub grading: bool,
    /// The summary, once the last answer is saved.
    pub done: Option<QuizSummary>,
}

impl QuizRun {
    pub fn new(quiz: Quiz, now: i64) -> Self {
        Self {
            quiz,
            index: 0,
            attempt: QuizAttempt {
                started_at: now,
                ..QuizAttempt::default()
            },
            picked: None,
            grading: false,
            done: None,
        }
    }

    pub fn total(&self) -> usize {
        self.quiz.questions.len()
    }

    pub fn question(&self) -> Option<&tern_agent::study::Question> {
        self.quiz.questions.get(self.index)
    }

    /// The grade of the question on screen, once answered.
    pub fn graded(&self) -> Option<&Answered> {
        let n = self.question()?.n;
        self.attempt.answers.iter().find(|a| a.n == n)
    }

    /// Records the grade for question `n`.
    pub fn record(&mut self, n: usize, given: String, grade: Grade) {
        self.grading = false;
        if !self.attempt.answers.iter().any(|a| a.n == n) {
            self.attempt.answers.push(Answered { n, given, grade });
        }
    }

    /// Moves to the next question. Returns `true` when the quiz is over.
    pub fn next(&mut self) -> bool {
        self.picked = None;
        if self.index + 1 >= self.total() {
            return true;
        }
        self.index += 1;
        false
    }

    pub fn is_last(&self) -> bool {
        self.index + 1 >= self.total()
    }

    /// `3 / 5 so far`.
    pub fn running_score(&self) -> String {
        format!(
            "{} / {} so far",
            fmt_score(self.attempt.score()),
            self.attempt.answers.len()
        )
    }
}

/// `3`, or `3.5`.
pub fn fmt_score(s: f32) -> String {
    if (s - s.round()).abs() < 0.01 {
        format!("{}", s.round() as i64)
    } else {
        format!("{s:.1}")
    }
}

/// The viva in progress.
#[derive(Clone, Debug, PartialEq)]
pub struct VivaRun {
    pub viva: Viva,
    pub started: Instant,
    /// When the open question was shown.
    pub asked: Instant,
    /// Waiting for the next question or the report.
    pub thinking: bool,
    pub report: Option<(VivaReport, Option<std::path::PathBuf>)>,
    /// When the viva ended, to stop the clock.
    pub ended: Option<Instant>,
}

impl VivaRun {
    pub fn new(viva: Viva, now: Instant) -> Self {
        Self {
            viva,
            started: now,
            asked: now,
            thinking: true,
            report: None,
            ended: None,
        }
    }

    /// The clock: `04:12`.
    pub fn clock(&self, now: Instant) -> String {
        let end = self.ended.unwrap_or(now);
        clock(end.saturating_duration_since(self.started))
    }

    /// Seconds on the open question.
    pub fn answer_secs(&self, now: Instant) -> u32 {
        now.saturating_duration_since(self.asked).as_secs() as u32
    }

    /// `Question 3 of 6`.
    pub fn progress(&self) -> String {
        format!(
            "Question {} of {}",
            self.viva.turns.len().max(1),
            self.viva.planned
        )
    }
}

pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Everything the Study screen shows.
#[derive(Debug, Default)]
pub struct StudyState {
    pub library: StudyLibrary,
    pub loaded: bool,
    pub view: StudyView,
    /// Work in flight, said in a few words: `Reading notes.pdf…`.
    pub busy: Option<String>,
    /// The last thing to tell the user (an error, or a fallback note).
    pub notice: Option<String>,
    // Tutor.
    pub tutor_topic: Option<Topic>,
    pub tutor_mode: TutorMode,
    pub tutor: Vec<TutorLine>,
    /// The reply streaming in.
    pub tutor_stream: String,
    pub tutor_waiting: bool,
    // Quiz.
    pub quiz_topic: Option<Topic>,
    pub quiz_count: usize,
    pub quiz_difficulty: Difficulty,
    pub quiz: Option<QuizRun>,
    // Viva.
    pub viva_questions: usize,
    pub viva: Option<VivaRun>,
    // Settings and connection.
    /// Study without a model.
    pub offline: bool,
    /// Who the passages go to: `Groq · llama-3.3-70b`; `None` offline.
    pub engine: Option<String>,
    // Flashcards.
    pub deck: Deck,
    pub deck_loaded: bool,
    pub review: Option<ReviewRun>,
    /// The deck screen's topic filter (a topic id).
    pub deck_topic: Option<String>,
    /// The card being edited on the deck screen.
    pub editing: Option<String>,
    /// The local clock's offset from UTC, for "today".
    pub offset: i64,
    /// The mode the one Study window shows.
    pub tab: StudyTab,
    /// The materials and topics panel is open over the chat.
    pub materials_open: bool,
    /// The 3D explainer shown in the Explain tab (its key).
    pub explain: Option<u32>,
}

impl StudyState {
    pub fn new() -> Self {
        Self {
            quiz_count: 5,
            viva_questions: 6,
            offset: tern_agent::cards::local_offset(),
            ..Self::default()
        }
    }

    /// The saved Study choices: quiz length and difficulty, viva length,
    /// and offline study.
    pub fn from_prefs(prefs: &tern_agent::Preferences) -> Self {
        let spec = prefs.quiz_spec(None);
        Self {
            quiz_count: spec.count,
            quiz_difficulty: spec.difficulty,
            viva_questions: prefs.viva_questions(),
            offline: prefs.study_offline,
            ..Self::new()
        }
    }

    pub fn loaded(&mut self, lib: StudyLibrary) {
        self.library = lib;
        self.loaded = true;
        self.busy = None;
    }

    pub fn fail(&mut self, msg: String) {
        self.busy = None;
        self.notice = Some(msg);
    }

    /// Switches the window's mode. A tool already under way (a quiz, a
    /// viva, a review) is kept, so moving between tabs never loses it.
    /// Shows a Study view (from the Library, a deck row, a review's
    /// end) with its tab lit.
    pub fn show(&mut self, view: StudyView) {
        self.view = view;
        self.tab = StudyTab::of(view);
    }

    pub fn set_tab(&mut self, tab: StudyTab) {
        self.tab = tab;
        self.notice = None;
        self.view = match tab {
            StudyTab::Chat if self.view == StudyView::Tutor => StudyView::Tutor,
            StudyTab::Chat | StudyTab::Explain => StudyView::Home,
            StudyTab::Cards if matches!(self.view, StudyView::Cards | StudyView::Deck) => self.view,
            StudyTab::Cards if self.review.is_some() => StudyView::Cards,
            StudyTab::Cards => StudyView::Deck,
            StudyTab::Quiz => StudyView::Quiz,
            StudyTab::Viva => StudyView::Viva,
        };
        if tab == StudyTab::Viva && self.viva.as_ref().is_some_and(|v| v.report.is_some()) {
            self.viva = None;
        }
    }

    /// The one-line status v2 puts in its status bar: `6 due today ·
    /// recall 78%`, or what to do first when there is nothing yet.
    pub fn status_line(&self, now: i64) -> String {
        if self.library.materials.is_empty() && self.deck.cards.is_empty() {
            return "Add a PDF, notes or a link to start".into();
        }
        let due = self.due(None, now);
        let mut parts = vec![if due == 0 {
            "nothing due".to_owned()
        } else {
            format!("{due} due today")
        }];
        if !self.library.topics.is_empty() {
            parts.push(format!("recall {:.0}%", self.library.overall() * 100.));
        }
        parts.join(" · ")
    }

    pub fn go_home(&mut self) {
        self.view = StudyView::Home;
        self.busy = None;
        self.editing = None;
    }

    // ---- Flashcards ----

    pub fn deck_ready(&mut self, deck: Deck) {
        self.deck = deck;
        self.deck_loaded = true;
        if let Some(id) = &self.editing
            && self.deck.get(id).is_none()
        {
            self.editing = None;
        }
    }

    /// Cards due today, on one topic or all.
    pub fn due(&self, topic: Option<&str>, now: i64) -> usize {
        self.deck.due_count(now, self.offset, topic)
    }

    /// Opens a review session and shows its first card.
    pub fn open_review(&mut self, topic: Option<String>, now: i64) {
        let due = self.deck.due_count(now, self.offset, topic.as_deref());
        self.review = Some(ReviewRun {
            topic,
            card: None,
            flipped: false,
            flip_at: None,
            done: 0,
            due_at_start: due,
            last: None,
        });
        self.view = StudyView::Cards;
        self.notice = None;
        self.next_card(now);
    }

    /// Puts the next card up, or none when the session is through.
    pub fn next_card(&mut self, now: i64) {
        let (deck, offset) = (&self.deck, self.offset);
        if let Some(r) = &mut self.review {
            r.card = deck.next_card(now, offset, r.topic.as_deref()).cloned();
            r.flipped = false;
            r.flip_at = None;
        }
    }

    /// Turns the card over (either way).
    pub fn flip(&mut self, at: Instant) {
        if let Some(r) = &mut self.review
            && r.card.is_some()
        {
            r.flipped = !r.flipped;
            r.flip_at = Some(at);
        }
    }

    /// Answers the card on screen, once it is turned over: schedules it
    /// here at once and returns (card id, rating) for the bridge to save.
    pub fn rate(&mut self, rating: Rating, now: i64) -> Option<(String, Rating)> {
        let r = self.review.as_mut()?;
        if !r.flipped {
            return None;
        }
        let id = r.card.as_ref()?.id.clone();
        self.deck.review(&id, rating, now).ok()?;
        let r = self.review.as_mut()?;
        r.done += 1;
        r.last = Some(rating);
        self.next_card(now);
        Some((id, rating))
    }

    /// The deck screen's cards, by the topic filter.
    pub fn deck_cards(&self) -> Vec<&Card> {
        self.deck
            .cards
            .iter()
            .filter(|c| {
                self.deck_topic
                    .as_ref()
                    .is_none_or(|t| c.topic.as_ref() == Some(t))
            })
            .collect()
    }

    // ---- Tutor ----

    /// Opens the tutor on a topic with an empty chat. Returns whether the
    /// first turn should be asked for (always, for a fresh chat).
    pub fn open_tutor(&mut self, topic: Topic) {
        let same = self.tutor_topic.as_ref().is_some_and(|t| t.id == topic.id);
        if !same {
            self.tutor.clear();
            self.tutor_stream.clear();
        }
        self.tutor_topic = Some(topic);
        self.view = StudyView::Tutor;
        self.notice = None;
    }

    /// The history to send: every line so far.
    pub fn tutor_history(&self) -> Vec<TutorTurn> {
        self.tutor.iter().map(|l| l.turn.clone()).collect()
    }

    /// Adds the user's message. `None` when there is nothing to send or a
    /// reply is still coming.
    pub fn tutor_say(&mut self, text: &str) -> Option<Vec<TutorTurn>> {
        let t = text.trim();
        if t.is_empty() || self.tutor_waiting || self.tutor_topic.is_none() {
            return None;
        }
        self.tutor.push(TutorLine {
            turn: TutorTurn::user(t),
            citations: Vec::new(),
            beyond: false,
            offline: false,
        });
        self.tutor_waiting = true;
        self.tutor_stream.clear();
        Some(self.tutor_history())
    }

    /// Starts a fresh session (the opening turn).
    pub fn tutor_start(&mut self) -> Option<Vec<TutorTurn>> {
        if self.tutor_waiting || self.tutor_topic.is_none() {
            return None;
        }
        let topic = self.tutor_topic.clone()?;
        self.tutor.clear();
        self.tutor.push(TutorLine {
            turn: TutorTurn::user(tern_agent::study::tutor_opening(self.tutor_mode, &topic)),
            citations: Vec::new(),
            beyond: false,
            offline: false,
        });
        self.tutor_waiting = true;
        self.tutor_stream.clear();
        // The opening is sent as an empty history; the agent adds it.
        Some(Vec::new())
    }

    pub fn tutor_text(&mut self, t: &str) {
        if self.tutor_waiting {
            self.tutor_stream.push_str(t);
        }
    }

    pub fn tutor_done(&mut self, reply: Result<tern_agent::study::TutorReply, String>) {
        self.tutor_waiting = false;
        self.tutor_stream.clear();
        match reply {
            Ok(r) => {
                self.notice = r.note.clone();
                self.tutor.push(TutorLine {
                    turn: TutorTurn::tutor(r.text),
                    citations: r.citations,
                    beyond: r.beyond,
                    offline: r.offline,
                });
            }
            Err(e) => self.notice = Some(e),
        }
    }

    /// Switching mode mid-chat tells the tutor; on an empty chat it only
    /// changes how the session will start.
    pub fn set_tutor_mode(&mut self, mode: TutorMode) -> Option<Vec<TutorTurn>> {
        if self.tutor_mode == mode {
            return None;
        }
        self.tutor_mode = mode;
        if self.tutor.is_empty() {
            return None;
        }
        self.tutor_say(&format!("Switch to {} mode and continue.", mode.label()))
    }

    // ---- Quiz ----

    /// Opens the quiz setup for a topic, or all materials.
    pub fn open_quiz(&mut self, topic: Option<Topic>) {
        self.quiz_topic = topic;
        self.quiz = None;
        self.view = StudyView::Quiz;
        self.notice = None;
    }

    pub fn quiz_ready(&mut self, quiz: Quiz, note: Option<String>, now: i64) {
        self.busy = None;
        self.notice = note;
        self.quiz = Some(QuizRun::new(quiz, now));
    }

    // ---- Viva ----

    pub fn open_viva(&mut self) {
        self.view = StudyView::Viva;
        self.notice = None;
        if self.viva.as_ref().is_some_and(|v| v.report.is_some()) {
            self.viva = None;
        }
    }

    /// The next question arrived (or the first).
    pub fn viva_next(&mut self, viva: Viva, now: Instant) {
        match &mut self.viva {
            Some(run) => {
                run.viva = viva;
                run.thinking = false;
                run.asked = now;
            }
            None => {
                let mut run = VivaRun::new(viva, now);
                run.thinking = false;
                self.viva = Some(run);
            }
        }
        self.busy = None;
    }

    pub fn viva_report(
        &mut self,
        viva: Viva,
        report: VivaReport,
        path: Option<std::path::PathBuf>,
        now: Instant,
    ) {
        if let Some(run) = &mut self.viva {
            run.viva = viva;
            run.thinking = false;
            run.report = Some((report, path));
            run.ended = Some(now);
        }
        self.busy = None;
    }

    /// The viva is running and waiting for an answer.
    pub fn viva_waiting(&self) -> bool {
        self.viva
            .as_ref()
            .is_some_and(|v| !v.thinking && v.report.is_none() && v.viva.waiting().is_some())
    }

    /// The clock should tick: a viva is on screen and not over.
    pub fn ticking(&self) -> bool {
        self.view == StudyView::Viva && self.viva.as_ref().is_some_and(|v| v.report.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_agent::cards::{Card, Deck, Rating};
    use tern_agent::study::{GradeMethod, Mark, Question, QuestionKind, TutorReply, VivaTurn};

    fn topic() -> Topic {
        Topic {
            id: "osmosis".into(),
            title: "Osmosis".into(),
            objectives: vec![],
            chunks: vec![],
            anchors: vec![],
        }
    }

    fn quiz(n: usize) -> Quiz {
        Quiz {
            id: "q1".into(),
            title: "Osmosis".into(),
            topic: Some("osmosis".into()),
            difficulty: Difficulty::Easy,
            questions: (1..=n)
                .map(|i| Question {
                    n: i,
                    kind: QuestionKind::TrueFalse,
                    prompt: format!("Statement {i}"),
                    choices: vec![],
                    answer: "true".into(),
                    explanation: String::new(),
                    topic: None,
                    anchor: None,
                    source_text: String::new(),
                })
                .collect(),
            created_at: 0,
            written_by: "fallback".into(),
        }
    }

    #[test]
    fn prefs_set_the_defaults() {
        let prefs = tern_agent::Preferences {
            study_quiz_count: Some(10),
            study_difficulty: Some("hard".into()),
            study_viva_questions: Some(4),
            study_offline: true,
            ..Default::default()
        };
        let s = StudyState::from_prefs(&prefs);
        assert_eq!(s.quiz_count, 10);
        assert_eq!(s.quiz_difficulty, Difficulty::Hard);
        assert_eq!(s.viva_questions, 4);
        assert!(s.offline);
        assert_eq!(StudyState::new().quiz_count, 5);
    }

    #[test]
    fn tutor_flow_streams_and_records_citations() {
        let mut s = StudyState::new();
        assert!(s.tutor_say("hi").is_none(), "no topic yet");
        s.open_tutor(topic());
        assert_eq!(s.view, StudyView::Tutor);
        assert_eq!(s.tutor_start(), Some(vec![]));
        assert_eq!(s.tutor.len(), 1);
        assert!(s.tutor_say("again").is_none(), "a reply is coming");
        s.tutor_text("Water ");
        s.tutor_text("moves [S1].");
        assert_eq!(s.tutor_stream, "Water moves [S1].");
        s.tutor_done(Ok(TutorReply {
            text: "Water moves [S1].".into(),
            citations: vec!["[S1] bio.pdf p. 4".into()],
            ..TutorReply::default()
        }));
        assert!(s.tutor_stream.is_empty());
        assert_eq!(s.tutor[1].citations.len(), 1);
        let h = s.tutor_say("why?").unwrap();
        assert_eq!(h.len(), 3);
        s.tutor_done(Err("down".into()));
        assert_eq!(s.notice.as_deref(), Some("down"));
        // Mode switch mid-chat sends a note.
        assert!(s.set_tutor_mode(TutorMode::Socratic).is_some());
        // Reopening the same topic keeps the chat.
        s.go_home();
        s.open_tutor(topic());
        assert_eq!(s.tutor.len(), 4);
    }

    #[test]
    fn quiz_steps_through_questions() {
        let mut s = StudyState::new();
        s.open_quiz(Some(topic()));
        s.quiz_ready(quiz(2), None, 0);
        let run = s.quiz.as_mut().unwrap();
        assert_eq!(run.question().unwrap().n, 1);
        assert!(run.graded().is_none());
        run.record(
            1,
            "t".into(),
            Grade::new(Mark::Correct, "Right.", GradeMethod::Exact),
        );
        run.record(
            1,
            "f".into(),
            Grade::new(Mark::Incorrect, "No.", GradeMethod::Exact),
        );
        assert_eq!(run.attempt.answers.len(), 1, "one grade per question");
        assert_eq!(run.graded().unwrap().grade.mark, Mark::Correct);
        assert!(!run.next());
        assert!(run.is_last());
        run.record(
            2,
            "f".into(),
            Grade::new(Mark::Partly, "Half.", GradeMethod::Lexical),
        );
        assert_eq!(run.running_score(), "1.5 / 2 so far");
        assert!(run.next());
    }

    #[test]
    fn viva_clock_and_waiting() {
        let mut s = StudyState::new();
        s.open_viva();
        assert!(!s.ticking());
        let t0 = Instant::now();
        let mut v = Viva::new("v1", "FYP", 3, 0);
        v.turns.push(VivaTurn {
            level: 1,
            question: "What is it?".into(),
            answer: String::new(),
            follow_up: false,
            topic: None,
            seconds: 0,
        });
        s.viva_next(v.clone(), t0);
        assert!(s.viva_waiting());
        assert!(s.ticking());
        let run = s.viva.as_ref().unwrap();
        assert_eq!(run.clock(t0 + Duration::from_secs(125)), "02:05");
        assert_eq!(run.progress(), "Question 1 of 3");
        let report = tern_study_report();
        s.viva_report(v, report, None, t0 + Duration::from_secs(200));
        assert!(!s.ticking());
        assert_eq!(
            s.viva
                .as_ref()
                .unwrap()
                .clock(t0 + Duration::from_secs(999)),
            "03:20"
        );
        s.open_viva();
        assert!(s.viva.is_none(), "a finished viva makes way for a new one");
    }

    #[test]
    fn review_flips_rates_and_moves_on() {
        use tern_agent::cards::{CardKind, CardState};
        let mut s = StudyState::new();
        s.offset = 0;
        let mut d = Deck::default();
        d.add(vec![
            Card::new(CardKind::Basic, "Front one?", "Back one", "you", 0),
            Card::new(CardKind::Cloze, "A {{c1::gap}} here", "", "you", 0),
        ]);
        s.deck_ready(d);
        let now = 1_000_000;
        assert_eq!(s.due(None, now), 2);
        s.open_review(None, now);
        assert_eq!(s.view, StudyView::Cards);
        let first = s.review.as_ref().unwrap().card.clone().unwrap();
        assert_eq!(first.id, "k1");
        assert!(s.rate(Rating::Good, now).is_none(), "turn it over first");
        let t0 = Instant::now();
        s.flip(t0);
        let r = s.review.as_ref().unwrap();
        let mid = r.turn(t0 + FLIP / 4, false);
        assert!(mid > 0.5, "front-loaded: {mid}");
        assert_eq!(r.turn(t0, true), 1.0, "reduced motion turns at once");
        assert!(r.turning(t0, false));
        assert_eq!(ReviewRun::face(0.0), (1.0, false));
        assert_eq!(ReviewRun::face(1.0), (1.0, true));
        assert!(ReviewRun::face(0.5).0 < 0.05);
        assert_eq!(s.rate(Rating::Good, now), Some(("k1".into(), Rating::Good)));
        assert_eq!(s.deck.get("k1").unwrap().sched.state, CardState::Learning);
        let r = s.review.as_ref().unwrap();
        assert_eq!(r.card.as_ref().unwrap().id, "k2");
        assert!(!r.flipped);
        assert_eq!((r.done, r.last), (1, Some(Rating::Good)));
        s.flip(t0);
        s.rate(Rating::Easy, now);
        // k1 is learning, due in ten minutes: learn-ahead brings it back.
        assert_eq!(s.review.as_ref().unwrap().card.as_ref().unwrap().id, "k1");
        s.flip(t0);
        s.rate(Rating::Good, now + 600);
        assert!(s.review.as_ref().unwrap().card.is_none(), "all done");
        s.deck_topic = Some("none".into());
        assert!(s.deck_cards().is_empty());
        s.deck_topic = None;
        assert_eq!(s.deck_cards().len(), 2);
        s.editing = Some("k9".into());
        s.deck_ready(s.deck.clone());
        assert!(s.editing.is_none(), "a deleted card stops editing");
    }

    fn tern_study_report() -> VivaReport {
        VivaReport {
            rubric: tern_agent::study::Rubric {
                clarity: 3,
                correctness: 3,
                depth: 3,
                sources: 3,
            },
            summary: String::new(),
            strengths: vec![],
            weaknesses: vec![],
            suggested_topics: vec![],
            notes: vec![],
            method: "offline estimate".into(),
        }
    }

    #[test]
    fn tabs_keep_a_tool_under_way() {
        let mut s = StudyState::new();
        assert_eq!(s.tab, StudyTab::Chat);
        s.set_tab(StudyTab::Quiz);
        assert_eq!((s.tab, s.view), (StudyTab::Quiz, StudyView::Quiz));
        s.set_tab(StudyTab::Cards);
        assert_eq!(s.view, StudyView::Deck);
        s.view = StudyView::Cards;
        s.set_tab(StudyTab::Viva);
        s.set_tab(StudyTab::Cards);
        assert_eq!(s.view, StudyView::Deck, "no review was open");
        s.view = StudyView::Tutor;
        s.set_tab(StudyTab::Chat);
        assert_eq!(s.view, StudyView::Tutor, "the tutor stays in the chat tab");
        s.set_tab(StudyTab::Explain);
        assert_eq!(s.view, StudyView::Home);
        for t in StudyTab::ALL {
            assert_eq!(StudyTab::ALL[t.index()], t);
        }
        assert_eq!(StudyTab::of(StudyView::Deck), StudyTab::Cards);
        assert!(StudyTab::Quiz.needs_materials() && !StudyTab::Chat.needs_materials());
    }

    #[test]
    fn the_composer_can_drive_a_mode() {
        assert_eq!(
            intent("Quiz me on paging"),
            Some((StudyTab::Quiz, "paging".into()))
        );
        assert_eq!(
            intent("explain TLB lookups in 3D"),
            Some((StudyTab::Explain, "TLB lookups".into()))
        );
        assert_eq!(
            intent("Flashcards for chapter 3").map(|i| i.0),
            Some(StudyTab::Cards)
        );
        assert_eq!(intent("Examine me").map(|i| i.0), Some(StudyTab::Viva));
        assert_eq!(intent("What is a page table?"), None);
    }

    #[test]
    fn the_status_line_says_what_to_do_first() {
        let s = StudyState::new();
        assert!(s.status_line(0).starts_with("Add a PDF"));
    }
}
