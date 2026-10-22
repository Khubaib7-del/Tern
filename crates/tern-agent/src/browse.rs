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

/// Opens one page with the standard network guard.
pub async fn open(url: &str) -> Result<BrowsedPage, String> {
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
