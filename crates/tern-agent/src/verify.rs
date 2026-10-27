//! Checking an answer's claims against the pages it cites.
//!
//! The pure work (finding claims, scoring passages, reading a judge's
//! reply) lives in `tern-research`. This module gives it what only the
//! engine has: which page each `[n]` means, the pages' text from Tern
//! Crawl's cache, and a model to judge with.

use tern_crawl::Fetcher;
use tern_providers::{ChatMessage, ChatRequest};
pub use tern_research::claims::{Claim, extract as extract_claims};
pub use tern_research::verdict::{
    ClaimCheck, Evidence, Method, SourceText, Tally, Verdict, apply_judgments, check_lexical,
    judge_prompt, parse_judgments,
};

use crate::engine::Model;
use crate::sources::PageRef;

/// Most characters of a page compared against claims.
pub const TEXT_CHARS: usize = 200_000;
/// Most claims sent to the model judge in one call; the rest keep their
/// lexical verdict.
pub const JUDGE_CLAIMS: usize = 16;

/// How claims are judged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckClaims {
    /// The chat's own model reads the best passages, with the lexical
    /// verdict kept for any claim it does not answer.
    #[default]
    Model,
    /// Word overlap only: offline, free, deterministic.
    Lexical,
    /// No checks and no report.
    Off,
}

/// The model a judge call goes to.
#[allow(missing_debug_implementations)]
pub struct Judge<'a> {
    pub model: &'a dyn Model,
    pub model_name: &'a str,
}

/// Checks and what the judge cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Checked {
    pub checks: Vec<ClaimCheck>,
    /// Tokens in and out of the judge call.
    pub judge_tokens: Option<(u64, u64)>,
    /// Why the judge was not used, when it failed.
    pub judge_error: Option<String>,
    /// The decision model's pre-score of each claim, in claim order.
    pub decisions: Vec<crate::decide::Decision>,
}

/// The pre-score's choices, in this order.
pub const PRESCORE_CHOICES: [&str; 3] = ["supported", "partly", "not_supported"];
/// A pre-score below this confidence leaves the word-overlap verdict alone.
pub const PRESCORE_CONFIDENCE: f32 = 0.3;

fn verdict_rank(v: Verdict) -> i32 {
    match v {
        Verdict::Supported => 0,
        Verdict::Partly => 1,
        Verdict::NotSupported => 2,
        Verdict::NoSource => 3,
    }
}

