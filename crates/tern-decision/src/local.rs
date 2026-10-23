//! The local decision model: deterministic, offline, free.
//!
//! BM25 and word coverage between the question and each choice, plus a few
//! heuristics per decision kind (source authority, claim overlap with the
//! numbers and negation checked, cue words for tools and urgency), turned
//! into probabilities with a softmax. No model download, no network.

use std::time::Instant;

use crate::calibrate::{argmax, confidence, softmax};
use crate::model::{
    BoxFuture, Choice, Cost, Decision, DecisionError, DecisionKind, DecisionModel, DecisionRequest,
    clip,
};
use crate::text::{bm25, coverage, negated, numbers, sentences, term_set, terms};

/// The local model's id in logs.
pub const LOCAL_MODEL: &str = "local-lexical-1";

/// The local scorer.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalModel;

/// Scores and a reason per choice, before the softmax.
struct Scored {
    logits: Vec<f32>,
    temperature: f32,
    reasons: Vec<String>,
}

impl LocalModel {
    /// Scores `req` at once. Never fails on a valid request.
    pub fn decide(&self, req: &DecisionRequest) -> Decision {
        let started = Instant::now();
        let s = match req.kind {
            DecisionKind::SourceRank => source_rank(req),
            DecisionKind::ClaimCheck => claim_check(req).unwrap_or_else(|| general(req, 0.7)),
            DecisionKind::NextTool => next_tool(req),
            DecisionKind::Triage => triage(req),
            DecisionKind::General => general(req, 0.7),
        };
        let probabilities = softmax(&s.logits, s.temperature);
        let chosen = argmax(&probabilities);
        Decision {
            kind: req.kind,
            question: clip(&req.question, 200),
            choices: req.choices.iter().map(|c| c.id.clone()).collect(),
            confidence: confidence(&probabilities),
            rationale: s.reasons.get(chosen).cloned().unwrap_or_default(),
            probabilities,
            chosen,
            model: LOCAL_MODEL.into(),
            provider: "local".into(),
            latency_ms: started.elapsed().as_millis() as u64,
            cost: Cost::default(),
            ..Decision::default()
        }
    }
}

impl DecisionModel for LocalModel {
    fn provider(&self) -> &str {
        "local"
    }
    fn model(&self) -> &str {
        LOCAL_MODEL
    }
    fn score<'a>(
        &'a self,
        req: &'a DecisionRequest,
    ) -> BoxFuture<'a, Result<Decision, DecisionError>> {
        Box::pin(async move {
            req.validate()?;
            Ok(self.decide(req))
        })
    }
}

/// BM25 of the query against the choices, scaled so the best is 1.
fn relevance(query: &str, choices: &[Choice]) -> (Vec<f32>, Vec<f32>) {
    let q = terms(query);
    let docs: Vec<Vec<String>> = choices.iter().map(|c| terms(&c.text)).collect();
    let raw = bm25(&q, &docs);
    let max = raw.iter().copied().fold(0.0, f32::max);
    let scaled = raw
        .iter()
        .map(|s| if max > 0.0 { s / max } else { 0.0 })
        .collect();
    let want = term_set(query);
    let cov = choices
        .iter()
        .map(|c| coverage(&want, &term_set(&c.text)))
        .collect();
    (scaled, cov)
}

fn has_any(text: &str, cues: &[&str]) -> bool {
    let t = format!(" {} ", text.to_lowercase());
    cues.iter().any(|c| t.contains(c))
}

fn general(req: &DecisionRequest, temperature: f32) -> Scored {
    let query = format!("{} {}", req.question, req.context);
    let (rel, cov) = relevance(&query, &req.choices);
    let mut logits = Vec::new();
    let mut reasons = Vec::new();
    for (i, c) in req.choices.iter().enumerate() {
        let prior = c.prior.unwrap_or(0.0);
        logits.push(2.0 * rel[i] + 1.5 * cov[i] + 0.8 * prior);
        reasons.push(format!(
            "Shares {:.0}% of the question's words",
            cov[i] * 100.0
        ));
    }
    Scored {
        logits,
        temperature,
        reasons,
    }
}

