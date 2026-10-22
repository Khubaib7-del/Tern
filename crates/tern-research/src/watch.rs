//! What changed between two runs of the same question: the watcher's diff.
//!
//! Sources are matched by URL (no fragment, scheme, `www.`, or trailing
//! slash); claims by their content words, so a reworded sentence that says
//! the same thing is the same claim. A changed verdict is a matched claim
//! whose verdict moved; new and dropped claims are listed apart.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::report::RunReport;
use crate::verdict::{Verdict, content_words};

/// A source one run read and the other did not.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedSource {
    pub url: String,
    pub title: String,
    pub site: String,
}

/// A claim whose verdict moved, or that is new or gone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictChange {
    /// The claim as the newer run put it (or the older, when it is gone).
    pub claim: String,
    pub before: Option<Verdict>,
    pub after: Option<Verdict>,
}

/// The difference between a watch's last report and the new one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhatChanged {
    pub new_sources: Vec<ChangedSource>,
    pub gone_sources: Vec<ChangedSource>,
    /// Matched claims whose verdict changed.
    pub changed: Vec<VerdictChange>,
    /// Claims only the newer run makes.
    pub new_claims: Vec<VerdictChange>,
    /// Claims only the older run made.
    pub dropped_claims: Vec<VerdictChange>,
    /// When each run started, RFC 3339.
    pub before_started: String,
    pub after_started: String,
}

impl WhatChanged {
    /// Nothing worth telling the user.
    pub fn is_empty(&self) -> bool {
        self.new_sources.is_empty()
            && self.gone_sources.is_empty()
            && self.changed.is_empty()
            && self.new_claims.is_empty()
            && self.dropped_claims.is_empty()
    }

    /// "2 new sources · 1 verdict changed · 1 new claim", or "No changes".
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        let mut push = |n: usize, one: &str, many: &str| {
            if n > 0 {
                parts.push(format!("{n} {}", if n == 1 { one } else { many }));
            }
        };
        push(self.new_sources.len(), "new source", "new sources");
        push(self.changed.len(), "verdict changed", "verdicts changed");
        push(self.new_claims.len(), "new claim", "new claims");
        push(self.gone_sources.len(), "source gone", "sources gone");
        push(self.dropped_claims.len(), "claim dropped", "claims dropped");
        if parts.is_empty() {
            "No changes".to_owned()
        } else {
            parts.join(" · ")
        }
    }

    /// The changes as Markdown.
    pub fn to_markdown(&self) -> String {
        let mut md = format!("## What changed\n\n{}\n", self.line());
        if !self.before_started.is_empty() {
            md.push_str(&format!(
                "\nSince the run of {} (this run: {}).\n",
                self.before_started, self.after_started
            ));
        }
        if !self.new_sources.is_empty() {
            md.push_str("\n### New sources\n\n");
            for s in &self.new_sources {
                md.push_str(&format!(
                    "- [{}]({}) · {}\n",
                    title_or_url(s),
                    s.url,
                    s.site
                ));
            }
        }
        if !self.changed.is_empty() {
            md.push_str("\n### Changed verdicts\n\n");
            for c in &self.changed {
                md.push_str(&format!(
                    "- {} → {}: {}\n",
                    label(c.before),
                    label(c.after),
                    c.claim
                ));
            }
        }
        if !self.new_claims.is_empty() {
            md.push_str("\n### New claims\n\n");
            for c in &self.new_claims {
                md.push_str(&format!("- {}: {}\n", label(c.after), c.claim));
            }
        }
        if !self.gone_sources.is_empty() {
            md.push_str("\n### Sources no longer read\n\n");
            for s in &self.gone_sources {
                md.push_str(&format!("- {} · {}\n", s.url, s.site));
            }
        }
        if !self.dropped_claims.is_empty() {
            md.push_str("\n### Claims no longer made\n\n");
            for c in &self.dropped_claims {
                md.push_str(&format!("- {}: {}\n", label(c.before), c.claim));
            }
        }
        md
    }
}

fn label(v: Option<Verdict>) -> &'static str {
    v.map(Verdict::label).unwrap_or("–")
}

fn title_or_url(s: &ChangedSource) -> &str {
    if s.title.is_empty() { &s.url } else { &s.title }
}

/// A URL as the diff compares it.
pub fn same_url_key(url: &str) -> String {
    let u = url.split('#').next().unwrap_or(url).trim().to_lowercase();
    let u = u.split("://").nth(1).unwrap_or(&u).to_owned();
    let u = u.strip_prefix("www.").unwrap_or(&u);
    u.trim_end_matches('/').to_owned()
}

/// Share of shared content words, 0.0 to 1.0 (Jaccard).
fn similarity(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let shared = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 { 0.0 } else { shared / union }
}

/// Claims at least this similar are the same claim.
const SAME_CLAIM: f32 = 0.6;

