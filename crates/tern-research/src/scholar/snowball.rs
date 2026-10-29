//! Citation snowballing: from seed papers, follow their references
//! (backward) and the papers citing them (forward), round by round, and
//! stop when new relevant finds level off (saturation), the yield of a
//! round falls too low, or a round or paper cap is reached, whichever
//! comes first. The caller fetches each round's links; this keeps what was
//! seen and says when to stop.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::Paper;
use super::funnel::{coverage, key_terms};

/// Which links to follow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// The papers a seed cites.
    References,
    /// The papers that cite a seed.
    Citations,
    #[default]
    Both,
}

impl Direction {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "references" | "backward" | "refs" => Self::References,
            "citations" | "forward" | "cited_by" | "cited-by" => Self::Citations,
            _ => Self::Both,
        }
    }
}

/// When to stop.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StopRule {
    /// Rounds at most (each round follows the last round's relevant finds).
    pub max_rounds: usize,
    /// Relevant papers kept at most.
    pub max_papers: usize,
    /// A round that adds fewer new relevant papers than this is the last
    /// (the discovery curve has flattened).
    pub min_new_relevant: usize,
    /// A round whose share of relevant papers among its new ones is below
    /// this is the last.
    pub min_yield: f32,
    /// The least share of the topic's key terms a paper's title and
    /// abstract must hold to count as relevant.
    pub min_coverage: f32,
}

impl Default for StopRule {
    fn default() -> Self {
        Self {
            max_rounds: 3,
            max_papers: 40,
            min_new_relevant: 2,
            min_yield: 0.1,
            min_coverage: 0.5,
        }
    }
}

/// Why snowballing stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The round cap.
    Rounds,
    /// The paper cap.
    Papers,
    /// Fewer new relevant papers than the rule asks for.
    Saturated,
    /// Too few of the round's new papers were relevant.
    LowYield,
    /// Nothing left to follow.
    NoSeeds,
}

impl StopReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rounds => "reached the round limit",
            Self::Papers => "reached the paper limit",
            Self::Saturated => "new relevant papers levelled off",
            Self::LowYield => "too few new papers were on topic",
            Self::NoSeeds => "nothing left to follow",
        }
    }
}

/// One round's outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Round {
    /// 1-based.
    pub n: usize,
    /// Papers the links returned that had not been seen.
    pub new: usize,
    /// Of those, the relevant ones.
    pub new_relevant: usize,
    /// When this round is the last, why.
    pub stop: Option<StopReason>,
}

/// The snowball's state between rounds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snowball {
    pub rule: StopRule,
    terms: Vec<String>,
    seen: BTreeSet<String>,
    /// Relevant papers found so far, in the order found (seeds not
    /// included).
    pub found: Vec<Paper>,
    pub rounds: Vec<Round>,
    /// The papers whose links the next round follows.
    pub frontier: Vec<Paper>,
}

impl Snowball {
    /// Starts from `seeds` for `topic`.
    pub fn new(topic: &str, seeds: Vec<Paper>, rule: StopRule) -> Self {
        let mut seen = BTreeSet::new();
        for s in &seeds {
            seen.extend(s.keys());
        }
        Self {
            rule,
            terms: key_terms(topic),
            seen,
            found: Vec::new(),
            rounds: Vec::new(),
            frontier: seeds,
        }
    }

    /// Whether `p` is on the topic.
    pub fn relevant(&self, p: &Paper) -> bool {
        !p.retracted
            && (self.terms.is_empty()
                || coverage(&self.terms, &p.text()) + 1e-6 >= self.rule.min_coverage)
    }

    /// Whether another round should run.
    pub fn done(&self) -> bool {
        self.frontier.is_empty() || self.rounds.last().is_some_and(|r| r.stop.is_some())
    }

