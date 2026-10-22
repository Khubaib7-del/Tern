//! Viva practice: an examiner asks progressively harder questions about
//! the user's project or materials, follows up on weak answers, and ends
//! with a scored rubric report.
//!
//! Levels run 1 (recall) to 5 (defend and extend). Each answered question
//! moves the level up one; a short or off-material answer gets a follow-up
//! at the same level instead ([`next_step`]). The report scores clarity,
//! correctness, depth, and use of sources from 1 to 5, from the model
//! ([`report_prompt`], [`parse_report`]) or, offline, from length,
//! overlap with the materials, and references to them
//! ([`fallback_report`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::material::{Source, sources_block};
use crate::text::{clip, field, json_in, list, one_line, overlap, words};
use crate::topics::Topic;

/// Highest level.
pub const TOP_LEVEL: u8 = 5;
/// Questions in a default viva.
pub const DEFAULT_QUESTIONS: usize = 6;

/// One question and its answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VivaTurn {
    pub level: u8,
    pub question: String,
    #[serde(default)]
    pub answer: String,
    /// A follow-up to the previous answer.
    #[serde(default)]
    pub follow_up: bool,
    /// The topic the question is about, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Seconds taken to answer.
    #[serde(default)]
    pub seconds: u32,
}

/// A viva session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viva {
    pub id: String,
    /// What is being examined: the project title or the materials.
    pub title: String,
    pub turns: Vec<VivaTurn>,
    /// How many questions to ask.
    pub planned: usize,
    pub started_at: i64,
    #[serde(default)]
    pub ended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<VivaReport>,
}

impl Viva {
    pub fn new(id: &str, title: &str, planned: usize, now: i64) -> Self {
        Self {
            id: id.to_owned(),
            title: one_line(title),
            turns: Vec::new(),
            planned: planned.clamp(2, 15),
            started_at: now,
            ended_at: None,
            report: None,
        }
    }

    pub fn answered(&self) -> usize {
        self.turns
            .iter()
            .filter(|t| !t.answer.trim().is_empty())
            .count()
    }

    /// Every planned question has been asked and answered.
    pub fn is_done(&self) -> bool {
        self.turns.len() >= self.planned
            && self
                .turns
                .last()
                .is_none_or(|t| !t.answer.trim().is_empty())
    }

    /// The open question, if one is waiting for an answer.
    pub fn waiting(&self) -> Option<&VivaTurn> {
        self.turns.last().filter(|t| t.answer.trim().is_empty())
    }

    /// Records the answer to the open question.
    pub fn answer(&mut self, text: &str, seconds: u32) -> bool {
        match self.turns.last_mut() {
            Some(t) if t.answer.trim().is_empty() => {
                t.answer = text.trim().to_owned();
                t.seconds = seconds;
                true
            }
            _ => false,
        }
    }
}

/// What comes next: the level and whether it is a follow-up.
pub fn next_step(viva: &Viva, sources: &[Source]) -> (u8, bool) {
    let Some(last) = viva.turns.last() else {
        return (1, false);
    };
    // One follow-up at most in a row.
    if !last.follow_up && weak(&last.answer, sources) {
        return (last.level, true);
    }
    ((last.level + 1).min(TOP_LEVEL), false)
}

/// A short answer, or one that touches nothing in the materials.
pub fn weak(answer: &str, sources: &[Source]) -> bool {
    let n = words(answer).len();
    if n < 8 {
        return true;
    }
    if sources.is_empty() {
        return false;
    }
    let all: String = sources
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    overlap(answer, &all) < 0.15
}

