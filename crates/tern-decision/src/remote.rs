//! Remote decision models, bring-your-own-key.
//!
//! Two wire formats, each checked against its provider's public API
//! reference (see ADR 0007):
//!
//! | Wire | Providers | Endpoint |
//! | --- | --- | --- |
//! | System One | TypeSafe (Jev), Subquadratic (SubQ Snap) | `POST /v1/systemone`: a `state`, a `choice` question, probabilities back |
//! | Rerank | Voyage AI | `POST /v1/rerank`: a query and documents, relevance scores back |

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tern_core::{Redactor, SecretString};

use crate::calibrate::{argmax, confidence, normalize, softmax};
use crate::model::{
    BoxFuture, Cost, Decision, DecisionError, DecisionKind, DecisionModel, DecisionRequest, clip,
};

/// How a provider is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    SystemOne,
    Rerank,
}

/// A remote decision-model provider Tern can call.
#[derive(Debug)]
pub struct RemoteSpec {
    pub id: &'static str,
    pub name: &'static str,
    /// The model family shown beside the name ("Jev").
    pub product: &'static str,
    pub wire: Wire,
    pub default_base: &'static str,
    pub default_model: &'static str,
    /// Other model ids the provider documents.
    pub models: &'static [&'static str],
    /// US dollars per million input tokens for the default model, from
    /// the provider's pricing page on 23 October 2026. Output is free.
    pub usd_per_m_input: f64,
    pub key_url: &'static str,
    pub docs_url: &'static str,
    /// The built-in logo the app shows for this service.
    pub icon: &'static str,
    /// Most choices one request may carry.
    pub max_choices: usize,
    /// Most characters of context sent.
    pub max_context_chars: usize,
}

pub static DECISION_CATALOG: &[RemoteSpec] = &[
    RemoteSpec {
        id: "typesafe",
        name: "TypeSafe",
        product: "Jev",
        wire: Wire::SystemOne,
        default_base: "https://api.typesafe.ai/v1",
        default_model: "jev-latest",
        models: &["jev-latest", "jev-preview", "jev-1.14.0", "jev-1.13.0"],
        usd_per_m_input: 0.042,
        key_url: "https://console.typesafe.ai",
        docs_url: "https://docs.typesafe.ai/api",
        icon: "typesafe",
        max_choices: 255,
        // Jev reads 64k tokens per request and 32k per question by default.
        max_context_chars: 80_000,
    },
    RemoteSpec {
        id: "subq",
        name: "Subquadratic",
        product: "SubQ Snap",
        wire: Wire::SystemOne,
        default_base: "https://api.subq.ai/v1",
        default_model: "subq-snap-1.1",
        models: &["subq-snap-1.1"],
        usd_per_m_input: 0.04,
        key_url: "https://console.subq.ai",
        docs_url: "https://docs.subq.ai/endpoints/decisions-api",
        icon: "subq",
        max_choices: 255,
        // A 384K-token window; Tern still sends a bounded slice.
        max_context_chars: 200_000,
    },
    RemoteSpec {
        id: "voyage",
        name: "Voyage AI",
        product: "Rerank",
        wire: Wire::Rerank,
        default_base: "https://api.voyageai.com/v1",
        default_model: "rerank-2.5-lite",
        models: &["rerank-2.5-lite", "rerank-2.5"],
        usd_per_m_input: 0.02,
        key_url: "https://dashboard.voyageai.com",
        docs_url: "https://docs.voyageai.com/reference/reranker-api",
        icon: "voyage",
        max_choices: 1000,
        max_context_chars: 24_000,
    },
];

pub fn find_remote(id: &str) -> Option<&'static RemoteSpec> {
    DECISION_CATALOG
        .iter()
        .find(|s| s.id.eq_ignore_ascii_case(id) || s.product.eq_ignore_ascii_case(id))
}

/// Price per million input tokens for a model of `spec`.
pub fn price_of(spec: &RemoteSpec, model: &str) -> f64 {
    match (spec.id, model) {
        ("voyage", "rerank-2.5") => 0.05,
        _ => spec.usd_per_m_input,
    }
}

/// The question names used on the wire. The model never sees them.
const QUESTION: &str = "decision";

/// A ready remote decision model.
#[derive(Debug)]
pub struct RemoteModel {
    spec: &'static RemoteSpec,
    base: String,
    model: String,
    key: SecretString,
    http: reqwest::Client,
}

