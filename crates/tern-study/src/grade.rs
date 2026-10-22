//! Grading answers with the verdict vocabulary.
//!
//! Multiple choice, true/false, and fill-in-the-blank are graded exactly
//! against the key, with no model. Short answers go to the model judge
//! ([`judge_prompt`], [`parse_judgment`]) with the key and the passage; a
//! word-overlap check ([`grade_lexical`]) works offline and stands in when
//! the judge cannot be read.

use serde::{Deserialize, Serialize};

use crate::quiz::{Question, QuestionKind, truth};
use crate::text::{clip, edit_distance, field, json_in, normalise, overlap};

/// How an answer did. The glyphs match research claim verdicts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    Correct,
    Partly,
    Incorrect,
    #[default]
    Unanswered,
}

impl Mark {
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Correct => "✓",
            Self::Partly => "◐",
            Self::Incorrect => "✕",
            Self::Unanswered => "○",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Correct => "Correct",
            Self::Partly => "Partly right",
            Self::Incorrect => "Not right",
            Self::Unanswered => "No answer",
        }
    }

    /// Credit toward the score and mastery.
    pub fn credit(self) -> f32 {
        match self {
            Self::Correct => 1.0,
            Self::Partly => 0.5,
            Self::Incorrect | Self::Unanswered => 0.0,
        }
    }

    fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase().replace([' ', '-'], "_");
        Some(match s.as_str() {
            "correct" | "right" | "supported" | "full" | "yes" => Self::Correct,
            "partly" | "partial" | "partly_correct" | "partially_correct" | "partly_right" => {
                Self::Partly
            }
            "incorrect" | "wrong" | "not_correct" | "no" | "not_supported" => Self::Incorrect,
            "unanswered" | "blank" | "none" | "no_answer" => Self::Unanswered,
            _ => return None,
        })
    }
}

/// Who graded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradeMethod {
    /// Checked against the answer key.
    #[default]
    Exact,
    /// The model judged it against the key and the passage.
    Model,
    /// Word overlap with the key and the passage.
    Lexical,
}

impl GradeMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Exact => "answer key",
            Self::Model => "model judge",
            Self::Lexical => "word overlap",
        }
    }
}

/// A graded answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Grade {
    pub mark: Mark,
    pub credit: f32,
    pub feedback: String,
    pub method: GradeMethod,
}

impl Grade {
    pub fn new(mark: Mark, feedback: impl Into<String>, method: GradeMethod) -> Self {
        Self {
            mark,
            credit: mark.credit(),
            feedback: feedback.into(),
            method,
        }
    }
}

fn source_line(q: &Question) -> String {
    match &q.anchor {
        Some(a) => format!(" (see {})", a.label()),
        None => String::new(),
    }
}

fn with_explanation(q: &Question, lead: String) -> String {
    let mut s = lead;
    if !q.explanation.is_empty() {
        s.push(' ');
        s.push_str(&q.explanation);
    }
    s.push_str(&source_line(q));
    s
}

/// Grades against the key when the question has an exact answer
/// (multiple choice, true/false, fill-in-the-blank); `None` for short
/// answers.
pub fn grade_exact(q: &Question, given: &str) -> Option<Grade> {
    if given.trim().is_empty() {
        return Some(Grade::new(
            Mark::Unanswered,
            with_explanation(q, format!("The answer is {}.", q.answer)),
            GradeMethod::Exact,
        ));
    }
    let right = match q.kind {
        QuestionKind::ShortAnswer => return None,
        QuestionKind::MultipleChoice => {
            let g = given.trim();
            let picked = if g.chars().count() == 1 {
                g.chars()
                    .next()
                    .map(|c| c.to_ascii_uppercase())
                    .filter(char::is_ascii_uppercase)
                    .and_then(|c| q.choices.get((c as u8 - b'A') as usize))
                    .cloned()
                    .unwrap_or_else(|| g.to_owned())
            } else {
                g.to_owned()
            };
            normalise(&picked) == normalise(&q.answer)
        }
        QuestionKind::TrueFalse => truth(given) == truth(&q.answer),
        QuestionKind::FillBlank => blank_matches(given, &q.answer),
    };
    Some(if right {
        Grade::new(
            Mark::Correct,
            with_explanation(q, "Right.".into()),
            GradeMethod::Exact,
        )
    } else {
        Grade::new(
            Mark::Incorrect,
            with_explanation(q, format!("The answer is {}.", q.answer)),
            GradeMethod::Exact,
        )
    })
}