/// The examiner's instructions.
pub fn examiner_system(title: &str, topics: &[Topic], sources: &[Source]) -> String {
    let mut s = format!(
        "You are a fair but demanding viva examiner for a final-year project or course: \"{title}\". You ask one question at a time, grounded in the candidate's own materials (numbered passages below). Questions get harder by level: 1 recall and define, 2 explain how it works, 3 justify choices and assumptions, 4 limitations, alternatives, and threats to validity, 5 defend against critique and extend to new situations. A follow-up probes the candidate's last answer: quote a few of their words and ask for precision, evidence, or the step they skipped. Reply with the question only, one or two sentences, no preamble, no feedback, no numbering.\n"
    );
    if !topics.is_empty() {
        s.push_str("\nTopics in the materials:\n");
        for t in topics.iter().take(16) {
            s.push_str(&format!("- {}\n", t.title));
        }
    }
    s.push_str("\nPassages:\n\n");
    s.push_str(&sources_block(sources));
    s
}

/// The request for the next question.
pub fn question_prompt(viva: &Viva, level: u8, follow_up: bool) -> String {
    let mut s = String::new();
    if viva.turns.is_empty() {
        s.push_str("The viva is starting.\n");
    } else {
        s.push_str("So far:\n");
        for (i, t) in viva.turns.iter().enumerate() {
            s.push_str(&format!(
                "Q{} (level {}): {}\nA: {}\n",
                i + 1,
                t.level,
                t.question,
                if t.answer.is_empty() {
                    "(no answer)".into()
                } else {
                    clip(&t.answer, 800)
                }
            ));
        }
    }
    if follow_up {
        s.push_str(&format!(
            "\nAsk a follow-up at level {level} on the last answer."
        ));
    } else {
        s.push_str(&format!(
            "\nAsk question {} of {} at level {level}, on a part of the materials not yet covered.",
            viva.turns.len() + 1,
            viva.planned
        ));
    }
    s
}

/// The model's question, cleaned: no `Q3:` label, no quotes, one paragraph.
pub fn parse_question(text: &str) -> Option<String> {
    let mut t = one_line(text.trim());
    for p in ["Question:", "Examiner:", "Q:"] {
        if let Some(rest) = t.strip_prefix(p) {
            t = rest.trim().to_owned();
        }
    }
    if let Some(rest) = t.strip_prefix('Q')
        && let Some((num, rest)) = rest.split_once([':', '.', ')'])
        && num.trim().chars().all(|c| c.is_ascii_digit())
        && !num.trim().is_empty()
    {
        t = rest.trim().to_owned();
    }
    let t = t.trim_matches(['"', '“', '”']).trim().to_owned();
    (t.chars().count() >= 8).then_some(t)
}

/// A question without a model, from templates per level over the topics.
pub fn fallback_question(viva: &Viva, level: u8, follow_up: bool, topics: &[Topic]) -> String {
    if follow_up && let Some(last) = viva.turns.last() {
        let quote = clip(&last.answer, 80);
        return if quote.is_empty() {
            "You did not answer that one. Take another go: what is the core idea, in your own words?".into()
        } else {
            format!(
                "You said \"{quote}\". Can you be more precise, and point to where your material supports it?"
            )
        };
    }
    let i = viva.turns.iter().filter(|t| !t.follow_up).count();
    let topic = if topics.is_empty() {
        viva.title.clone()
    } else {
        topics[i % topics.len()].title.clone()
    };
    match level {
        1 => format!(
            "To start: in two or three sentences, what is {topic} and why does it matter in this work?"
        ),
        2 => format!("Walk me through how {topic} works, step by step."),
        3 => format!(
            "Why this approach to {topic}? What assumptions does it rest on, and what did you consider instead?"
        ),
        4 => format!(
            "What are the main limitations of {topic} here, and how would they affect your conclusions?"
        ),
        _ => format!(
            "Suppose an examiner argues your treatment of {topic} does not hold in a different setting. Defend it, or say how you would extend it."
        ),
    }
}

/// The four rubric scores, each 1 to 5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rubric {
    pub clarity: u8,
    pub correctness: u8,
    pub depth: u8,
    pub sources: u8,
}

