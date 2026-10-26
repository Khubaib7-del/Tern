//! Search service keys: list, add (tested first), test, remove. Same rules as
//! model provider keys: the key lives only in the OS keychain.

use serde::Serialize;
use tern_core::SecretString;
use tern_crawl::{
    KEYLESS_SEARCH, SEARCH_CATALOG, SearchClient, SearchError, SearchSpec, find_search,
};

use crate::providers::SecretStore;

pub fn key_name(spec: &SearchSpec) -> String {
    format!("search.{}", spec.id)
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchRow {
    pub id: &'static str,
    pub name: &'static str,
    pub self_hosted: bool,
    /// Needs no key and no server of the user's own (Wikipedia).
    pub keyless: bool,
    pub key_saved: bool,
    pub key_url: &'static str,
}

pub fn list(secrets: Option<&dyn SecretStore>) -> Vec<SearchRow> {
    SEARCH_CATALOG
        .iter()
        .map(|s| SearchRow {
            id: s.id,
            name: s.name,
            self_hosted: s.self_hosted,
            keyless: !s.needs_key && !s.self_hosted,
            key_saved: secrets.is_some_and(|k| matches!(k.get(&key_name(s)), Ok(Some(_)))),
            key_url: s.key_url,
        })
        .collect()
}

fn spec(id: &str) -> Result<&'static SearchSpec, String> {
    find_search(id).ok_or_else(|| SearchError::Unknown(id.to_owned()).to_string())
}

/// Where the keyless Wikipedia search goes instead of en.wikipedia.org:
/// set only by tests and the UI tour's mock server.
static WIKIPEDIA_BASE: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// Points Wikipedia searches at `base` (tests and the UI tour), or back at
/// en.wikipedia.org with `None`.
pub fn set_wikipedia_base(base: Option<String>) {
    if let Ok(mut b) = WIKIPEDIA_BASE.write() {
        *b = base;
    }
}

/// A ready client. With no id, the first service with a saved key, else
/// the keyless default ([`KEYLESS_SEARCH`]). A base URL only applies to a
/// self-hosted service.
pub fn connect(
    id: Option<&str>,
    secrets: Option<&dyn SecretStore>,
    base_url: Option<&str>,
) -> Result<SearchClient, String> {
    let spec = match id {
        Some(id) => spec(id)?,
        None => SEARCH_CATALOG
            .iter()
            .filter(|s| s.needs_key)
            .find(|s| secrets.is_some_and(|k| matches!(k.get(&key_name(s)), Ok(Some(_)))))
            .or_else(|| find_search(KEYLESS_SEARCH))
            .ok_or_else(|| {
                "No web search is set up yet. Add a key with `tern search add tavily` (or `brave`), or point Tern at your own SearXNG with `--search searxng`.".to_owned()
            })?,
    };
    let wiki = (spec.id == KEYLESS_SEARCH)
        .then(|| WIKIPEDIA_BASE.read().ok().and_then(|b| b.clone()))
        .flatten();
    let base_url = base_url
        .map(str::trim)
        .filter(|u| spec.self_hosted && !u.is_empty())
        .or(wiki.as_deref());
    let key = if spec.needs_key {
        match secrets {
            Some(s) => s
                .get(&key_name(spec))
                .map_err(|e| format!("The keychain refused: {e}"))?,
            None => None,
        }
    } else {
        None
    };
    SearchClient::new(spec, key, base_url).map_err(|e| e.to_string())
}

/// Runs one real query with the key and saves it only if it works.
pub async fn add(
    id: &str,
    key: SecretString,
    secrets: &dyn SecretStore,
    base_url: Option<&str>,
) -> Result<usize, String> {
    let spec = spec(id)?;
    if !spec.needs_key {
        return Err(format!(
            "{} is self-hosted and needs no key. Use `tern search test {}`.",
            spec.name, spec.id
        ));
    }
    let key = SecretString::new(key.expose().trim());
    if key.is_empty() {
        return Err("The key was empty, so nothing was saved.".into());
    }
    let n = SearchClient::new(spec, Some(key.clone()), base_url)
        .map_err(|e| e.to_string())?
        .search("Tern search key check", 1)
        .await
        .map_err(|e| e.to_string())?
        .len();
    secrets
        .set(&key_name(spec), &key)
        .map_err(|e| format!("The keychain refused: {e}"))?;
    Ok(n)
}

pub async fn test(
    id: &str,
    secrets: Option<&dyn SecretStore>,
    base_url: Option<&str>,
) -> Result<usize, String> {
    Ok(connect(Some(id), secrets, base_url)?
        .search("Tern search check", 3)
        .await
        .map_err(|e| e.to_string())?
        .len())
}

/// One real query before a deep run spends a plan call: `Err` says, in the
/// service's own words, why web search is not working.
pub async fn preflight(client: &SearchClient, query: &str) -> Result<usize, String> {
    let q = if query.trim().is_empty() {
        "Tern search check"
    } else {
        query
    };
    client
        .search(q, 1)
        .await
        .map(|h| h.len())
        .map_err(|e| e.to_string())
}

pub fn remove(id: &str, secrets: &dyn SecretStore) -> Result<bool, String> {
    secrets
        .delete(&key_name(spec(id)?))
        .map_err(|e| format!("The keychain refused: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_platform::MemorySecretStore;

    #[test]
    fn connect_picks_the_first_saved_key_or_explains() {
        let store = MemorySecretStore::default();
        // Nothing saved: the keyless default.
        assert_eq!(
            connect(None, Some(&store), None).unwrap().spec().id,
            KEYLESS_SEARCH
        );
        store.set("search.brave", &SecretString::new("k")).unwrap();
        assert_eq!(
            connect(None, Some(&store), None).unwrap().spec().id,
            "brave"
        );
        assert!(
            list(Some(&store))
                .iter()
                .any(|r| r.id == "brave" && r.key_saved)
        );
        assert!(connect(Some("searxng"), None, None).is_ok());
        assert!(remove("brave", &store).unwrap());
    }

    #[test]
    fn a_server_address_only_moves_a_self_hosted_service() {
        let c = connect(Some("searxng"), None, Some("http://10.0.0.5:8888/")).unwrap();
        assert_eq!(c.base(), "http://10.0.0.5:8888");
        let c = connect(Some("searxng"), None, Some("  ")).unwrap();
        assert_eq!(c.base(), "http://localhost:8080");
        let c = connect(Some("wikipedia"), None, Some("http://10.0.0.5:8888")).unwrap();
        assert_eq!(c.base(), "https://en.wikipedia.org");
        let rows = list(None);
        let w = rows.iter().find(|r| r.id == "wikipedia").unwrap();
        assert!(w.keyless && !w.self_hosted);
        let x = rows.iter().find(|r| r.id == "searxng").unwrap();
        assert!(x.self_hosted && !x.keyless);
    }

    #[tokio::test]
    async fn preflight_reports_the_service_error() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let at = format!("http://127.0.0.1:{port}");
        let c = connect(Some("searxng"), None, Some(&at)).unwrap();
        let e = preflight(&c, "what is a gpu").await.unwrap_err();
        assert!(e.contains(&at) && e.contains("did not answer"), "{e}");
    }
}
