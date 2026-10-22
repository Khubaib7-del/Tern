//! robots.txt, pacing, and the cache, against a local server.

use std::time::{Duration, Instant};

use tern_crawl::Fetcher;
use tern_crawl::fetch::FetchError;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn html(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "text/html")
}

fn page(text: &str) -> String {
    format!(
        "<html><head><title>{text}</title></head><body><article><p>{text}</p></article></body></html>"
    )
}

async fn robots(s: &MockServer, body: &str) {
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "text/plain"),
        )
        .mount(s)
        .await;
}

#[tokio::test]
async fn automated_reads_keep_to_robots_and_people_do_not_have_to() {
    let s = MockServer::start().await;
    robots(&s, "User-agent: *\nDisallow: /private\n").await;
    Mock::given(method("GET"))
        .and(path("/private/notes"))
        .respond_with(html(&page("Private notes")))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/open"))
        .respond_with(html(&page("Open page")))
        .mount(&s)
        .await;
    let f = Fetcher::allowing_private();
    let err = f
        .fetch(&format!("{}/private/notes", s.uri()))
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Robots { .. }), "{err}");
    assert!(err.to_string().contains("robots.txt"));
    assert_eq!(
        f.fetch(&format!("{}/open", s.uri())).await.unwrap().title,
        "Open page"
    );
    // The user opening the page in the browser is not a crawl.
    let p = f
        .fetch_for_user(&format!("{}/private/notes", s.uri()))
        .await
        .unwrap();
    assert_eq!(p.title, "Private notes");
}

#[tokio::test]
async fn a_failing_robots_file_blocks_for_now() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(html(&page("A")))
        .mount(&s)
        .await;
    let err = Fetcher::allowing_private()
        .fetch(&format!("{}/a", s.uri()))
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Robots { .. }));
}

#[tokio::test]
async fn crawl_delay_paces_one_host() {
    let s = MockServer::start().await;
    robots(&s, "User-agent: *\nCrawl-delay: 0.4\n").await;
    for p in ["/one", "/two", "/three"] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(html(&page(p)))
            .mount(&s)
            .await;
    }
    let f = Fetcher::allowing_private();
    let start = Instant::now();
    let (u1, u2, u3) = (
        format!("{}/one", s.uri()),
        format!("{}/two", s.uri()),
        format!("{}/three", s.uri()),
    );
    let (a, b, c) = tokio::join!(f.fetch(&u1), f.fetch(&u2), f.fetch(&u3));
    assert!(a.is_ok() && b.is_ok() && c.is_ok());
    assert!(
        start.elapsed() >= Duration::from_millis(780),
        "three reads with a 0.4 s delay took {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn pages_are_read_once_unless_no_store() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/kept"))
        .respond_with(html(&page("Kept")))
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/fresh"))
        .respond_with(html(&page("Fresh")).insert_header("cache-control", "no-store"))
        .expect(2)
        .mount(&s)
        .await;
    let f = Fetcher::allowing_private();
    let copy = f.clone();
    for _ in 0..2 {
        f.fetch(&format!("{}/kept", s.uri())).await.unwrap();
        f.fetch(&format!("{}/fresh", s.uri())).await.unwrap();
    }
    // Clones share the cache, and a fragment is the same page.
    assert_eq!(
        copy.fetch(&format!("{}/kept#intro", s.uri()))
            .await
            .unwrap()
            .title,
        "Kept"
    );
}

#[tokio::test]
async fn the_disk_cache_survives_a_new_fetcher() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/paper"))
        .respond_with(html(&page("Paper")))
        .expect(1)
        .mount(&s)
        .await;
    let dir = std::env::temp_dir().join(format!("tern-crawl-disk-{}", std::process::id()));
    let url = format!("{}/paper", s.uri());
    Fetcher::allowing_private()
        .with_cache_dir(&dir)
        .fetch(&url)
        .await
        .unwrap();
    let again = Fetcher::allowing_private()
        .with_cache_dir(&dir)
        .fetch(&url)
        .await
        .unwrap();
    assert_eq!(again.title, "Paper");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn reads_pdfs_page_by_page() {
    let s = MockServer::start().await;
    let pdf = tern_crawl::pdf::sample(&[
        "Dense Passage Retrieval for Open Domain QA",
        "Dense retrievers beat BM25 on four of five datasets.",
    ]);
    Mock::given(method("GET"))
        .and(path("/papers/dpr.pdf"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(pdf, "application/pdf"))
        .mount(&s)
        .await;
    let p = Fetcher::allowing_private()
        .fetch(&format!("{}/papers/dpr.pdf", s.uri()))
        .await
        .unwrap();
    assert_eq!(p.title, "Dense Passage Retrieval for Open Domain QA");
    assert_eq!(p.content_type, "application/pdf");
    assert_eq!(p.description.as_deref(), Some("PDF, 2 pages"));
    assert!(
        p.text.contains("[page 2]\nDense retrievers beat BM25"),
        "{}",
        p.text
    );
}
