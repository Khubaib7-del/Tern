//! The decider: which model answers, hard checks over its score, a cache,
//! and the log of what was decided.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::calibrate::{argmax, confidence, normalize};
use crate::local::LocalModel;
use crate::model::{Decision, DecisionModel, DecisionRequest, Override};

/// Whether decisions are made, and by what.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// No decision model: Tern keeps its fixed order (search order, plan
    /// order) and the word-overlap checks.
    Off,
    /// The local scorer only. Offline and free.
    #[default]
    Local,
    /// A remote decision model, with the local scorer as fallback.
    Remote,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Self::Off, Self::Local, Self::Remote];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Local => "Local",
            Self::Remote => "Remote",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "off" | "none" => Some(Self::Off),
            "local" | "offline" => Some(Self::Local),
            "remote" | "on" => Some(Self::Remote),
            _ => None,
        }
    }
}

/// A hard check: a fact a score never overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    /// This choice is impossible (already read, blocked by robots.txt, a
    /// number in the claim missing from the passage).
    RuleOut { choice: usize, reason: String },
    /// This choice is certain (no source text: the claim has no source).
    Force { choice: usize, reason: String },
}

/// Applies hard checks to probabilities. A forced choice takes all the
/// probability; ruled-out choices lose theirs and the rest is rescaled.
/// If every choice is ruled out the scores stay and the overrides say so.
pub fn apply_rules(probs: &[f32], rules: &[Rule]) -> (Vec<f32>, Vec<Override>) {
    let mut out = probs.to_vec();
    let mut overrides = Vec::new();
    if let Some(Rule::Force { choice, reason }) = rules
        .iter()
        .find(|r| matches!(r, Rule::Force { choice, .. } if *choice < probs.len()))
    {
        for (i, p) in out.iter_mut().enumerate() {
            *p = if i == *choice { 1.0 } else { 0.0 };
        }
        overrides.push(Override {
            choice: *choice,
            action: "forced".into(),
            reason: reason.clone(),
        });
        return (out, overrides);
    }
    let mut ruled = vec![false; probs.len()];
    for r in rules {
        if let Rule::RuleOut { choice, reason } = r
            && *choice < probs.len()
            && !ruled[*choice]
        {
            ruled[*choice] = true;
            overrides.push(Override {
                choice: *choice,
                action: "ruled_out".into(),
                reason: reason.clone(),
            });
        }
    }
    if ruled.iter().all(|r| *r) {
        return (out, overrides);
    }
    for (p, r) in out.iter_mut().zip(&ruled) {
        if *r {
            *p = 0.0;
        }
    }
    let all_zero = out.iter().all(|p| *p <= 0.0);
    let rescaled = if all_zero {
        // The model put everything on ruled-out choices: spread evenly
        // over what is left.
        let left: Vec<f32> = ruled.iter().map(|r| if *r { 0.0 } else { 1.0 }).collect();
        normalize(&left)
    } else {
        normalize(&out)
    };
    (rescaled, overrides)
}

/// Most decisions remembered by the cache.
pub const CACHE_SIZE: usize = 256;
/// Most decisions kept in the log.
pub const LOG_SIZE: usize = 500;
/// How long a remote model may take before the local scorer answers.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(4000);

/// Scores decisions with the chosen model, falls back to the local one,
/// applies hard checks, caches, and logs.
pub struct Decider {
    mode: Mode,
    remote: Option<Box<dyn DecisionModel>>,
    /// Why remote was asked for but is not set up, for the fallback note.
    remote_missing: Option<String>,
    local: LocalModel,
    timeout: Duration,
    cache: Mutex<(HashMap<u64, Decision>, VecDeque<u64>)>,
    log: Mutex<Vec<Decision>>,
}