/// A blank's answer: the same words, or one small typo in a long word,
/// or a plural.
pub fn blank_matches(given: &str, key: &str) -> bool {
    let (g, k) = (normalise(given), normalise(key));
    if g.is_empty() {
        return false;
    }
    if g == k || g.trim_end_matches('s') == k.trim_end_matches('s') {
        return true;
    }
    k.chars().count() >= 5 && edit_distance(&g, &k) <= 1
}

/// Grades a short answer by word overlap with the key (and, less, with
/// the passage).
pub fn grade_lexical(q: &Question, given: &str) -> Grade {
    if given.trim().is_empty() {
        return Grade::new(
            Mark::Unanswered,
            with_explanation(q, format!("A good answer: {}", q.answer)),
            GradeMethod::Lexical,
        );
    }
    if let Some(g) = grade_exact(q, given) {
        return g;
    }
    let key = overlap(&q.answer, given);
    let passage = if q.source_text.is_empty() {
        0.0
    } else {
        overlap(given, &q.source_text)
    };
    let score = key.max(key * 0.7 + passage * 0.3);
    let (mark, lead) = if score >= 0.6 {
        (Mark::Correct, "That covers the key points.".to_owned())
    } else if score >= 0.3 {
        (
            Mark::Partly,
            format!("Partly there. A fuller answer: {}", q.answer),
        )
    } else {
        (
            Mark::Incorrect,
            format!("Not quite. A good answer: {}", q.answer),
        )
    };
    Grade::new(
        mark,
        format!("{lead}{}", source_line(q)),
        GradeMethod::Lexical,
    )
}

/// The prompt for the model judge: (system, user).
pub fn judge_prompt(q: &Question, given: &str) -> (String, String) {
    let system = "You grade a student's short answer against a model answer and the passage it came from. Be fair: accept other wording and extra correct detail; mark partly when a key point is missing or vague; mark incorrect when it is wrong or off topic. Give one or two sentences of feedback addressed to the student, saying what was right and what was missing. Reply with JSON only: {\"mark\": \"correct\" | \"partly\" | \"incorrect\", \"feedback\": \"…\"}".to_owned();
    let mut user = format!(
        "Question: {}\nModel answer: {}\nStudent's answer: {}\n",
        q.prompt,
        q.answer,
        clip(given, 1500)
    );
    if !q.source_text.is_empty() {
        user.push_str(&format!("Passage: {}\n", clip(&q.source_text, 2000)));
    }
    (system, user)
}

