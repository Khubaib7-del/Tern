//! The decision model for front ends and the engine.
//!
//! Setup (keys for remote decision models live in the OS keychain under
//! `decision.<id>`, tested before they are saved), the [`Decider`] a chat
//! or a run uses, and the decision points Tern wires it into:
//!
//! | Decision point | Choices | Hard checks |
//! | --- | --- | --- |
//! | [`rank_hits`]: which search results to read first | the hits | pages already read are ruled out |
//! | [`next_tool_hint`]: which tool fits the next step | the chat's tools and "answer now" | tools the chat does not have are never offered |
//! | claim pre-score ([`crate::verify`]) | supported, partly, not supported | no source text forces "not supported"; a missing number or flipped negation rules out "supported"; the model judge has the final word |
//! | [`order_sub_questions`], [`order_watches`]: what to run now and what later | sub-questions, due watches | none; only the start order changes |
//!
//! Every decision is logged as a `decision.made` event (which model, the
//! choices, probabilities, latency, cost), which the run panel's Decisions
//! view and the run report read.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tern_core::{EffectClass, EventEnvelope, SecretString, SessionId, TurnId};
use tern_crawl::SearchHit;
pub use tern_decision::{
    Choice, DECISION_CATALOG, Decider, Decision, DecisionKind, DecisionModel, DecisionRequest,
    LOCAL_MODEL, LocalModel, Mode, Override, RemoteModel, RemoteSpec, Rule, bench as fixtures,
    find_remote,
};

use crate::providers::SecretStore;

/// The decision-model choices kept in `preferences.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DecisionPrefs {
    /// `off`, `local`, or `remote`. `None` means local.
    pub mode: Option<String>,
    /// Remote provider id (`typesafe`, `subq`, `voyage`).
    pub provider: Option<String>,
    /// Remote model id; `None` uses the provider's default.
    pub model: Option<String>,
    /// Milliseconds a remote call may take before the local scorer
    /// answers. `None` means 4000.
    pub timeout_ms: Option<u64>,
    /// Another address for the provider's API (a proxy or a test server).
    pub base_url: Option<String>,
    /// Jobs the user switched off (`tool`, `rank`, `support`); see [`Job`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub jobs_off: Vec<String>,
}

/// What the decision model decides, each with its own switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Job {
    /// Which tool a turn starts with (a hint in the system prompt).
    Tool,
    /// Which search results are read first.
    Rank,
    /// Whether a source's passage supports a claim, before the check.
    Support,
}

impl Job {
    pub const ALL: [Self; 3] = [Self::Tool, Self::Rank, Self::Support];

    pub fn id(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Rank => "rank",
            Self::Support => "support",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tool => "Tool choice",
            Self::Rank => "Page order",
            Self::Support => "Passage support",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Self::Tool => "Suggests the first tool for a message (search, read, vault, or none).",
            Self::Rank => "Orders search results so the pages most worth reading are read first.",
            Self::Support => {
                "Pre-scores whether a cited passage backs a claim before the claim check."
            }
        }
    }
}

impl DecisionPrefs {
    /// Whether `job` is switched on.
    pub fn job_on(&self, job: Job) -> bool {
        !self.jobs_off.iter().any(|j| j == job.id())
    }

    pub fn set_job(&mut self, job: Job, on: bool) {
        self.jobs_off.retain(|j| j != job.id());
        if !on {
            self.jobs_off.push(job.id().to_owned());
        }
    }
}

fn jobs_off_store() -> &'static std::sync::RwLock<Vec<String>> {
    static S: std::sync::OnceLock<std::sync::RwLock<Vec<String>>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// Sets the switched-off jobs process-wide (from the preferences).
pub fn set_jobs_off(off: &[String]) {
    *jobs_off_store().write().unwrap_or_else(|e| e.into_inner()) = off.to_vec();
}

/// Whether `job` runs (process-wide setting).
pub fn job_running(job: Job) -> bool {
    !jobs_off_store()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|j| j == job.id())
}

