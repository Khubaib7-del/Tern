//! The user's chosen defaults: which provider, model, and search service a
//! new chat opens with.
//!
//! Stored as `preferences.json` in the app data directory. Keys never go
//! here; they live only in the OS keychain. A missing or unreadable file
//! means "no choice yet", so Tern falls back to the first saved key.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tern_platform::AppPaths;

use crate::ChatOptions;

const FILE: &str = "preferences.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Provider id (`groq`). `None` means the first provider with a key.
    pub provider: Option<String>,
    /// Model id for that provider. `None` means its first chat model.
    pub model: Option<String>,
    /// Search service id (`tavily`). `None` means the first with a key.
    pub search: Option<String>,
    /// Turns motion off: the mark stays still, panels appear without
    /// sliding, and the theme switches at once. `TERN_REDUCED_MOTION=1`
    /// does the same.
    pub reduced_motion: bool,
    /// The vault folder. `None` means `<data>/vault`.
    pub vault: Option<String>,
    /// Name and email for vault checkpoints; `None` uses git's own config.
    pub git_name: Option<String>,
    pub git_email: Option<String>,
    /// Credit Tern with a `Co-authored-by` trailer on checkpoints.
    pub git_credit_tern: bool,
    /// Deep research workers that run at once. `None` means 3.
    pub deep_workers: Option<usize>,
}

impl Preferences {
    /// Who commits vault checkpoints.
    pub fn git_identity(&self) -> crate::git::Identity {
        crate::git::Identity {
            name: self.git_name.clone(),
            email: self.git_email.clone(),
            credit_tern: self.git_credit_tern,
        }
    }

    /// Whether motion is off, from the saved choice or the environment.
    pub fn wants_reduced_motion(&self) -> bool {
        self.reduced_motion
            || std::env::var("TERN_REDUCED_MOTION").is_ok_and(|v| v == "1" || v == "true")
    }

    pub fn path(paths: &AppPaths) -> PathBuf {
        paths.data_dir.join(FILE)
    }

    /// Reads the saved choices. Anything missing or malformed reads as none.
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        // Write then rename, so a crash never leaves half a file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    /// The saved choices from the default data directory.
    pub fn load() -> Self {
        AppPaths::discover()
            .map(|p| Self::load_from(&Self::path(&p)))
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let paths = AppPaths::discover().map_err(|e| e.to_string())?;
        self.save_to(&Self::path(&paths))
    }

    /// Picks a provider. A different provider forgets the old model, since
    /// model ids do not carry across providers.
    pub fn choose_provider(&mut self, id: &str) {
        if self.provider.as_deref() != Some(id) {
            self.model = None;
        }
        self.provider = Some(id.to_owned());
    }

    /// Forgets choices that point at a provider whose key was removed.
    pub fn forget_provider(&mut self, id: &str) {
        if self.provider.as_deref() == Some(id) {
            self.provider = None;
            self.model = None;
        }
    }

    pub fn forget_search(&mut self, id: &str) {
        if self.search.as_deref() == Some(id) {
            self.search = None;
        }
    }

    /// How deep research runs: workers at once from these choices, 1 to
    /// [`crate::deep::MAX_WORKERS`].
    pub fn deep_options(&self) -> crate::deep::DeepOptions {
        let mut opts = crate::deep::DeepOptions::default();
        if let Some(n) = self.deep_workers {
            opts.workers = n.clamp(1, crate::deep::MAX_WORKERS);
        }
        opts
    }

    /// Chat options that open these choices.
    pub fn chat_options(&self) -> ChatOptions {
        ChatOptions {
            provider: self.provider.clone(),
            model: self.model.clone(),
            search: self.search.clone(),
            vault: crate::vault::default_root(self).ok(),
            ..ChatOptions::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(FILE);
        assert_eq!(Preferences::load_from(&path), Preferences::default());
        let p = Preferences {
            provider: Some("groq".into()),
            model: Some("llama-3.3-70b-versatile".into()),
            search: Some("tavily".into()),
            reduced_motion: true,
            vault: Some("D:/Notes".into()),
            git_name: Some("Khubaib".into()),
            git_email: None,
            git_credit_tern: true,
            deep_workers: Some(4),
        };
        p.save_to(&path).unwrap();
        assert_eq!(Preferences::load_from(&path), p);
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(Preferences::load_from(&path), Preferences::default());
        std::fs::write(&path, r#"{"provider":"openai"}"#).unwrap();
        assert_eq!(
            Preferences::load_from(&path).provider.as_deref(),
            Some("openai")
        );
    }

    #[test]
    fn switching_provider_drops_the_model() {
        let mut p = Preferences::default();
        p.choose_provider("groq");
        p.model = Some("llama".into());
        p.choose_provider("groq");
        assert_eq!(p.model.as_deref(), Some("llama"));
        p.choose_provider("openai");
        assert_eq!(p.model, None);
        p.model = Some("gpt".into());
        p.forget_provider("groq");
        assert_eq!(p.provider.as_deref(), Some("openai"));
        p.forget_provider("openai");
        assert_eq!((p.provider, p.model), (None, None));
    }

    #[test]
    fn chat_options_carry_the_choices() {
        let p = Preferences {
            provider: Some("groq".into()),
            model: Some("m".into()),
            search: None,
            ..Preferences::default()
        };
        let o = p.chat_options();
        assert_eq!(o.provider.as_deref(), Some("groq"));
        assert_eq!(o.model.as_deref(), Some("m"));
        assert_eq!(o.search, None);
    }

    #[test]
    fn deep_workers_default_to_three_and_are_capped() {
        assert_eq!(Preferences::default().deep_options().workers, 3);
        let p = Preferences {
            deep_workers: Some(40),
            ..Preferences::default()
        };
        assert_eq!(p.deep_options().workers, crate::deep::MAX_WORKERS);
        let p = Preferences {
            deep_workers: Some(0),
            ..Preferences::default()
        };
        assert_eq!(p.deep_options().workers, 1);
        let old: Preferences = serde_json::from_str(r#"{"provider": "groq"}"#).unwrap();
        assert_eq!(old.deep_workers, None);
    }
}
