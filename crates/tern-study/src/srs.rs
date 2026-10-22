//! Spaced repetition for flashcards: SM-2, the way Anki runs it.
//!
//! Why SM-2 and not FSRS: SM-2's whole state is an interval and an ease
//! factor, both of which can be said out loud ("next in 6 days, ease
//! 250%") and both of which Anki stores per card, so a deck exported to
//! Anki keeps its schedule exactly. FSRS predicts recall better, but only
//! with weights fitted to hundreds of the user's own reviews; a fresh
//! student deck has none, and FSRS with default weights is no better than
//! SM-2 while being far harder to explain. The numbers below are Anki's
//! defaults.
//!
//! * New cards go through learning steps (1 minute, then 10 minutes) and
//!   graduate to a 1 day interval (4 days on Easy).
//! * Review cards: Good multiplies the interval by the ease (start 250%),
//!   Hard by 1.2 and lowers the ease by 15 points, Easy by ease × 1.3 and
//!   raises it by 15. Every success adds at least one day.
//! * Again on a review card is a lapse: the ease drops 20 points (never
//!   under 130%), the card relearns for 10 minutes, and comes back after
//!   one day.
//!
//! No random fuzz is added, so a schedule is deterministic and testable.

use serde::{Deserialize, Serialize};

pub const DAY: i64 = 86_400;
/// Learning steps for new cards, in seconds.
pub const LEARN_STEPS: [i64; 2] = [60, 600];
/// Relearning steps after a lapse, in seconds.
pub const RELEARN_STEPS: [i64; 1] = [600];
pub const START_EASE: f32 = 2.5;
pub const MIN_EASE: f32 = 1.3;
/// Days a card gets when it graduates on Good, and on Easy.
pub const GRADUATE_DAYS: f32 = 1.0;
pub const EASY_GRADUATE_DAYS: f32 = 4.0;
pub const HARD_FACTOR: f32 = 1.2;
pub const EASY_BONUS: f32 = 1.3;
/// Share of the interval kept after a lapse (Anki's default: none).
pub const LAPSE_KEEP: f32 = 0.0;
pub const MAX_DAYS: f32 = 36_500.0;
/// An interval this long or longer makes a card "mature".
pub const MATURE_DAYS: f32 = 21.0;

/// The four answer buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rating {
    Again,
    Hard,
    Good,
    Easy,
}

impl Rating {
    pub const ALL: [Rating; 4] = [Rating::Again, Rating::Hard, Rating::Good, Rating::Easy];

    pub fn label(self) -> &'static str {
        match self {
            Self::Again => "Again",
            Self::Hard => "Hard",
            Self::Good => "Good",
            Self::Easy => "Easy",
        }
    }

    /// The keyboard key: 1 to 4, as in Anki.
    pub fn key(self) -> u8 {
        match self {
            Self::Again => 1,
            Self::Hard => 2,
            Self::Good => 3,
            Self::Easy => 4,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "1" | "again" | "a" => Self::Again,
            "2" | "hard" | "h" => Self::Hard,
            "3" | "good" | "g" => Self::Good,
            "4" | "easy" | "e" => Self::Easy,
            _ => return None,
        })
    }

    /// Credit toward the topic's mastery: Again 0, Hard half, Good and
    /// Easy full.
    pub fn credit(self) -> f32 {
        match self {
            Self::Again => 0.0,
            Self::Hard => 0.5,
            Self::Good | Self::Easy => 1.0,
        }
    }

    /// Anki's `ease` number in its review log (1 to 4).
    pub fn anki(self) -> i64 {
        i64::from(self.key())
    }
}

/// Where a card is in its life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardState {
    #[default]
    New,
    Learning,
    Review,
    Relearning,
}

impl CardState {
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Learning => "Learning",
            Self::Review => "Review",
            Self::Relearning => "Relearning",
        }
    }
}

