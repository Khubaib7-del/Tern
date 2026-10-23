//! Evaluation runs on Tern's real pieces (`docs/evaluation.md`).
//!
//! [`TernSubject`] hands `tern-eval` the trust scan, the untrusted-data
//! wrapper, the policy gate with default settings, the reader, the
//! decision models (the fixed-order baseline and the local scorer), and,
//! for live runs, the chat model. [`EvalStore`] keeps runs in
//! `<data>/evals/runs/` as JSON and Markdown and reads extra suites from
//! `<data>/evals/suites/`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use tern_core::EffectClass;
use tern_decision::{DecisionModel, LocalModel};
use tern_providers::{ChatMessage, ChatRequest};

pub use tern_eval::report::{Better, Change, Comparison, Delta, Failure, Metric, Unit};
pub use tern_eval::suite::{Level, Suite, SuiteKind};
pub use tern_eval::usability::{SUS, TASKS, TaskResult, sus_score};
use tern_eval::{BoxFuture, Completion, GateAnswer, Subject};
pub use tern_eval::{EvalRun, MIN_TRIALS, RunSummary, SuiteResult, check_baseline, compare};

use crate::engine::Model;
use crate::policy::{CallView, Decision, PolicyGate};
use crate::providers::{self, SecretStore};

/// The model a live run asks.
#[derive(Clone)]
pub struct LiveModel {
    pub model: Arc<dyn Model>,
    pub model_name: String,
    pub provider_id: String,
    pub provider_name: String,
}

impl std::fmt::Debug for LiveModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LiveModel({} · {})", self.provider_name, self.model_name)
    }
}

impl LiveModel {
    /// Connects with the chat's provider and model choices.
    pub async fn open(opts: &crate::ChatOptions) -> Result<Self, String> {
        let keychain = providers::keychain();
        let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
        let (provider, model, _) = crate::chat::connect_model(opts, secrets).await?;
        Ok(Self {
            provider_id: provider.spec().id.to_owned(),
            provider_name: provider.spec().name.to_owned(),
            model: Arc::new(provider),
            model_name: model,
        })
    }
}

/// Tern as the system under test.
#[derive(Debug, Default)]
pub struct TernSubject {
    live: Option<LiveModel>,
    /// US dollars per million input and output tokens, when the user gave them.
    price: Option<(f64, f64)>,
}

impl TernSubject {
    /// No model: recorded answers and Tern's offline checks.
    pub fn offline() -> Self {
        Self::default()
    }

    pub fn live(model: LiveModel) -> Self {
        Self {
            live: Some(model),
            price: None,
        }
    }

    pub fn with_price(mut self, price: Option<(f64, f64)>) -> Self {
        self.price = price;
        self
    }
}

/// An effect class from its name (`network`, `external_side_effect`).
fn effect(name: &str) -> Result<EffectClass, String> {
    serde_json::from_value(json!(
        name.trim().to_ascii_lowercase().replace([' ', '-'], "_")
    ))
    .map_err(|_| format!("no effect class `{name}`"))
}

impl Subject for TernSubject {
    fn label(&self) -> String {
        match &self.live {
            Some(m) => format!("{} · {}", m.provider_name, m.model_name),
            None => "offline".into(),
        }
    }

    fn provider(&self) -> Option<String> {
        self.live.as_ref().map(|m| m.provider_id.clone())
    }

    fn live(&self) -> bool {
        self.live.is_some()
    }

