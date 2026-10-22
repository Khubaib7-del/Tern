//! Quizzes: typed questions with an answer key and a source anchor each.
//!
//! The model writes questions from numbered passages ([`quiz_prompt`]);
//! [`parse_quiz`] reads its reply leniently and drops any question it
//! cannot check (a multiple-choice question whose answer is not one of its
//! choices, a true/false question without a truth value). With no model,
//! or nothing usable back, [`fallback_quiz`] builds questions straight from
//! the passages' sentences, deterministically.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::grade::Grade;
use crate::material::{Anchor, Source, sources_block};
use crate::text::{capitalise, clip, field, json_in, keywords, list, one_line, sentences, words};
use crate::topics::definition;

/// Most questions in one quiz.
pub const MAX_QUESTIONS: usize = 30;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    #[default]
    MultipleChoice,
    ShortAnswer,
    TrueFalse,
    FillBlank,
}

impl QuestionKind {
    pub const ALL: [Self; 4] = [
        Self::MultipleChoice,
        Self::ShortAnswer,
        Self::TrueFalse,
        Self::FillBlank,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::MultipleChoice => "Multiple choice",
            Self::ShortAnswer => "Short answer",
            Self::TrueFalse => "True or false",
            Self::FillBlank => "Fill in the blank",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::MultipleChoice => "mcq",
            Self::ShortAnswer => "short",
            Self::TrueFalse => "tf",
            Self::FillBlank => "blank",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase().replace([' ', '-', '/'], "_");
        Some(match s.as_str() {
            "mcq" | "multiple_choice" | "multiplechoice" | "choice" | "mc" => Self::MultipleChoice,
            "short" | "short_answer" | "open" | "free_text" | "sa" => Self::ShortAnswer,
            "tf" | "true_false" | "truefalse" | "true_or_false" | "boolean" => Self::TrueFalse,
            "blank" | "fill_blank" | "fill_in_the_blank" | "fill_in_blank" | "cloze" | "fill" => {
                Self::FillBlank
            }
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Easy,
    #[default]
    Medium,
    Hard,
}

impl Difficulty {
    pub fn label(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Medium => "Medium",
            Self::Hard => "Hard",
        }
    }

    /// Where on the mastery scale (0 to 1) a question of this difficulty sits.
    pub fn weight(self) -> f32 {
        match self {
            Self::Easy => 0.25,
            Self::Medium => 0.5,
            Self::Hard => 0.75,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "easy" | "e" | "1" => Self::Easy,
            "medium" | "m" | "2" | "normal" => Self::Medium,
            "hard" | "h" | "3" | "difficult" => Self::Hard,
            _ => return None,
        })
    }
}

/// What to quiz on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuizSpec {
    /// A topic id, or `None` for all the materials.
    pub topic: Option<String>,
    pub count: usize,
    pub difficulty: Difficulty,
    /// The kinds to mix; empty means all four.
    pub kinds: Vec<QuestionKind>,
}

impl Default for QuizSpec {
    fn default() -> Self {
        Self {
            topic: None,
            count: 5,
            difficulty: Difficulty::Medium,
            kinds: Vec::new(),
        }
    }
}

impl QuizSpec {
    pub fn kinds(&self) -> Vec<QuestionKind> {
        if self.kinds.is_empty() {
            QuestionKind::ALL.to_vec()
        } else {
            self.kinds.clone()
        }
    }

    pub fn count(&self) -> usize {
        self.count.clamp(1, MAX_QUESTIONS)
    }
}

/// One question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// 1-based.
    pub n: usize,
    pub kind: QuestionKind,
    pub prompt: String,
    /// Multiple choice only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// The key: the right choice's text, `true`/`false`, the missing word,
    /// or a model answer for short answers.
    pub answer: String,
    /// Why the answer is right, shown after answering.
    #[serde(default)]
    pub explanation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    /// The passage the question was written from, for grading.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_text: String,
}

