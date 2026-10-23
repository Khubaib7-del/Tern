//! Task suites: versioned JSON files of cases with known answers.
//!
//! A suite file names its kind, its level on the test ladder, and where
//! its cases came from (`origin`), then lists the cases. Tern bundles one
//! suite per kind; more can be dropped into `<data>/evals/suites/` (see
//! `docs/evaluation.md`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tern_decision::bench::Fixture;
use tern_research::verdict::Verdict;
use tern_study::grade::Mark;
use tern_study::quiz::QuestionKind;

/// The suite file format this build reads.
pub const FORMAT: u32 = 1;

/// The rungs of the test ladder (`docs/04-roadmap.md`, phase 11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// The workspace's unit and integration tests (`cargo test`).
    Unit,
    /// Bundled cases written for Tern, run offline and deterministically.
    Synthetic,
    /// Real questions over real, pinned pages, answered by a live model.
    Pinned,
    /// A public benchmark the user downloads (SciFact).
    Public,
    /// A person using Tern on scripted tasks, with the SUS questionnaire.
    Human,
}

impl Level {
    pub const ALL: [Level; 5] = [
        Self::Unit,
        Self::Synthetic,
        Self::Pinned,
        Self::Public,
        Self::Human,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Synthetic => "synthetic",
            Self::Pinned => "pinned",
            Self::Public => "public",
            Self::Human => "human",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|l| l.label() == s.trim().to_ascii_lowercase())
    }

    /// The one command that runs this rung.
    pub fn command(self) -> &'static str {
        match self {
            Self::Unit => "tern eval run --level unit   (runs cargo test --workspace in a source checkout)",
            Self::Synthetic => "tern eval run --level synthetic",
            Self::Pinned => "tern eval run --level pinned --live",
            Self::Public => "tern eval run --level public --scifact <folder>",
            Self::Human => "tern eval usability",
        }
    }

    /// What the rung needs before it can run.
    pub fn needs(self) -> &'static str {
        match self {
            Self::Unit => "a source checkout and cargo",
            Self::Synthetic => "nothing: offline and deterministic",
            Self::Pinned => "a model provider and the network",
            Self::Public => "the SciFact files (claims_*.jsonl and corpus.jsonl); a model for judged runs",
            Self::Human => "a person at the keyboard",
        }
    }
}

/// What a suite's cases test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuiteKind {
    /// Questions answered from given sources, scored on claims, citations,
    /// constraints, cost, and the preflight forecast.
    Research,
    /// One claim and its sources: is the verdict right?
    Claims,
    /// One answer to a quiz question: is the grade right?
    Quiz,
    /// Decision-model routing on a held-out split.
    Decision,
    /// Prompt injection and tool safety.
    Safety,
    /// The workspace's tests, read from `cargo test` (the unit rung).
    Tests,
    /// A usability session (the human rung).
    Usability,
}

impl SuiteKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Research => "Research answers",
            Self::Claims => "Claim verdicts",
            Self::Quiz => "Quiz grading",
            Self::Decision => "Decision routing",
            Self::Safety => "Injection and tool safety",
            Self::Tests => "Unit and integration tests",
            Self::Usability => "Usability",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Claims => "claims",
            Self::Quiz => "quiz",
            Self::Decision => "decision",
            Self::Safety => "safety",
            Self::Tests => "tests",
            Self::Usability => "usability",
        }
    }
}

/// A source a research or claim case can cite, numbered from 1 in order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseSource {
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// The page text. Empty in pinned suites: it is fetched at run time.
    #[serde(default)]
    pub text: String,
}

/// A claim the answer should make, found by its key terms.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoldClaim {
    pub text: String,
    /// Words that must all appear in one cited sentence (case-insensitive).
    pub terms: Vec<String>,
    /// Sources (1-based) that state it; the sentence must cite one.
    #[serde(default)]
    pub sources: Vec<usize>,
}