impl Rubric {
    /// Out of 100.
    pub fn overall(&self) -> u32 {
        let sum = u32::from(self.clarity)
            + u32::from(self.correctness)
            + u32::from(self.depth)
            + u32::from(self.sources);
        (sum * 100 + 10) / 20
    }

    pub fn rows(&self) -> [(&'static str, u8); 4] {
        [
            ("Clarity", self.clarity),
            ("Correctness", self.correctness),
            ("Depth", self.depth),
            ("Use of sources", self.sources),
        ]
    }

    fn clamp(mut self) -> Self {
        for v in [
            &mut self.clarity,
            &mut self.correctness,
            &mut self.depth,
            &mut self.sources,
        ] {
            *v = (*v).clamp(1, 5);
        }
        self
    }
}

/// The scored report at the end of a viva.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VivaReport {
    pub rubric: Rubric,
    pub summary: String,
    pub strengths: Vec<String>,
    pub weaknesses: Vec<String>,
    /// Topics to revise next.
    pub suggested_topics: Vec<String>,
    /// A line per question.
    pub notes: Vec<String>,
    /// `model` or `offline estimate`.
    pub method: String,
}

impl VivaReport {
    pub fn overall(&self) -> u32 {
        self.rubric.overall()
    }

    /// The viva and its report as Markdown.
    pub fn to_markdown(&self, viva: &Viva) -> String {
        let mut s = format!("# Viva practice: {}\n\n", viva.title);
        let mins = viva.ended_at.map(|e| (e - viva.started_at).max(0) / 60);
        s.push_str(&format!(
            "{} questions{} · scored by {}\n\n",
            viva.turns.len(),
            mins.map(|m| format!(" · {m} min")).unwrap_or_default(),
            self.method
        ));
        s.push_str(&format!("**Overall: {} / 100**\n\n", self.overall()));
        if !self.summary.is_empty() {
            s.push_str(&format!("{}\n\n", self.summary));
        }
        s.push_str("| Criterion | Score |\n| --- | --- |\n");
        for (name, v) in self.rubric.rows() {
            s.push_str(&format!("| {name} | {v} / 5 |\n"));
        }
        for (head, items) in [
            ("Strengths", &self.strengths),
            ("Weaknesses", &self.weaknesses),
            ("Revise next", &self.suggested_topics),
        ] {
            if items.is_empty() {
                continue;
            }
            s.push_str(&format!("\n## {head}\n\n"));
            for i in items {
                s.push_str(&format!("- {i}\n"));
            }
        }
        s.push_str("\n## Questions\n\n");
        for (i, t) in viva.turns.iter().enumerate() {
            s.push_str(&format!(
                "### Q{} · level {}{}\n\n{}\n\n**Answer:** {}\n\n",
                i + 1,
                t.level,
                if t.follow_up { " · follow-up" } else { "" },
                t.question,
                if t.answer.is_empty() {
                    "(no answer)"
                } else {
                    &t.answer
                }
            ));
            if let Some(n) = self.notes.get(i) {
                s.push_str(&format!("*{n}*\n\n"));
            }
        }
        s
    }
}

/// The request for the rubric report: (system, user).
pub fn report_prompt(viva: &Viva, sources: &[Source]) -> (String, String) {
    let system = "You are the viva examiner writing your assessment. Score the candidate from 1 to 5 on clarity (structured, precise answers), correctness (agrees with the materials), depth (explains why, handles harder levels), and use of sources (refers to their materials and evidence). Be specific and fair: name what they did well and what was weak, with the question numbers. Suggest topics from the materials to revise. Reply with JSON only: {\"clarity\": 1-5, \"correctness\": 1-5, \"depth\": 1-5, \"sources\": 1-5, \"summary\": \"…\", \"strengths\": [\"…\"], \"weaknesses\": [\"…\"], \"suggested_topics\": [\"…\"], \"notes\": [\"one line per question\"]}".to_owned();
    let mut user = format!("Viva on: {}\n\nTranscript:\n", viva.title);
    for (i, t) in viva.turns.iter().enumerate() {
        user.push_str(&format!(
            "Q{} (level {}{}): {}\nA: {}\n",
            i + 1,
            t.level,
            if t.follow_up { ", follow-up" } else { "" },
            t.question,
            if t.answer.is_empty() {
                "(no answer)".into()
            } else {
                clip(&t.answer, 1500)
            }
        ));
    }
    user.push_str("\nMaterials:\n\n");
    user.push_str(&sources_block(sources));
    (system, user)
}