/// A quiz and its key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quiz {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    pub difficulty: Difficulty,
    pub questions: Vec<Question>,
    pub created_at: i64,
    /// `model` or `fallback`: who wrote the questions.
    pub written_by: String,
}

/// One answer given.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Answered {
    pub n: usize,
    pub given: String,
    pub grade: Grade,
}

/// One go at a quiz.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QuizAttempt {
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub answers: Vec<Answered>,
}

impl QuizAttempt {
    /// Credit earned (right 1, partly 0.5).
    pub fn score(&self) -> f32 {
        self.answers.iter().map(|a| a.grade.credit).sum()
    }

    /// `3.5 / 5 (70%)`.
    pub fn score_label(&self, total: usize) -> String {
        let s = self.score();
        let pct = if total == 0 {
            0.0
        } else {
            s / total as f32 * 100.0
        };
        let s = if (s - s.round()).abs() < 0.01 {
            format!("{}", s.round() as u32)
        } else {
            format!("{s:.1}")
        };
        format!("{s} / {total} ({pct:.0}%)")
    }
}

/// A quiz with every attempt at it, saved in the quiz history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuizRecord {
    pub quiz: Quiz,
    pub attempts: Vec<QuizAttempt>,
}

impl QuizRecord {
    /// The quiz and its latest attempt as Markdown.
    pub fn to_markdown(&self) -> String {
        let q = &self.quiz;
        let mut s = format!("# Quiz: {}\n\n", q.title);
        s.push_str(&format!(
            "{} · {} questions · written by {}\n\n",
            q.difficulty.label(),
            q.questions.len(),
            q.written_by
        ));
        let attempt = self.attempts.last();
        if let Some(a) = attempt {
            s.push_str(&format!(
                "**Score:** {}\n\n",
                a.score_label(q.questions.len())
            ));
        }
        for question in &q.questions {
            s.push_str(&format!(
                "## {}. {}\n\n{}\n\n",
                question.n,
                question.kind.label(),
                question.prompt
            ));
            for (i, c) in question.choices.iter().enumerate() {
                s.push_str(&format!("- {}. {}\n", letter(i), c));
            }
            if !question.choices.is_empty() {
                s.push('\n');
            }
            s.push_str(&format!("**Answer:** {}\n\n", question.answer));
            if let Some(a) = attempt.and_then(|a| a.answers.iter().find(|x| x.n == question.n)) {
                s.push_str(&format!(
                    "**You said:** {} — {} {}\n\n{}\n\n",
                    if a.given.trim().is_empty() {
                        "(nothing)"
                    } else {
                        a.given.trim()
                    },
                    a.grade.mark.glyph(),
                    a.grade.mark.label(),
                    a.grade.feedback
                ));
            }
            if let Some(anchor) = &question.anchor {
                s.push_str(&format!("Source: {}\n\n", anchor.label()));
            }
        }
        s
    }
}

/// `A`, `B`, `C`…
pub fn letter(i: usize) -> char {
    (b'A' + (i as u8 % 26)) as char
}

/// The prompt asking a model for a quiz: (system, user).
pub fn quiz_prompt(spec: &QuizSpec, title: &str, sources: &[Source]) -> (String, String) {
    let kinds: Vec<&str> = spec.kinds().iter().map(|k| k.key()).collect();
    let system = format!(
        "You write quiz questions for a student, using only the numbered passages from their own study materials. Write exactly {n} questions at {d} difficulty, mixing these types: {kinds}. Types: \"mcq\" (4 choices, one right; the answer is the right choice's exact text), \"tf\" (a statement; the answer is true or false), \"blank\" (a sentence with ___ for one missing key term; the answer is that term), \"short\" (a question answered in one to three sentences; the answer is a model answer). Every question names the passage it comes from as \"source\": \"S2\" and gives a one-sentence explanation. Do not ask about anything the passages do not say. Easy questions recall facts; medium ones check understanding; hard ones apply or compare ideas. Reply with JSON only: {{\"questions\": [{{\"type\": \"mcq\", \"question\": \"…\", \"choices\": [\"…\"], \"answer\": \"…\", \"explanation\": \"…\", \"source\": \"S1\"}}]}}",
        n = spec.count(),
        d = spec.difficulty.label().to_lowercase(),
        kinds = kinds.join(", "),
    );
    let user = format!(
        "Quiz on: {title}\n\nPassages:\n\n{}",
        sources_block(sources)
    );
    (system, user)
}