impl DecisionPrefs {
    pub fn mode(&self) -> Mode {
        self.mode
            .as_deref()
            .and_then(Mode::parse)
            .unwrap_or(Mode::Local)
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.unwrap_or(4000).clamp(200, 30_000))
    }

    /// The remote spec chosen, if any.
    pub fn spec(&self) -> Option<&'static RemoteSpec> {
        self.provider.as_deref().and_then(find_remote)
    }

    /// "Local", "Off", or "TypeSafe Jev (jev-latest)".
    pub fn label(&self) -> String {
        match (self.mode(), self.spec()) {
            (Mode::Off, _) => "Off".into(),
            (Mode::Remote, Some(s)) => format!(
                "{} {} ({})",
                s.name,
                s.product,
                self.model.as_deref().unwrap_or(s.default_model)
            ),
            (Mode::Remote, None) => "Remote (no provider chosen; local in use)".into(),
            (Mode::Local, _) => "Local".into(),
        }
    }
}

/// Keychain name for a decision provider's key.
pub fn key_name(spec: &RemoteSpec) -> String {
    format!("decision.{}", spec.id)
}

fn spec(id: &str) -> Result<&'static RemoteSpec, String> {
    find_remote(id).ok_or_else(|| {
        format!(
            "Tern does not know a decision model called `{id}`. Choose one of: {}.",
            DECISION_CATALOG
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

/// One catalog row with whether this machine has its key.
#[derive(Clone, Debug, Serialize)]
pub struct DecisionRow {
    pub id: &'static str,
    pub name: &'static str,
    pub product: &'static str,
    pub icon: &'static str,
    pub key_saved: bool,
    pub key_url: &'static str,
    pub docs_url: &'static str,
    pub default_model: &'static str,
    pub models: &'static [&'static str],
    /// "$0.042 per million input tokens, output free".
    pub price: String,
    /// What it can answer.
    pub scope: &'static str,
}

/// The remote catalog with key status.
pub fn list(secrets: Option<&dyn SecretStore>) -> Vec<DecisionRow> {
    DECISION_CATALOG
        .iter()
        .map(|s| DecisionRow {
            id: s.id,
            name: s.name,
            product: s.product,
            icon: s.icon,
            key_saved: secrets.is_some_and(|k| matches!(k.get(&key_name(s)), Ok(Some(_)))),
            key_url: s.key_url,
            docs_url: s.docs_url,
            default_model: s.default_model,
            models: s.models,
            price: format!(
                "${} per million input tokens, output free",
                s.usd_per_m_input
            ),
            scope: match s.wire {
                tern_decision::Wire::SystemOne => "Every decision point",
                tern_decision::Wire::Rerank => {
                    "Source ranking and now-or-later; the rest stays local"
                }
            },
        })
        .collect()
}

fn connect_remote(
    spec: &'static RemoteSpec,
    key: Option<SecretString>,
    prefs: &DecisionPrefs,
) -> Result<RemoteModel, String> {
    let model = prefs
        .model
        .as_deref()
        .filter(|_| prefs.provider.as_deref() == Some(spec.id));
    RemoteModel::new(spec, key, prefs.base_url.as_deref(), model, prefs.timeout())
        .map_err(|e| e.to_string())
}

/// The decider for these choices. Remote without a provider or a key
/// decides locally, each decision noting why.
pub fn decider_from(prefs: &DecisionPrefs, secrets: Option<&dyn SecretStore>) -> Decider {
    match prefs.mode() {
        Mode::Off => Decider::off(),
        Mode::Local => Decider::local(),
        Mode::Remote => {
            let Some(spec) = prefs.spec() else {
                return Decider::remote_unavailable(
                    "No remote decision model is chosen; Tern decided locally",
                );
            };
            let key = secrets.and_then(|s| s.get(&key_name(spec)).ok().flatten());
            match connect_remote(spec, key, prefs) {
                Ok(m) => Decider::remote(Box::new(m)).with_timeout(prefs.timeout()),
                Err(e) => Decider::remote_unavailable(e),
            }
        }
    }
}

/// The decider from the saved preferences and the OS keychain.
pub fn decider() -> Arc<Decider> {
    let prefs = crate::Preferences::load().decision;
    let secrets = crate::providers::keychain().ok();
    Arc::new(decider_from(&prefs, secrets.as_deref()))
}

/// A small real decision used to test a key.
fn probe() -> DecisionRequest {
    DecisionRequest::new(
        DecisionKind::General,
        "Which team should handle this message?",
        vec![
            Choice::new("returns", "Returns: exchanges, wrong or damaged items"),
            Choice::new("billing", "Billing: charges, invoices, payment problems"),
        ],
    )
    .with_context("My running shoes arrived in the wrong size. Can I swap them?")
}

/// Asks the provider one small decision with `key`.
pub async fn test_key(
    id: &str,
    key: SecretString,
    model: Option<&str>,
    base_url: Option<&str>,
) -> Result<Decision, String> {
    let spec = spec(id)?;
    let prefs = DecisionPrefs {
        provider: Some(spec.id.into()),
        model: model.map(str::to_owned),
        base_url: base_url.map(str::to_owned),
        ..DecisionPrefs::default()
    };
    let m = connect_remote(spec, Some(key), &prefs)?;
    m.score(&probe()).await.map_err(|e| e.to_string())
}

/// Tests `key` and saves it only if the provider answers.
pub async fn add(
    id: &str,
    key: SecretString,
    secrets: &dyn SecretStore,
    model: Option<&str>,
    base_url: Option<&str>,
) -> Result<Decision, String> {
    if key.expose().trim().is_empty() {
        return Err("The key was empty, so nothing was saved.".into());
    }
    let spec = spec(id)?;
    let d = test_key(id, key.clone(), model, base_url).await?;
    secrets
        .set(&key_name(spec), &key)
        .map_err(|e| format!("The keychain refused: {e}"))?;
    Ok(d)
}

/// Tests the saved key.
pub async fn test_saved(
    id: &str,
    secrets: &dyn SecretStore,
    model: Option<&str>,
    base_url: Option<&str>,
) -> Result<Decision, String> {
    let spec = spec(id)?;
    let key = secrets
        .get(&key_name(spec))
        .map_err(|e| format!("The keychain refused: {e}"))?
        .ok_or_else(|| format!("No key is saved for {}.", spec.name))?;
    test_key(id, key, model, base_url).await
}

/// Removes the saved key. Returns whether there was one.
pub fn remove(id: &str, secrets: &dyn SecretStore) -> Result<bool, String> {
    let spec = spec(id)?;
    secrets
        .delete(&key_name(spec))
        .map_err(|e| format!("The keychain refused: {e}"))
}

/// The `decision.made` event for `d`.
pub fn event(session: SessionId, turn: Option<TurnId>, d: &Decision) -> EventEnvelope {
    let e = EventEnvelope::new(
        session,
        "decision.made",
        EffectClass::Analysis,
        d.line(),
        serde_json::to_value(d).unwrap_or_else(|_| json!({})),
    );
    match turn {
        Some(t) => e.with_turn(t),
        None => e,
    }
}

/// Orders `hits` by the decision model, best first, leaving out none.
/// Pages in `seen` are ruled out (they sink to the end). With decisions
/// off, the hits keep the search service's order and no decision is made.
pub async fn rank_hits(
    decider: &Decider,
    question: &str,
    hits: &[SearchHit],
    seen: &(dyn Fn(&str) -> bool + Sync),
) -> (Vec<SearchHit>, Option<Decision>) {
    if hits.len() < 2 || !decider.is_on() || !job_running(Job::Rank) {
        return (hits.to_vec(), None);
    }
    let n = hits.len().min(60);
    let choices: Vec<Choice> = hits
        .iter()
        .take(n)
        .enumerate()
        .map(|(i, h)| {
            Choice::new(
                h.url.clone(),
                format!("{} ({}) {}", h.title, h.site, h.snippet),
            )
            .with_prior(1.0 - i as f32 / n as f32)
        })
        .collect();
    let rules: Vec<Rule> = hits
        .iter()
        .take(n)
        .enumerate()
        .filter(|(_, h)| seen(&h.url))
        .map(|(i, _)| Rule::RuleOut {
            choice: i,
            reason: "already read".into(),
        })
        .collect();
    let req = DecisionRequest::new(
        DecisionKind::SourceRank,
        format!("Which search result should be read first to answer: {question}"),
        choices,
    );
    let Some(d) = decider.decide(&req, &rules).await else {
        return (hits.to_vec(), None);
    };
    let mut out: Vec<SearchHit> = d.ranking().into_iter().map(|i| hits[i].clone()).collect();
    out.extend(hits.iter().skip(n).cloned());
    (out, Some(d))
}

/// What each tool is for, as the decision model reads it.
fn tool_text(name: &str) -> &'static str {
    match name {
        "web_search" => "Search the web for pages about it",
        "read_url" => "Read one web page by its link",
        "vault_search" => "Search the user's own notes in the vault",
        "vault_read" => "Read one of the user's notes",
        _ => "Use this tool",
    }
}

/// A hint for the model's first step: which of `tools` (or none) fits
/// `message`. Returns the line added to the system prompt and the
/// decision. A hint below 50% is not sent; the decision is still logged.
pub async fn next_tool_hint(
    decider: &Decider,
    message: &str,
    tools: &[&str],
    attachments: usize,
) -> Option<(Option<String>, Decision)> {
    if !job_running(Job::Tool) {
        return None;
    }
    let mut choices: Vec<Choice> = tools
        .iter()
        .filter(|t| matches!(**t, "web_search" | "read_url" | "vault_search"))
        .map(|t| Choice::new(*t, tool_text(t)))
        .collect();
    if choices.is_empty() {
        return None;
    }
    choices.push(Choice::new("answer", "Answer now without a tool"));
    let req = DecisionRequest::new(
        DecisionKind::NextTool,
        format!("Which step should come first for this message: {message}"),
        choices,
    )
    .with_context(format!("attachments: {attachments}"));
    let d = decider.decide(&req, &[]).await?;
    let p = d.probabilities[d.chosen];
    let hint = (p >= 0.5).then(|| {
        let what = match d.chosen_id() {
            "answer" => "answering from what you already have, without a tool".to_owned(),
            t => format!("the `{t}` tool"),
        };
        format!(
            "Decision model hint ({:.0}%): this message most likely calls for {what} first. It is a hint, not a rule; use your judgment.",
            p * 100.0
        )
    });
    Some((hint, d))
}

/// The order to start sub-questions in, most pressing first: indices into
/// `subs`. `None` when decisions are off or there is nothing to order.
pub async fn order_sub_questions(
    decider: &Decider,
    question: &str,
    subs: &[String],
) -> Option<(Vec<usize>, Decision)> {
    if subs.len() < 2 {
        return None;
    }
    let n = subs.len();
    let choices = subs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            Choice::new((i + 1).to_string(), s.clone()).with_prior(1.0 - i as f32 / n as f32)
        })
        .collect();
    let req = DecisionRequest::new(
        DecisionKind::Triage,
        format!("Which part should be researched first to answer: {question}"),
        choices,
    );
    let d = decider.decide(&req, &[]).await?;
    Some((d.ranking(), d))
}