fn score(v: &Value, keys: &[&str]) -> Option<u8> {
    for k in keys {
        let x = v
            .get(*k)
            .or_else(|| v.get("scores").and_then(|s| s.get(*k)))
            .or_else(|| v.get("rubric").and_then(|s| s.get(*k)));
        match x {
            Some(Value::Number(n)) => return n.as_f64().map(|f| f.round().clamp(1.0, 5.0) as u8),
            Some(Value::String(s)) => {
                if let Some(n) = s
                    .split(['/', ' '])
                    .next()
                    .and_then(|n| n.trim().parse::<f64>().ok())
                {
                    return Some(n.round().clamp(1.0, 5.0) as u8);
                }
            }
            _ => {}
        }
    }
    None
}

/// Reads the model's report. `None` without all four scores.
pub fn parse_report(text: &str) -> Option<VivaReport> {
    let v = json_in(text)?;
    let rubric = Rubric {
        clarity: score(&v, &["clarity"])?,
        correctness: score(&v, &["correctness", "accuracy"])?,
        depth: score(&v, &["depth"])?,
        sources: score(&v, &["sources", "use_of_sources", "evidence"])?,
    }
    .clamp();
    Some(VivaReport {
        rubric,
        summary: field(&v, &["summary", "overall_comment", "verdict"])
            .map(one_line)
            .unwrap_or_default(),
        strengths: list(&v, &["strengths"]),
        weaknesses: list(&v, &["weaknesses", "gaps"]),
        suggested_topics: list(&v, &["suggested_topics", "revise", "topics"]),
        notes: list(&v, &["notes", "per_question"]),
        method: "model".into(),
    })
}

