//! Search service keys: list, add (tested first), test, remove. Same rules as
//! model provider keys: the key lives only in the OS keychain.

use serde::Serialize;
use tern_core::SecretString;
use tern_crawl::{SEARCH_CATALOG, SearchClient, SearchError, SearchSpec, find_search};

use crate::providers::SecretStore;

pub fn key_name(spec: &SearchSpec) -> String {
    format!("search.{}", spec.id)
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchRow {
    pub id: &'static str,
    pub name: &'static str,
    pub self_hosted: bool,
    pub key_saved: bool,
    pub key_url: &'static str,
}

pub fn list(secrets: Option<&dyn SecretStore>) -> Vec<SearchRow> {
    SEARCH_CATALOG
        .iter()
        .map(|s| SearchRow {
            id: s.id,
            name: s.name,
            self_hosted: !s.needs_key,
            key_saved: secrets.is_some_and(|k| matches!(k.get(&key_name(s)), Ok(Some(_)))),
            key_url: s.key_url,
        })
        .collect()
}

fn spec(id: &str) -> Result<&'static SearchSpec, String> {
    find_search(id).ok_or_else(|| SearchError::Unknown(id.to_owned()).to_string())
}

/// A ready client. With no id, the first service with a saved key.
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
            .ok_or_else(|| {
                "No web search is set up yet. Add a key with `tern search add tavily` (or `brave`), or point Tern at your own SearXNG with `--search searxng`.".to_owned()
            })?,
    };
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
        let err = connect(None, Some(&store), None).unwrap_err();
        assert!(err.contains("tern search add tavily"), "{err}");
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
}