/// What changed from `before` to `after`.
pub fn diff(before: &RunReport, after: &RunReport) -> WhatChanged {
    let read = |r: &RunReport| -> Vec<ChangedSource> {
        let mut out: Vec<ChangedSource> = Vec::new();
        for s in r.sources.iter().filter(|s| s.error.is_none()) {
            if out
                .iter()
                .any(|o| same_url_key(&o.url) == same_url_key(&s.url))
            {
                continue;
            }
            out.push(ChangedSource {
                url: s.url.clone(),
                title: s.title.clone(),
                site: s.site.clone(),
            });
        }
        out
    };
    let (old, new) = (read(before), read(after));
    let has = |list: &[ChangedSource], url: &str| {
        let key = same_url_key(url);
        list.iter().any(|s| same_url_key(&s.url) == key)
    };
    let new_sources = new.iter().filter(|s| !has(&old, &s.url)).cloned().collect();
    let gone_sources = old.iter().filter(|s| !has(&new, &s.url)).cloned().collect();

    let old_words: Vec<BTreeSet<String>> = before
        .claims
        .iter()
        .map(|c| content_words(&c.text))
        .collect();
    let mut taken = vec![false; before.claims.len()];
    let mut changed = Vec::new();
    let mut new_claims = Vec::new();
    for c in &after.claims {
        let words = content_words(&c.text);
        let best = old_words
            .iter()
            .enumerate()
            .filter(|(i, _)| !taken[*i])
            .map(|(i, w)| (i, similarity(w, &words)))
            .filter(|(_, s)| *s >= SAME_CLAIM)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, _)) => {
                taken[i] = true;
                let was = before.claims[i].verdict;
                if was != c.verdict {
                    changed.push(VerdictChange {
                        claim: c.text.clone(),
                        before: Some(was),
                        after: Some(c.verdict),
                    });
                }
            }
            None => new_claims.push(VerdictChange {
                claim: c.text.clone(),
                before: None,
                after: Some(c.verdict),
            }),
        }
    }
    let dropped_claims = before
        .claims
        .iter()
        .zip(&taken)
        .filter(|(_, t)| !**t)
        .map(|(c, _)| VerdictChange {
            claim: c.text.clone(),
            before: Some(c.verdict),
            after: None,
        })
        .collect();
    WhatChanged {
        new_sources,
        gone_sources,
        changed,
        new_claims,
        dropped_claims,
        before_started: before.started.clone(),
        after_started: after.started.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::ReportSource;
    use crate::verdict::{ClaimCheck, Method};

    fn source(url: &str) -> ReportSource {
        ReportSource {
            n: 1,
            title: format!("{url} title"),
            url: url.into(),
            site: "x.test".into(),
            reader: "Tern Crawl".into(),
            robots: "allowed".into(),
            cache: "miss".into(),
            cited: true,
            error: None,
        }
    }

    fn claim(text: &str, verdict: Verdict) -> ClaimCheck {
        ClaimCheck {
            claim: 1,
            text: text.into(),
            citations: vec![1],
            verdict,
            evidence: None,
            overlap: 0,
            method: Method::default(),
            note: String::new(),
        }
    }

    #[test]
    fn new_sources_and_changed_verdicts() {
        let before = RunReport {
            started: "2026-10-01T09:00:00Z".into(),
            sources: vec![source("https://a.test/x"), source("https://gone.test/")],
            claims: vec![
                claim(
                    "The Rust compiler release ships every six weeks.",
                    Verdict::Supported,
                ),
                claim(
                    "Go modules became the default in version 1.16.",
                    Verdict::Partly,
                ),
                claim("Zig has no package manager at all.", Verdict::NotSupported),
            ],
            ..RunReport::default()
        };
        let after = RunReport {
            started: "2026-10-08T09:00:00Z".into(),
            sources: vec![
                source("http://www.a.test/x/#frag"),
                source("https://new.test/post"),
            ],
            claims: vec![
                claim(
                    "A Rust compiler release ships every six weeks.",
                    Verdict::Supported,
                ),
                claim(
                    "Go modules became the default in version 1.16.",
                    Verdict::Supported,
                ),
                claim("Odin added a new build system in 2026.", Verdict::NoSource),
            ],
            ..RunReport::default()
        };
        let d = diff(&before, &after);
        assert_eq!(d.new_sources.len(), 1);
        assert_eq!(d.new_sources[0].url, "https://new.test/post");
        assert_eq!(d.gone_sources.len(), 1);
        assert_eq!(
            d.changed,
            vec![VerdictChange {
                claim: "Go modules became the default in version 1.16.".into(),
                before: Some(Verdict::Partly),
                after: Some(Verdict::Supported),
            }]
        );
        assert_eq!(d.new_claims.len(), 1);
        assert_eq!(d.dropped_claims.len(), 1);
        assert_eq!(
            d.line(),
            "1 new source · 1 verdict changed · 1 new claim · 1 source gone · 1 claim dropped"
        );
        let md = d.to_markdown();
        assert!(md.contains("### New sources"));
        assert!(md.contains("Partly supported → Supported"));
    }

    #[test]
    fn the_same_run_has_no_changes() {
        let r = RunReport {
            sources: vec![source("https://a.test")],
            claims: vec![claim(
                "Water boils at 100 C at sea level.",
                Verdict::Supported,
            )],
            ..RunReport::default()
        };
        let d = diff(&r, &r);
        assert!(d.is_empty());
        assert_eq!(d.line(), "No changes");
    }
}