/// A report without a model, from what can be measured: answer length,
/// overlap with the materials, references to them, and how far up the
/// levels the answers held.
pub fn fallback_report(viva: &Viva, sources: &[Source], topics: &[Topic]) -> VivaReport {
    let all: String = sources
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let mut notes = Vec::new();
    let (mut len_sum, mut ov_sum, mut refs, mut deep_ok, mut answered) =
        (0f32, 0f32, 0u32, 0u32, 0u32);
    let mut weak_qs = Vec::new();
    let mut strong_qs = Vec::new();
    for (i, t) in viva.turns.iter().enumerate() {
        let n = words(&t.answer).len() as f32;
        let ov = if all.is_empty() {
            0.0
        } else {
            overlap(&t.answer, &all)
        };
        let cites = t.answer.to_lowercase().contains("page")
            || t.answer.contains("p.")
            || t.answer.to_lowercase().contains("my report")
            || t.answer.to_lowercase().contains("chapter")
            || t.answer.to_lowercase().contains("section")
            || t.answer.to_lowercase().contains("figure")
            || t.answer.to_lowercase().contains("table");
        if !t.answer.trim().is_empty() {
            answered += 1;
        }
        len_sum += (n / 40.0).min(1.0);
        // Overlap only counts as far as the answer says something.
        ov_sum += ov * (n / 10.0).min(1.0);
        refs += u32::from(cites);
        let ok = n >= 10.0 && ov >= 0.2;
        if ok && t.level >= 3 {
            deep_ok += 1;
        }
        if ok {
            strong_qs.push(i + 1);
        } else {
            weak_qs.push(i + 1);
        }
        notes.push(if t.answer.trim().is_empty() {
            "No answer.".to_owned()
        } else if ok {
            format!(
                "Grounded answer ({} words, touches the materials).",
                n as u32
            )
        } else if n < 10.0 {
            format!(
                "Short ({} words): expand with the reasoning and evidence.",
                n as u32
            )
        } else {
            "Little overlap with your materials: tie it back to them.".to_owned()
        });
    }
    let q = viva.turns.len().max(1) as f32;
    let to5 = |x: f32| (1.0 + x.clamp(0.0, 1.0) * 4.0).round() as u8;
    let high_levels = viva.turns.iter().filter(|t| t.level >= 3).count().max(1) as f32;
    let rubric = Rubric {
        clarity: to5(len_sum / q * (answered as f32 / q)),
        correctness: to5(ov_sum / q),
        depth: to5(deep_ok as f32 / high_levels),
        sources: to5(refs as f32 / q + (ov_sum / q)),
    }
    .clamp();
    let mut strengths = Vec::new();
    let mut weaknesses = Vec::new();
    if !strong_qs.is_empty() {
        strengths.push(format!(
            "Answers grounded in the materials: Q{}",
            join_nums(&strong_qs)
        ));
    }
    if rubric.sources >= 4 {
        strengths.push("You pointed to your materials (pages, sections, figures).".into());
    } else {
        weaknesses.push("Name where your evidence is: a page, section, figure, or table.".into());
    }
    if !weak_qs.is_empty() {
        weaknesses.push(format!(
            "Thin or off-material answers: Q{}",
            join_nums(&weak_qs)
        ));
    }
    if rubric.depth <= 2 {
        weaknesses.push(
            "Harder questions (assumptions, limitations, critique) need more reasoning.".into(),
        );
    }
    // Revise the topics the weak answers were about, else the least covered.
    let mut suggested: Vec<String> = Vec::new();
    for n in &weak_qs {
        let t = &viva.turns[n - 1];
        let by_id = t
            .topic
            .as_ref()
            .and_then(|id| topics.iter().find(|tp| &tp.id == id))
            .map(|tp| tp.title.clone());
        let pick = by_id.or_else(|| {
            topics
                .iter()
                .find(|tp| t.question.contains(&tp.title))
                .map(|tp| tp.title.clone())
        });
        if let Some(p) = pick
            && !suggested.contains(&p)
        {
            suggested.push(p);
        }
    }
    if suggested.is_empty() {
        suggested.extend(
            topics
                .iter()
                .filter(|tp| !viva.turns.iter().any(|t| t.question.contains(&tp.title)))
                .take(3)
                .map(|tp| tp.title.clone()),
        );
    }
    VivaReport {
        summary: format!(
            "Offline estimate from answer length, overlap with your materials, and references to them: {} of {} answers were grounded.",
            strong_qs.len(),
            viva.turns.len()
        ),
        rubric,
        strengths,
        weaknesses,
        suggested_topics: suggested,
        notes,
        method: "offline estimate".into(),
    }
}

