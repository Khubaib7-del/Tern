//! Provider setup for front ends: add, test, list, and remove keys.
//!
//! Keys go straight from the user to the OS keychain under
//! `provider.<id>`. A key is saved only after the provider accepts it.

use serde::Serialize;
use tern_core::SecretString;
pub use tern_platform::SecretStore;
pub use tern_providers::{
    CATALOG, Capability, Category, Lifecycle, ModelInfo, ModelKind, ProbeReport, ProviderError,
    ProviderSpec,
};
use tern_providers::{Provider, find};

/// One catalog row with whether this machine has a key for it.
#[derive(Clone, Debug, Serialize)]
pub struct ProviderRow {
    pub id: &'static str,
    pub name: &'static str,
    pub local: bool,
    pub key_saved: bool,
    pub key_url: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SetupError {
    UnknownProvider(String),
    NoKeyNeeded(&'static str),
    EmptyKey,
    Provider(ProviderError),
    Keychain(String),
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownProvider(id) => write!(
                f,
                "Tern does not know a provider called `{id}`. Run `tern provider list`."
            ),
            Self::NoKeyNeeded(name) => write!(
                f,
                "{name} runs on this machine and needs no key. Use `tern provider test` instead."
            ),
            Self::EmptyKey => write!(f, "The key was empty, so nothing was saved."),
            Self::Provider(e) => write!(f, "{e}"),
            Self::Keychain(e) => write!(f, "The keychain refused: {e}"),
        }
    }
}

impl std::error::Error for SetupError {}

impl From<ProviderError> for SetupError {
    fn from(e: ProviderError) -> Self {
        Self::Provider(e)
    }
}

pub type SetupResult<T> = Result<T, SetupError>;

/// The OS keychain, or the reason it cannot be used on this machine.
pub fn keychain() -> Result<Box<dyn SecretStore>, String> {
    tern_platform::default_secret_store().map_err(|e| e.to_string())
}

/// Keychain name for a provider's key.
pub fn key_name(spec: &ProviderSpec) -> String {
    format!("provider.{}", spec.id)
}

fn spec(id: &str) -> SetupResult<&'static ProviderSpec> {
    find(id).ok_or_else(|| SetupError::UnknownProvider(id.to_owned()))
}

/// The catalog with key status. A keychain that cannot be read shows no keys.
pub fn list(secrets: Option<&dyn SecretStore>) -> Vec<ProviderRow> {
    CATALOG
        .iter()
        .map(|p| ProviderRow {
            id: p.id,
            name: p.name,
            local: p.local,
            key_saved: secrets.is_some_and(|s| matches!(s.get(&key_name(p)), Ok(Some(_)))),
            key_url: p.key_url,
        })
        .collect()
}

pub fn spec_for(id: &str) -> SetupResult<&'static ProviderSpec> {
    spec(id)
}

fn connect(
    spec: &'static ProviderSpec,
    secrets: Option<&dyn SecretStore>,
    base_url: Option<&str>,
) -> SetupResult<Provider> {
    let key = if spec.needs_key {
        match secrets {
            Some(s) => s
                .get(&key_name(spec))
                .map_err(|e| SetupError::Keychain(e.to_string()))?,
            None => None,
        }
    } else {
        None
    };
    Ok(Provider::new(spec, key, base_url)?)
}

/// A ready provider by id, with its saved key when it needs one.
pub fn connect_by_id(
    id: &str,
    secrets: Option<&dyn SecretStore>,
    base_url: Option<&str>,
) -> SetupResult<Provider> {
    connect(spec(id)?, secrets, base_url)
}

/// Tests `key` against the provider and saves it only if it is accepted.
pub async fn add(
    id: &str,
    key: SecretString,
    secrets: &dyn SecretStore,
    base_url: Option<&str>,
) -> SetupResult<ProbeReport> {
    let spec = spec(id)?;
    if !spec.needs_key {
        return Err(SetupError::NoKeyNeeded(spec.name));
    }
    let key = SecretString::new(key.expose().trim());
    if key.is_empty() {
        return Err(SetupError::EmptyKey);
    }
    let report = Provider::new(spec, Some(key.clone()), base_url)?
        .probe()
        .await?;
    secrets
        .set(&key_name(spec), &key)
        .map_err(|e| SetupError::Keychain(e.to_string()))?;
    Ok(report)
}

