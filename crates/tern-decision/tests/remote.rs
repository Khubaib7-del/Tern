//! Remote adapters against a local mock server, in the shapes the
//! providers' API references document.

use std::time::Duration;

use tern_core::SecretString;
use tern_decision::{
    Choice, Decider, DecisionKind, DecisionModel, DecisionRequest, RemoteModel, find_remote,
};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "ts-test-0123456789abcdefghijklmn";

fn req(kind: DecisionKind) -> DecisionRequest {
    DecisionRequest::new(
        kind,
        "Which team should handle this?",
        vec![
            Choice::new("returns", "Exchanges, wrong or damaged items"),
            Choice::new("billing", "Charges, invoices, payment problems"),
        ],
    )
    .with_context("My shoes arrived in the wrong size.")
}

fn model(id: &str, base: &str) -> RemoteModel {
    RemoteModel::new(
        find_remote(id).unwrap(),
        Some(SecretString::new(KEY)),
        Some(base),
        None,
        Duration::from_secs(5),
    )
    .unwrap()
}

#[tokio::test]
async fn system_one_choice_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(body_partial_json(serde_json::json!({
            "model": "jev-latest",
            "state": "My shoes arrived in the wrong size.",
            "questions": {"decision": {"type": "choice", "instructions": "Which team should handle this?",
                "criteria": {"c1": "Exchanges, wrong or damaged items", "c2": "Charges, invoices, payment problems"}}}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {"decision": {"type": "choice", "choice": "c1", "confidence": 0.84,
                "probabilities": {"c1": 0.92, "c2": 0.08}}},
            "usage": {"input_tokens": 1000, "output_tokens": 12}
        })))
        .mount(&server)
        .await;
    let m = model("typesafe", &format!("{}/v1", server.uri()));
    let d = m.score(&req(DecisionKind::General)).await.unwrap();
    assert_eq!(d.chosen_id(), "returns");
    assert_eq!(d.model, "jev-1.13.0");
    assert_eq!(d.provider, "typesafe");
    assert!((d.confidence - 0.84).abs() < 1e-6);
    assert_eq!(d.cost.input_tokens, 1000);
    assert!((d.cost.usd - 0.000042).abs() < 1e-9);
}

#[tokio::test]
async fn subq_uses_its_model_and_the_same_wire() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_partial_json(
            serde_json::json!({"model": "subq-snap-1.1"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "subq-snap-1.1",
            "answers": {"decision": {"type": "choice", "choice": "c2", "confidence": 0.5,
                "probabilities": {"c1": 0.3, "c2": 0.7}}},
            "usage": {"input_tokens": 477, "output_tokens": 0}
        })))
        .mount(&server)
        .await;
    let m = model("subq", &format!("{}/v1", server.uri()));
    let d = m.score(&req(DecisionKind::Triage)).await.unwrap();
    assert_eq!(d.chosen_id(), "billing");
}

#[tokio::test]
async fn voyage_rerank_scores_become_probabilities() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/rerank"))
        .and(body_partial_json(
            serde_json::json!({"model": "rerank-2.5-lite"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [{"index": 1, "relevance_score": 0.81}, {"index": 0, "relevance_score": 0.12}],
            "model": "rerank-2.5-lite",
            "usage": {"total_tokens": 50}
        })))
        .mount(&server)
        .await;
    let m = model("voyage", &format!("{}/v1", server.uri()));
    let d = m.score(&req(DecisionKind::SourceRank)).await.unwrap();
    assert_eq!(d.chosen_id(), "billing");
    assert!((d.probabilities.iter().sum::<f32>() - 1.0).abs() < 1e-4);
    assert!(!m.supports(DecisionKind::ClaimCheck));
}

#[tokio::test]
async fn refused_key_falls_back_to_local_without_leaking_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(format!("{{\"detail\":\"bad key {KEY}\"}}")),
        )
        .mount(&server)
        .await;
    let d = Decider::remote(Box::new(model("typesafe", &format!("{}/v1", server.uri()))));
    let out = d.decide(&req(DecisionKind::General), &[]).await.unwrap();
    assert_eq!(out.provider, "local");
    let why = out.fallback.unwrap();
    assert!(why.contains("401"), "{why}");
    assert!(!why.contains(KEY), "{why}");
}