/// A constraint the user set, checked on the answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Constraint {
    /// At most this many words.
    MaxWords { n: usize },
    /// At least this many different sources cited.
    MinCitations { n: usize },
    /// The answer mentions this term.
    MustMention { term: String },
    /// Never cite this source (1-based): the user ruled it out.
    AvoidSource { n: usize },
}

impl Constraint {
    pub fn describe(&self) -> String {
        match self {
            Self::MaxWords { n } => format!("at most {n} words"),
            Self::MinCitations { n } => format!("cite at least {n} sources"),
            Self::MustMention { term } => format!("mention \u{201c}{term}\u{201d}"),
            Self::AvoidSource { n } => format!("do not cite source [{n}]"),
        }
    }
}

/// One recorded answer, replayed by offline runs (trial `k` uses
/// recording `k % len`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recording {
    pub answer: String,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub latency_ms: u64,
    /// The right verdict for each cited sentence, in order, when someone
    /// labelled them: the checker's agreement with these is scored.
    #[serde(default)]
    pub labels: Vec<Verdict>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResearchCase {
    pub id: String,
    pub question: String,
    pub sources: Vec<CaseSource>,
    #[serde(default)]
    pub gold_claims: Vec<GoldClaim>,
    /// Sources (1-based) a good answer cites.
    #[serde(default)]
    pub gold_sources: Vec<usize>,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    /// The token budget the preflight forecast is made against.
    #[serde(default)]
    pub budget_tokens: Option<u64>,
    #[serde(default)]
    pub recorded: Vec<Recording>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimCase {
    pub id: String,
    pub claim: String,
    pub sources: Vec<CaseSource>,
    pub gold: Verdict,
    /// What the case probes (`number`, `negation`, `misquote`, `unrelated`).
    #[serde(default)]
    pub tags: Vec<String>,
}


#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuizCase {
    pub id: String,
    #[serde(default)]
    pub kind: QuestionKind,
    pub prompt: String,
    #[serde(default)]
    pub choices: Vec<String>,
    /// The key.
    pub key: String,
    /// The passage the question came from.
    #[serde(default)]
    pub passage: String,
    /// The student's answer being graded.
    pub given: String,
    /// The right grade.
    pub gold: Mark,
}

/// What a safety case checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyCheck {
    /// The trust scan on a server's tool or prompt description.
    Scan,
    /// Text from a page or server, wrapped as untrusted data: it must stay
    /// inside one block with nothing invisible left.
    Wrap,
    /// A tool call an injection talked the model into: the policy gate's
    /// answer under default settings.
    Gate,
    /// A live model reads an injected page; it must not obey it. Skipped
    /// offline.
    Canary,
}

/// What should happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyExpect {
    /// Scan: at least one danger flag.
    Flagged,
    /// Scan: no danger flag (a benign description).
    Clean,
    /// Wrap: one block, nothing escapes.
    Contained,
    /// Gate: asks or refuses, never runs on its own.
    Blocked,
    /// Gate: runs (a read-only step the defaults allow).
    Allowed,
    /// Canary: the reply does not contain the canary.
    Resisted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyCase {
    pub id: String,
    pub check: SafetyCheck,
    #[serde(default)]
    pub text: String,
    /// Where the text came from (`mcp:notes/search`, `web:example.com`).
    #[serde(default)]
    pub source: String,
    /// Gate cases: the tool and its effect class.
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub effect: String,
    #[serde(default)]
    pub target: String,
    /// Canary cases: the word the injection asks for.
    #[serde(default)]
    pub canary: String,
    pub expect: SafetyExpect,
}

/// A suite's cases, by kind.
#[derive(Clone, Debug, PartialEq)]
pub enum Cases {
    Research(Vec<ResearchCase>),
    Claims(Vec<ClaimCase>),
    Quiz(Vec<QuizCase>),
    Decision(Vec<Fixture>),
    Safety(Vec<SafetyCase>),
}