    fn complete<'a>(
        &'a self,
        system: &'a str,
        user: &'a str,
        max_tokens: u32,
    ) -> BoxFuture<'a, Result<Completion, String>> {
        Box::pin(async move {
            let Some(m) = &self.live else {
                return Err("offline: no model".into());
            };
            let mut req = ChatRequest::new(&m.model_name, vec![ChatMessage::user(user)]);
            req.system = Some(system.to_owned());
            req.max_tokens = max_tokens;
            req.temperature = Some(0.0);
            let started = Instant::now();
            let resp = m.model.complete(&req).await.map_err(|e| e.to_string())?;
            Ok(Completion {
                text: resp.text,
                input_tokens: resp.usage.input_tokens,
                output_tokens: resp.usage.output_tokens,
                latency_ms: started.elapsed().as_millis() as u64,
            })
        })
    }

    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            crate::web::shared_reader()
                .fetch(url)
                .await
                .map(|p| p.text)
                .map_err(|e| e.to_string())
        })
    }

    fn scan(&self, text: &str) -> Vec<String> {
        crate::mcp::trust::scan(text)
            .into_iter()
            .filter(|f| f.severity == crate::mcp::trust::Severity::Danger)
            .map(|f| f.reason)
            .collect()
    }

    fn wrap(&self, source: &str, text: &str) -> String {
        crate::untrusted::wrap(source, text)
    }

    fn untrusted_rule(&self) -> String {
        crate::untrusted::RULE.to_owned()
    }

    fn gate(&self, tool: &str, effect_name: &str, target: &str) -> Result<GateAnswer, String> {
        let args = json!({});
        let call = CallView {
            tool,
            effect: effect(effect_name)?,
            target: (!target.is_empty()).then_some(target),
            args: &args,
        };
        Ok(match PolicyGate::default().check(&call) {
            Decision::Allow { .. } => GateAnswer::Allow,
            Decision::Ask => GateAnswer::Ask,
            Decision::Deny { .. } => GateAnswer::Deny,
        })
    }

    fn deciders(&self) -> Vec<Box<dyn DecisionModel>> {
        vec![
            Box::new(crate::decide::bench::FixedOrder),
            Box::new(LocalModel),
        ]
    }

    fn price(&self) -> Option<(f64, f64)> {
        self.price
    }
}

/// `<data>/evals`: runs and extra suites.
#[derive(Clone, Debug)]
pub struct EvalStore {
    root: PathBuf,
}

impl EvalStore {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn discover() -> Result<Self, String> {
        Ok(Self::at(crate::mcp::config::data_dir()?.join("evals")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    pub fn suites_dir(&self) -> PathBuf {
        self.root.join("suites")
    }

    /// Saves `run` as `<id>.json` and `<id>.md`; returns the JSON path.
    pub fn save(&self, run: &EvalRun) -> Result<PathBuf, String> {
        let dir = self.runs_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let json = dir.join(format!("{}.json", run.id));
        std::fs::write(&json, run.to_json()).map_err(|e| format!("{}: {e}", json.display()))?;
        let md = dir.join(format!("{}.md", run.id));
        std::fs::write(&md, run.to_markdown()).map_err(|e| format!("{}: {e}", md.display()))?;
        Ok(json)
    }

    /// Every saved run, newest first. Files that do not read are skipped.
    pub fn runs(&self) -> Vec<EvalRun> {
        let Ok(rd) = std::fs::read_dir(self.runs_dir()) else {
            return Vec::new();
        };
        let mut out: Vec<EvalRun> = rd
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|t| EvalRun::from_json(&t).ok())
            .collect();
        out.sort_by(|a, b| (&b.started, &b.id).cmp(&(&a.started, &a.id)));
        out
    }

    pub fn summaries(&self) -> Vec<RunSummary> {
        self.runs().iter().map(RunSummary::from).collect()
    }

    /// A run by id, by the start of its id, `last`, or `prev`.
    pub fn find(&self, which: &str) -> Result<EvalRun, String> {
        let runs = self.runs();
        let w = which.trim();
        let found = match w {
            "" | "last" | "latest" => runs.first(),
            "prev" | "previous" => runs.get(1),
            _ => {
                let hits: Vec<&EvalRun> = runs.iter().filter(|r| r.id.starts_with(w)).collect();
                if hits.len() > 1 && !hits.iter().any(|r| r.id == w) {
                    return Err(format!(
                        "`{w}` matches {} runs; give more of the id",
                        hits.len()
                    ));
                }
                hits.iter().find(|r| r.id == w).or(hits.first()).copied()
            }
        };
        found.cloned().ok_or_else(|| {
            if runs.is_empty() {
                "No evaluation runs yet. Start one with `tern eval run`.".into()
            } else {
                format!("No evaluation run `{w}`. `tern eval report --list` shows them.")
            }
        })
    }

    /// The run before `run` on the same rung and mode, for "diff vs previous".
    pub fn previous(&self, run: &EvalRun) -> Option<EvalRun> {
        self.runs()
            .into_iter()
            .filter(|r| r.id != run.id && r.level == run.level && r.mode == run.mode)
            .find(|r| (&r.started, &r.id) < (&run.started, &run.id))
    }

    /// Suites dropped into `<data>/evals/suites/`, with any that do not read.
    pub fn extra_suites(&self) -> (Vec<Suite>, Vec<String>) {
        let mut ok = Vec::new();
        let mut bad = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.suites_dir()) else {
            return (ok, bad);
        };
        let mut paths: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        for p in paths {
            match std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|t| Suite::parse(&t, &p.display().to_string()))
            {
                Ok(s) => ok.push(s),
                Err(e) => bad.push(e),
            }
        }
        (ok, bad)
    }
}