/// The order to run due watches in: indices into `watches`, given as
/// (question, hours overdue).
pub async fn order_watches(
    decider: &Decider,
    watches: &[(String, f32)],
) -> Option<(Vec<usize>, Decision)> {
    if watches.len() < 2 {
        return None;
    }
    let most = watches.iter().map(|w| w.1).fold(1.0, f32::max);
    let choices = watches
        .iter()
        .enumerate()
        .map(|(i, (q, h))| {
            Choice::new(
                (i + 1).to_string(),
                format!("Watch: {q}, overdue by {h:.0} hours"),
            )
            .with_prior((h / most).clamp(0.0, 1.0))
        })
        .collect();
    let req = DecisionRequest::new(
        DecisionKind::Triage,
        "Which watched question should run first?",
        choices,
    );
    let d = decider.decide(&req, &[]).await?;
    Some((d.ranking(), d))
}

/// Asks one decision, as `tern decide` does.
pub async fn ask(
    decider: &Decider,
    kind: DecisionKind,
    question: &str,
    choices: &[String],
    context: &str,
) -> Result<Decision, String> {
    let req = DecisionRequest::new(
        kind,
        question,
        choices.iter().map(|c| Choice::plain(c.trim())).collect(),
    )
    .with_context(context);
    req.validate().map_err(|e| e.to_string())?;
    decider.decide(&req, &[]).await.ok_or_else(|| {
        "The decision model is off. Turn it on with `tern decide mode local` (or `remote`).".into()
    })
}