impl std::fmt::Debug for Decider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decider")
            .field("mode", &self.mode)
            .field("model", &self.describe())
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl Decider {
    fn with(mode: Mode, remote: Option<Box<dyn DecisionModel>>) -> Self {
        Self {
            mode,
            remote,
            remote_missing: None,
            local: LocalModel,
            timeout: DEFAULT_TIMEOUT,
            cache: Mutex::new((HashMap::new(), VecDeque::new())),
            log: Mutex::new(Vec::new()),
        }
    }

    /// No decisions: every call returns `None`.
    pub fn off() -> Self {
        Self::with(Mode::Off, None)
    }

    /// The local scorer only.
    pub fn local() -> Self {
        Self::with(Mode::Local, None)
    }

    /// A remote model with the local scorer as fallback.
    pub fn remote(model: Box<dyn DecisionModel>) -> Self {
        Self::with(Mode::Remote, Some(model))
    }

    /// Remote was chosen but could not be set up (no key): decisions are
    /// local, each noting `why`.
    pub fn remote_unavailable(why: impl Into<String>) -> Self {
        let mut d = Self::with(Mode::Remote, None);
        d.remote_missing = Some(why.into());
        d
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn is_on(&self) -> bool {
        self.mode != Mode::Off
    }

    /// "Local" or "TypeSafe jev-latest".
    pub fn describe(&self) -> String {
        match (self.mode, &self.remote) {
            (Mode::Off, _) => "Off".into(),
            (Mode::Local, _) | (Mode::Remote, None) => "Local".into(),
            (Mode::Remote, Some(m)) => format!("{} {}", m.provider(), m.model()),
        }
    }

    fn key(&self, req: &DecisionRequest) -> u64 {
        let mut h = DefaultHasher::new();
        self.describe().hash(&mut h);
        req.kind.hash(&mut h);
        req.context.hash(&mut h);
        req.question.hash(&mut h);
        for c in &req.choices {
            c.id.hash(&mut h);
            c.text.hash(&mut h);
            c.prior.map(f32::to_bits).hash(&mut h);
        }
        h.finish()
    }

    /// Scores `req` and applies `rules`. `None` when decisions are off or
    /// the request is not answerable (fewer than two choices). A remote
    /// error, timeout, or unsupported kind falls back to the local scorer,
    /// with the reason in [`Decision::fallback`].
    pub async fn decide(&self, req: &DecisionRequest, rules: &[Rule]) -> Option<Decision> {
        if self.mode == Mode::Off || req.validate().is_err() {
            return None;
        }
        let key = self.key(req);
        let cached = self.cache.lock().ok().and_then(|c| c.0.get(&key).cloned());
        let mut d = match cached {
            Some(mut d) => {
                d.cached = true;
                d.latency_ms = 0;
                d.cost = Default::default();
                d
            }
            None => {
                let d = self.score(req).await;
                if d.fallback.is_none()
                    && let Ok(mut c) = self.cache.lock()
                {
                    if c.1.len() >= CACHE_SIZE
                        && let Some(old) = c.1.pop_front()
                    {
                        c.0.remove(&old);
                    }
                    c.0.insert(key, d.clone());
                    c.1.push_back(key);
                }
                d
            }
        };
        if !rules.is_empty() {
            let (probs, overrides) = apply_rules(&d.probabilities, rules);
            if !overrides.is_empty() {
                d.chosen = argmax(&probs);
                d.confidence = confidence(&probs);
                d.probabilities = probs;
                if let Some(o) = overrides.iter().find(|o| o.action == "forced") {
                    d.rationale = format!("Hard check: {}", o.reason);
                }
                d.overrides = overrides;
            }
        }
        if let Ok(mut log) = self.log.lock() {
            if log.len() >= LOG_SIZE {
                log.remove(0);
            }
            log.push(d.clone());
        }
        Some(d)
    }

    async fn score(&self, req: &DecisionRequest) -> Decision {
        let fallback = match (&self.remote, self.mode) {
            (Some(m), Mode::Remote) if m.supports(req.kind) => {
                match tokio::time::timeout(self.timeout, m.score(req)).await {
                    Ok(Ok(d)) => return d,
                    Ok(Err(e)) => Some(e.to_string()),
                    Err(_) => Some(format!(
                        "{} took longer than {} ms",
                        m.provider(),
                        self.timeout.as_millis()
                    )),
                }
            }
            (Some(m), Mode::Remote) => Some(format!(
                "{} {} does not answer {} decisions",
                m.provider(),
                m.model(),
                req.kind.label().to_lowercase()
            )),
            (None, Mode::Remote) => self.remote_missing.clone(),
            _ => None,
        };
        let mut d = self.local.decide(req);
        d.fallback = fallback;
        d
    }

    /// Everything decided so far, oldest first.
    pub fn log(&self) -> Vec<Decision> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// Takes the log, leaving it empty.
    pub fn drain_log(&self) -> Vec<Decision> {
        self.log
            .lock()
            .map(|mut l| std::mem::take(&mut *l))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BoxFuture, Choice, DecisionError, DecisionKind};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn req() -> DecisionRequest {
        DecisionRequest::new(
            DecisionKind::General,
            "Which fruit is yellow?",
            vec![
                Choice::new("apple", "apple, red"),
                Choice::new("banana", "banana, yellow fruit"),
            ],
        )
    }

    struct Fixed(Result<Vec<f32>, DecisionError>, AtomicUsize, Duration);

    impl DecisionModel for Fixed {
        fn provider(&self) -> &str {
            "test"
        }
        fn model(&self) -> &str {
            "fixed"
        }
        fn supports(&self, kind: DecisionKind) -> bool {
            kind != DecisionKind::ClaimCheck
        }
        fn score<'a>(
            &'a self,
            req: &'a DecisionRequest,
        ) -> BoxFuture<'a, Result<Decision, DecisionError>> {
            Box::pin(async move {
                self.1.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(self.2).await;
                let p = self.0.clone()?;
                Ok(Decision {
                    kind: req.kind,
                    choices: req.choices.iter().map(|c| c.id.clone()).collect(),
                    chosen: argmax(&p),
                    probabilities: p,
                    model: "fixed".into(),
                    provider: "test".into(),
                    ..Decision::default()
                })
            })
        }
    }

    #[tokio::test]
    async fn off_decides_nothing_and_local_answers() {
        assert!(Decider::off().decide(&req(), &[]).await.is_none());
        let d = Decider::local().decide(&req(), &[]).await.unwrap();
        assert_eq!(d.chosen_id(), "banana");
        assert_eq!(d.provider, "local");
        let one = DecisionRequest::new(DecisionKind::General, "q", vec![Choice::plain("a")]);
        assert!(Decider::local().decide(&one, &[]).await.is_none());
    }

    #[tokio::test]
    async fn remote_answers_then_is_cached() {
        let m = Fixed(Ok(vec![0.9, 0.1]), AtomicUsize::new(0), Duration::ZERO);
        let d = Decider::remote(Box::new(m));
        let a = d.decide(&req(), &[]).await.unwrap();
        assert_eq!((a.chosen_id(), a.provider.as_str()), ("apple", "test"));
        let b = d.decide(&req(), &[]).await.unwrap();
        assert!(b.cached);
        assert_eq!(b.probabilities, a.probabilities);
        assert_eq!(d.log().len(), 2);
        assert_eq!(d.drain_log().len(), 2);
        assert!(d.log().is_empty());
    }

    #[tokio::test]
    async fn errors_timeouts_and_unsupported_fall_back_to_local() {
        let err = Decider::remote(Box::new(Fixed(
            Err(DecisionError::Http {
                status: 401,
                body: String::new(),
            }),
            AtomicUsize::new(0),
            Duration::ZERO,
        )));
        let d = err.decide(&req(), &[]).await.unwrap();
        assert_eq!(d.provider, "local");
        assert!(d.fallback.as_deref().unwrap().contains("refused"));
        let slow = Decider::remote(Box::new(Fixed(
            Ok(vec![0.9, 0.1]),
            AtomicUsize::new(0),
            Duration::from_millis(300),
        )))
        .with_timeout(Duration::from_millis(20));
        let d = slow.decide(&req(), &[]).await.unwrap();
        assert!(d.fallback.as_deref().unwrap().contains("longer"));
        let mut claim = req();
        claim.kind = DecisionKind::ClaimCheck;
        let d = slow.decide(&claim, &[]).await.unwrap();
        assert!(d.fallback.as_deref().unwrap().contains("does not answer"));
        let missing = Decider::remote_unavailable("TypeSafe needs a key");
        let d = missing.decide(&req(), &[]).await.unwrap();
        assert_eq!(d.fallback.as_deref(), Some("TypeSafe needs a key"));
        assert_eq!(missing.describe(), "Local");
    }

    #[tokio::test]
    async fn hard_checks_override_the_score() {
        let d = Decider::local();
        let ruled = d
            .decide(
                &req(),
                &[Rule::RuleOut {
                    choice: 1,
                    reason: "already read".into(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(ruled.chosen_id(), "apple");
        assert_eq!(ruled.probabilities[1], 0.0);
        assert_eq!(ruled.overrides[0].action, "ruled_out");
        let forced = d
            .decide(
                &req(),
                &[Rule::Force {
                    choice: 0,
                    reason: "no source".into(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(forced.probabilities, [1.0, 0.0]);
        assert!(forced.rationale.contains("no source"));
        let (p, o) = apply_rules(
            &[0.5, 0.5],
            &[
                Rule::RuleOut {
                    choice: 0,
                    reason: "x".into(),
                },
                Rule::RuleOut {
                    choice: 1,
                    reason: "y".into(),
                },
            ],
        );
        assert_eq!(
            (p, o.len()),
            (vec![0.5, 0.5], 2),
            "all ruled out keeps scores"
        );
        let (p, _) = apply_rules(
            &[1.0, 0.0, 0.0],
            &[Rule::RuleOut {
                choice: 0,
                reason: "x".into(),
            }],
        );
        assert_eq!(p, [0.0, 0.5, 0.5]);
    }
}