impl Cases {
    pub fn len(&self) -> usize {
        match self {
            Self::Research(c) => c.len(),
            Self::Claims(c) => c.len(),
            Self::Quiz(c) => c.len(),
            Self::Decision(c) => c.len(),
            Self::Safety(c) => c.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn ids(&self) -> Vec<String> {
        match self {
            Self::Research(c) => c.iter().map(|x| x.id.clone()).collect(),
            Self::Claims(c) => c.iter().map(|x| x.id.clone()).collect(),
            Self::Quiz(c) => c.iter().map(|x| x.id.clone()).collect(),
            Self::Decision(c) => c.iter().map(|x| x.id.clone()).collect(),
            Self::Safety(c) => c.iter().map(|x| x.id.clone()).collect(),
        }
    }
}

/// A suite as the file has it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct SuiteFile {
    format: u32,
    id: String,
    version: u32,
    title: String,
    kind: SuiteKind,
    level: Level,
    origin: String,
    #[serde(default)]
    description: String,
    /// Decision suites: which split this is (`held_out`).
    #[serde(default)]
    split: Option<String>,
    cases: Vec<Value>,
}

/// A loaded suite.
#[derive(Clone, Debug, PartialEq)]
pub struct Suite {
    pub id: String,
    pub version: u32,
    pub title: String,
    pub kind: SuiteKind,
    pub level: Level,
    /// Where the cases came from, said plainly (`synthetic: written for
    /// Tern's tests`), so a score is never mistaken for more than it is.
    pub origin: String,
    pub description: String,
    pub split: Option<String>,
    pub cases: Cases,
    /// Where it was loaded from: `bundled` or a file path.
    pub from: String,
}

impl Suite {
    /// `research@1`.
    pub fn tag(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }

    /// Reads a suite file. Every case must parse, ids must be unique, and
    /// references (gold sources, labels) must be in range.
    pub fn parse(text: &str, from: &str) -> Result<Self, String> {
        let f: SuiteFile = serde_json::from_str(text).map_err(|e| format!("{from}: {e}"))?;
        if f.format != FORMAT {
            return Err(format!(
                "{from}: suite format {} (this Tern reads format {FORMAT})",
                f.format
            ));
        }
        if f.id.trim().is_empty() || f.id.contains(char::is_whitespace) {
            return Err(format!("{from}: the suite id must be one word"));
        }
        fn each<T: serde::de::DeserializeOwned>(
            v: &[Value],
            from: &str,
        ) -> Result<Vec<T>, String> {
            v.iter()
                .enumerate()
                .map(|(i, c)| {
                    serde_json::from_value(c.clone())
                        .map_err(|e| format!("{from}: case {}: {e}", i + 1))
                })
                .collect()
        }
        let cases = match f.kind {
            SuiteKind::Research => Cases::Research(each(&f.cases, from)?),
            SuiteKind::Claims => Cases::Claims(each(&f.cases, from)?),
            SuiteKind::Quiz => Cases::Quiz(each(&f.cases, from)?),
            SuiteKind::Decision => Cases::Decision(each(&f.cases, from)?),
            SuiteKind::Safety => Cases::Safety(each(&f.cases, from)?),
            SuiteKind::Tests | SuiteKind::Usability => {
                return Err(format!(
                    "{from}: {} runs are made by Tern, not read from a suite file",
                    f.kind.key()
                ));
            }
        };
        let suite = Self {
            id: f.id,
            version: f.version,
            title: f.title,
            kind: f.kind,
            level: f.level,
            origin: f.origin,
            description: f.description,
            split: f.split,
            cases,
            from: from.to_owned(),
        };
        suite.validate()?;
        Ok(suite)
    }

    fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        for id in self.cases.ids() {
            if !seen.insert(id.clone()) {
                return Err(format!("{}: case id {id} is used twice", self.from));
            }
        }
        if self.cases.is_empty() {
            return Err(format!("{}: the suite has no cases", self.from));
        }
        let bad = |id: &str, what: String| Err(format!("{}: case {id}: {what}", self.from));
        match &self.cases {
            Cases::Research(cs) => {
                for c in cs {
                    let n = c.sources.len();
                    let in_range = |k: &usize| *k >= 1 && *k <= n;
                    if !c.gold_sources.iter().all(in_range) {
                        return bad(&c.id, "a gold source is out of range".into());
                    }
                    if !c.gold_claims.iter().all(|g| g.sources.iter().all(in_range)) {
                        return bad(&c.id, "a gold claim's source is out of range".into());
                    }
                    if self.level != Level::Pinned && c.recorded.is_empty() {
                        return bad(&c.id, "no recorded answer to replay offline".into());
                    }
                    for r in &c.recorded {
                        let cited = tern_research::claims::extract(&r.answer).len();
                        if !r.labels.is_empty() && r.labels.len() != cited {
                            return bad(
                                &c.id,
                                format!(
                                    "{} labels for {cited} cited sentences",
                                    r.labels.len()
                                ),
                            );
                        }
                    }
                }
            }
            Cases::Claims(cs) => {
                for c in cs {
                    if c.claim.trim().is_empty() {
                        return bad(&c.id, "the claim is empty".into());
                    }
                }
            }
            Cases::Quiz(cs) => {
                for c in cs {
                    if c.kind == QuestionKind::MultipleChoice && !c.choices.contains(&c.key) {
                        return bad(&c.id, "the key is not one of the choices".into());
                    }
                }
            }
            Cases::Decision(cs) => {
                for c in cs {
                    if c.gold >= c.choices.len() {
                        return bad(&c.id, "gold is out of range".into());
                    }
                    if let Err(e) = c.request().validate() {
                        return bad(&c.id, e.to_string());
                    }
                }
            }
            Cases::Safety(cs) => {
                for c in cs {
                    use SafetyCheck as C;
                    use SafetyExpect as E;
                    let ok = matches!(
                        (c.check, c.expect),
                        (C::Scan, E::Flagged | E::Clean)
                            | (C::Wrap, E::Contained)
                            | (C::Gate, E::Blocked | E::Allowed)
                            | (C::Canary, E::Resisted)
                    );
                    if !ok {
                        return bad(&c.id, "that expectation does not fit the check".into());
                    }
                    if c.check == C::Canary && c.canary.trim().is_empty() {
                        return bad(&c.id, "a canary case needs its canary word".into());
                    }
                }
            }
        }
        Ok(())
    }
}

const BUNDLED: &[(&str, &str)] = &[
    ("research", include_str!("../suites/research.v1.json")),
    ("claims", include_str!("../suites/claims.v1.json")),
    ("quiz", include_str!("../suites/quiz.v1.json")),
    (
        "decision-heldout",
        include_str!("../suites/decision-heldout.v1.json"),
    ),
    ("safety", include_str!("../suites/safety.v1.json")),
    ("pinned", include_str!("../suites/pinned.v1.json")),
];

/// The suites bundled with Tern.
pub fn bundled() -> Vec<Suite> {
    BUNDLED
        .iter()
        .map(|(name, text)| {
            Suite::parse(text, &format!("bundled:{name}"))
                .unwrap_or_else(|e| panic!("bundled suite {name}: {e}"))
        })
        .collect()
}

/// The suites on one rung, bundled and extra, in a stable order. An extra
/// suite with a bundled suite's id replaces it.
pub fn for_level(level: Level, extra: &[Suite]) -> Vec<Suite> {
    let mut out: Vec<Suite> = bundled()
        .into_iter()
        .filter(|s| !extra.iter().any(|e| e.id == s.id))
        .chain(extra.iter().cloned())
        .filter(|s| s.level == level)
        .collect();
    out.sort_by(|a, b| (a.kind, &a.id).cmp(&(b.kind, &b.id)));
    out
}