/// A new run id from the clock: `eval-20261023-143005`.
pub fn new_id(prefix: &str) -> (String, String) {
    let now = time::OffsetDateTime::now_utc();
    let id = format!(
        "{prefix}-{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let started = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    (id, started)
}

/// Every suite and rung there is, for `tern eval list` and the desktop.
pub fn catalog(store: &EvalStore) -> (Vec<Suite>, Vec<String>) {
    let (extra, bad) = store.extra_suites();
    let ids: Vec<String> = extra.iter().map(|e| e.id.clone()).collect();
    let mut all: Vec<Suite> = tern_eval::bundled()
        .into_iter()
        .filter(|s| !ids.contains(&s.id))
        .chain(extra)
        .collect();
    all.sort_by(|a, b| (a.level, a.kind, &a.id).cmp(&(b.level, b.kind, &b.id)));
    (all, bad)
}

/// What to run.
#[derive(Clone, Debug, Default)]
pub struct EvalRequest {
    pub level: Option<Level>,
    /// Only these suite ids.
    pub only: Vec<String>,
    pub trials: Option<usize>,
    /// SciFact's folder (claims_dev.jsonl or claims_train.jsonl, and corpus.jsonl).
    pub scifact: Option<PathBuf>,
    /// Keep the first N SciFact claims.
    pub limit: Option<usize>,
}

/// Reads SciFact from its folder.
pub fn load_scifact(dir: &Path, limit: Option<usize>) -> Result<Suite, String> {
    let corpus = std::fs::read_to_string(dir.join("corpus.jsonl"))
        .map_err(|e| format!("{}: {e}", dir.join("corpus.jsonl").display()))?;
    for split in ["dev", "train"] {
        let p = dir.join(format!("claims_{split}.jsonl"));
        if let Ok(claims) = std::fs::read_to_string(&p) {
            return tern_eval::public::scifact(&claims, &corpus, split, limit);
        }
    }
    Err(format!(
        "{} has no claims_dev.jsonl or claims_train.jsonl (claims_test.jsonl has no labels)",
        dir.display()
    ))
}

/// The suites a request selects.
pub fn select(store: &EvalStore, req: &EvalRequest) -> Result<(Level, Vec<Suite>), String> {
    let level = req.level.unwrap_or(Level::Synthetic);
    let mut suites = match level {
        Level::Unit | Level::Human => {
            return Err(format!(
                "the {} rung is run with `{}`",
                level.label(),
                level.command()
            ));
        }
        Level::Public => {
            let Some(dir) = &req.scifact else {
                return Err("the public rung reads SciFact from your copy: `tern eval run --level public --scifact <folder>` (from https://github.com/allenai/scifact, data.tar.gz)".into());
            };
            vec![load_scifact(dir, req.limit)?]
        }
        _ => tern_eval::for_level(level, &store.extra_suites().0),
    };
    if !req.only.is_empty() {
        suites.retain(|s| req.only.iter().any(|o| o == &s.id));
        if suites.is_empty() {
            return Err(format!(
                "no {} suite named {}",
                level.label(),
                req.only.join(", ")
            ));
        }
    }
    Ok((level, suites))
}

/// Runs a request against `subject`, saves it, and returns it.
pub async fn run(
    store: &EvalStore,
    req: &EvalRequest,
    subject: &TernSubject,
) -> Result<(EvalRun, PathBuf), String> {
    let (level, suites) = select(store, req)?;
    let (id, started) = new_id("eval");
    let t0 = Instant::now();
    let opts = tern_eval::RunOptions {
        trials: req.trials.unwrap_or(MIN_TRIALS).clamp(1, 20),
    };
    let mut run = tern_eval::run::run(id, started, level, &suites, subject, &opts).await;
    run.seconds = t0.elapsed().as_secs_f64();
    let path = store.save(&run)?;
    Ok((run, path))
}

/// The unit rung: `cargo test --workspace` in `root`, saved as a run.
pub fn run_unit(store: &EvalStore, root: &Path) -> Result<(EvalRun, PathBuf), String> {
    if !root.join("crates").join("tern-eval").is_dir() {
        return Err(format!(
            "{} is not a Tern source checkout; the unit rung runs `cargo test --workspace` there",
            root.display()
        ));
    }
    let (id, started) = new_id("unit");
    let t0 = Instant::now();
    let out = std::process::Command::new("cargo")
        .args(["test", "--workspace", "--no-fail-fast"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("could not start cargo: {e}"))?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let run = tern_eval::unit::unit_run(id, started, t0.elapsed().as_secs_f64(), &text);
    if run.suites[0].attempted == 0 {
        return Err("cargo test ran no tests (did it build?)".into());
    }
    let path = store.save(&run)?;
    Ok((run, path))
}

/// Saves a usability session.
pub fn save_session(
    store: &EvalStore,
    participant: &str,
    tasks: &[TaskResult],
    sus: &[u8; 10],
) -> Result<(EvalRun, PathBuf), String> {
    let (id, started) = new_id("human");
    let run = tern_eval::usability::session_run(id, started, participant, tasks, sus)?;
    let path = store.save(&run)?;
    Ok((run, path))
}

/// The baseline bundled with this build.
pub fn baseline() -> tern_eval::Baseline {
    tern_eval::baseline()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_reads_effect_names() {
        let s = TernSubject::offline();
        assert_eq!(s.gate("t", "read", "").unwrap(), GateAnswer::Allow);
        assert_eq!(
            s.gate("t", "external side effect", "x").unwrap(),
            GateAnswer::Ask
        );
        assert_eq!(s.gate("t", "destructive", "x").unwrap(), GateAnswer::Ask);
        assert!(s.gate("t", "teleport", "x").is_err());
    }

    #[tokio::test]
    async fn synthetic_rung_runs_on_terns_pieces_saves_and_compares() {
        let dir = tempfile::tempdir().unwrap();
        let store = EvalStore::at(dir.path());
        let req = EvalRequest::default();
        let (a, path) = run(&store, &req, &TernSubject::offline()).await.unwrap();
        assert!(path.exists() && path.with_extension("md").exists());
        assert_eq!(a.trials, 3);
        assert_eq!(a.suites.len(), 5);
        // The bundled baseline holds for an offline run of this build.
        let broken = check_baseline(&a, &baseline());
        assert!(broken.is_empty(), "{broken:#?}");
        // Safety: the poisoned descriptions are caught by the real scan.
        let s = a.suite("safety").unwrap();
        assert!(s.metric("scan_recall").unwrap().mean >= 0.75);
        assert_eq!(s.metric("containment").unwrap().mean, 1.0);
        // A second run reads back, has a previous, and does not regress.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let (b, _) = run(&store, &req, &TernSubject::offline()).await.unwrap();
        assert_eq!(store.find("last").unwrap().id, b.id);
        assert_eq!(store.previous(&b).unwrap().id, a.id);
        assert_eq!(compare(&a, &b).regressions(), 0);
        assert_eq!(store.summaries().len(), 2);
    }

    #[test]
    fn extra_suites_are_read_and_bad_ones_reported() {
        let dir = tempfile::tempdir().unwrap();
        let store = EvalStore::at(dir.path());
        std::fs::create_dir_all(store.suites_dir()).unwrap();
        std::fs::write(
            store.suites_dir().join("mine.json"),
            r#"{"format":1,"id":"mine","version":1,"title":"Mine","kind":"quiz","level":"synthetic","origin":"mine","cases":[{"id":"a","kind":"true_false","prompt":"Sky is blue","key":"true","given":"true","gold":"correct"}]}"#,
        )
        .unwrap();
        std::fs::write(store.suites_dir().join("bad.json"), "{").unwrap();
        let (ok, bad) = store.extra_suites();
        assert_eq!(ok.len(), 1);
        assert_eq!(bad.len(), 1);
        let (level, suites) = select(
            &store,
            &EvalRequest {
                only: vec!["mine".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(level, Level::Synthetic);
        assert_eq!(suites.len(), 1);
    }
}
