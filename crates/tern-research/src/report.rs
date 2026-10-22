//! The run report: one record per answered turn, as JSON for tools and as
//! Markdown for people.
//!
//! It holds the question, the plan and steps, every source read with how
//! Tern Crawl read it, each claim with its verdict and evidence, what the
//! run used against its budget, and what went wrong.

use serde::{Deserialize, Serialize};

use crate::verdict::{ClaimCheck, Tally, Verdict};

/// The report format's version, bumped when a field changes meaning.
pub const REPORT_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    pub version: u32,
    /// The chat's session id.
    pub session: String,
    /// 1-based turn number in the chat.
    pub turn: usize,
    /// When the turn started, RFC 3339.
    pub started: String,
    pub question: String,
    pub answer: String,
    /// "Answered", or why the turn stopped.
    pub outcome: String,
    pub provider: String,
    pub model: String,
    /// The plan the run followed. Empty for a quick turn, which has none.
    pub plan: Vec<String>,
    pub steps: Vec<ReportStep>,
    pub sources: Vec<ReportSource>,
    pub claims: Vec<ClaimCheck>,
    pub usage: ReportUsage,
    pub errors: Vec<String>,
}

/// One step, as the run panel's Detailed view shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportStep {
    /// Milliseconds since the turn started.
    pub at_ms: u64,
    pub verb: String,
    pub target: String,
    #[serde(default)]
    pub note: String,
    pub ok: bool,
}

/// One page the run read (or tried to).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSource {
    /// The number the answer cites it by.
    pub n: usize,
    pub title: String,
    pub url: String,
    pub site: String,
    /// Which reader fetched it ("Tern Crawl").
    pub reader: String,
    /// "allowed", "blocked", or "not checked".
    pub robots: String,
    /// "hit" (from Tern's page cache), "miss" (read live), or "unknown".
    pub cache: String,
    /// The answer cites it.
    pub cited: bool,
    /// Why it could not be read, for a failed read.
    #[serde(default)]
    pub error: Option<String>,
}

/// What the run used.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportUsage {
    pub model_calls: u64,
    pub tool_calls: u64,
    pub searches: u64,
    pub pages: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub took_ms: u64,
    /// Tokens the claim checks' model judge used, on top of the turn's.
    #[serde(default)]
    pub judge_tokens_in: u64,
    #[serde(default)]
    pub judge_tokens_out: u64,
    /// The turn's limits, when known.
    #[serde(default)]
    pub budget: Option<ReportBudget>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportBudget {
    pub max_model_calls: u64,
    pub max_tool_calls: u64,
    pub max_tokens: u64,
}

