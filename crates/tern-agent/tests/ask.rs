//! `run_ask` end to end against mock search, pages, and model.

use serde_json::json;
use tern_agent::ask::{AskDeps, AskOptions, run_ask};
use tern_agent::{MemorySink, providers};
use tern_core::SessionId;
use tern_crawl::{Fetcher, SearchClient, find_search};
use tern_providers::UsageLedger;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn page(title: &str, body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_string(format!("<html><head><title>{title}</title></head><body><main><p>{body}</p></main></body></html>"))
        .insert_header("content-type", "text/html")
}

async fn setup(answer: &str) -> MockServer {
    let s = MockServer::start().await;
    let u = s.uri();
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"title": "Paper", "url": format!("{u}/paper"), "content": "x"},
            {"title": "Gone", "url": format!("{u}/gone"), "content": "x"},
            {"title": "Blog", "url": format!("{u}/blog"), "content": "x"}
        ]})))
        .mount(&s)
        .await;
    Mock::given(path("/paper"))
        .respond_with(page(
            "Attention Is All You Need",
            "The Transformer uses only attention.",
        ))
        .mount(&s)
        .await;
    Mock::given(path("/blog"))
        .respond_with(page("A blog", "Transformers replaced RNNs."))
        .mount(&s)
        .await;
    Mock::given(path("/gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&s)
        .await;
    Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": answer}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 900, "completion_tokens": 60}
    }))).mount(&s).await;
    s
}

async fn ask(s: &MockServer, sink: &MemorySink) -> Result<tern_agent::AskReport, String> {
    let search = SearchClient::new(find_search("searxng").unwrap(), None, Some(&s.uri())).unwrap();
    let provider =
        providers::connect_by_id("ollama", None, Some(&format!("{}/v1", s.uri()))).unwrap();
    let fetcher = Fetcher::allowing_private();
    let ledger = UsageLedger::default();
    let deps = AskDeps {
        session: SessionId::new(),
        model: &provider,
        model_name: "llama3.2",
        search: &search,
        fetcher: &fetcher,
        sink,
        ledger: &ledger,
        decider: None,
    };
    let lines = std::sync::Mutex::new(Vec::<String>::new());
    let progress = |l: &str| lines.lock().unwrap().push(l.to_owned());
    let opts = AskOptions {
        reads: 3,
        ..Default::default()
    };
    run_ask(deps, "What did Vaswani et al. propose?", &opts, &progress).await
}

#[tokio::test]
async fn searches_reads_answers_and_checks_citations() {
    let s =
        setup("They proposed the Transformer, built only on attention [1]. It replaced RNNs [2].")
            .await;
    let sink = MemorySink::default();
    let r = ask(&s, &sink).await.unwrap();
    assert_eq!(r.sources.len(), 2, "the 404 page is not a source");
    assert_eq!(r.sources[0].title, "Attention Is All You Need");
    assert!(r.sources.iter().all(|x| x.cited));
    assert!(r.invalid_citations.is_empty());
    assert_eq!(r.unread.len(), 1);
    assert!(r.unread[0].reason.contains("404"));
    assert_eq!(r.usage.total(), 960);
    assert_eq!(r.search_service, "SearXNG");
    assert_eq!(r.reader_icon, "tern-crawl");

    let kinds: Vec<String> = sink.events().into_iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            "ask.started",
            "search.done",
            "source.read",
            "source.failed",
            "source.read",
            "model.replied",
            "ask.finished"
        ]
    );

    // The model saw numbered sources it can cite.
    let reqs = s.received_requests().await.unwrap();
    let chat = reqs
        .iter()
        .find(|r| r.url.path() == "/v1/chat/completions")
        .unwrap();
    let body: serde_json::Value = chat.body_json().unwrap();
    let prompt = body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(prompt.contains("[1] Attention Is All You Need"));
    assert!(prompt.contains("[2] A blog"));
}

#[tokio::test]
async fn citations_to_unread_sources_are_reported() {
    let s = setup("The Transformer [1]. Something else [7].").await;
    let r = ask(&s, &MemorySink::default()).await.unwrap();
    assert_eq!(r.invalid_citations, [7]);
    assert!(r.sources[0].cited);
    assert!(!r.sources[1].cited);
}

#[tokio::test]
async fn nothing_readable_fails_plainly() {
    let s = MockServer::start().await;
    Mock::given(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"title": "Gone", "url": format!("{}/gone", s.uri()), "content": "x"}
        ]})))
        .mount(&s)
        .await;
    let err = ask(&s, &MemorySink::default()).await.unwrap_err();
    assert!(err.contains("Could not read any"), "{err}");
}