/// Reads a model's quiz. Questions that cannot be checked are dropped.
pub fn parse_quiz(text: &str, sources: &[Source], topic: Option<&str>) -> Vec<Question> {
    let Some(v) = json_in(text) else {
        return Vec::new();
    };
    let items = match &v {
        Value::Array(a) => a.clone(),
        _ => v
            .get("questions")
            .or_else(|| v.get("quiz"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    let mut out = Vec::new();
    for item in items.iter().take(MAX_QUESTIONS) {
        if let Some(mut q) = parse_question(item, sources) {
            q.n = out.len() + 1;
            q.topic = topic.map(str::to_owned);
            out.push(q);
        }
    }
    out
}

fn parse_question(item: &Value, sources: &[Source]) -> Option<Question> {
    let prompt = one_line(field(
        item,
        &["question", "prompt", "statement", "text", "q"],
    )?);
    let choices = list(item, &["choices", "options", "answers"]);
    let kind = field(item, &["type", "kind", "format"])
        .and_then(QuestionKind::parse)
        .unwrap_or(if choices.len() >= 2 {
            QuestionKind::MultipleChoice
        } else {
            QuestionKind::ShortAnswer
        });
    let raw_answer = match item
        .get("answer")
        .or_else(|| item.get("correct"))
        .or_else(|| item.get("key"))
    {
        Some(Value::String(s)) => one_line(s),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return None,
    };
    if raw_answer.is_empty() {
        return None;
    }
    let mut q = Question {
        n: 0,
        kind,
        prompt,
        choices: Vec::new(),
        answer: raw_answer.clone(),
        explanation: field(item, &["explanation", "why", "rationale"])
            .map(one_line)
            .unwrap_or_default(),
        topic: None,
        anchor: None,
        source_text: String::new(),
    };
    match kind {
        QuestionKind::MultipleChoice => {
            let choices: Vec<String> = choices.into_iter().map(|c| strip_letter(&c)).collect();
            if choices.len() < 2 {
                return None;
            }
            let ans = strip_letter(&raw_answer);
            let pick = choices
                .iter()
                .position(|c| crate::text::normalise(c) == crate::text::normalise(&ans))
                .or_else(|| choice_index(&raw_answer, choices.len()));
            q.answer = choices.get(pick?)?.clone();
            q.choices = choices;
        }
        QuestionKind::TrueFalse => {
            q.answer = truth(&raw_answer)?.to_string();
        }
        QuestionKind::FillBlank => {
            if !q.prompt.contains("___") {
                // Blank the answer where the sentence has it.
                let r = crate::text::find_ci(&q.prompt, &raw_answer)?;
                q.prompt.replace_range(r, "___");
            }
        }
        QuestionKind::ShortAnswer => {}
    }
    let src = field(item, &["source", "passage", "ref"])
        .map(str::to_owned)
        .or_else(|| {
            item.get("source")
                .and_then(Value::as_u64)
                .map(|n| n.to_string())
        });
    let n = src.and_then(|s| {
        s.trim()
            .trim_matches(['[', ']'])
            .trim_start_matches(['S', 's'])
            .parse::<usize>()
            .ok()
    });
    let source = n
        .and_then(|n| sources.iter().find(|s| s.n == n))
        .or_else(|| best_source(&q, sources));
    if let Some(s) = source {
        q.anchor = Some(s.anchor.clone());
        q.source_text = s.text.clone();
    }
    Some(q)
}

/// The passage sharing the most words with the question and its answer.
fn best_source<'a>(q: &Question, sources: &'a [Source]) -> Option<&'a Source> {
    let key = format!("{} {}", q.prompt, q.answer);
    sources
        .iter()
        .map(|s| (crate::text::overlap(&key, &s.text), s))
        .filter(|(o, _)| *o > 0.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
}

/// `B) Paris` → `Paris`; `(c) x` → `x`.
fn strip_letter(s: &str) -> String {
    let t = s.trim();
    let mut c = t.chars();
    let (a, b) = (c.next(), c.next());
    if let (Some(a), Some(b)) = (a, b)
        && a.is_ascii_alphabetic()
        && matches!(b, ')' | '.' | ':')
        && t.len() > 2
    {
        return t[2..].trim().to_owned();
    }
    if let Some(rest) = t.strip_prefix('(')
        && let Some((l, r)) = rest.split_once(')')
        && l.len() == 1
    {
        return r.trim().to_owned();
    }
    t.to_owned()
}

/// `B`, `b)`, `2` (1-based) → index.
fn choice_index(s: &str, n: usize) -> Option<usize> {
    let t = s.trim().trim_matches(['(', ')', '.', ':']).trim();
    if t.len() == 1 {
        let c = t.chars().next()?.to_ascii_uppercase();
        if c.is_ascii_uppercase() {
            let i = (c as u8 - b'A') as usize;
            return (i < n).then_some(i);
        }
    }
    let i: usize = t.parse().ok()?;
    (1..=n).contains(&i).then(|| i - 1)
}

/// `true`, `T`, `yes`, `correct` → true.
pub fn truth(s: &str) -> Option<bool> {
    match crate::text::normalise(s).as_str() {
        "true" | "t" | "yes" | "y" | "correct" | "right" => Some(true),
        "false" | "f" | "no" | "n" | "incorrect" | "wrong" => Some(false),
        _ => None,
    }
}

/// Questions without a model, built from the passages' sentences: blanks
/// on key terms, true/false on real and altered statements, multiple
/// choice on definitions with other terms as distractors, and short
/// answers asking to explain a passage. Deterministic for the same input.
pub fn fallback_quiz(spec: &QuizSpec, sources: &[Source], topic: Option<&str>) -> Vec<Question> {
    // Candidate facts: (sentence, key term, source).
    let mut facts: Vec<(String, String, &Source)> = Vec::new();
    for s in sources {
        let body: String = s
            .text
            .lines()
            .filter(|l| crate::material::heading(l).is_none())
            .collect::<Vec<_>>()
            .join("\n");
        for sent in sentences(&body) {
            let n = sent.split_whitespace().count();
            if !(6..=40).contains(&n) {
                continue;
            }
            let term = definition(&sent)
                .map(|(t, _)| t)
                .or_else(|| key_term(&sent, &s.text));
            if let Some(t) = term {
                facts.push((sent, t, s));
            }
        }
    }
    if facts.is_empty() {
        return Vec::new();
    }
    let all_terms: Vec<String> = {
        let mut v: Vec<String> = facts.iter().map(|f| f.1.clone()).collect();
        let joined: String = sources
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        v.extend(keywords(&joined, 12));
        let mut seen = std::collections::BTreeSet::new();
        v.retain(|t| seen.insert(t.to_lowercase()));
        v
    };
    let kinds = spec.kinds();
    let mut out: Vec<Question> = Vec::new();
    // Spread picks across the facts.
    let step = (facts.len() / spec.count()).max(1);
    let mut i = 0usize;
    let mut tries = 0usize;
    while out.len() < spec.count() && tries < facts.len() * 2 + spec.count() {
        let (sent, term, src) = &facts[(i * step + i / facts.len().max(1)) % facts.len()];
        let kind = kinds[out.len() % kinds.len()];
        if let Some(mut q) = make_fallback(kind, sent, term, src, &all_terms, out.len()) {
            if !out.iter().any(|x| x.prompt == q.prompt) {
                q.n = out.len() + 1;
                q.topic = topic.map(str::to_owned);
                out.push(q);
            }
        }
        i += 1;
        tries += 1;
    }
    out
}

/// The sentence's rarest-looking content word that also recurs in the
/// passage: a term worth blanking.
fn key_term(sentence: &str, passage: &str) -> Option<String> {
    let pw = words(passage);
    words(sentence)
        .into_iter()
        .filter(|w| w.chars().count() >= 5)
        .max_by_key(|w| {
            (
                pw.iter().filter(|x| *x == w).count().min(3),
                w.chars().count(),
            )
        })
}

fn make_fallback(
    kind: QuestionKind,
    sentence: &str,
    term: &str,
    src: &Source,
    terms: &[String],
    salt: usize,
) -> Option<Question> {
    let lower = sentence.to_lowercase();
    let r = crate::text::find_ci(sentence, term)?;
    let (at, end) = (r.start, r.end);
    let base = Question {
        n: 0,
        kind,
        prompt: String::new(),
        choices: Vec::new(),
        answer: String::new(),
        explanation: format!("From {}: \"{}\"", src.label(), clip(sentence, 200)),
        topic: None,
        anchor: Some(src.anchor.clone()),
        source_text: src.text.clone(),
    };
    let blanked = {
        let mut s = sentence.to_owned();
        s.replace_range(at..end, "___");
        s
    };
    let others: Vec<&String> = terms
        .iter()
        .filter(|t| t.to_lowercase() != term.to_lowercase() && !lower.contains(&t.to_lowercase()))
        .collect();
    Some(match kind {
        QuestionKind::FillBlank => Question {
            prompt: blanked,
            answer: sentence[at..end].to_owned(),
            ..base
        },
        QuestionKind::MultipleChoice => {
            if others.len() < 2 {
                return None;
            }
            let mut choices: Vec<String> = others
                .iter()
                .cycle()
                .skip(salt % others.len())
                .take(3.min(others.len()))
                .map(|t| capitalise(t))
                .collect();
            let right = capitalise(&sentence[at..end]);
            let pos = salt % (choices.len() + 1);
            choices.insert(pos, right.clone());
            Question {
                prompt: format!("Which term completes this? {blanked}"),
                choices,
                answer: right,
                ..base
            }
        }
        QuestionKind::TrueFalse => {
            // Every other one is altered by swapping the term.
            if salt % 2 == 1
                && let Some(other) = others.get(salt % others.len().max(1))
            {
                let mut altered = sentence.to_owned();
                altered.replace_range(at..end, other);
                Question {
                    prompt: altered,
                    answer: "false".into(),
                    explanation: format!(
                        "The material says {} here, not {other}. {}",
                        &sentence[at..end],
                        base.explanation
                    ),
                    ..base
                }
            } else {
                Question {
                    prompt: sentence.to_owned(),
                    answer: "true".into(),
                    ..base
                }
            }
        }
        QuestionKind::ShortAnswer => Question {
            prompt: format!(
                "In one or two sentences: what does your material say about {}?",
                term
            ),
            answer: sentence.to_owned(),
            ..base
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources() -> Vec<Source> {
        let texts = [
            "Photosynthesis is a process that converts light energy into chemical energy in plants. Chlorophyll absorbs mostly blue and red light.",
            "Mitochondria produce ATP through cellular respiration in the cell. Respiration uses oxygen and releases carbon dioxide as waste.",
            "Osmosis is the movement of water across a semipermeable membrane toward higher solute concentration.",
        ];
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| Source {
                n: i + 1,
                chunk: format!("m1#{}", i + 1),
                anchor: Anchor {
                    material: "m1".into(),
                    file: "bio.pdf".into(),
                    page: Some(i as u32 + 1),
                    section: None,
                },
                text: (*t).into(),
            })
            .collect()
    }

    #[test]
    fn parses_a_messy_model_quiz_and_drops_bad_questions() {
        let reply = r#"Sure! ```json
{"questions": [
 {"type": "multiple choice", "question": "What do mitochondria make?", "choices": ["A) Glucose", "B) ATP", "C) DNA", "D) Water"], "answer": "B", "explanation": "They make ATP.", "source": "S2"},
 {"type": "tf", "question": "Osmosis moves water.", "answer": true, "source": 3},
 {"type": "blank", "question": "Chlorophyll absorbs blue and red light.", "answer": "Chlorophyll"},
 {"type": "mcq", "question": "Bad one", "choices": ["x", "y"], "answer": "z"},
 {"type": "tf", "question": "Unclear", "answer": "maybe"},
 {"type": "short", "question": "Explain osmosis.", "answer": "Water moves across a membrane toward more solute."}
]}
```"#;
        let qs = parse_quiz(reply, &sources(), Some("cells"));
        assert_eq!(qs.len(), 4, "{qs:#?}");
        assert_eq!(qs[0].answer, "ATP");
        assert_eq!(qs[0].choices[0], "Glucose");
        assert_eq!(qs[0].anchor.as_ref().unwrap().page, Some(2));
        assert_eq!(qs[1].answer, "true");
        assert_eq!(qs[1].anchor.as_ref().unwrap().page, Some(3));
        assert_eq!(qs[2].prompt, "___ absorbs blue and red light.");
        assert_eq!(
            qs[2].anchor.as_ref().unwrap().page,
            Some(1),
            "best source by overlap"
        );
        assert_eq!(qs[3].kind, QuestionKind::ShortAnswer);
        assert_eq!(qs[3].n, 4);
        assert_eq!(qs[3].topic.as_deref(), Some("cells"));
        assert!(parse_quiz("nothing", &sources(), None).is_empty());
    }

    #[test]
    fn fallback_is_deterministic_and_typed() {
        let spec = QuizSpec {
            count: 6,
            ..QuizSpec::default()
        };
        let a = fallback_quiz(&spec, &sources(), None);
        let b = fallback_quiz(&spec, &sources(), None);
        assert_eq!(a, b);
        assert!(a.len() >= 4, "{a:#?}");
        for q in &a {
            assert!(q.anchor.is_some());
            assert!(!q.answer.is_empty());
            match q.kind {
                QuestionKind::MultipleChoice => assert!(q.choices.contains(&q.answer)),
                QuestionKind::FillBlank => assert!(q.prompt.contains("___")),
                QuestionKind::TrueFalse => assert!(truth(&q.answer).is_some()),
                QuestionKind::ShortAnswer => {}
            }
        }
    }

    #[test]
    fn fallback_respects_kinds() {
        let spec = QuizSpec {
            count: 3,
            kinds: vec![QuestionKind::FillBlank],
            ..QuizSpec::default()
        };
        let qs = fallback_quiz(&spec, &sources(), None);
        assert_eq!(qs.len(), 3);
        assert!(qs.iter().all(|q| q.kind == QuestionKind::FillBlank));
    }

    #[test]
    fn quiz_round_trips_and_renders() {
        let qs = fallback_quiz(&QuizSpec::default(), &sources(), None);
        let rec = QuizRecord {
            quiz: Quiz {
                id: "q1".into(),
                title: "Cells".into(),
                topic: None,
                difficulty: Difficulty::Easy,
                questions: qs,
                created_at: 0,
                written_by: "fallback".into(),
            },
            attempts: vec![],
        };
        let json = serde_json::to_string(&rec).unwrap();
        let back: QuizRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rec);
        assert!(rec.to_markdown().starts_with("# Quiz: Cells"));
        assert_eq!(Difficulty::parse("HARD"), Some(Difficulty::Hard));
        assert_eq!(
            QuestionKind::parse("fill-in-the-blank"),
            Some(QuestionKind::FillBlank)
        );
    }

    #[test]
    fn prompt_names_count_and_passages() {
        let (sys, user) = quiz_prompt(&QuizSpec::default(), "Cells", &sources());
        assert!(sys.contains("exactly 5 questions"));
        assert!(user.contains("[S2] bio.pdf p. 2"));
    }
}
