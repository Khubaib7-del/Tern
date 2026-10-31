//! Citation snowballing: from seed papers, follow their references
//! (backward) and the papers citing them (forward), round by round, and
//! stop when new relevant finds level off (saturation), the yield of a
//! round falls too low, or a round or paper cap is reached, whichever
//! comes first. The caller fetches each round's links; this keeps what was
//! seen and says when to stop.
//!
//! Each round's relevant finds are ranked by relevance to the topic, the
//! question and the seed papers, then by influence (Semantic Scholar's
//! influential citations and influential links, overall citations), never
//! by date (#83: a seed's newest citing papers, off topic and uncited,
//! came first). The best go on to the next round and lead the list.

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
    /// The key terms of the seeds' titles.
    seed_terms: Vec<String>,
    /// The key terms of the user's question, when one was given.
    question_terms: Vec<String>,
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
        let mut titles = String::new();
        for s in &seeds {
            seen.extend(s.keys());
            titles.push(' ');
            titles.push_str(&s.title);
        }
        Self {
            rule,
            terms: key_terms(topic),
            seed_terms: key_terms(&titles),
            question_terms: Vec::new(),
            seen,
            found: Vec::new(),
            rounds: Vec::new(),
            frontier: seeds,
        }
    }

    /// Adds the user's question to what a find is ranked against.
    pub fn with_question(mut self, question: &str) -> Self {
        self.question_terms = key_terms(question);
        self
    }

    /// How strongly `p` should be followed and listed, 0 to 1: relevance
    /// to the topic and question (half), to the seeds (a fifth), then
    /// influence: influential citations, an influential link to a seed,
    /// and citations overall. The year plays no part.
    pub fn score(&self, p: &Paper) -> f32 {
        let text = p.text();
        let mut asked: Vec<String> = self.terms.clone();
        for t in &self.question_terms {
            if !asked.contains(t) {
                asked.push(t.clone());
            }
        }
        let topic = if asked.is_empty() {
            0.0
        } else {
            coverage(&asked, &text)
        };
        let seed = if self.seed_terms.is_empty() {
            topic
        } else {
            coverage(&self.seed_terms, &text)
        };
        let log_share = |n: Option<u64>, top: f32| {
            n.map_or(0.0, |n| ((1.0 + n as f32).ln() / (1.0 + top).ln()).min(1.0))
        };
        let influential = log_share(p.influential_citations, 1_000.0);
        let cited = log_share(p.citations, 100_000.0);
        let link = if p.influential { 1.0 } else { 0.0 };
        0.5 * topic + 0.2 * seed + 0.15 * influential + 0.05 * cited + 0.1 * link
    }

    /// `papers` best first by [`Snowball::score`], ties by title.
    fn sort(&self, papers: &mut [Paper]) {
        let mut scored: Vec<(f32, Paper)> =
            papers.iter().map(|p| (self.score(p), p.clone())).collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.title.cmp(&b.1.title))
        });
        for (slot, (_, p)) in papers.iter_mut().zip(scored) {
            *slot = p;
        }
    }

    /// Every relevant find, best first by [`Snowball::score`].
    pub fn ranked(&self) -> Vec<Paper> {
        let mut out = self.found.clone();
        self.sort(&mut out);
        out
    }

    /// Whether `p` is on the topic: it covers the topic's key terms, or
    /// the seed's own title terms (#85, A11: a seed found by its arXiv id
    /// or DOI is its own best description of what is on topic).
    pub fn relevant(&self, p: &Paper) -> bool {
        if p.retracted {
            return false;
        }
        let text = p.text();
        self.terms.is_empty()
            || coverage(&self.terms, &text) + 1e-6 >= self.rule.min_coverage
            || (!self.seed_terms.is_empty()
                && coverage(&self.seed_terms, &text) + 1e-6 >= self.rule.min_coverage)
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
        // The strongest finds are kept when the cap cuts, and followed first.
        self.sort(&mut fresh);
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
    fn influential_and_relevant_finds_lead_not_the_newest() {
        let seed = Paper {
            title: "Attention Is All You Need".into(),
            year: Some(2017),
            ..Paper::default()
        };
        let newest = |title: &str| Paper {
            title: title.into(),
            year: Some(2026),
            citations: Some(0),
            influential_citations: Some(0),
            venue: Some("Elsevier".into()),
            ..Paper::default()
        };
        let mut s = Snowball::new("attention transformer", vec![seed], StopRule::default())
            .with_question("how does self-attention in the transformer work");
        s.round(vec![
            newest("Attention-guided transformer for patient-facing mental healthcare chat"),
            newest("Orchard LiDAR segmentation with an attention transformer"),
            Paper {
                title: "BERT: Pre-training of Deep Bidirectional Transformers with self-attention"
                    .into(),
                year: Some(2019),
                citations: Some(90_000),
                influential_citations: Some(20_000),
                influential: true,
                ..Paper::default()
            },
            newest("Wind power forecasting with an attention transformer"),
            Paper {
                title: "Transformer-XL: attention language models beyond a fixed-length context"
                    .into(),
                year: Some(2019),
                citations: Some(3_000),
                influential_citations: Some(400),
                ..Paper::default()
            },
        ]);
        let ranked: Vec<String> = s.ranked().into_iter().map(|p| p.title).collect();
        assert!(ranked[0].starts_with("BERT"), "{ranked:?}");
        assert!(ranked[1].starts_with("Transformer-XL"), "{ranked:?}");
        assert!(s.frontier[0].title.starts_with("BERT"));
        // The year never lifts a paper: the same paper made newer scores
        // the same.
        let mut a = newest("Attention transformer for wind power");
        let b = a.clone();
        a.year = Some(1999);
        assert_eq!(s.score(&a), s.score(&b));
    }

    /// A-T10 (#85): snowballing from arXiv:1706.03762 (a recorded set of
    /// its citers) puts at least five on-topic citers in the top ten, even
    /// when the topic the model gave is loose; off-topic citers stay out.
    #[test]
    fn a_seed_by_arxiv_id_finds_its_on_topic_citers() {
        let seed = Paper {
            title: "Attention Is All You Need".into(),
            arxiv: Some("1706.03762".into()),
            abstract_text: Some(
                "The dominant sequence transduction models are based on recurrent or convolutional neural networks. We propose the Transformer, based solely on attention mechanisms."
                    .into(),
            ),
            ..Paper::default()
        };
        let cite = |title: &str, cites: u64| Paper {
            title: title.into(),
            citations: Some(cites),
            ..Paper::default()
        };
        let on_topic = [
            "BERT: bidirectional transformers with self-attention for language understanding",
            "Self-attention with relative position representations",
            "Linformer: self-attention with linear complexity",
            "Vision transformers: attention for image recognition at scale",
            "Longformer: attention for long documents",
            "Efficient attention for long sequences",
            "Reformer: efficient attention with hashing",
        ];
        let off_topic = [
            "Protein structure prediction from sequence",
            "Crop yield forecasting in arid regions",
            "Wind turbine gearbox fault detection",
            "A survey of blockchain consensus",
            "Soil moisture estimation from satellites",
        ];
        let mut linked: Vec<Paper> = on_topic.iter().map(|t| cite(t, 5_000)).collect();
        linked.extend(off_topic.iter().map(|t| cite(t, 50_000)));
        let mut s = Snowball::new("transformer citers", vec![seed], StopRule::default());
        s.round(linked);
        let top: Vec<String> = s.ranked().into_iter().take(10).map(|p| p.title).collect();
        let hits = top
            .iter()
            .filter(|t| on_topic.contains(&t.as_str()))
            .count();
        assert!(hits >= 5, "{top:?}");
        assert!(
            top.iter().all(|t| !off_topic.contains(&t.as_str())),
            "{top:?}"
        );
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