impl RemoteModel {
    /// A client for `spec`. `base_url` and `model` override the defaults.
    pub fn new(
        spec: &'static RemoteSpec,
        key: Option<SecretString>,
        base_url: Option<&str>,
        model: Option<&str>,
        timeout: Duration,
    ) -> Result<Self, DecisionError> {
        let key = key
            .filter(|k| !k.is_empty())
            .ok_or(DecisionError::MissingKey(spec.name))?;
        let base = base_url
            .unwrap_or(spec.default_base)
            .trim_end_matches('/')
            .to_owned();
        let mut builder = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("Tern/", env!("CARGO_PKG_VERSION")));
        if base.contains("://127.0.0.1") || base.contains("://localhost") {
            builder = builder.no_proxy();
        }
        let http = builder
            .build()
            .map_err(|e| DecisionError::Network(e.to_string()))?;
        Ok(Self {
            spec,
            base,
            model: model
                .filter(|m| !m.trim().is_empty())
                .unwrap_or(spec.default_model)
                .to_owned(),
            key,
            http,
        })
    }

    pub fn spec(&self) -> &'static RemoteSpec {
        self.spec
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, DecisionError> {
        let resp = self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(self.key.expose())
            .json(body)
            .send()
            .await
            .map_err(|e| {
                DecisionError::Network(if e.is_timeout() {
                    "the request timed out".into()
                } else if e.is_connect() {
                    "the connection failed".into()
                } else {
                    let mut s = e.to_string();
                    if let Some(u) = e.url() {
                        s = s.replace(u.as_str(), "<url>");
                    }
                    s
                })
            })?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| DecisionError::Network(e.to_string()))?;
        if !status.is_success() {
            // A provider may echo the key back; it never reaches a log.
            let red = Redactor::default()
                .redact(&text.replace(self.key.expose(), "[redacted]"))
                .into_owned();
            return Err(DecisionError::Http {
                status: status.as_u16(),
                body: clip(&red, 300),
            });
        }
        serde_json::from_str(&text).map_err(|e| DecisionError::BadResponse(e.to_string()))
    }

    fn context_of(&self, req: &DecisionRequest) -> String {
        req.context
            .chars()
            .take(self.spec.max_context_chars)
            .collect()
    }

    fn finish(
        &self,
        req: &DecisionRequest,
        probabilities: Vec<f32>,
        conf: Option<f32>,
        model: Option<&str>,
        tokens: u64,
        started: Instant,
    ) -> Decision {
        let chosen = argmax(&probabilities);
        Decision {
            kind: req.kind,
            question: clip(&req.question, 200),
            choices: req.choices.iter().map(|c| c.id.clone()).collect(),
            confidence: conf.unwrap_or_else(|| confidence(&probabilities)),
            rationale: format!(
                "{} {} put {:.0}% on it",
                self.spec.product,
                self.model,
                probabilities.get(chosen).copied().unwrap_or(0.0) * 100.0
            ),
            probabilities,
            chosen,
            model: model.unwrap_or(&self.model).to_owned(),
            provider: self.spec.id.into(),
            latency_ms: started.elapsed().as_millis() as u64,
            cost: Cost {
                input_tokens: tokens,
                usd: tokens as f64 * price_of(self.spec, &self.model) / 1_000_000.0,
            },
            ..Decision::default()
        }
    }

    /// The System One request for `req`: one `choice` question whose
    /// options are `c1`, `c2`, ... with the choices' text as descriptions.
    pub fn system_one_body(&self, req: &DecisionRequest) -> Value {
        let mut criteria = serde_json::Map::new();
        for (i, c) in req.choices.iter().take(self.spec.max_choices).enumerate() {
            criteria.insert(format!("c{}", i + 1), json!(clip(&c.text, 2000)));
        }
        let context = self.context_of(req);
        let state = if context.trim().is_empty() {
            req.question.clone()
        } else {
            context
        };
        json!({
            "model": self.model,
            "state": state,
            "questions": {
                QUESTION: {
                    "type": "choice",
                    "instructions": req.question,
                    "criteria": criteria,
                }
            }
        })
    }

    async fn system_one(&self, req: &DecisionRequest) -> Result<Decision, DecisionError> {
        let started = Instant::now();
        let v = self.post("/systemone", &self.system_one_body(req)).await?;
        let answer = &v["answers"][QUESTION];
        let probs = answer["probabilities"]
            .as_object()
            .ok_or_else(|| DecisionError::BadResponse("no probabilities in the answer".into()))?;
        let raw: Vec<f32> = (0..req.choices.len())
            .map(|i| {
                probs
                    .get(&format!("c{}", i + 1))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32
            })
            .collect();
        if raw.iter().all(|p| *p == 0.0) {
            return Err(DecisionError::BadResponse(
                "the answer named none of the choices".into(),
            ));
        }
        let conf = answer["confidence"].as_f64().map(|c| c as f32);
        let tokens = v["usage"]["input_tokens"].as_u64().unwrap_or(0);
        Ok(self.finish(
            req,
            normalize(&raw),
            conf,
            v["model"].as_str(),
            tokens,
            started,
        ))
    }

    async fn rerank(&self, req: &DecisionRequest) -> Result<Decision, DecisionError> {
        let started = Instant::now();
        let context = self.context_of(req);
        let query = if context.trim().is_empty() {
            req.question.clone()
        } else {
            format!("{}\n\n{}", req.question, context)
        };
        let docs: Vec<String> = req
            .choices
            .iter()
            .take(self.spec.max_choices)
            .map(|c| clip(&c.text, 4000))
            .collect();
        let body = json!({
            "model": self.model,
            "query": query,
            "documents": docs,
            "truncation": true,
        });
        let v = self.post("/rerank", &body).await?;
        let data = v["data"]
            .as_array()
            .ok_or_else(|| DecisionError::BadResponse("no data in the reply".into()))?;
        let mut scores = vec![0.0f32; req.choices.len()];
        let mut seen = 0;
        for row in data {
            if let (Some(i), Some(s)) = (row["index"].as_u64(), row["relevance_score"].as_f64())
                && let Some(slot) = scores.get_mut(i as usize)
            {
                *slot = s as f32;
                seen += 1;
            }
        }
        if seen == 0 {
            return Err(DecisionError::BadResponse(
                "the reply scored no documents".into(),
            ));
        }
        // Relevance scores are not probabilities; a sharp softmax keeps
        // their order and makes them sum to 1.
        let probs = softmax(&scores, 0.1);
        let tokens = v["usage"]["total_tokens"].as_u64().unwrap_or(0);
        Ok(self.finish(req, probs, None, v["model"].as_str(), tokens, started))
    }
}

