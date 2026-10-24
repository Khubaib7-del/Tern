//! The bridge's Settings jobs from the settings review: page readers
//! (a hosted reader's key, testing a reader) and custom providers (save
//! with a test, delete). Keys go to the keychain only after they work;
//! nothing here runs on the UI thread.

use tern_agent::Preferences;
use tern_agent::crawlers::{self, Crawler};
use tern_agent::providers::{self as setup, CustomProvider, SecretStore};
use tern_core::SecretString;

use super::{Bridge, Msg};
use crate::providers::Kind;

/// What the Settings jobs tell the window.
#[derive(Debug)]
pub enum SetupMsg {
    /// Each page reader and whether a key is saved for it.
    Crawlers(Vec<(Crawler, bool)>),
    /// A reader was tested (and its key saved, when one was given and it
    /// worked).
    CrawlerChecked(Crawler, Result<String, String>),
    /// A custom provider was saved under this id, or why not.
    CustomSaved(Result<&'static str, String>),
    /// A custom provider was deleted, or why not.
    CustomRemoved(String, Result<(), String>),
}

fn send(tx: &tokio::sync::mpsc::UnboundedSender<Msg>, m: SetupMsg) {
    let _ = tx.send(Msg::Setup(Box::new(m)));
}

fn crawler_keys() -> Vec<(Crawler, bool)> {
    let keychain = setup::keychain();
    let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
    Crawler::ALL
        .into_iter()
        .map(|c| (c, c.takes_key() && crawlers::has_key(c, secrets)))
        .collect()
}

impl Bridge {
    /// Which page readers have a key.
    pub fn crawlers_load(&self) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || send(&tx, SetupMsg::Crawlers(crawler_keys())));
    }

    /// Tests a hosted reader with a new key and saves it only if it works.
    pub fn crawler_add_key(&self, c: Crawler, key: SecretString) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let r = match setup::keychain() {
                Err(e) => Err(format!("The keychain is not available: {e}")),
                Ok(store) => crawlers::add_key(c, key, store.as_ref()).await,
            };
            send(&tx, SetupMsg::CrawlerChecked(c, r));
            let keys = tokio::task::spawn_blocking(crawler_keys)
                .await
                .unwrap_or_default();
            send(&tx, SetupMsg::Crawlers(keys));
            // The default reader may have become usable.
            let _ = tokio::task::spawn_blocking(|| Preferences::load().apply_runtime()).await;
        });
    }

    /// Reads one small page through `c` (with its saved key, if any).
    pub fn crawler_test(&self, c: Crawler) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let key = tokio::task::spawn_blocking(move || {
                let keychain = setup::keychain();
                let secrets: Option<&dyn SecretStore> =
                    keychain.as_ref().ok().map(|b| b.as_ref());
                crawlers::saved_key(c, secrets)
            })
            .await
            .ok()
            .flatten();
            if c.needs_key() && key.is_none() {
                send(
                    &tx,
                    SetupMsg::CrawlerChecked(c, Err(format!("Add a {} key first.", c.name()))),
                );
                return;
            }
            let r = crawlers::test(c, key).await;
            send(&tx, SetupMsg::CrawlerChecked(c, r));
        });
    }

    /// Deletes a hosted reader's key.
    pub fn crawler_remove_key(&self, c: Crawler) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = setup::keychain()
                .map_err(|e| format!("The keychain is not available: {e}"))
                .and_then(|s| crawlers::remove_key(c, s.as_ref()))
                .map(|_| "Key removed".to_owned());
            send(&tx, SetupMsg::CrawlerChecked(c, r));
            send(&tx, SetupMsg::Crawlers(crawler_keys()));
            Preferences::load().apply_runtime();
        });
    }

    /// Saves a custom provider, then tests it: with `key` (saved to the
    /// keychain only if it works) or with the key already saved, or none
    /// for a local server. The test's result comes back like a built-in
    /// provider's, and the Providers rows are sent again.
    pub fn custom_save(&self, p: CustomProvider, key: Option<SecretString>) {
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let id = p.id.clone();
            let saved = tokio::task::spawn_blocking(move || -> Result<&'static str, String> {
                let mut prefs = Preferences::load();
                prefs.upsert_custom_provider(p);
                prefs.save()?;
                prefs.apply_runtime();
                setup::custom::find(&id)
                    .map(|s| s.id)
                    .ok_or_else(|| format!("“{id}” could not be registered."))
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let id = match saved {
                Ok(id) => id,
                Err(e) => {
                    send(&tx, SetupMsg::CustomSaved(Err(e)));
                    return;
                }
            };
            send(&tx, SetupMsg::CustomSaved(Ok(id)));
            let keychain = setup::keychain();
            let result = match (key, keychain.as_ref()) {
                (Some(k), Ok(store)) => setup::add(id, k, store.as_ref(), None)
                    .await
                    .map(|r| super::model_report(r.chat_models, r.models, r.latency_ms))
                    .map_err(|e| e.to_string()),
                (Some(_), Err(e)) => Err(format!("The keychain is not available: {e}")),
                (None, k) => {
                    let secrets: Option<&dyn SecretStore> = k.ok().map(|b| b.as_ref());
                    setup::test(id, secrets, None)
                        .await
                        .map(|r| super::model_report(r.chat_models, r.models, r.latency_ms))
                        .map_err(|e| e.to_string())
                }
            };
            let _ = tx.send(Msg::Checked(Kind::Model, id, result));
            let _ = tx.send(tokio::task::spawn_blocking(super::providers_msg)
                .await
                .unwrap_or_else(|e| Msg::PrefsFailed(e.to_string())));
        });
    }

    /// Deletes a custom provider, its key, and the choices that used it.
    pub fn custom_remove(&self, id: String) {
        let tx = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let r = (|| -> Result<(), String> {
                if let Ok(store) = setup::keychain() {
                    // Missing keys are fine; a local server never had one.
                    let _ = setup::remove(&id, store.as_ref());
                }
                let mut prefs = Preferences::load();
                prefs.remove_custom_provider(&id);
                prefs.save()?;
                prefs.apply_runtime();
                Ok(())
            })();
            send(&tx, SetupMsg::CustomRemoved(id, r));
            let _ = tx.send(super::providers_msg());
        });
    }
}
