//! The built-in browser's engine (`docs/02-ui-definition.md`, built-in
//! browser): pages open as clean, readable Markdown through Tern's reader,
//! with links kept, so a source can be checked without leaving the app.
//! It never runs a page's scripts, and the same network guard as the
//! agent's reads applies (no private addresses).

use tern_crawl::Fetcher;

use crate::sources::PageRef;

/// A page as the browser shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowsedPage {
    /// Title, final address, site, and icon, as source rows show them.
    pub page: PageRef,
    pub markdown: String,
    /// Links in the main text: (text, url).
    pub links: Vec<(String, String)>,
    /// The page was longer than the reader keeps.
    pub truncated: bool,
}

/// Most characters of a page attached to the composer.
const ATTACH_CHARS: usize = 60_000;

impl BrowsedPage {
    /// The page as a composer attachment, so the next message can ask
    /// about it.
    pub fn attachment(&self) -> crate::attach::Attachment {
        let text: String = self.markdown.chars().take(ATTACH_CHARS).collect();
        let cut = self.truncated || text.len() < self.markdown.len();
        crate::attach::Attachment {
            name: self.page.label().to_owned(),
            source: crate::attach::Source::Link,
            role: crate::attach::Role::Context,
            files: vec![crate::attach::FileText {
                path: self.page.url.clone(),
                text,
                cut,
            }],
            skipped: Vec::new(),
            images: Vec::new(),
        }
    }
}

/// Turns what was typed in the address bar into an address: adds
/// `https://` to bare hosts. `None` for text that is not an address.
pub fn normalize(input: &str) -> Option<String> {
    let t = input.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return None;
    }
    let candidate = if t.contains("://") {
        t.to_owned()
    } else if t.contains('.') || t.starts_with("localhost") {
        format!("https://{t}")
    } else {
        return None;
    };
    let u = url::Url::parse(&candidate).ok()?;
    matches!(u.scheme(), "http" | "https")
        .then_some(())
        .and(u.host_str())?;
    Some(u.to_string())
}

/// How a search typed in the address bar is kept in the browser's
/// history: `tern-search:` and the words.
pub const SEARCH_SCHEME: &str = "tern-search:";

/// What the address bar was given: a page to open, or words to search for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Page(String),
    Search(String),
}

/// An address opens as a page; anything else (`mistral le chat`) is a
/// search with the chat's search service. `None` only for blank input.
pub fn target(input: &str) -> Option<Target> {
    if let Some(q) = search_query(input.trim()) {
        return Some(Target::Search(q.to_owned()));
    }
    if let Some(url) = normalize(input) {
        return Some(Target::Page(url));
    }
    let q = input.split_whitespace().collect::<Vec<_>>().join(" ");
    (!q.is_empty()).then_some(Target::Search(q))
}

/// The history address of a search.
pub fn search_address(query: &str) -> String {
    format!("{SEARCH_SCHEME}{query}")
}

/// The words of a search address, if it is one.
pub fn search_query(address: &str) -> Option<&str> {
    address
        .strip_prefix(SEARCH_SCHEME)
        .map(str::trim)
        .filter(|q| !q.is_empty())
}

/// What the address bar shows: the words for a search, else the address.
pub fn display_address(address: &str) -> String {
    search_query(address).unwrap_or(address).to_owned()
}

/// A results page for the reader: each hit as a link with its site and
/// snippet, numbered, so a click opens it in the reader.
pub fn results_page(query: &str, service: &str, hits: &[tern_crawl::SearchHit]) -> BrowsedPage {
    let mut md = format!("# Search: {query}\n\n");
    if hits.is_empty() {
        md.push_str(&format!(
            "{service} found nothing for these words. Try other words, or type a web address.\n"
        ));
    } else {
        md.push_str(&format!(
            "{} result{} from {service}. Pick one to open it in the reader.\n\n",
            hits.len(),
            if hits.len() == 1 { "" } else { "s" }
        ));
    }
    let mut links = Vec::new();
    for (i, h) in hits.iter().enumerate() {
        let title = if h.title.trim().is_empty() {
            h.url.as_str()
        } else {
            h.title.trim()
        };
        md.push_str(&format!("{}. [{}]({}) · {}\n", i + 1, title, h.url, h.site));
        if !h.snippet.trim().is_empty() {
            md.push_str(&format!("   {}\n", h.snippet.trim()));
        }
        md.push('\n');
        links.push((title.to_owned(), h.url.clone()));
    }
    BrowsedPage {
        page: PageRef {
            title: format!("Search: {query}"),
            url: search_address(query),
            site: service.to_owned(),
            builtin: None,
            favicon: None,
        },
        markdown: md,
        links,
        truncated: false,
    }
}

/// Runs a search typed in the address bar with the saved search choice
/// (the keyless default when none is set up).
pub async fn search(query: &str) -> Result<BrowsedPage, String> {
    let prefs = crate::Preferences::load();
    if prefs
        .search
        .as_deref()
        .is_some_and(|s| s == "off" || s == "none")
    {
        return Err(
            "Web search is off. Turn it on in Providers › Web search, or type a web address."
                .into(),
        );
    }
    let keychain = crate::providers::keychain();
    let secrets = keychain.as_ref().ok().map(|b| b.as_ref());
    let client = crate::search::connect(
        prefs.effective_search().as_deref(),
        secrets,
        prefs.search_url.as_deref(),
    )?;
    let hits = client
        .search(query, 10)
        .await
        .map_err(|e| format!("Search failed: {e}"))?;
    Ok(results_page(query, client.spec().name, &hits))
}

