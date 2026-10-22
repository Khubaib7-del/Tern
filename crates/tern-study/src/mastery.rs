//! A small, explainable mastery model per topic.
//!
//! Two numbers, both easy to say out loud:
//!
//! * a **rating** from 0 to 1, moved Elo-style by each graded answer: an
//!   answer counts for more when it beats what the rating expected at that
//!   difficulty (a hard question right moves it up a lot, an easy one wrong
//!   moves it down a lot);
//! * a **Leitner box** from 1 to 5: a fully right answer moves the topic up
//!   a box, a wrong one sends it back to box 1, a partly right one keeps it
//!   where it is. The box sets when the topic is next due (1, 2, 4, 8, then
//!   16 days), which flashcards will reuse.

use serde::{Deserialize, Serialize};

use crate::quiz::Difficulty;

/// How far one answer can move the rating.
pub const K: f32 = 0.25;
/// How far one flashcard review can move the rating.
pub const CARD_K: f32 = 0.08;
/// Days until a topic in box `n` is due again (index 0 is box 1).
pub const BOX_DAYS: [i64; 5] = [1, 2, 4, 8, 16];
const DAY: i64 = 86_400;

/// One topic's mastery.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mastery {
    pub rating: f32,
    /// Leitner box, 1 to 5.
    pub leitner: u8,
    /// Graded answers.
    pub attempts: u32,
    /// Sum of credit (1 right, 0.5 partly, 0 wrong).
    pub credit: f32,
    /// Last graded answer, card review, or tutor session, Unix seconds.
    pub last_seen: Option<i64>,
    /// Flashcard reviews on the topic's cards.
    #[serde(default)]
    pub card_reviews: u32,
}

impl Default for Mastery {
    fn default() -> Self {
        Self {
            rating: 0.0,
            leitner: 1,
            attempts: 0,
            credit: 0.0,
            last_seen: None,
            card_reviews: 0,
        }
    }
}

/// A word for where a topic stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    New,
    Learning,
    Shaky,
    Solid,
    Mastered,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Learning => "Learning",
            Self::Shaky => "Getting there",
            Self::Solid => "Solid",
            Self::Mastered => "Mastered",
        }
    }
}

/// The chance of a right answer the rating predicts at `difficulty`.
pub fn expected(rating: f32, difficulty: Difficulty) -> f32 {
    1.0 / (1.0 + (-(rating - difficulty.weight()) * 6.0).exp())
}

impl Mastery {
    /// Applies one graded answer: `credit` is 1 (right), 0.5 (partly), or 0.
    pub fn record(&mut self, credit: f32, difficulty: Difficulty, now: i64) {
        let credit = credit.clamp(0.0, 1.0);
        let e = expected(self.rating, difficulty);
        self.rating = (self.rating + K * (credit - e)).clamp(0.0, 1.0);
        if credit >= 0.99 {
            self.leitner = (self.leitner + 1).min(5);
        } else if credit <= 0.01 {
            self.leitner = 1;
        }
        self.attempts += 1;
        self.credit += credit;
        self.last_seen = Some(now);
    }

    /// A flashcard review on one of the topic's cards: `credit` from the
    /// rating (Again 0, Hard 0.5, Good and Easy 1). Moves the rating by at
    /// most [`CARD_K`] and leaves the Leitner box alone.
    pub fn nudge(&mut self, credit: f32, now: i64) {
        let credit = credit.clamp(0.0, 1.0);
        let e = expected(self.rating, Difficulty::Medium);
        self.rating = (self.rating + CARD_K * (credit - e)).clamp(0.0, 1.0);
        self.card_reviews += 1;
        self.last_seen = Some(now);
    }

    /// A tutor session on the topic: it counts as seen, nothing more.
    pub fn touch(&mut self, now: i64) {
        self.last_seen = Some(now);
    }

    pub fn level(&self) -> Level {
        if self.attempts == 0 && self.card_reviews == 0 {
            Level::New
        } else if self.rating >= 0.85 && self.leitner >= 4 {
            Level::Mastered
        } else if self.rating >= 0.6 {
            Level::Solid
        } else if self.rating >= 0.3 {
            Level::Shaky
        } else {
            Level::Learning
        }
    }

    /// When the topic is due for review, by its box.
    pub fn due_at(&self) -> Option<i64> {
        let box_ix = usize::from(self.leitner.clamp(1, 5) - 1);
        self.last_seen.map(|t| t + BOX_DAYS[box_ix] * DAY)
    }

    pub fn is_due(&self, now: i64) -> bool {
        self.attempts > 0 && self.due_at().is_some_and(|d| d <= now)
    }

    /// The question difficulty that suits the rating.
    pub fn suggested_difficulty(&self) -> Difficulty {
        if self.rating < 0.35 {
            Difficulty::Easy
        } else if self.rating < 0.7 {
            Difficulty::Medium
        } else {
            Difficulty::Hard
        }
    }

