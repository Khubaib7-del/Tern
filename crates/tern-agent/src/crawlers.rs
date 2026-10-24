//! Which reader `read_url` uses: Tern Crawl (built in, the default),
//! Firecrawl or Jina Reader (hosted readers that render JavaScript), or a
//! plain fetch. Chosen in Settings › Search & Crawler and set process-wide
//! from the preferences, so every chat reads the same way.
//!
//! The HTTP lives in `tern_crawl` (Tern Crawl and `tern_crawl::remote`);
//! this module only picks one, keeps its key in the keychain under
//! `crawler.<id>`, and falls back to Tern Crawl when a hosted reader fails.

use std::sync::{OnceLock, RwLock};

use tern_core::SecretString;
use tern_crawl::{Fetcher, Page, RemoteKind, RemoteReader};

use crate::providers::SecretStore;

/// One reader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Crawler {
    /// Tern's own reader: robots.txt, per-site pacing, a day's cache,
    /// Readability-style main text, PDFs.
    #[default]
    Tern,
    /// Firecrawl's hosted scraper (needs a key).
    Firecrawl,
    /// Jina Reader, `r.jina.ai` (a key is optional).
    Jina,
    /// One guarded request per page with no shared disk cache.
    Fetch,
}

impl Crawler {
    pub const ALL: [Self; 4] = [Self::Tern, Self::Firecrawl, Self::Jina, Self::Fetch];

    pub fn id(self) -> &'static str {
        match self {
            Self::Tern => "tern",
            Self::Firecrawl => "firecrawl",
            Self::Jina => "jina",
            Self::Fetch => "fetch",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Tern => "Tern Crawl",
            Self::Firecrawl => "Firecrawl",
            Self::Jina => "Jina Reader",
            Self::Fetch => "Plain fetch",
        }
    }

    /// One line on what it does, for its row.
    pub fn blurb(self) -> &'static str {
        match self {
            Self::Tern => {
                "Built in. Reads pages on this computer: keeps to robots.txt, paces each site, caches for a day, keeps the main text, reads PDFs."
            }
            Self::Firecrawl => {
                "Hosted. Renders JavaScript-heavy pages on Firecrawl's servers and returns clean Markdown. Needs an API key."
            }
            Self::Jina => {
                "Hosted. r.jina.ai turns any page into Markdown; works without a key at a low rate, a key lifts the limit."
            }
            Self::Fetch => {
                "One guarded request per page from this computer, no shared cache. For checking what a site serves."
            }
        }
    }

    /// Where the user gets a key, for hosted readers.
    pub fn key_url(self) -> &'static str {
        match self {
            Self::Firecrawl => "https://www.firecrawl.dev/app/api-keys",
            Self::Jina => "https://jina.ai/api-dashboard",
            _ => "",
        }
    }

    pub fn needs_key(self) -> bool {
        matches!(self, Self::Firecrawl)
    }

    pub fn takes_key(self) -> bool {
        matches!(self, Self::Firecrawl | Self::Jina)
    }

    /// Whether the page link leaves this computer.
    pub fn hosted(self) -> bool {
        self.takes_key()
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|c| c.id().eq_ignore_ascii_case(id.trim()))
    }

    fn remote(self) -> Option<RemoteKind> {
        match self {
            Self::Firecrawl => Some(RemoteKind::Firecrawl),
            Self::Jina => Some(RemoteKind::Jina),
            _ => None,
        }
    }
}

/// The keychain name for a reader's key.
pub fn key_name(c: Crawler) -> String {
    format!("crawler.{}", c.id())
}

/// The reader to use: the saved default when it is switched on and usable
/// (a key saved when it needs one), else Tern Crawl. Tern Crawl cannot be
/// switched off: it is what every other reader falls back to.
pub fn pick(saved: Option<&str>, off: &[String], has_key: impl Fn(Crawler) -> bool) -> Crawler {
    let Some(c) = saved.and_then(Crawler::parse) else {
        return Crawler::Tern;
    };
    let on = c == Crawler::Tern || !off.iter().any(|o| o.eq_ignore_ascii_case(c.id()));
    if on && (!c.needs_key() || has_key(c)) {
        c
    } else {
        Crawler::Tern
    }
}

fn current() -> &'static RwLock<Crawler> {
    static C: OnceLock<RwLock<Crawler>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Sets the reader every `read_url` uses from now on.
pub fn set(c: Crawler) {
    *current().write().unwrap_or_else(|e| e.into_inner()) = c;
}

/// The reader in use.
pub fn active() -> Crawler {
    *current().read().unwrap_or_else(|e| e.into_inner())
}

fn plain_reader() -> Fetcher {
    static R: OnceLock<Fetcher> = OnceLock::new();
    R.get_or_init(Fetcher::new).clone()
}

/// The saved key for `crawler`, if any.
pub fn saved_key(c: Crawler, secrets: Option<&dyn SecretStore>) -> Option<SecretString> {
    secrets.and_then(|s| s.get(&key_name(c)).ok().flatten())
}

/// How a page was read, for the run report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Read {
    pub page: Page,
    /// The reader that produced the page.
    pub reader: Crawler,
    /// Why the chosen reader was not used, when Tern Crawl stood in.
    pub fell_back: Option<String>,
}