/// Opens one page through the reader.
pub async fn open_with(fetcher: &Fetcher, url: &str) -> Result<BrowsedPage, String> {
    // The user opened this page: a person reading, not a crawl.
    let p = fetcher
        .fetch_for_user(url)
        .await
        .map_err(|e| e.to_string())?;
    Ok(BrowsedPage {
        page: PageRef {
            title: p.title,
            url: p.final_url,
            site: p.site,
            builtin: p.icon.builtin.map(str::to_owned),
            favicon: p.icon.favicon,
        },
        markdown: p.markdown,
        links: p.links.into_iter().map(|a| (a.text, a.url)).collect(),
        truncated: p.truncated,
    })
}

/// Opens one page with the standard network guard, or runs a search
/// when `url` is a search address.
pub async fn open(url: &str) -> Result<BrowsedPage, String> {
    if let Some(q) = search_query(url) {
        return search(q).await;
    }
    open_with(&crate::web::shared_reader(), url).await
}

/// Back and forward through visited addresses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    stack: Vec<String>,
    at: usize,
}

impl History {
    pub fn current(&self) -> Option<&str> {
        self.stack.get(self.at).map(String::as_str)
    }

    /// A new page: drops anything forward of here. Visiting the current
    /// address again is a reload, not a new entry.
    pub fn visit(&mut self, url: &str) {
        if self.current() == Some(url) {
            return;
        }
        if !self.stack.is_empty() {
            self.stack.truncate(self.at + 1);
        }
        self.stack.push(url.to_owned());
        self.at = self.stack.len() - 1;
    }

    /// A redirect: the current entry becomes its final address.
    pub fn replace(&mut self, url: &str) {
        match self.stack.get_mut(self.at) {
            Some(cur) => *cur = url.to_owned(),
            None => self.visit(url),
        }
    }

    pub fn can_back(&self) -> bool {
        self.at > 0
    }

    pub fn can_forward(&self) -> bool {
        self.at + 1 < self.stack.len()
    }

    pub fn back(&mut self) -> Option<&str> {
        if !self.can_back() {
            return None;
        }
        self.at -= 1;
        self.current()
    }

    pub fn forward(&mut self) -> Option<&str> {
        if !self.can_forward() {
            return None;
        }
        self.at += 1;
        self.current()
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    #[test]
    fn results_read_as_links_with_their_sites() {
        let u = url::Url::parse("https://mistral.ai/news/le-chat").unwrap();
        let hit = tern_crawl::SearchHit {
            title: "Le Chat".into(),
            url: u.to_string(),
            snippet: "Mistral's assistant".into(),
            site: "mistral.ai".into(),
            icon: tern_crawl::SourceIcon::for_url(&u),
        };
        let p = results_page("mistral le chat", "Wikipedia", &[hit]);
        assert_eq!(p.page.url, "tern-search:mistral le chat");
        assert!(
            p.markdown
                .contains("1. [Le Chat](https://mistral.ai/news/le-chat) · mistral.ai")
        );
        assert!(p.markdown.contains("Mistral's assistant"));
        assert_eq!(p.links.len(), 1);
        let none = results_page("zzz", "Wikipedia", &[]);
        assert!(none.markdown.contains("found nothing"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_from_the_bar() {
        assert_eq!(
            normalize("arxiv.org/abs/1706.03762").as_deref(),
            Some("https://arxiv.org/abs/1706.03762")
        );
        assert_eq!(
            normalize(" http://example.org ").as_deref(),
            Some("http://example.org/")
        );
        assert_eq!(normalize("what is rag"), None);
        assert_eq!(
            target("mistral le chat"),
            Some(Target::Search("mistral le chat".into()))
        );
        assert_eq!(
            target("transformers"),
            Some(Target::Search("transformers".into()))
        );
        assert_eq!(
            target("arxiv.org/abs/1706.03762"),
            Some(Target::Page("https://arxiv.org/abs/1706.03762".into()))
        );
        assert_eq!(target("   "), None);
        let a = search_address("what is rag");
        assert_eq!(target(&a), Some(Target::Search("what is rag".into())));
        assert_eq!(display_address(&a), "what is rag");
        assert_eq!(display_address("https://x.org/"), "https://x.org/");
        assert_eq!(normalize("transformers"), None);
        assert_eq!(normalize("file:///etc/passwd"), None);
        assert_eq!(normalize("ftp://x.org"), None);
        assert_eq!(normalize(""), None);
    }

    #[test]
    fn back_and_forward() {
        let mut h = History::default();
        assert!(!h.can_back() && !h.can_forward() && h.current().is_none());
        h.visit("a");
        h.visit("b");
        h.visit("b");
        h.visit("c");
        assert_eq!(h.back(), Some("b"));
        assert_eq!(h.back(), Some("a"));
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), Some("b"));
        h.visit("d");
        assert!(!h.can_forward());
        assert_eq!(h.back(), Some("b"));
        h.replace("b2");
        assert_eq!(h.current(), Some("b2"));
        assert_eq!(h.forward(), Some("d"));
    }

    #[tokio::test]
    async fn opens_a_page_as_markdown() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                "<title>Doc</title><main><h2>Hi</h2><p>See <a href=\"/b\">B</a>.</p></main>",
                "text/html",
            ))
            .mount(&server)
            .await;
        let page = open_with(&Fetcher::allowing_private(), &server.uri())
            .await
            .unwrap();
        assert_eq!(page.page.title, "Doc");
        assert!(page.markdown.starts_with("## Hi"));
        let a = page.attachment();
        assert_eq!(
            (a.name.as_str(), a.files[0].path.as_str()),
            ("Doc", page.page.url.as_str())
        );
        assert_eq!(
            page.links,
            [("B".to_owned(), format!("{}/b", server.uri()))]
        );
    }
}
