use serde_json::json;
use tern_core::SecretString;
use tern_crawl::{SearchClient, SearchError, find_search};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn key() -> Option<SecretString> {
    Some(SecretString::new("k-123"))
}

#[tokio::test]
async fn tavily_sends_the_key_and_maps_results() {
    let s = MockServer::start().await;
    Mock::given(method("POST")).and(path("/search")).and(header("authorization", "Bearer k-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"title": "Attention Is All You Need", "url": "https://arxiv.org/abs/1706.03762", "content": "We propose the Transformer"},
            {"title": "dup", "url": "https://arxiv.org/abs/1706.03762", "content": ""},
            {"title": "bad", "url": "javascript:alert(1)", "content": ""},
            {"title": "Blog", "url": "https://www.example.org/post", "content": "A post"}
        ]})))
        .mount(&s).await;
    let c = SearchClient::new(find_search("tavily").unwrap(), key(), Some(&s.uri())).unwrap();
    let hits = c.search("transformer", 5).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].site, "arxiv.org");
    assert_eq!(hits[0].icon.builtin, Some("arxiv"));
    assert_eq!(
        hits[1].icon.favicon.as_deref(),
        Some("https://www.example.org/favicon.ico")
    );
    let body: serde_json::Value = s.received_requests().await.unwrap()[0].body_json().unwrap();
    assert_eq!(body["max_results"], 5);
}

#[tokio::test]
async fn brave_uses_its_header_and_shape() {
    let s = MockServer::start().await;
    Mock::given(method("GET")).and(path("/res/v1/web/search")).and(header("x-subscription-token", "k-123")).and(query_param("q", "rust gpui"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"web": {"results": [
            {"title": "GPUI", "url": "https://github.com/zed-industries/zed", "description": "<strong>GPUI</strong> is Zed's UI framework"}
        ]}})))
        .mount(&s).await;
    let c = SearchClient::new(find_search("brave").unwrap(), key(), Some(&s.uri())).unwrap();
    let hits = c.search("rust gpui", 5).await.unwrap();
    assert_eq!(hits[0].snippet, "GPUI is Zed's UI framework");
    assert_eq!(hits[0].icon.builtin, Some("github"));
}

#[tokio::test]
async fn searxng_needs_no_key_and_limits() {
    let s = MockServer::start().await;
    let rows: Vec<_> = (0..10).map(|i| json!({"title": format!("r{i}"), "url": format!("https://site{i}.org/"), "content": "c"})).collect();
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("format", "json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": rows})))
        .mount(&s)
        .await;
    let c = SearchClient::new(find_search("searxng").unwrap(), None, Some(&s.uri())).unwrap();
    assert_eq!(c.search("q", 3).await.unwrap().len(), 3);
}

#[tokio::test]
async fn key_and_rate_errors_are_plain() {
    let s = MockServer::start().await;
    Mock::given(path("/search"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&s)
        .await;
    let c = SearchClient::new(find_search("tavily").unwrap(), key(), Some(&s.uri())).unwrap();
    assert_eq!(
        c.search("q", 3).await.unwrap_err(),
        SearchError::BadKey("Tavily", 401)
    );
    let s2 = MockServer::start().await;
    Mock::given(path("/search"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&s2)
        .await;
    let c = SearchClient::new(find_search("tavily").unwrap(), key(), Some(&s2.uri())).unwrap();
    assert_eq!(
        c.search("q", 3).await.unwrap_err(),
        SearchError::RateLimited("Tavily")
    );
}
