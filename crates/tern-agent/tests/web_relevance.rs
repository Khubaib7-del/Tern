//! The relevance guard on real search replies (#67): Wikipedia answers a
//! question it does not know with pages that match one common word. The
//! fixtures are shaped like the user's log ("SubQ Snap 1.1 AI model
//! definition or meaning" → six Japanese and medical pages; "harness2 by
//! Google AI model or project" → pages about harnesses).

use std::sync::Arc;

use serde_json::{Value, json};
use tern_agent::Tool;
use tern_agent::web::WebSearchTool;
use tern_crawl::{SearchClient, find_search};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn wikipedia_answering(fixture: &str) -> (MockServer, WebSearchTool) {
    let server = MockServer::start().await;
    let body: Value = serde_json::from_str(fixture).unwrap();
    Mock::given(method("GET"))
        .and(path("/w/api.php"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let client =
        SearchClient::new(find_search("wikipedia").unwrap(), None, Some(&server.uri())).unwrap();
    (server, WebSearchTool(Arc::new(client), None))
}

#[tokio::test]
async fn off_topic_wikipedia_pages_are_dropped_and_the_tool_says_so() {
    let (_s, tool) = wikipedia_answering(include_str!("fixtures/wiki-subq-snap.json")).await;
    let out = tool
        .call(json!({"query": "SubQ Snap 1.1 AI model definition or meaning"}))
        .await
        .unwrap();
    assert_eq!(out.meta["results"], 0, "{}", out.content);
    // The six pages come back for every ladder query; each counts once.
    assert_eq!(out.meta["filtered"], 6, "{}", out.meta);
    assert_eq!(out.meta["found"], 6);
    assert_eq!(out.meta["notice"], "wikipedia_empty");
    assert_eq!(out.meta["service"], "Wikipedia");
    assert!(out.content.starts_with("Wikipedia has nothing on"));
    assert!(out.content.contains("Tavily"));
    assert!(out.evidence_refs.is_empty());
    // The ladder tried the topic's own words, never a generic one alone.
    let tried: Vec<&str> = out.meta["tried"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t.as_str())
        .collect();
    assert!(
        !tried
            .iter()
            .any(|t| ["model", "definition", "meaning", "AI"].contains(t))
    );
}

#[tokio::test]
async fn a_lone_generic_word_never_lets_a_page_through() {
    let (_s, tool) = wikipedia_answering(include_str!("fixtures/wiki-harness.json")).await;
    let out = tool
        .call(json!({"query": "harness2 by Google AI model or project"}))
        .await
        .unwrap();
    assert_eq!(out.meta["results"], 0, "{}", out.content);
    assert_eq!(out.meta["notice"], "wikipedia_empty");
    let tried = out.meta["tried"].to_string();
    assert!(!tried.contains("\"harness\""), "{tried}");
}

#[tokio::test]
async fn on_topic_pages_stay_and_the_rest_are_counted() {
    let (_s, tool) = wikipedia_answering(include_str!("fixtures/wiki-mixed.json")).await;
    let out = tool
        .call(json!({"query": "langchain creator"}))
        .await
        .unwrap();
    assert_eq!(out.meta["results"], 1, "{}", out.content);
    assert_eq!(out.meta["filtered"], 2);
    assert!(out.meta["notice"].is_null());
    assert!(out.content.contains("LangChain"));
    assert!(!out.content.contains("Volcano"));
    assert!(out.content.contains("from Wikipedia"));
}