/// Reads `url` with `crawler`; a hosted reader that fails (no key, an
/// error, a page it cannot read) falls back to Tern Crawl (`tern`), and
/// the reason is kept.
pub async fn read(
    crawler: Crawler,
    url: &str,
    tern: &Fetcher,
    secrets: Option<&dyn SecretStore>,
) -> Result<Read, tern_crawl::FetchError> {
    match crawler {
        Crawler::Tern => Ok(Read {
            page: tern.fetch(url).await?,
            reader: Crawler::Tern,
            fell_back: None,
        }),
        Crawler::Fetch => Ok(Read {
            page: plain_reader().fetch(url).await?,
            reader: Crawler::Fetch,
            fell_back: None,
        }),
        Crawler::Firecrawl | Crawler::Jina => {
            let kind = crawler.remote().unwrap_or(RemoteKind::Jina);
            let tried = match RemoteReader::new(kind, saved_key(crawler, secrets)) {
                Ok(r) => r.read(url).await,
                Err(e) => Err(e),
            };
            match tried {
                Ok(page) => Ok(Read {
                    page,
                    reader: crawler,
                    fell_back: None,
                }),
                // A blocked link is blocked for every reader.
                Err(e @ tern_crawl::FetchError::Blocked(_)) => Err(e),
                Err(e) => Ok(Read {
                    page: tern.fetch(url).await?,
                    reader: Crawler::Tern,
                    fell_back: Some(format!("{}: {e}", crawler.name())),
                }),
            }
        }
    }
}

/// Checks a hosted reader with `key` by reading a small public page;
/// returns a one-line result for its row.
pub async fn test(crawler: Crawler, key: Option<SecretString>) -> Result<String, String> {
    const PAGE: &str = "https://example.com/";
    match crawler.remote() {
        None => {
            let f = if crawler == Crawler::Fetch {
                plain_reader()
            } else {
                crate::web::shared_reader()
            };
            f.fetch(PAGE)
                .await
                .map(|p| format!("Works · read “{}”", p.title))
                .map_err(|e| e.to_string())
        }
        Some(kind) => {
            let r = RemoteReader::new(kind, key).map_err(|e| e.to_string())?;
            r.read(PAGE)
                .await
                .map(|p| format!("Works · read “{}” through {}", p.title, crawler.name()))
                .map_err(|e| e.to_string())
        }
    }
}

/// Tests `key` for a hosted reader and saves it only if the test passes.
pub async fn add_key(
    crawler: Crawler,
    key: SecretString,
    secrets: &dyn SecretStore,
) -> Result<String, String> {
    let key = SecretString::new(key.expose().trim());
    if key.is_empty() {
        return Err("The key was empty, so nothing was saved.".into());
    }
    let ok = test(crawler, Some(key.clone())).await?;
    secrets
        .set(&key_name(crawler), &key)
        .map_err(|e| format!("The keychain refused: {e}"))?;
    Ok(ok)
}

/// Whether a key is saved for `crawler`.
pub fn has_key(crawler: Crawler, secrets: Option<&dyn SecretStore>) -> bool {
    saved_key(crawler, secrets).is_some()
}

/// Deletes a hosted reader's key.
pub fn remove_key(crawler: Crawler, secrets: &dyn SecretStore) -> Result<bool, String> {
    secrets
        .delete(&key_name(crawler))
        .map_err(|e| format!("The keychain refused: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_tern_crawl_and_unusable_choices_fall_back() {
        let none = |_| false;
        let all = |_| true;
        assert_eq!(pick(None, &[], all), Crawler::Tern);
        assert_eq!(pick(Some("nope"), &[], all), Crawler::Tern);
        assert_eq!(pick(Some("jina"), &[], none), Crawler::Jina, "Jina needs no key");
        assert_eq!(pick(Some("firecrawl"), &[], none), Crawler::Tern, "no key yet");
        assert_eq!(pick(Some("firecrawl"), &[], all), Crawler::Firecrawl);
        assert_eq!(
            pick(Some("Firecrawl"), &["firecrawl".into()], all),
            Crawler::Tern,
            "switched off"
        );
        assert_eq!(pick(Some("tern"), &["tern".into()], all), Crawler::Tern);
        assert_eq!(pick(Some("fetch"), &[], none), Crawler::Fetch);
    }

    #[test]
    fn rows_have_what_the_screen_needs() {
        for c in Crawler::ALL {
            assert_eq!(Crawler::parse(c.id()), Some(c));
            assert!(!c.name().is_empty() && !c.blurb().is_empty());
            assert_eq!(c.takes_key(), !c.key_url().is_empty());
            assert!(key_name(c).starts_with("crawler."));
        }
        assert!(Crawler::Firecrawl.needs_key() && !Crawler::Jina.needs_key());
        assert!(Crawler::Jina.hosted() && !Crawler::Tern.hosted());
    }

    #[test]
    fn the_active_reader_is_process_wide() {
        set(Crawler::Jina);
        assert_eq!(active(), Crawler::Jina);
        set(Crawler::Tern);
        assert_eq!(active(), Crawler::Tern);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_hosted_reader_without_a_key_falls_back_to_tern_crawl() {
        // Firecrawl has no key here, so Tern Crawl reads instead; the page
        // is a blocked private address, so both refuse and the error says so.
        let tern = Fetcher::new();
        let err = read(Crawler::Firecrawl, "http://127.0.0.1:9/x", &tern, None).await;
        assert!(err.is_err());
        let store = tern_platform::MemorySecretStore::default();
        assert!(!has_key(Crawler::Firecrawl, Some(&store)));
        assert!(add_key(Crawler::Firecrawl, SecretString::new("  "), &store).await.is_err());
    }
}