/// One card's schedule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sched {
    pub state: CardState,
    /// The learning or relearning step the card is on.
    #[serde(default)]
    pub step: u8,
    /// Days between reviews once the card has graduated.
    #[serde(default)]
    pub interval: f32,
    pub ease: f32,
    /// When the card is next due, Unix seconds (0 for a new card).
    #[serde(default)]
    pub due: i64,
    #[serde(default)]
    pub reps: u32,
    #[serde(default)]
    pub lapses: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<i64>,
}

impl Default for Sched {
    fn default() -> Self {
        Self {
            state: CardState::New,
            step: 0,
            interval: 0.0,
            ease: START_EASE,
            due: 0,
            reps: 0,
            lapses: 0,
            last: None,
        }
    }
}

fn days(d: f32) -> i64 {
    (d.round() as i64).max(1) * DAY
}

impl Sched {
    /// The schedule after answering `rating` at `now`.
    pub fn next(&self, rating: Rating, now: i64) -> Sched {
        let mut s = self.clone();
        s.reps += 1;
        s.last = Some(now);
        match self.state {
            CardState::New | CardState::Learning => {
                s.state = CardState::Learning;
                let step = usize::from(self.step).min(LEARN_STEPS.len() - 1);
                match rating {
                    Rating::Again => {
                        s.step = 0;
                        s.due = now + LEARN_STEPS[0];
                    }
                    Rating::Hard => {
                        // The first step's Hard waits halfway to the next.
                        let wait = if step == 0 && LEARN_STEPS.len() > 1 {
                            (LEARN_STEPS[0] + LEARN_STEPS[1]) / 2
                        } else {
                            LEARN_STEPS[step]
                        };
                        s.due = now + wait;
                    }
                    Rating::Good if step + 1 < LEARN_STEPS.len() => {
                        s.step = (step + 1) as u8;
                        s.due = now + LEARN_STEPS[step + 1];
                    }
                    Rating::Good => s.graduate(GRADUATE_DAYS, now),
                    Rating::Easy => s.graduate(EASY_GRADUATE_DAYS, now),
                }
            }
            CardState::Review => {
                let ivl = self.interval.max(1.0);
                match rating {
                    Rating::Again => {
                        s.lapses += 1;
                        s.ease = (self.ease - 0.2).max(MIN_EASE);
                        s.interval = (ivl * LAPSE_KEEP).max(1.0);
                        s.state = CardState::Relearning;
                        s.step = 0;
                        s.due = now + RELEARN_STEPS[0];
                    }
                    Rating::Hard => {
                        s.ease = (self.ease - 0.15).max(MIN_EASE);
                        s.review((ivl * HARD_FACTOR).max(ivl + 1.0), now);
                    }
                    Rating::Good => s.review((ivl * self.ease).max(ivl + 1.0), now),
                    Rating::Easy => {
                        s.ease = self.ease + 0.15;
                        s.review((ivl * self.ease * EASY_BONUS).max(ivl + 1.0), now);
                    }
                }
            }
            CardState::Relearning => {
                let step = usize::from(self.step).min(RELEARN_STEPS.len() - 1);
                match rating {
                    Rating::Again => {
                        s.step = 0;
                        s.due = now + RELEARN_STEPS[0];
                    }
                    Rating::Hard => s.due = now + RELEARN_STEPS[step],
                    Rating::Good if step + 1 < RELEARN_STEPS.len() => {
                        s.step = (step + 1) as u8;
                        s.due = now + RELEARN_STEPS[step + 1];
                    }
                    Rating::Good => s.review(self.interval.max(1.0), now),
                    Rating::Easy => s.review(self.interval.max(1.0) + 1.0, now),
                }
            }
        }
        s
    }

    fn graduate(&mut self, d: f32, now: i64) {
        self.step = 0;
        self.review(d, now);
    }

    fn review(&mut self, d: f32, now: i64) {
        self.state = CardState::Review;
        self.step = 0;
        // A hair over, so 32.5 days (stored as 32.4999…) rounds up.
        self.interval = (d.min(MAX_DAYS) + 1e-3).round().clamp(1.0, MAX_DAYS);
        self.due = now + days(self.interval);
    }