/// How much a source's site is worth before reading it: 1 for scholarly
/// and official sites, 0.5 for reference works and documentation, below
/// zero for social media and content farms.
pub fn authority(url_or_site: &str) -> f32 {
    let s = url_or_site.to_lowercase();
    let host = s
        .split("://")
        .nth(1)
        .unwrap_or(&s)
        .split(['/', '?', '#', ' '])
        .next()
        .unwrap_or("")
        .trim_start_matches("www.")
        .to_owned();
    let ends = |suffixes: &[&str]| {
        suffixes
            .iter()
            .any(|x| host == x.trim_start_matches('.') || host.ends_with(x))
    };
    if ends(&[
        ".edu",
        ".gov",
        ".ac.uk",
        ".ac.jp",
        ".edu.pk",
        ".gov.uk",
        ".int",
        "arxiv.org",
        "doi.org",
        "nih.gov",
        "nature.com",
        "science.org",
        "acm.org",
        "ieee.org",
        "springer.com",
        "sciencedirect.com",
        "wiley.com",
        "jstor.org",
        "plos.org",
        "semanticscholar.org",
        "aclanthology.org",
        "openreview.net",
        "europa.eu",
    ]) {
        1.0
    } else if ends(&[
        "wikipedia.org",
        "britannica.com",
        "python.org",
        "rust-lang.org",
        "mozilla.org",
        "w3.org",
        "ietf.org",
        "rfc-editor.org",
        "github.com",
        "reuters.com",
        "apnews.com",
        "bbc.co.uk",
        "bbc.com",
    ]) || host.starts_with("docs.")
        || host.starts_with("developer.")
    {
        0.5
    } else if ends(&[
        "pinterest.com",
        "quora.com",
        "facebook.com",
        "instagram.com",
        "tiktok.com",
        "twitter.com",
        "x.com",
        "answers.com",
        "ehow.com",
        "wikihow.com",
    ]) {
        -1.0
    } else if ends(&["reddit.com", "medium.com", "youtube.com", "linkedin.com"]) {
        -0.4
    } else {
        0.0
    }
}

fn source_rank(req: &DecisionRequest) -> Scored {
    let query = format!("{} {}", req.question, req.context);
    let (rel, cov) = relevance(&query, &req.choices);
    let mut logits = Vec::new();
    let mut reasons = Vec::new();
    for (i, c) in req.choices.iter().enumerate() {
        let auth = authority(&c.id).max(authority(&c.text));
        let auth = if authority(&c.id) < 0.0 || authority(&c.text) < 0.0 {
            authority(&c.id).min(authority(&c.text))
        } else {
            auth
        };
        let prior = c.prior.unwrap_or(0.0);
        let low_value = has_any(
            &c.text,
            &[
                " top 10 ",
                " top ten ",
                " you won't believe ",
                " sponsored ",
                " buy now ",
            ],
        );
        logits.push(
            2.0 * rel[i] + 1.5 * cov[i] + 0.6 * auth + 0.8 * prior
                - if low_value { 0.8 } else { 0.0 },
        );
        let mut why = format!("Matches {:.0}% of the question", cov[i] * 100.0);
        if auth >= 1.0 {
            why.push_str("; scholarly or official site");
        } else if auth > 0.0 {
            why.push_str("; reference or documentation site");
        } else if auth < 0.0 {
            why.push_str("; social or low-authority site");
        }
        reasons.push(why);
    }
    Scored {
        logits,
        temperature: 0.5,
        reasons,
    }
}

/// Which verdict a claim-check choice stands for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Label {
    Supported,
    Partly,
    Not,
}

fn label_of(c: &Choice) -> Option<Label> {
    let t = format!(" {} {} ", c.id, c.text)
        .to_lowercase()
        .replace('_', " ");
    if has_any(
        &t,
        &[" not ", " no ", " unsupported", " false", " contradict"],
    ) {
        Some(Label::Not)
    } else if has_any(&t, &[" partly", " partial", " somewhat", " in part"]) {
        Some(Label::Partly)
    } else if has_any(&t, &[" supported", " yes ", " true ", " pass"]) {
        Some(Label::Supported)
    } else {
        None
    }
}