/// Reads the judge's reply. `None` when no mark can be found.
pub fn parse_judgment(text: &str, q: &Question) -> Option<Grade> {
    let (mark, feedback) = match json_in(text) {
        Some(v) => (
            field(&v, &["mark", "grade", "verdict", "result"]).and_then(Mark::parse)?,
            field(&v, &["feedback", "comment", "reason"])
                .unwrap_or_default()
                .to_owned(),
        ),
        None => {
            // A bare word first: `Partly — you missed …`.
            let first = text
                .split(|c: char| !c.is_alphabetic() && c != '_')
                .find(|w| !w.is_empty())?;
            let mark = Mark::parse(first)?;
            let rest = text.trim()[first.len()..]
                .trim_start_matches([' ', ':', '-', '—', '.'])
                .trim()
                .to_owned();
            (mark, rest)
        }
    };
    let mut fb = if feedback.is_empty() {
        format!("A good answer: {}", q.answer)
    } else {
        feedback
    };
    fb.push_str(&source_line(q));
    Some(Grade::new(mark, fb, GradeMethod::Model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::Anchor;

    fn q(kind: QuestionKind, answer: &str, choices: &[&str]) -> Question {
        Question {
            n: 1,
            kind,
            prompt: "?".into(),
            choices: choices.iter().map(|s| s.to_string()).collect(),
            answer: answer.into(),
            explanation: String::new(),
            topic: None,
            anchor: Some(Anchor {
                material: "m1".into(),
                file: "bio.pdf".into(),
                page: Some(2),
                section: None,
            }),
            source_text: "Mitochondria produce ATP through cellular respiration.".into(),
        }
    }

    #[test]
    fn exact_grading_for_keyed_kinds() {
        let mcq = q(
            QuestionKind::MultipleChoice,
            "ATP",
            &["Glucose", "ATP", "DNA"],
        );
        assert_eq!(grade_exact(&mcq, "b").unwrap().mark, Mark::Correct);
        assert_eq!(grade_exact(&mcq, "atp").unwrap().mark, Mark::Correct);
        let wrong = grade_exact(&mcq, "A").unwrap();
        assert_eq!(wrong.mark, Mark::Incorrect);
        assert!(wrong.feedback.contains("The answer is ATP"));
        assert!(wrong.feedback.contains("bio.pdf p. 2"));
        let tf = q(QuestionKind::TrueFalse, "false", &[]);
        assert_eq!(grade_exact(&tf, "F").unwrap().mark, Mark::Correct);
        assert_eq!(grade_exact(&tf, "true").unwrap().mark, Mark::Incorrect);
        assert_eq!(grade_exact(&tf, " ").unwrap().mark, Mark::Unanswered);
        let blank = q(QuestionKind::FillBlank, "mitochondria", &[]);
        assert_eq!(
            grade_exact(&blank, "Mitochondrias").unwrap().mark,
            Mark::Correct
        );
        assert_eq!(
            grade_exact(&blank, "mitocondria").unwrap().mark,
            Mark::Correct
        );
        assert_eq!(
            grade_exact(&blank, "ribosome").unwrap().mark,
            Mark::Incorrect
        );
        assert!(grade_exact(&q(QuestionKind::ShortAnswer, "x", &[]), "y").is_none());
    }

    #[test]
    fn lexical_grading_for_short_answers() {
        let sa = q(
            QuestionKind::ShortAnswer,
            "Mitochondria make ATP by cellular respiration",
            &[],
        );
        assert_eq!(
            grade_lexical(
                &sa,
                "They make ATP via cellular respiration in mitochondria"
            )
            .mark,
            Mark::Correct
        );
        assert_eq!(grade_lexical(&sa, "They make ATP").mark, Mark::Partly);
        assert_eq!(
            grade_lexical(&sa, "Photosynthesis in leaves").mark,
            Mark::Incorrect
        );
        assert_eq!(grade_lexical(&sa, "").credit, 0.0);
    }

    #[test]
    fn judge_replies_parse() {
        let sa = q(QuestionKind::ShortAnswer, "key", &[]);
        let g = parse_judgment(
            "```json\n{\"mark\": \"Partly\", \"feedback\": \"Missing respiration.\"}\n```",
            &sa,
        )
        .unwrap();
        assert_eq!(g.mark, Mark::Partly);
        assert_eq!(g.method, GradeMethod::Model);
        assert!(g.feedback.starts_with("Missing respiration."));
        let g = parse_judgment("Incorrect: that is photosynthesis.", &sa).unwrap();
        assert_eq!(g.mark, Mark::Incorrect);
        assert!(g.feedback.starts_with("that is photosynthesis."));
        assert!(parse_judgment("I am not sure", &sa).is_none());
        let (_, user) = judge_prompt(&sa, "my answer");
        assert!(user.contains("Passage: Mitochondria"));
    }
}
