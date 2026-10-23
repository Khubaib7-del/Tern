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

#[tokio::test]
async fn searxng_with_json_off_says_how_to_turn_it_on() {
    let s = MockServer::start().await;
    Mock::given(path("/search"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&s)
        .await;
    let c = SearchClient::new(find_search("searxng").unwrap(), None, Some(&s.uri())).unwrap();
    let e = c.search("q", 3).await.unwrap_err();
    assert_eq!(e, SearchError::JsonOff(s.uri()));
    let text = e.to_string();
    assert!(text.contains("formats: [html, json]"), "{text}");
    assert!(text.contains(&s.uri()), "{text}");
}

#[tokio::test]
async fn an_address_with_nothing_listening_names_the_address() {
    // Bind and drop a listener so the port is closed.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let at = format!("http://127.0.0.1:{port}");
    let c = SearchClient::new(find_search("searxng").unwrap(), None, Some(&at)).unwrap();
    let e = c.search("q", 3).await.unwrap_err();
    match &e {
        SearchError::Unreachable { name, at: a, .. } => {
            assert_eq!(*name, "SearXNG");
            assert_eq!(a, &at);
        }
        other => panic!("expected Unreachable, got {other:?}"),
    }
    let text = e.to_string();
    assert!(text.contains("Providers › Search"), "{text}");
}

#[tokio::test]
async fn wikipedia_is_keyless_and_links_articles() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/w/api.php"))
        .and(query_param("list", "search"))
        .and(query_param("srsearch", "graphics processing unit"))
        .and(query_param("format", "json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"query": {"search": [
            {"title": "Graphics processing unit", "snippet": "A <span class=\"searchmatch\">GPU</span> is a specialized circuit"},
            {"title": "General-purpose computing on graphics processing units", "snippet": "GPGPU"},
            {"title": "", "snippet": "skipped"}
        ]}})))
        .mount(&s)
        .await;
    let c = SearchClient::new(find_search("wikipedia").unwrap(), None, Some(&s.uri())).unwrap();
    let hits = c.search("graphics processing unit", 5).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].title, "Graphics processing unit");
    assert_eq!(
        hits[0].url,
        format!("{}/wiki/Graphics_processing_unit", s.uri())
    );
    assert_eq!(hits[0].snippet, "A GPU is a specialized circuit");
    let req = &s.received_requests().await.unwrap()[0];
    let ua = req.headers.get("user-agent").unwrap().to_str().unwrap();
    assert!(ua.contains("github.com/Khubaib7-del/Tern"), "{ua}");
}

#[tokio::test]
async fn wikipedia_errors_are_reported() {
    let s = MockServer::start().await;
    Mock::given(path("/w/api.php"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"error": {"code": "maxlag", "info": "Waiting for a database server"}}),
        ))
        .mount(&s)
        .await;
    let c = SearchClient::new(find_search("wikipedia").unwrap(), None, Some(&s.uri())).unwrap();
    let e = c.search("q", 3).await.unwrap_err();
    assert!(
        e.to_string().contains("Waiting for a database server"),
        "{e}"
    );
}
