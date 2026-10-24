//! Web search and the guarded reader.
//!
//! Only this crate fetches arbitrary URLs. Every fetch goes through the
//! network guard in [`guard`]: http and https only, no credentials in URLs,
//! no private, loopback, or link-local addresses (unless a local service was
//! asked for by name), redirects re-checked hop by hop, and a size cap.
//!
//! Automated reads keep to each site's robots.txt and are paced per host
//! (its Crawl-delay, else a short default); pages are cached for the
//! session and, with a cache folder, for a day. The reader keeps the main
//! text Readability-style and drops page furniture.
//!
//! Every page and search hit carries its site and a [`SourceIcon`], so the
//! TUI and the desktop app can show the service's logo or the site's favicon
//! beside it.

#![forbid(unsafe_code)]

pub mod cache;
pub mod extract;
pub mod fetch;
pub mod github;
pub mod guard;
pub mod icon;
pub mod pdf;
pub mod remote;
pub mod robots;
pub mod search;

pub use extract::Anchor;
pub use fetch::{FetchError, Fetcher, Page};
pub use icon::{SourceIcon, TERN_CRAWL, TERN_READER, site_of};
pub use pdf::{PdfError, PdfText};
pub use remote::{RemoteKind, RemoteReader};
pub use robots::Robots;
pub use search::{
    KEYLESS_SEARCH, SEARCH_CATALOG, SearchClient, SearchError, SearchHit, SearchSpec, find_search,
};
