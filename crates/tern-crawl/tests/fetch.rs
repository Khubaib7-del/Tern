use tern_crawl::Fetcher;
use tern_crawl::fetch::FetchError;
use tern_crawl::guard::GuardError;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn html(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/html; charset=utf-8")
        .set_body_string(body.to_owned())
}

#[tokio::test]
async fn reads_a_page_with_title_icon_and_text() {
    let s = MockServer::start().await;
    Mock::given(method("GET")).and(path("/paper"))
        .respond_with(html(r#"<html><head><title>Paper</title><link rel="icon" href="/i.svg"></head><body><article><p>Main text.</p></article></body></html>"#))
        .mount(&s).await;
    let p = Fetcher::allowing_private()
        .fetch(&format!("{}/paper", s.uri()))
        .await
        .unwrap();
    assert_eq!(p.title, "Paper");
    assert_eq!(p.text, "Main text.");
    assert_eq!(p.icon.favicon, Some(format!("{}/i.svg", s.uri())));
    assert_eq!(p.icon.builtin, None);
    assert_eq!(p.site, "127.0.0.1");
    assert!(!p.truncated);
}

#[tokio::test]
async fn follows_redirects_and_cites_the_final_url() {
    let s = MockServer::start().await;
    Mock::given(path("/old"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/new"))
        .mount(&s)
        .await;
    Mock::given(path("/new"))
        .respond_with(html("<p>moved here</p>"))
        .mount(&s)
        .await;
    let p = Fetcher::allowing_private()
        .fetch(&format!("{}/old", s.uri()))
        .await
        .unwrap();
    assert_eq!(p.final_url, format!("{}/new", s.uri()));
    assert_eq!(p.url, format!("{}/old", s.uri()));
}

#[tokio::test]
async fn redirect_loops_stop() {
    let s = MockServer::start().await;
    Mock::given(path("/a"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/a"))
        .mount(&s)
        .await;
    let e = Fetcher::allowing_private()
        .fetch(&format!("{}/a", s.uri()))
        .await
        .unwrap_err();
    assert!(matches!(e, FetchError::TooManyRedirects { .. }));
}

#[tokio::test]
async fn errors_are_plain() {
    let s = MockServer::start().await;
    Mock::given(path("/gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&s)
        .await;
    Mock::given(path("/pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/pdf")
                .set_body_bytes(b"%PDF".to_vec()),
        )
        .mount(&s)
        .await;
    Mock::given(path("/img"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(vec![1, 2]),
        )
        .mount(&s)
        .await;
    let f = Fetcher::allowing_private();
    let e = f.fetch(&format!("{}/gone", s.uri())).await.unwrap_err();
    assert_eq!(e.to_string(), "127.0.0.1 answered HTTP 404");
    let e = f.fetch(&format!("{}/pdf", s.uri())).await.unwrap_err();
    assert!(e.to_string().contains("PDF reading arrives"), "{e}");
    assert!(matches!(
        f.fetch(&format!("{}/img", s.uri())).await.unwrap_err(),
        FetchError::Unsupported { .. }
    ));
}

#[tokio::test]
async fn big_pages_are_cut_off() {
    let s = MockServer::start().await;
    let big = "a ".repeat(tern_crawl::fetch::MAX_BYTES);
    Mock::given(path("/big"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_body_string(big),
        )
        .mount(&s)
        .await;
    let p = Fetcher::allowing_private()
        .fetch(&format!("{}/big", s.uri()))
        .await
        .unwrap();
    assert!(p.truncated);
    assert_eq!(p.bytes, tern_crawl::fetch::MAX_BYTES);
}

#[tokio::test]
async fn the_open_web_reader_refuses_local_addresses_and_redirects_to_them() {
    let s = MockServer::start().await;
    Mock::given(path("/x"))
        .respond_with(html("<p>local</p>"))
        .mount(&s)
        .await;
    let e = Fetcher::new()
        .fetch(&format!("{}/x", s.uri()))
        .await
        .unwrap_err();
    assert!(
        matches!(e, FetchError::Blocked(GuardError::Private { .. })),
        "{e}"
    );
    let e = Fetcher::new()
        .fetch("file:///etc/passwd")
        .await
        .unwrap_err();
    assert!(matches!(e, FetchError::Blocked(GuardError::Scheme(_))));
}