pub mod bench;

#[cfg(test)]
mod tests {
    use super::*;
    use tern_platform::MemorySecretStore;

    fn hit(url: &str, title: &str) -> SearchHit {
        SearchHit {
            title: title.into(),
            url: url.into(),
            snippet: String::new(),
            site: url.split('/').nth(2).unwrap_or_default().into(),
            icon: tern_crawl::SourceIcon {
                builtin: None,
                favicon: None,
            },
        }
    }

    #[test]
    fn prefs_default_to_local_and_read_their_mode() {
        let p = DecisionPrefs::default();
        assert_eq!(p.mode(), Mode::Local);
        assert_eq!(p.label(), "Local");
        let r = DecisionPrefs {
            mode: Some("remote".into()),
            provider: Some("typesafe".into()),
            ..DecisionPrefs::default()
        };
        assert_eq!(r.label(), "TypeSafe Jev (jev-latest)");
        assert_eq!(r.timeout(), Duration::from_millis(4000));
    }

    #[tokio::test]
    async fn remote_without_a_key_decides_locally_and_says_why() {
        let secrets = MemorySecretStore::default();
        let prefs = DecisionPrefs {
            mode: Some("remote".into()),
            provider: Some("subq".into()),
            ..DecisionPrefs::default()
        };
        let d = decider_from(&prefs, Some(&secrets));
        let out = ask(
            &d,
            DecisionKind::General,
            "Which is a fruit?",
            &["apple fruit".into(), "car".into()],
            "",
        )
        .await
        .unwrap();
        assert_eq!(out.provider, "local");
        assert!(out.fallback.unwrap().contains("needs a key"));
        assert!(!list(Some(&secrets))[1].key_saved);
        let off = decider_from(
            &DecisionPrefs {
                mode: Some("off".into()),
                ..DecisionPrefs::default()
            },
            None,
        );
        assert!(
            ask(
                &off,
                DecisionKind::General,
                "q",
                &["a".into(), "b".into()],
                ""
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn ranks_hits_and_sinks_pages_already_read() {
        let d = Decider::local();
        let hits = vec![
            hit("https://www.pinterest.com/x", "Transformer toys"),
            hit(
                "https://arxiv.org/abs/1706.03762",
                "Attention Is All You Need transformer",
            ),
            hit(
                "https://en.wikipedia.org/wiki/Transformer_(deep_learning)",
                "Transformer (deep learning) attention",
            ),
        ];
        let (ranked, dec) = rank_hits(
            &d,
            "What is the transformer attention architecture?",
            &hits,
            &|u| u.contains("arxiv"),
        )
        .await;
        assert_eq!(ranked.len(), 3);
        assert!(ranked[0].url.contains("wikipedia"));
        assert!(ranked[2].url.contains("arxiv"), "already read goes last");
        assert_eq!(dec.unwrap().overrides.len(), 1);
        let (same, none) = rank_hits(&Decider::off(), "q", &hits, &|_| false).await;
        assert_eq!(same, hits);
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn hints_and_orders() {
        let d = Decider::local();
        let (hint, dec) = next_tool_hint(
            &d,
            "What is the latest Rust release?",
            &["web_search", "read_url"],
            0,
        )
        .await
        .unwrap();
        assert_eq!(dec.chosen_id(), "web_search");
        assert!(hint.unwrap().contains("web_search"));
        assert!(next_tool_hint(&d, "hi", &["vault_read"], 0).await.is_none());
        let (order, _) = order_sub_questions(
            &d,
            "How do solid-state batteries compare on energy density?",
            &[
                "History of batteries (optional, later)".into(),
                "Energy density of solid-state batteries".into(),
            ],
        )
        .await
        .unwrap();
        assert_eq!(order, [1, 0]);
        let (order, _) = order_watches(
            &d,
            &[
                ("monthly news".into(), 1.0),
                ("weekly digest".into(), 200.0),
            ],
        )
        .await
        .unwrap();
        assert_eq!(order[0], 1);
    }

    #[test]
    fn events_carry_the_decision() {
        let d = Decision {
            kind: DecisionKind::SourceRank,
            choices: vec!["a".into(), "b".into()],
            probabilities: vec![0.7, 0.3],
            model: LOCAL_MODEL.into(),
            provider: "local".into(),
            ..Decision::default()
        };
        let e = event(SessionId::new(), None, &d);
        assert_eq!(e.kind, "decision.made");
        assert_eq!(e.payload["model"], LOCAL_MODEL);
        let back: Decision = serde_json::from_value(e.payload).unwrap();
        assert_eq!(back, d);
    }
}