/// The best stretch of `passage` for `claim`: coverage, numbers, negation.
pub fn best_window(claim: &str, passage: &str) -> (f32, bool, bool) {
    let want = term_set(claim);
    let want_nums = numbers(claim);
    let want_neg = negated(claim);
    let s = sentences(passage);
    let mut best = (0.0f32, want_nums.is_empty(), !want_neg);
    for k in 0..s.len() {
        for w in 1..=2 {
            if k + w > s.len() {
                continue;
            }
            let text = s[k..k + w].join(" ");
            let cov = coverage(&want, &term_set(&text));
            let nums = want_nums.is_subset(&numbers(&text));
            let pol = negated(&text) == want_neg;
            let better = cov > best.0 + 1e-6 || ((cov - best.0).abs() < 1e-6 && nums && !best.1);
            if better {
                best = (cov, nums, pol);
            }
        }
    }
    best
}

fn claim_check(req: &DecisionRequest) -> Option<Scored> {
    let labels: Vec<Label> = req.choices.iter().map(label_of).collect::<Option<_>>()?;
    let (cov, nums, pol) = best_window(&req.question, &req.context);
    let mut logits = Vec::new();
    let mut reasons = Vec::new();
    for l in &labels {
        let (logit, why) = match l {
            Label::Supported => (
                9.0 * (cov - 0.72) - if nums { 0.0 } else { 4.0 } - if pol { 0.0 } else { 4.0 },
                format!(
                    "The best passage has {:.0}% of the claim's words",
                    cov * 100.0
                ),
            ),
            Label::Partly => (
                2.5 - 12.0 * (cov - 0.55).abs()
                    + if !nums && cov > 0.7 { 1.5 } else { 0.0 }
                    + if !pol && cov > 0.7 { 1.0 } else { 0.0 },
                if !nums {
                    "A number in the claim is not in the passage".into()
                } else if !pol {
                    "The passage may say the opposite".into()
                } else {
                    format!("The passage covers {:.0}% of the claim", cov * 100.0)
                },
            ),
            Label::Not => (
                9.0 * (0.42 - cov) + if !pol && cov > 0.6 { 1.0 } else { 0.0 },
                format!("Only {:.0}% of the claim's words are there", cov * 100.0),
            ),
        };
        logits.push(logit);
        reasons.push(why);
    }
    Some(Scored {
        logits,
        temperature: 1.0,
        reasons,
    })
}

/// What a tool choice is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Web,
    Read,
    Notes,
    Answer,
    Other,
}

fn tool_of(c: &Choice) -> Tool {
    let id = c.id.to_lowercase();
    if id.contains("read") || id.contains("url") || id.contains("open") {
        Tool::Read
    } else if id.contains("vault") || id.contains("note") || id.contains("file") {
        Tool::Notes
    } else if id.contains("search") || id.contains("web") {
        Tool::Web
    } else if id.contains("answer") || id.contains("reply") || id == "none" || id == "respond" {
        Tool::Answer
    } else {
        Tool::Other
    }
}

/// A count stated in the context as `name: N`.
fn count_in(context: &str, name: &str) -> usize {
    let lower = context.to_lowercase();
    lower
        .find(&format!("{name}:"))
        .and_then(|at| {
            lower[at + name.len() + 1..]
                .split(|c: char| !c.is_ascii_digit() && c != ' ')
                .next()
                .and_then(|s| s.trim().parse().ok())
        })
        .unwrap_or(0)
}