/// Checks the saved key (or the local server) by listing models.
pub async fn test(
    id: &str,
    secrets: Option<&dyn SecretStore>,
    base_url: Option<&str>,
) -> SetupResult<ProbeReport> {
    Ok(connect(spec(id)?, secrets, base_url)?.probe().await?)
}

pub async fn models(
    id: &str,
    secrets: Option<&dyn SecretStore>,
    base_url: Option<&str>,
) -> SetupResult<Vec<ModelInfo>> {
    Ok(connect(spec(id)?, secrets, base_url)?
        .list_models_cached()
        .await?)
}

/// [`models`], fetched again even when a recent listing is cached (the
/// picker's Retry).
pub async fn models_fresh(
    id: &str,
    secrets: Option<&dyn SecretStore>,
    base_url: Option<&str>,
) -> SetupResult<Vec<ModelInfo>> {
    let p = connect(spec(id)?, secrets, base_url)?;
    p.forget_cached_models();
    Ok(p.list_models().await?)
}

/// Deletes the saved key. Returns whether one existed.
pub fn remove(id: &str, secrets: &dyn SecretStore) -> SetupResult<bool> {
    let spec = spec(id)?;
    secrets
        .delete(&key_name(spec))
        .map_err(|e| SetupError::Keychain(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_platform::MemorySecretStore;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KEY: &str = "gsk_0123456789abcdefghijABCDEFGHIJ";

    async fn server(status: u16) -> MockServer {
        let s = MockServer::start().await;
        Mock::given(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_json(serde_json::json!({"data": [{"id": "llama-3.3-70b"}]})),
            )
            .mount(&s)
            .await;
        s
    }

    #[tokio::test]
    async fn accepted_key_is_saved_and_listed() {
        let s = server(200).await;
        let base = format!("{}/v1", s.uri());
        let store = MemorySecretStore::default();
        let r = add(
            "groq",
            SecretString::new(format!("  {KEY}\n")),
            &store,
            Some(&base),
        )
        .await
        .unwrap();
        assert_eq!(r.models, 1);
        assert_eq!(store.get("provider.groq").unwrap().unwrap().expose(), KEY);
        let row = list(Some(&store))
            .into_iter()
            .find(|r| r.id == "groq")
            .unwrap();
        assert!(row.key_saved);
        assert_eq!(
            models("groq", Some(&store), Some(&base)).await.unwrap()[0].id,
            "llama-3.3-70b"
        );
        assert!(remove("groq", &store).unwrap());
        assert!(!remove("groq", &store).unwrap());
    }

    #[tokio::test]
    async fn refused_key_is_not_saved() {
        let s = server(401).await;
        let store = MemorySecretStore::default();
        let e = add(
            "groq",
            SecretString::new(KEY),
            &store,
            Some(&format!("{}/v1", s.uri())),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            e,
            SetupError::Provider(ProviderError::Auth { .. })
        ));
        assert!(store.get("provider.groq").unwrap().is_none());
    }

    #[tokio::test]
    async fn plain_errors_for_wrong_input() {
        let store = MemorySecretStore::default();
        assert_eq!(
            add("nope", SecretString::new("x"), &store, None)
                .await
                .unwrap_err(),
            SetupError::UnknownProvider("nope".into())
        );
        assert_eq!(
            add("ollama", SecretString::new("x"), &store, None)
                .await
                .unwrap_err(),
            SetupError::NoKeyNeeded("Ollama")
        );
        assert_eq!(
            add("openai", SecretString::new("  "), &store, None)
                .await
                .unwrap_err(),
            SetupError::EmptyKey
        );
        let e = test("openai", Some(&store), None).await.unwrap_err();
        assert!(e.to_string().contains("tern provider add openai"));
    }
}
