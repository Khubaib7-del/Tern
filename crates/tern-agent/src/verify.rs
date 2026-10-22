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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    pub checks: Vec<ClaimCheck>,
    /// Tokens in and out of the judge call.
    pub judge_tokens: Option<(u64, u64)>,
    /// Why the judge was not used, when it failed.
    pub judge_error: Option<String>,
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
    let claims = extract_claims(answer);
    let mut out = Checked {
        checks: claims.iter().map(|c| check_lexical(c, sources)).collect(),
        ..Checked::default()
    };
    let Some(judge) = judge else {
        return out;
    };
    let some: Vec<Claim> = claims.into_iter().take(JUDGE_CLAIMS).collect();
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
    let numbered = numbered(answer, pages);
    let sources = source_texts(&numbered, fetcher).await;
    check_with(answer, &sources, judge).await
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
