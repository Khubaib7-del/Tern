//! Web search and the guarded reader.
//!
//! Only this crate fetches arbitrary URLs. Every fetch goes through the
//! network guard in [`guard`]: http and https only, no credentials in URLs,
//! no private, loopback, or link-local addresses (unless a local service was
//! asked for by name), redirects re-checked hop by hop, and a size cap.
//!
//! Every page and search hit carries its site and a [`SourceIcon`], so the
//! TUI and the desktop app can show the service's logo or the site's favicon
//! beside it.

#![forbid(unsafe_code)]

pub mod extract;
pub mod fetch;
pub mod guard;
pub mod icon;
pub mod search;

pub use fetch::{FetchError, Fetcher, Page};
pub use icon::{SourceIcon, TERN_READER, site_of};
pub use search::{SEARCH_CATALOG, SearchClient, SearchError, SearchHit, SearchSpec, find_search};