    /// Takes in the papers the frontier's links returned and decides
    /// whether this was the last round.
    pub fn round(&mut self, linked: Vec<Paper>) -> Round {
        let n = self.rounds.len() + 1;
        let mut new = 0;
        let mut fresh = Vec::new();
        for p in super::merge_all(linked) {
            let keys = p.keys();
            if keys.iter().any(|k| self.seen.contains(k)) {
                continue;
            }
            self.seen.extend(keys);
            new += 1;
            if self.relevant(&p) {
                fresh.push(p);
            }
        }
        let new_relevant = fresh.len();
        let room = self.rule.max_papers.saturating_sub(self.found.len());
        fresh.truncate(room);
        self.found.extend(fresh.iter().cloned());
        self.frontier = fresh;
        let stop = if self.found.len() >= self.rule.max_papers {
            Some(StopReason::Papers)
        } else if new_relevant < self.rule.min_new_relevant {
            Some(StopReason::Saturated)
        } else if new > 0 && (new_relevant as f32) < self.rule.min_yield * new as f32 {
            Some(StopReason::LowYield)
        } else if n >= self.rule.max_rounds {
            Some(StopReason::Rounds)
        } else if self.frontier.is_empty() {
            Some(StopReason::NoSeeds)
        } else {
            None
        };
        let r = Round {
            n,
            new,
            new_relevant,
            stop,
        };
        self.rounds.push(r.clone());
        r
    }

    /// Why it stopped, once it has.
    pub fn stopped(&self) -> Option<StopReason> {
        if self.rounds.is_empty() && self.frontier.is_empty() {
            return Some(StopReason::NoSeeds);
        }
        self.rounds.last().and_then(|r| r.stop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(title: &str) -> Paper {
        Paper {
            title: title.into(),
            ..Paper::default()
        }
    }

    #[test]
    fn stops_when_new_relevant_finds_level_off() {
        let mut s = Snowball::new(
            "retrieval augmented generation",
            vec![p("Retrieval augmented generation for NLP")],
            StopRule::default(),
        );
        let r1 = s.round(vec![
            p("Dense retrieval augmented generation at scale"),
            p("Retrieval augmented generation with citations"),
            p("Retrieval augmented generation for code"),
            p("Image classification with residual networks"),
        ]);
        assert_eq!((r1.new, r1.new_relevant, r1.stop), (4, 3, None));
        // The same papers again, and one new relevant one: saturated.
        let r2 = s.round(vec![
            p("Retrieval augmented generation with citations"),
            p("Generation augmented by retrieval of passages for QA"),
            p("Protein folding"),
        ]);
        assert_eq!(r2.new, 2);
        assert_eq!(r2.new_relevant, 1);
        assert_eq!(r2.stop, Some(StopReason::Saturated));
        assert!(s.done());
        assert_eq!(s.found.len(), 4);
        assert_eq!(s.stopped(), Some(StopReason::Saturated));
    }

    #[test]
    fn caps_hold() {
        let rule = StopRule {
            max_papers: 2,
            ..StopRule::default()
        };
        let mut s = Snowball::new(
            "graph neural networks",
            vec![p("Graph neural networks")],
            rule,
        );
        let r = s.round(vec![
            p("Graph neural networks for molecules"),
            p("Graph neural networks for traffic"),
            p("Graph neural networks for recommendation"),
        ]);
        assert_eq!(r.stop, Some(StopReason::Papers));
        assert_eq!(s.found.len(), 2);
        let rule = StopRule {
            max_rounds: 1,
            ..StopRule::default()
        };
        let mut s = Snowball::new(
            "graph neural networks",
            vec![p("Graph neural networks")],
            rule,
        );
        let r = s.round(vec![
            p("Graph neural networks for molecules"),
            p("Graph neural networks for traffic"),
        ]);
        assert_eq!(r.stop, Some(StopReason::Rounds));
        assert_eq!(Direction::parse("forward"), Direction::Citations);
        let s = Snowball::new("x", Vec::new(), StopRule::default());
        assert_eq!(s.stopped(), Some(StopReason::NoSeeds));
    }
}
