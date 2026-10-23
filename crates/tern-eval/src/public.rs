//! Public benchmarks read from the user's own copy. Nothing from them is
//! bundled with Tern.
//!
//! SciFact (Wadden et al., 2020, "Fact or Fiction: Verifying Scientific
//! Claims", licence CC BY-NC 2.0): `claims_dev.jsonl` (or `claims_train`)
//! and `corpus.jsonl` from its data release. Each claim becomes a claim
//! case whose sources are the abstracts it cites. SciFact's `SUPPORT` is
//! Tern's Supported; `CONTRADICT` and no evidence (not enough info) are
//! Not supported, since Tern has no separate "contradicted" verdict.

use std::collections::BTreeMap;

use serde::Deserialize;
use tern_research::verdict::Verdict;

use crate::suite::{CaseSource, Cases, ClaimCase, Level, Suite, SuiteKind};

#[derive(Deserialize)]
struct SciClaim {
    id: u64,
    claim: String,
    #[serde(default)]
    evidence: BTreeMap<String, Vec<SciEvidence>>,
    #[serde(default)]
    cited_doc_ids: Vec<u64>,
}

#[derive(Deserialize)]
struct SciEvidence {
    label: String,
}

#[derive(Deserialize)]
struct SciDoc {
    doc_id: u64,
    #[serde(default)]
    title: String,
    #[serde(rename = "abstract", default)]
    abstract_: Vec<String>,
}

/// Reads SciFact's claims and corpus (JSON lines) into a claim suite.
/// `limit` keeps the first N labelled claims.
pub fn scifact(
    claims_jsonl: &str,
    corpus_jsonl: &str,
    name: &str,
    limit: Option<usize>,
) -> Result<Suite, String> {
    let mut corpus = BTreeMap::new();
    for (i, line) in corpus_jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let d: SciDoc =
            serde_json::from_str(line).map_err(|e| format!("corpus line {}: {e}", i + 1))?;
        corpus.insert(d.doc_id, d);
    }
    let mut cases = Vec::new();
    for (i, line) in claims_jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let c: SciClaim =
            serde_json::from_str(line).map_err(|e| format!("claims line {}: {e}", i + 1))?;
        if c.cited_doc_ids.is_empty() {
            continue;
        }
        let labels: Vec<&str> = c
            .evidence
            .values()
            .flatten()
            .map(|e| e.label.as_str())
            .collect();
        let gold = if labels.contains(&"SUPPORT") && !labels.contains(&"CONTRADICT") {
            Verdict::Supported
        } else {
            Verdict::NotSupported
        };
        let sources: Vec<CaseSource> = c
            .cited_doc_ids
            .iter()
            .filter_map(|id| corpus.get(id))
            .map(|d| CaseSource {
                url: format!("https://scifact.example/doc/{}", d.doc_id),
                title: d.title.clone(),
                text: d.abstract_.join(" "),
            })
            .collect();
        if sources.is_empty() {
            continue;
        }
        let tag = if labels.is_empty() {
            "not enough info"
        } else if labels.contains(&"CONTRADICT") {
            "contradicted"
        } else {
            "supported"
        };
        cases.push(ClaimCase {
            id: format!("scifact-{}", c.id),
            claim: c.claim,
            sources,
            gold,
            tags: vec![tag.into()],
        });
        if limit.is_some_and(|n| cases.len() >= n) {
            break;
        }
    }
    if cases.is_empty() {
        return Err("no SciFact claims with cited abstracts were found".into());
    }
    Ok(Suite {
        id: format!("scifact-{name}"),
        version: 1,
        title: format!("SciFact claim verification ({name})"),
        kind: SuiteKind::Claims,
        level: Level::Public,
        origin: "SciFact (Wadden et al., 2020; CC BY-NC 2.0), read from your copy; not bundled with Tern".into(),
        description: "Scientific claims against the abstracts they cite.".into(),
        split: Some(name.to_owned()),
        cases: Cases::Claims(cases),
        from: "scifact".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Written for this test in SciFact's format; not SciFact data.
    const CLAIMS: &str = r#"{"id": 1, "claim": "Vitamin K2 lowers arterial calcification in rats.", "evidence": {"10": [{"sentences": [0], "label": "SUPPORT"}]}, "cited_doc_ids": [10]}
{"id": 2, "claim": "Vitamin K2 raises arterial calcification in rats.", "evidence": {"10": [{"sentences": [0], "label": "CONTRADICT"}]}, "cited_doc_ids": [10]}
{"id": 3, "claim": "Sleep loss improves memory.", "evidence": {}, "cited_doc_ids": [11]}
{"id": 4, "claim": "A claim citing nothing.", "evidence": {}, "cited_doc_ids": []}
"#;
    const CORPUS: &str = r#"{"doc_id": 10, "title": "K2 and calcification", "abstract": ["In rats, vitamin K2 lowered arterial calcification.", "The effect was dose dependent."], "structured": false}
{"doc_id": 11, "title": "Sleep", "abstract": ["We studied sleep in mice."], "structured": false}
"#;

    #[test]
    fn reads_scifact_shaped_files() {
        let s = scifact(CLAIMS, CORPUS, "dev", None).unwrap();
        let Cases::Claims(cs) = &s.cases else {
            panic!()
        };
        assert_eq!(cs.len(), 3);
        assert_eq!(cs[0].gold, Verdict::Supported);
        assert_eq!(cs[1].gold, Verdict::NotSupported);
        assert_eq!(cs[2].tags, vec!["not enough info".to_owned()]);
        assert_eq!(s.level, Level::Public);
        assert_eq!(
            scifact(CLAIMS, CORPUS, "dev", Some(1)).unwrap().cases.len(),
            1
        );
        assert!(scifact("", CORPUS, "dev", None).is_err());
    }
}
