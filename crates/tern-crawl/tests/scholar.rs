use std::time::Duration;

use tern_crawl::{ApiRequest, ScholarError, ScholarHttp};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http() -> ScholarHttp {
    ScholarHttp::new().with_backoff_unit(Duration::from_millis(5))
}

#[tokio::test]
async fn a_rate_limit_is_retried_then_cached() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/graph/v1/paper/search"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
        .up_to_n_times(1)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/graph/v1/paper/search"))
        .and(header("x-api-key", "k-1"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"data\": []}"))
        .mount(&s)
        .await;
    let h = http();
    let url = format!("{}/graph/v1/paper/search?query=rag", s.uri());
    let req = ApiRequest::get("Semantic Scholar", &url, Duration::ZERO).header("x-api-key", "k-1");
    let a = h.get(&req).await.unwrap();
    assert_eq!(a.body, "{\"data\": []}");
    assert_eq!(a.tries, 2);
    assert!(!a.cached);
    let b = h.get(&req).await.unwrap();
    assert!(b.cached);
    assert_eq!(s.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn bad_keys_and_missing_pages_are_not_retried() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v3/search/works"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&s)
        .await;
    let h = http();
    let req = ApiRequest::get(
        "CORE",
        format!("{}/v3/search/works?q=x", s.uri()),
        Duration::ZERO,
    );
    assert_eq!(
        h.get(&req).await.unwrap_err(),
        ScholarError::BadKey("CORE".into(), 401)
    );
    assert_eq!(s.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_service_that_keeps_refusing_gives_up() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&s)
        .await;
    let h = http();
    let req = ApiRequest::get("arXiv", format!("{}/api/query", s.uri()), Duration::ZERO);
    let e = h.get(&req).await.unwrap_err();
    assert!(e.transient());
    assert_eq!(s.received_requests().await.unwrap().len(), 4);
}