    /// `Solid · 7 of 9 right · box 4 · seen 2 days ago`.
    pub fn explain(&self, now: i64) -> String {
        if self.attempts == 0 && self.card_reviews > 0 {
            let mut s = format!(
                "{} · {} card review{} · no quiz yet",
                self.level().label(),
                self.card_reviews,
                if self.card_reviews == 1 { "" } else { "s" }
            );
            if let Some(t) = self.last_seen {
                s.push_str(&format!(" · seen {}", ago(now - t)));
            }
            return s;
        }
        if self.attempts == 0 {
            return match self.last_seen {
                Some(t) => format!("New · no quiz yet · tutored {}", ago(now - t)),
                None => "New · not studied yet".into(),
            };
        }
        let right = if (self.credit - self.credit.round()).abs() < 0.01 {
            format!("{}", self.credit.round() as u32)
        } else {
            format!("{:.1}", self.credit)
        };
        let mut s = format!(
            "{} · {right} of {} right · box {}",
            self.level().label(),
            self.attempts,
            self.leitner
        );
        if let Some(t) = self.last_seen {
            s.push_str(&format!(" · seen {}", ago(now - t)));
        }
        s
    }
}

/// `just now`, `3 hours ago`, `2 days ago`.
pub fn ago(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => "just now".into(),
        60..3600 => plural(secs / 60, "minute"),
        3600..86_400 => plural(secs / 3600, "hour"),
        _ => plural(secs / 86_400, "day"),
    }
}

fn plural(n: i64, unit: &str) -> String {
    if n == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{n} {unit}s ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_answers_raise_rating_and_box() {
        let mut m = Mastery::default();
        for i in 0..4 {
            m.record(1.0, Difficulty::Medium, i);
        }
        assert!(m.rating > 0.5, "{}", m.rating);
        assert_eq!(m.leitner, 5);
        assert_eq!(m.level(), Level::Solid);
        assert_eq!(
            m.explain(3 + 2 * 86_400),
            "Solid · 4 of 4 right · box 5 · seen 2 days ago"
        );
    }

    #[test]
    fn wrong_answer_resets_box_and_costs_more_when_expected_right() {
        let mut strong = Mastery {
            rating: 0.9,
            leitner: 4,
            attempts: 5,
            credit: 5.0,
            last_seen: Some(0),
            card_reviews: 0,
        };
        let mut weak = Mastery {
            rating: 0.1,
            ..strong.clone()
        };
        strong.record(0.0, Difficulty::Easy, 10);
        weak.record(0.0, Difficulty::Easy, 10);
        assert_eq!(strong.leitner, 1);
        assert!(0.9 - strong.rating > 0.1 - weak.rating);
    }

    #[test]
    fn partial_keeps_the_box() {
        let mut m = Mastery {
            leitner: 3,
            ..Mastery::default()
        };
        m.record(0.5, Difficulty::Hard, 0);
        assert_eq!(m.leitner, 3);
        assert_eq!(
            m.explain(30),
            "Learning · 0.5 of 1 right · box 3 · seen just now"
        );
    }

    #[test]
    fn due_dates_follow_the_box() {
        let mut m = Mastery::default();
        assert!(!m.is_due(1_000_000));
        m.record(1.0, Difficulty::Easy, 0);
        assert_eq!(m.leitner, 2);
        assert_eq!(m.due_at(), Some(2 * DAY));
        assert!(m.is_due(2 * DAY));
        assert!(!m.is_due(DAY));
    }

    #[test]
    fn card_reviews_nudge_gently_and_keep_the_box() {
        let mut quiz = Mastery::default();
        let mut card = Mastery {
            leitner: 3,
            ..Mastery::default()
        };
        quiz.record(1.0, Difficulty::Medium, 0);
        card.nudge(1.0, 0);
        assert!(card.rating > 0.0 && card.rating < quiz.rating);
        assert_eq!(card.leitner, 3);
        assert_eq!(card.attempts, 0);
        assert_eq!(card.level(), Level::Learning);
        assert_eq!(
            card.explain(60),
            "Learning · 1 card review · no quiz yet · seen 1 minute ago"
        );
        let before = card.rating;
        card.nudge(0.0, 1);
        assert!(card.rating < before);
    }

    #[test]
    fn difficulty_follows_rating() {
        let m = Mastery {
            rating: 0.8,
            ..Mastery::default()
        };
        assert_eq!(m.suggested_difficulty(), Difficulty::Hard);
        assert_eq!(Mastery::default().suggested_difficulty(), Difficulty::Easy);
        assert_eq!(Mastery::default().explain(0), "New · not studied yet");
    }
}