fn next_tool(req: &DecisionRequest) -> Scored {
    let q = req.question.to_lowercase();
    let has_url = q.contains("http://") || q.contains("https://") || q.contains("www.");
    let recency = has_any(
        &q,
        &[
            " latest",
            " today",
            " current",
            " recent",
            " news",
            " this week",
            " this year",
            " now ",
            " price",
            " release",
            " update",
            " 2025",
            " 2026",
            " yesterday",
            " announced",
            " version",
        ],
    );
    let notes = has_any(
        &q,
        &[
            " my notes",
            " vault",
            " i wrote",
            " my files",
            " in my note",
            " my note ",
        ],
    );
    let attached = count_in(&req.context, "attachments") > 0
        && has_any(
            &q,
            &[
                " attached",
                " this pdf",
                " this file",
                " the file",
                " the document",
                " this document",
                " uploaded",
                " the attachment",
                " this paper",
                " summarize this",
                " summarise this",
            ],
        );
    let words = q.split_whitespace().count();
    let chit = words <= 5
        && has_any(
            &q,
            &[
                " hi ",
                " hello",
                " thanks",
                " thank you",
                " hey ",
                " good morning",
            ],
        );
    let arithmetic = q
        .chars()
        .filter(|c| "0123456789+-*/=^()".contains(*c))
        .count()
        * 2
        > q.chars().filter(|c| !c.is_whitespace()).count()
        && q.chars().any(|c| "+-*/^".contains(c));
    let unread = count_in(&req.context, "unread results");
    let (rel, _) = relevance(&req.question, &req.choices);
    let mut logits = Vec::new();
    let mut reasons = Vec::new();
    for (i, c) in req.choices.iter().enumerate() {
        let (base, why): (f32, &str) = match tool_of(c) {
            Tool::Web => (
                1.0 + if recency { 2.0 } else { 0.0 }
                    - if has_url { 2.5 } else { 0.0 }
                    - if notes { 2.0 } else { 0.0 }
                    - if attached { 2.0 } else { 0.0 }
                    - if chit || arithmetic { 2.5 } else { 0.0 }
                    - if unread > 0 { 1.5 } else { 0.0 },
                if recency {
                    "The question asks about something recent"
                } else {
                    "A factual question the web can answer"
                },
            ),
            Tool::Read => (
                -0.5 + if has_url { 3.5 } else { 0.0 } + if unread > 0 { 2.5 } else { 0.0 },
                if has_url {
                    "The question names a page"
                } else {
                    "There are search results not read yet"
                },
            ),
            Tool::Notes => (
                -1.0 + if notes { 3.5 } else { 0.0 },
                "The question is about the user's own notes",
            ),
            Tool::Answer => (
                if chit || arithmetic { 3.0 } else { 0.0 } + if attached { 3.0 } else { 0.0 }
                    - if recency { 1.0 } else { 0.0 },
                if attached {
                    "The attachments already hold the material"
                } else {
                    "No tool is needed"
                },
            ),
            Tool::Other => (0.0, "Matches the question's words"),
        };
        logits.push(base + 0.5 * rel[i]);
        reasons.push(why.to_owned());
    }
    Scored {
        logits,
        temperature: 1.0,
        reasons,
    }
}