fn join_nums(v: &[usize]) -> String {
    v.iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", Q")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::Anchor;

    fn sources() -> Vec<Source> {
        vec![Source {
            n: 1,
            chunk: "m1#1".into(),
            anchor: Anchor { material: "m1".into(), file: "fyp.pdf".into(), page: Some(3), section: None },
            text: "The system uses a transformer encoder to classify support tickets. Accuracy reached 91 percent on the held-out test set of tickets. A limitation is class imbalance in the ticket categories.".into(),
        }]
    }

    fn topic(t: &str) -> Topic {
        Topic {
            id: t.to_lowercase(),
            title: t.into(),
            objectives: vec![],
            chunks: vec![],
            anchors: vec![],
        }
    }

    #[test]
    fn levels_climb_and_weak_answers_get_one_follow_up() {
        let mut v = Viva::new("v1", "Ticket classifier", 4, 0);
        assert_eq!(next_step(&v, &sources()), (1, false));
        v.turns.push(VivaTurn {
            level: 1,
            question: "What is it?".into(),
            answer: String::new(),
            follow_up: false,
            topic: None,
            seconds: 0,
        });
        assert!(v.waiting().is_some());
        assert!(v.answer("A classifier.", 5));
        assert_eq!(next_step(&v, &sources()), (1, true));
        v.turns.push(VivaTurn {
            level: 1,
            question: "More?".into(),
            answer: "Still short.".into(),
            follow_up: true,
            topic: None,
            seconds: 3,
        });
        // No second follow-up in a row.
        assert_eq!(next_step(&v, &sources()), (2, false));
        v.turns.push(VivaTurn { level: 2, question: "How?".into(), answer: "It uses a transformer encoder to classify support tickets into categories, trained on labelled tickets.".into(), follow_up: false, topic: None, seconds: 30 });
        assert_eq!(next_step(&v, &sources()), (3, false));
        assert!(!v.is_done());
    }

    #[test]
    fn fallback_questions_by_level() {
        let v = Viva::new("v1", "Ticket classifier", 4, 0);
        let topics = vec![topic("Transformer encoder"), topic("Evaluation")];
        assert!(fallback_question(&v, 1, false, &topics).contains("Transformer encoder"));
        assert!(fallback_question(&v, 4, false, &[]).contains("limitations of Ticket classifier"));
    }

    #[test]
    fn question_parse_strips_labels() {
        assert_eq!(
            parse_question("Q3: \"Why a transformer?\"").as_deref(),
            Some("Why a transformer?")
        );
        assert_eq!(
            parse_question("Question: What is X exactly?").as_deref(),
            Some("What is X exactly?")
        );
        assert!(parse_question("ok").is_none());
    }

    #[test]
    fn model_report_parses_and_overall_is_out_of_100() {
        let r = parse_report("```json\n{\"clarity\": 4, \"correctness\": \"5/5\", \"depth\": 3, \"use_of_sources\": 2.6, \"summary\": \"Good.\", \"strengths\": [\"clear\"], \"weaknesses\": \"thin evidence\", \"suggested_topics\": [\"Evaluation\"]}\n```").unwrap();
        assert_eq!(
            r.rubric,
            Rubric {
                clarity: 4,
                correctness: 5,
                depth: 3,
                sources: 3
            }
        );
        assert_eq!(r.overall(), 75);
        assert_eq!(r.weaknesses, vec!["thin evidence"]);
        assert!(parse_report("{\"clarity\": 4}").is_none());
    }

    #[test]
    fn fallback_report_scores_grounded_answers_higher() {
        let mut good = Viva::new("v1", "Ticket classifier", 2, 0);
        good.turns = vec![
            VivaTurn { level: 1, question: "What?".into(), answer: "The system uses a transformer encoder to classify support tickets; on page 3 accuracy reached 91 percent on the held-out test set.".into(), follow_up: false, topic: None, seconds: 0 },
            VivaTurn { level: 4, question: "Limitations?".into(), answer: "A limitation is class imbalance in the ticket categories, which can inflate accuracy on the held-out test set; section 5 discusses reweighting the categories.".into(), follow_up: false, topic: None, seconds: 0 },
        ];
        let mut bad = good.clone();
        bad.turns[0].answer = "Not sure.".into();
        bad.turns[1].answer = String::new();
        let topics = vec![topic("Evaluation")];
        let g = fallback_report(&good, &sources(), &topics);
        let b = fallback_report(&bad, &sources(), &topics);
        assert!(
            g.overall() > b.overall(),
            "{} vs {}",
            g.overall(),
            b.overall()
        );
        assert!(g.rubric.sources >= 4);
        assert!(b.weaknesses.iter().any(|w| w.contains("Q1, Q2")));
        assert_eq!(b.suggested_topics, vec!["Evaluation"]);
        let md = g.to_markdown(&good);
        assert!(md.contains("| Use of sources |"));
        assert!(md.contains("### Q2 · level 4"));
    }
}
