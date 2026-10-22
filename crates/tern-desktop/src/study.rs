//! The Study harness's window state, free of GPUI so it can be tested:
//! the library as last loaded, which Study view is open, the tutor chat,
//! the quiz in progress, and the viva with its timer.
//!
//! The bridge does the work (`tern_agent::study`); this only holds what
//! the screen shows and moves between steps.

use std::time::{Duration, Instant};

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
}

impl StudyState {
    pub fn new() -> Self {
        Self {
            quiz_count: 5,
            viva_questions: 6,
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

    pub fn go_home(&mut self) {
        self.view = StudyView::Home;
        self.busy = None;
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
}