/// The decision model's pre-score of each claim: supported, partly, or
/// not, read from the best passages of its cited sources. Hard checks
/// rule "supported" out when a number in the claim is missing from the
/// best passage or the passage says the opposite; claims with no source
/// text are not asked (they stay "No source"). A confident pre-score may
/// move the word-overlap verdict one step; the model judge, when it runs,
/// still has the final word.
pub async fn prescore(
    checks: &mut [ClaimCheck],
    sources: &[SourceText],
    decider: &crate::decide::Decider,
) -> Vec<(usize, crate::decide::Decision)> {
    use crate::decide::{Choice, DecisionKind, DecisionRequest, Rule};
    use tern_research::verdict::passages;
    let mut out = Vec::new();
    if !decider.is_on() || !crate::decide::job_running(crate::decide::Job::Support) {
        return out;
    }
    for check in checks.iter_mut() {
        if check.verdict == Verdict::NoSource {
            continue;
        }
        let cited: Vec<&SourceText> = check
            .citations
            .iter()
            .filter_map(|n| sources.iter().find(|s| s.n == *n))
            .filter(|s| !s.text.trim().is_empty())
            .collect();
        let found = passages(&check.text, &cited);
        let Some(best) = found.first() else {
            continue;
        };
        let context: String = found
            .iter()
            .take(3)
            .filter_map(|p| {
                cited
                    .iter()
                    .find(|s| s.n == p.source)
                    .map(|s| s.text[p.start..p.end].to_owned())
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut rules = Vec::new();
        if !best.numbers_match {
            rules.push(Rule::RuleOut {
                choice: 0,
                reason: "a number in the claim is not in the passage".into(),
            });
        }
        if !best.polarity_match {
            rules.push(Rule::RuleOut {
                choice: 0,
                reason: "the passage may say the opposite".into(),
            });
        }
        let req = DecisionRequest::new(
            DecisionKind::ClaimCheck,
            check.text.clone(),
            vec![
                Choice::new(
                    PRESCORE_CHOICES[0],
                    "Supported: the passage states the claim",
                ),
                Choice::new(
                    PRESCORE_CHOICES[1],
                    "Partly supported: the passage states some of it",
                ),
                Choice::new(
                    PRESCORE_CHOICES[2],
                    "Not supported: the passage does not state it",
                ),
            ],
        )
        .with_context(context);
        let Some(d) = decider.decide(&req, &rules).await else {
            continue;
        };
        let proposed = match d.chosen {
            0 => Verdict::Supported,
            1 => Verdict::Partly,
            _ => Verdict::NotSupported,
        };
        let step = verdict_rank(proposed) - verdict_rank(check.verdict);
        if proposed != check.verdict && d.confidence >= PRESCORE_CONFIDENCE {
            let to = match step.signum() {
                1 => match check.verdict {
                    Verdict::Supported => Verdict::Partly,
                    _ => Verdict::NotSupported,
                },
                _ => match check.verdict {
                    Verdict::NotSupported => Verdict::Partly,
                    _ => Verdict::Supported,
                },
            };
            check.verdict = to;
            check.method = Method::Decision;
            check.note = format!("Decision model: {}", d.rationale);
            if to == Verdict::NotSupported {
                check.evidence = None;
            } else if check.evidence.is_none() {
                check.evidence = Some(tern_research::verdict::evidence_of(best, &cited));
            }
        }
        out.push((check.claim, d));
    }
    out
}

/// A URL compared loosely: no fragment, no trailing slash, no `www.`,
/// either scheme.
fn loose(url: &str) -> String {
    let u = url.split('#').next().unwrap_or(url).trim();
    let u = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let u = u.strip_prefix("www.").unwrap_or(u);
    u.trim_end_matches('/').to_lowercase()
}

/// Which page each citation number means: the answer's own closing link
/// list where it names a page Tern read, otherwise the n-th page read.
pub fn numbered(answer: &str, pages: &[PageRef]) -> Vec<(usize, PageRef)> {
    let listed = tern_research::claims::link_list(answer);
    let mut out: Vec<(usize, PageRef)> = Vec::new();
    for (n, url) in &listed {
        if let Some(p) = pages.iter().find(|p| loose(&p.url) == loose(url)) {
            out.push((*n, p.clone()));
        }
    }
    for n in tern_research::claims::citations(answer) {
        if out.iter().any(|(m, _)| *m == n) {
            continue;
        }
        // A number the list names with a page Tern never read stays
        // unmatched: that citation has no source.
        if listed.iter().any(|(m, _)| *m == n) {
            continue;
        }
        if n >= 1
            && let Some(p) = pages.get(n - 1)
        {
            out.push((n, p.clone()));
        }
    }
    out.sort_by_key(|(n, _)| *n);
    out
}

/// Where each citation number in `answer` leads when clicked (#72): the
/// page [`numbered`] matches it with, else the URL the answer's own closing
/// link list gives it, even for a page Tern did not read (a search hit).
pub fn cite_targets(answer: &str, pages: &[PageRef]) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = numbered(answer, pages)
        .into_iter()
        .map(|(n, p)| (n, p.url))
        .collect();
    for (n, url) in tern_research::claims::link_list(answer) {
        if !out.iter().any(|(m, _)| *m == n) {
            out.push((n, url));
        }
    }
    out.sort_by_key(|(n, _)| *n);
    out
}

/// The cited pages' text, read through `fetcher` (a cache hit for pages
/// the turn just read). A page that cannot be read again has empty text.
pub async fn source_texts(numbered: &[(usize, PageRef)], fetcher: &Fetcher) -> Vec<SourceText> {
    let reads =
        futures::future::join_all(numbered.iter().map(|(_, p)| fetcher.fetch(&p.url))).await;
    numbered
        .iter()
        .zip(reads)
        .map(|((n, p), r)| SourceText {
            n: *n,
            url: p.url.clone(),
            title: p.title.clone(),
            site: p.site.clone(),
            text: r
                .map(|pg| pg.text.chars().take(TEXT_CHARS).collect())
                .unwrap_or_default(),
        })
        .collect()
}

/// Checks every claim of `answer` against `sources`, with the judge when
/// one is given.
pub async fn check_with(answer: &str, sources: &[SourceText], judge: Option<Judge<'_>>) -> Checked {
    check_with_decider(answer, sources, judge, None).await
}

/// [`check_with`] with the decision model's pre-score: word overlap first,
/// then the pre-score (within the hard checks), then the model judge. When
/// there are more claims than the judge takes, the least certain go to it.
pub async fn check_with_decider(
    answer: &str,
    sources: &[SourceText],
    judge: Option<Judge<'_>>,
    decider: Option<&crate::decide::Decider>,
) -> Checked {
    let claims = extract_claims(answer);
    let mut out = Checked {
        checks: claims.iter().map(|c| check_lexical(c, sources)).collect(),
        ..Checked::default()
    };
    let scored = match decider {
        Some(d) => prescore(&mut out.checks, sources, d).await,
        None => Vec::new(),
    };
    out.decisions = scored.iter().map(|(_, d)| d.clone()).collect();
    let Some(judge) = judge else {
        return out;
    };
    let mut claims = claims;
    if claims.len() > JUDGE_CLAIMS && !scored.is_empty() {
        let certainty = |c: &Claim| {
            scored
                .iter()
                .find(|(n, _)| *n == c.n)
                .map(|(_, d)| d.confidence)
                .unwrap_or(0.0)
        };
        claims.sort_by(|a, b| {
            certainty(a)
                .partial_cmp(&certainty(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.n.cmp(&b.n))
        });
    }
    let mut some: Vec<Claim> = claims.into_iter().take(JUDGE_CLAIMS).collect();
    some.sort_by_key(|c| c.n);
    let Some((system, message)) = judge_prompt(&some, sources) else {
        return out;
    };
    let mut req = ChatRequest::new(judge.model_name, vec![ChatMessage::user(message)]);
    req.system = Some(system);
    req.max_tokens = 1600;
    req.temperature = Some(0.0);
    match judge.model.complete(&req).await {
        Ok(resp) => {
            out.judge_tokens = Some((resp.usage.input_tokens, resp.usage.output_tokens));
            let judgments = parse_judgments(&resp.text);
            if judgments.is_empty() {
                out.judge_error = Some("The model's verdicts could not be read".into());
            }
            apply_judgments(&mut out.checks, &judgments, sources);
        }
        Err(e) => out.judge_error = Some(e.to_string()),
    }
    out
}

/// Checks an answer against the pages its turn read.
pub async fn check_answer(
    answer: &str,
    pages: &[PageRef],
    fetcher: &Fetcher,
    judge: Option<Judge<'_>>,
) -> Checked {
    check_answer_with(answer, pages, fetcher, judge, None).await
}

/// [`check_answer`] with the decision model's pre-score.
pub async fn check_answer_with(
    answer: &str,
    pages: &[PageRef],
    fetcher: &Fetcher,
    judge: Option<Judge<'_>>,
    decider: Option<&crate::decide::Decider>,
) -> Checked {
    let numbered = numbered(answer, pages);
    let sources = source_texts(&numbered, fetcher).await;
    check_with_decider(answer, &sources, judge, decider).await
}

/// The answer with a small verdict badge after each checked claim: a
/// Markdown link whose text is the verdict's glyph and whose target is
/// `link(claim number)`, so a front end can open the claim in its report.
pub fn annotate(answer: &str, checks: &[ClaimCheck], link: impl Fn(usize) -> String) -> String {
    if checks.is_empty() {
        return answer.to_owned();
    }
    let claims = extract_claims(answer);
    let mut out = answer.to_owned();
    for c in claims.iter().rev() {
        let Some(check) = checks.iter().find(|k| k.claim == c.n) else {
            continue;
        };
        let badge = format!(" [{}]({})", check.verdict.glyph(), link(c.n));
        out.insert_str(c.span.1, &badge);
    }
    out
}

/// The claim number a badge link points at, for links made by
/// [`annotate`] with [`claim_link`].
pub fn claim_of_link(url: &str) -> Option<(usize, usize)> {
    let rest = url.strip_prefix(CLAIM_SCHEME)?;
    let (entry, claim) = rest.split_once('/')?;
    Some((entry.parse().ok()?, claim.parse().ok()?))
}

const CLAIM_SCHEME: &str = "tern://claim/";

/// A badge link for claim `claim` of transcript entry `entry`.
pub fn claim_link(entry: usize, claim: usize) -> String {
    format!("{CLAIM_SCHEME}{entry}/{claim}")
}

/// Whether the answer cites anything at all (a cheap pre-check).
pub fn has_claims(answer: &str) -> bool {
    !tern_research::claims::citations(answer).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_providers::{ChatResponse, ProviderResult};

    fn page(url: &str) -> PageRef {
        PageRef {
            title: url.into(),
            url: url.into(),
            site: url.split('/').nth(2).unwrap_or_default().into(),
            builtin: None,
            favicon: None,
        }
    }

    #[test]
    fn numbers_follow_the_link_list_then_reading_order() {
        let pages = [page("https://a.test/x"), page("https://b.test/y/")];
        let answer =
            "A [1]. B [2]. C [3].\n\nSources:\n[1] https://www.b.test/y\n[3] https://never.test\n";
        let n: Vec<_> = numbered(answer, &pages)
            .into_iter()
            .map(|(n, p)| (n, p.url))
            .collect();
        assert_eq!(
            n,
            [
                (1, "https://b.test/y/".to_owned()),
                (2, "https://b.test/y/".to_owned()),
            ],
            "[2] falls back to the second page read; [3] names an unread page"
        );
        let plain = numbered("A [1]. B [2].", &pages);
        assert_eq!(plain[0].1.url, "https://a.test/x");
    }

    #[test]
    fn chips_lead_to_listed_pages_tern_did_not_read() {
        let pages = [page("https://a.test/x"), page("https://b.test/y")];
        let answer = "From the web: X [1]. Y [2].\n\n[1] LangChain - Wikipedia — https://en.wikipedia.org/wiki/LangChain\n";
        assert_eq!(
            cite_targets(answer, &pages),
            [
                (1, "https://en.wikipedia.org/wiki/LangChain".to_owned()),
                (2, "https://b.test/y".to_owned()),
            ],
            "[1] from the link list though unread; [2] the second page read"
        );
    }

    #[test]
    fn badges_follow_their_claims() {
        let answer = "Water boils at 100 C [1]. Ice is cold [2].\n";
        let checks = vec![
            ClaimCheck {
                claim: 1,
                text: String::new(),
                citations: vec![1],
                verdict: Verdict::Supported,
                evidence: None,
                overlap: 100,
                method: Method::Lexical,
                note: String::new(),
            },
            ClaimCheck {
                claim: 2,
                text: String::new(),
                citations: vec![2],
                verdict: Verdict::NoSource,
                evidence: None,
                overlap: 0,
                method: Method::Lexical,
                note: String::new(),
            },
        ];
        let out = annotate(answer, &checks, |c| claim_link(4, c));
        assert_eq!(
            out,
            "Water boils at 100 C [1]. [✓](tern://claim/4/1) Ice is cold [2]. [○](tern://claim/4/2)\n"
        );
        assert_eq!(claim_of_link("tern://claim/4/2"), Some((4, 2)));
        assert_eq!(claim_of_link("https://x.test"), None);
        assert_eq!(annotate(answer, &[], |_| String::new()), answer);
        assert!(has_claims(answer));
    }

    struct Canned(&'static str);

    impl Model for Canned {
        fn provider_id(&self) -> &str {
            "test"
        }
        fn complete<'a>(
            &'a self,
            _req: &'a ChatRequest,
        ) -> crate::tools::BoxFuture<'a, ProviderResult<ChatResponse>> {
            Box::pin(async move {
                Ok(ChatResponse {
                    text: self.0.to_owned(),
                    usage: tern_providers::Usage::measured(100, 20),
                    finish_reason: None,
                    model: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn the_judge_overrides_and_falls_back() {
        let sources = vec![SourceText {
            n: 1,
            url: "https://e.test".into(),
            title: "E".into(),
            site: "e.test".into(),
            text: "The tower was completed in 1889 for the World's Fair. It is 330 metres tall."
                .into(),
        }];
        let answer = "The tower hosted many visitors [1]. It is 330 metres tall [1].";
        let lexical = check_with(answer, &sources, None).await;
        assert_eq!(lexical.checks[0].verdict, Verdict::NotSupported);
        assert_eq!(lexical.checks[1].verdict, Verdict::Supported);
        let model = Canned(
            r#"[{"claim": 1, "verdict": "supported", "quote": "completed in 1889 for the World's Fair"}]"#,
        );
        let judged = check_with(
            answer,
            &sources,
            Some(Judge {
                model: &model,
                model_name: "m",
            }),
        )
        .await;
        assert_eq!(judged.checks[0].verdict, Verdict::Supported);
        assert_eq!(judged.checks[0].method, Method::Model);
        assert_eq!(
            judged.checks[1].method,
            Method::Lexical,
            "unanswered keeps lexical"
        );
        assert!(judged.judge_error.is_none());
        let garbled = Canned("I think they are fine.");
        let g = check_with(
            answer,
            &sources,
            Some(Judge {
                model: &garbled,
                model_name: "m",
            }),
        )
        .await;
        assert!(g.judge_error.is_some());
        assert_eq!(g.checks, lexical.checks);
    }
}