fn triage(req: &DecisionRequest) -> Scored {
    let query = format!("{} {}", req.question, req.context);
    let (rel, cov) = relevance(&query, &req.choices);
    let mut logits = Vec::new();
    let mut reasons = Vec::new();
    for (i, c) in req.choices.iter().enumerate() {
        let later = has_any(
            &c.text,
            &[
                " optional",
                " later",
                " nice to have",
                " future work",
                " tangent",
                " side note",
                " if time",
                " eventually",
                " someday",
                " low priority",
            ],
        );
        let urgent = has_any(
            &c.text,
            &[
                " deadline",
                " due ",
                " today",
                " urgent",
                " blocking",
                " blocker",
                " first",
                " before ",
                " asap",
                " overdue",
            ],
        );
        let basic = has_any(
            &c.text,
            &[
                " define",
                " definition",
                " what is",
                " overview",
                " background",
                " basics",
            ],
        );
        let prior = c.prior.unwrap_or(0.0);
        logits.push(
            1.5 * rel[i] + 1.5 * cov[i] + 1.0 * prior - if later { 1.5 } else { 0.0 }
                + if urgent { 1.0 } else { 0.0 }
                + if basic { 0.5 } else { 0.0 },
        );
        reasons.push(if later {
            "Marked as something for later".into()
        } else if urgent {
            "Urgent or blocking".into()
        } else {
            format!("Central to the goal ({:.0}% of its words)", cov[i] * 100.0)
        });
    }
    Scored {
        logits,
        temperature: 0.6,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(kind: DecisionKind, q: &str, ctx: &str, choices: &[(&str, &str)]) -> DecisionRequest {
        DecisionRequest::new(
            kind,
            q,
            choices.iter().map(|(i, t)| Choice::new(*i, *t)).collect(),
        )
        .with_context(ctx)
    }

    #[test]
    fn ranks_the_relevant_scholarly_source_first() {
        let r = req(
            DecisionKind::SourceRank,
            "What did Vaswani et al. propose in the transformer paper?",
            "",
            &[
                (
                    "https://www.pinterest.com/pin/1",
                    "Transformers toys and robots",
                ),
                (
                    "https://arxiv.org/abs/1706.03762",
                    "Attention Is All You Need - Vaswani et al. propose the Transformer",
                ),
                ("https://example.com/blog", "My weekend cooking blog"),
            ],
        );
        let d = LocalModel.decide(&r);
        assert_eq!(d.chosen, 1);
        assert!((d.probabilities.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        assert!(d.rationale.contains("scholarly"));
        assert_eq!(d.cost, Cost::default());
    }

    #[test]
    fn claim_checks_follow_overlap_numbers_and_negation() {
        let choices = [
            ("supported", "Supported"),
            ("partly", "Partly supported"),
            ("not_supported", "Not supported"),
        ];
        let ctx = "The tower was completed in 1889 for the World's Fair. It is 330 metres tall.";
        let yes = LocalModel.decide(&req(
            DecisionKind::ClaimCheck,
            "The tower is 330 metres tall",
            ctx,
            &choices,
        ));
        assert_eq!(yes.chosen_id(), "supported");
        let wrong_number = LocalModel.decide(&req(
            DecisionKind::ClaimCheck,
            "The tower is 300 metres tall",
            ctx,
            &choices,
        ));
        assert_ne!(wrong_number.chosen_id(), "supported");
        let no = LocalModel.decide(&req(
            DecisionKind::ClaimCheck,
            "The bridge carries six lanes of traffic",
            ctx,
            &choices,
        ));
        assert_eq!(no.chosen_id(), "not_supported");
    }

    #[test]
    fn next_tool_reads_cues() {
        let tools = [
            ("web_search", "Search the web"),
            ("read_url", "Read one page"),
            ("vault_search", "Search the user's notes"),
            ("answer", "Answer without a tool"),
        ];
        let pick = |q: &str, ctx: &str| {
            LocalModel
                .decide(&req(DecisionKind::NextTool, q, ctx, &tools))
                .chosen_id()
                .to_owned()
        };
        assert_eq!(pick("What is the latest Rust release?", ""), "web_search");
        assert_eq!(pick("Summarise https://example.com/post", ""), "read_url");
        assert_eq!(
            pick("What did I write in my notes about SM-2?", ""),
            "vault_search"
        );
        assert_eq!(pick("thanks!", ""), "answer");
        assert_eq!(
            pick("Summarize this pdf for me", "attachments: 1"),
            "answer"
        );
    }

    #[test]
    fn triage_puts_later_items_last() {
        let d = LocalModel.decide(&req(
            DecisionKind::Triage,
            "How do solid-state batteries compare with lithium-ion on energy density?",
            "",
            &[
                ("1", "History of the battery industry (optional, later)"),
                (
                    "2",
                    "Energy density of solid-state batteries versus lithium-ion",
                ),
            ],
        ));
        assert_eq!(d.chosen, 1);
    }

    #[test]
    fn authority_tiers() {
        assert_eq!(authority("https://www.cs.stanford.edu/x"), 1.0);
        assert_eq!(authority("https://en.wikipedia.org/wiki/X"), 0.5);
        assert_eq!(authority("pinterest.com"), -1.0);
        assert_eq!(authority("https://example.com"), 0.0);
    }
}