impl RunReport {
    pub fn tally(&self) -> Tally {
        Tally::of(&self.claims)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("not a Tern run report: {e}"))
    }

    /// The report as Markdown.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        let title: String = one_line(&self.question).chars().take(90).collect();
        md.push_str(&format!("# Run report: {title}\n\n"));
        md.push_str(&format!(
            "Turn {} of chat `{}` · started {} · {} on {} · {}\n\n",
            self.turn,
            self.session,
            or_dash(&self.started),
            or_dash(&self.model),
            or_dash(&self.provider),
            self.outcome
        ));

        md.push_str("## Question\n\n");
        md.push_str(&quote_block(&self.question));

        md.push_str("\n## Plan and steps\n\n");
        if self.plan.is_empty() {
            md.push_str("Quick turn: no written plan.\n\n");
        } else {
            for (i, p) in self.plan.iter().enumerate() {
                md.push_str(&format!("{}. {}\n", i + 1, one_line(p)));
            }
            md.push('\n');
        }
        if self.steps.is_empty() {
            md.push_str("No steps were logged.\n");
        } else {
            md.push_str("| Time | Step | Target | Note |\n| --- | --- | --- | --- |\n");
            for s in &self.steps {
                md.push_str(&format!(
                    "| {:.1}s | {}{} | {} | {} |\n",
                    s.at_ms as f64 / 1000.0,
                    if s.ok { "" } else { "⚠ " },
                    cell(&s.verb),
                    cell(&s.target),
                    cell(&s.note)
                ));
            }
        }

        md.push_str("\n## Sources\n\n");
        if self.sources.is_empty() {
            md.push_str("No pages were read.\n");
        } else {
            md.push_str("| # | Source | Reader | robots.txt | Cache | Cited |\n| --- | --- | --- | --- | --- | --- |\n");
            for s in &self.sources {
                let name = if s.title.trim().is_empty() {
                    &s.site
                } else {
                    &s.title
                };
                let num = if s.n == 0 {
                    "–".to_owned()
                } else {
                    s.n.to_string()
                };
                let cache = match &s.error {
                    Some(e) => format!("not read: {e}"),
                    None => s.cache.clone(),
                };
                md.push_str(&format!(
                    "| {num} | [{}]({}) ({}) | {} | {} | {} | {} |\n",
                    cell(name),
                    s.url,
                    cell(&s.site),
                    cell(&s.reader),
                    cell(&s.robots),
                    cell(&cache),
                    if s.cited { "yes" } else { "" }
                ));
            }
        }

        md.push_str("\n## Claims\n\n");
        let tally = self.tally();
        if self.claims.is_empty() {
            md.push_str("The answer makes no cited claims.\n");
        } else {
            md.push_str(&format!("{}.\n\n", tally.line()));
            for c in &self.claims {
                md.push_str(&format!(
                    "{}. **{} {}** {} {}\n",
                    c.claim,
                    c.verdict.glyph(),
                    c.verdict.label(),
                    one_line(&c.text),
                    c.citations
                        .iter()
                        .map(|n| format!("[{n}]"))
                        .collect::<String>()
                ));
                if let Some(ev) = &c.evidence {
                    let page = ev.page.map(|p| format!(", page {p}")).unwrap_or_default();
                    md.push_str(&format!(
                        "   > {}\n   > — [{}{}]({})\n",
                        one_line(&ev.quote),
                        ev.site,
                        page,
                        ev.anchor
                    ));
                }
                if !c.note.is_empty() {
                    md.push_str(&format!("   {}\n", c.note));
                }
                md.push('\n');
            }
            let judged = self
                .claims
                .iter()
                .filter(|c| c.method == crate::verdict::Method::Model)
                .count();
            md.push_str(&format!(
                "Verdicts: {} by the model reading the passages, {} by word overlap.\n",
                judged,
                self.claims.len() - judged
            ));
        }

        md.push_str("\n## Budget and usage\n\n");
        let u = &self.usage;
        let of = |used: u64, max: Option<u64>| match max {
            Some(m) if m > 0 => format!("{used} of {m}"),
            _ => used.to_string(),
        };
        let b = u.budget;
        md.push_str(&format!(
            "| Model calls | Tool calls | Searches | Pages | Tokens in | Tokens out | Time |\n| --- | --- | --- | --- | --- | --- | --- |\n| {} | {} | {} | {} | {} | {} | {:.1}s |\n",
            of(u.model_calls, b.map(|b| b.max_model_calls)),
            of(u.tool_calls, b.map(|b| b.max_tool_calls)),
            u.searches,
            u.pages,
            u.tokens_in,
            of(u.tokens_out, None),
            u.took_ms as f64 / 1000.0
        ));
        if let Some(b) = b
            && b.max_tokens > 0
        {
            md.push_str(&format!(
                "\nTokens: {} of {} budgeted.\n",
                u.tokens_in + u.tokens_out,
                b.max_tokens
            ));
        }

        if u.judge_tokens_in + u.judge_tokens_out > 0 {
            md.push_str(&format!(
                "\nChecking the claims used {} tokens in and {} out.\n",
                u.judge_tokens_in, u.judge_tokens_out
            ));
        }

        md.push_str("\n## Errors\n\n");
        if self.errors.is_empty() {
            md.push_str("None.\n");
        } else {
            for e in &self.errors {
                md.push_str(&format!("- {}\n", one_line(e)));
            }
        }

        md.push_str("\n## Answer\n\n");
        md.push_str(self.answer.trim());
        md.push('\n');
        md
    }

    /// Claims with a given verdict.
    pub fn with_verdict(&self, v: Verdict) -> impl Iterator<Item = &ClaimCheck> {
        self.claims.iter().filter(move |c| c.verdict == v)
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cell(s: &str) -> String {
    one_line(s).replace('|', "\\|")
}

fn or_dash(s: &str) -> &str {
    if s.trim().is_empty() { "–" } else { s }
}

fn quote_block(s: &str) -> String {
    let mut out = String::new();
    for line in s.trim().lines() {
        out.push_str("> ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::{Evidence, Method};

    fn sample() -> RunReport {
        RunReport {
            version: REPORT_VERSION,
            session: "01J0".into(),
            turn: 2,
            started: "2026-10-22T13:00:00Z".into(),
            question: "When was the Eiffel Tower finished?".into(),
            answer: "In 1889 [1].".into(),
            outcome: "Answered".into(),
            provider: "groq".into(),
            model: "llama".into(),
            plan: vec![],
            steps: vec![ReportStep {
                at_ms: 1200,
                verb: "Read".into(),
                target: "https://e.test/a|b".into(),
                note: String::new(),
                ok: true,
            }],
            sources: vec![
                ReportSource {
                    n: 1,
                    title: "Eiffel".into(),
                    url: "https://e.test/a".into(),
                    site: "e.test".into(),
                    reader: "Tern Crawl".into(),
                    robots: "allowed".into(),
                    cache: "miss".into(),
                    cited: true,
                    error: None,
                },
                ReportSource {
                    n: 0,
                    title: String::new(),
                    url: "https://x.test".into(),
                    site: "x.test".into(),
                    reader: "Tern Crawl".into(),
                    robots: "blocked".into(),
                    cache: "unknown".into(),
                    cited: false,
                    error: Some("robots.txt asks not to".into()),
                },
            ],
            claims: vec![ClaimCheck {
                claim: 1,
                text: "In 1889.".into(),
                citations: vec![1],
                verdict: Verdict::Supported,
                evidence: Some(Evidence {
                    source: 1,
                    url: "https://e.test/a".into(),
                    title: "Eiffel".into(),
                    site: "e.test".into(),
                    quote: "completed in 1889".into(),
                    anchor: "https://e.test/a#:~:text=completed%20in%201889".into(),
                    page: None,
                }),
                overlap: 100,
                method: Method::Lexical,
                note: String::new(),
            }],
            usage: ReportUsage {
                model_calls: 2,
                tool_calls: 1,
                searches: 0,
                pages: 1,
                tokens_in: 900,
                tokens_out: 100,
                took_ms: 4200,
                judge_tokens_in: 0,
                judge_tokens_out: 0,
                budget: Some(ReportBudget {
                    max_model_calls: 8,
                    max_tool_calls: 12,
                    max_tokens: 200_000,
                }),
            },
            errors: vec!["`read_url` failed: robots".into()],
        }
    }

    #[test]
    fn markdown_has_every_section() {
        let md = sample().to_markdown();
        for part in [
            "# Run report: When was the Eiffel Tower finished?",
            "## Question",
            "Quick turn: no written plan.",
            "| 1.2s | Read | https://e.test/a\\|b |  |",
            "| 1 | [Eiffel](https://e.test/a) (e.test) | Tern Crawl | allowed | miss | yes |",
            "not read: robots.txt asks not to",
            "1 supported.",
            "1. **✓ Supported** In 1889. [1]",
            "   > completed in 1889",
            "| 2 of 8 | 1 of 12 | 0 | 1 | 900 | 100 | 4.2s |",
            "Tokens: 1000 of 200000 budgeted.",
            "- `read_url` failed: robots",
            "## Answer\n\nIn 1889 [1].",
        ] {
            assert!(md.contains(part), "missing {part:?} in\n{md}");
        }
    }

    #[test]
    fn json_round_trips() {
        let r = sample();
        assert_eq!(RunReport::from_json(&r.to_json()).unwrap(), r);
        assert!(RunReport::from_json("{}").is_err());
    }
}