impl DecisionModel for RemoteModel {
    fn provider(&self) -> &str {
        self.spec.id
    }
    fn model(&self) -> &str {
        &self.model
    }
    fn supports(&self, kind: DecisionKind) -> bool {
        match self.spec.wire {
            Wire::SystemOne => true,
            // A reranker orders documents; it cannot judge a claim or pick
            // a tool from labels.
            Wire::Rerank => matches!(
                kind,
                DecisionKind::SourceRank | DecisionKind::Triage | DecisionKind::General
            ),
        }
    }
    fn score<'a>(
        &'a self,
        req: &'a DecisionRequest,
    ) -> BoxFuture<'a, Result<Decision, DecisionError>> {
        Box::pin(async move {
            req.validate()?;
            if !self.supports(req.kind) {
                return Err(DecisionError::Unsupported(format!(
                    "{} ranks documents and cannot answer a {} decision",
                    self.spec.name,
                    req.kind.label().to_lowercase()
                )));
            }
            match self.spec.wire {
                Wire::SystemOne => self.system_one(req).await,
                Wire::Rerank => self.rerank(req).await,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Choice;

    #[test]
    fn catalog_is_complete() {
        for s in DECISION_CATALOG {
            assert!(s.default_base.starts_with("https://"));
            assert!(s.models.contains(&s.default_model));
            assert!(s.usd_per_m_input > 0.0);
        }
        assert_eq!(find_remote("jev").unwrap().id, "typesafe");
        assert_eq!(find_remote("SubQ Snap").unwrap().id, "subq");
        assert!(find_remote("nope").is_none());
        assert_eq!(price_of(find_remote("voyage").unwrap(), "rerank-2.5"), 0.05);
    }

    #[test]
    fn needs_a_key_and_shapes_the_body() {
        let spec = find_remote("typesafe").unwrap();
        assert_eq!(
            RemoteModel::new(spec, None, None, None, Duration::from_secs(1)).unwrap_err(),
            DecisionError::MissingKey("TypeSafe")
        );
        let m = RemoteModel::new(
            spec,
            Some(SecretString::new("k")),
            None,
            None,
            Duration::from_secs(1),
        )
        .unwrap();
        let req = DecisionRequest::new(
            DecisionKind::General,
            "Which?",
            vec![Choice::plain("a"), Choice::plain("b")],
        );
        let b = m.system_one_body(&req);
        assert_eq!(b["model"], "jev-latest");
        assert_eq!(b["state"], "Which?");
        assert_eq!(b["questions"]["decision"]["criteria"]["c2"], "b");
        assert!(!format!("{m:?}").contains("\"k\""));
    }
}