    pub fn is_due(&self, now: i64) -> bool {
        self.state != CardState::New && self.due <= now
    }

    pub fn is_mature(&self) -> bool {
        self.state == CardState::Review && self.interval >= MATURE_DAYS
    }
}

/// How long until `due`, as a button hint: `1m`, `10m`, `1d`, `3.2mo`.
pub fn wait_label(secs: i64) -> String {
    let s = secs.max(0);
    if s < 3600 {
        format!("{}m", (s / 60).max(1))
    } else if s < DAY {
        format!("{}h", s / 3600)
    } else {
        let d = s as f32 / DAY as f32;
        if d < 30.0 {
            format!("{}d", d.round() as i64)
        } else if d < 365.0 {
            format!("{:.1}mo", d / 30.0)
        } else {
            format!("{:.1}y", d / 365.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_card_walks_the_learning_steps_and_graduates() {
        let s = Sched::default();
        let a = s.next(Rating::Good, 0);
        assert_eq!(a.state, CardState::Learning);
        assert_eq!(a.due, 600);
        let b = a.next(Rating::Good, 600);
        assert_eq!(b.state, CardState::Review);
        assert_eq!(b.interval, 1.0);
        assert_eq!(b.due, 600 + DAY);
        let e = s.next(Rating::Easy, 0);
        assert_eq!(e.interval, 4.0);
        assert_eq!(s.next(Rating::Again, 0).due, 60);
        assert_eq!(s.next(Rating::Hard, 0).due, 330);
    }

    #[test]
    fn review_intervals_follow_the_ease() {
        let mut s = Sched::default().next(Rating::Easy, 0);
        assert_eq!(s.interval, 4.0);
        s = s.next(Rating::Good, s.due);
        assert_eq!(s.interval, 10.0, "4 × 2.5");
        let hard = s.next(Rating::Hard, s.due);
        assert_eq!(hard.interval, 12.0);
        assert!((hard.ease - 2.35).abs() < 1e-4);
        let easy = s.next(Rating::Easy, s.due);
        assert_eq!(easy.interval, 33.0, "10 × 2.5 × 1.3 = 32.5 → 33");
        assert!(easy.is_mature());
        assert!((easy.ease - 2.65).abs() < 1e-4);
    }

    #[test]
    fn a_lapse_relearns_and_lowers_the_ease() {
        let s = Sched {
            state: CardState::Review,
            interval: 20.0,
            ease: 1.4,
            due: 0,
            ..Sched::default()
        };
        let l = s.next(Rating::Again, 100);
        assert_eq!(l.state, CardState::Relearning);
        assert_eq!(l.lapses, 1);
        assert!((l.ease - MIN_EASE).abs() < 1e-4, "floored");
        assert_eq!(l.due, 700);
        let back = l.next(Rating::Good, 700);
        assert_eq!(back.state, CardState::Review);
        assert_eq!(back.interval, 1.0);
        assert_eq!(back.due, 700 + DAY);
    }

    #[test]
    fn every_success_adds_a_day_and_the_cap_holds() {
        let s = Sched {
            state: CardState::Review,
            interval: 1.0,
            ease: MIN_EASE,
            ..Sched::default()
        };
        assert_eq!(s.next(Rating::Hard, 0).interval, 2.0);
        let big = Sched {
            interval: 30_000.0,
            ease: 3.0,
            ..s
        };
        assert_eq!(big.next(Rating::Easy, 0).interval, MAX_DAYS);
    }

    #[test]
    fn labels_and_parsing() {
        assert_eq!(wait_label(60), "1m");
        assert_eq!(wait_label(600), "10m");
        assert_eq!(wait_label(DAY), "1d");
        assert_eq!(wait_label(90 * DAY), "3.0mo");
        assert_eq!(Rating::parse("3"), Some(Rating::Good));
        assert_eq!(Rating::parse("again"), Some(Rating::Again));
        assert_eq!(Rating::Easy.key(), 4);
        assert!(!Sched::default().is_due(i64::MAX));
    }
}
