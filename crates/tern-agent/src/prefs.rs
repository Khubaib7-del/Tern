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

/// Where one harness was left in the desktop app.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LastChat {
    /// The chat's id; `None` for a fresh, unsaved one.
    pub chat: Option<String>,
    /// Text typed in the composer and not sent.
    pub draft: String,
    /// How far the transcript was scrolled, in pixels from the top.
    pub scroll: i64,
    /// The right panel was open.
    pub side_open: bool,
    /// The browser was open.
    pub browser_open: bool,
}

/// One chat tab the desktop had open.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenTab {
    /// The chat's id; `None` for a fresh, unsaved one.
    pub chat: Option<String>,
    pub title: String,
}

/// A harness's chat tabs, in order, and the one in front.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenTabs {
    pub tabs: Vec<OpenTab>,
    pub active: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Provider id (`groq`). `None` means the first provider with a key.
    pub provider: Option<String>,
    /// Model id for that provider. `None` means its first chat model.
    pub model: Option<String>,
    /// Search service id (`tavily`). `None` means the first with a key,
    /// else the keyless default (Wikipedia). `off` turns web search off.
    pub search: Option<String>,
    /// The self-hosted SearXNG's address. `None` means `http://localhost:8080`.
    pub search_url: Option<String>,
    /// Most output tokens per answer. `None` picks one per model (8192,
    /// or less where a model caps lower).
    pub max_output_tokens: Option<u32>,

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
    /// Study without a model: the tutor quotes passages, quizzes are
    /// written from sentences, and the viva uses its own questions.
    pub study_offline: bool,
    /// Questions in a new quiz. `None` means 5.
    pub study_quiz_count: Option<usize>,
    /// `easy`, `medium`, or `hard`. `None` means medium.
    pub study_difficulty: Option<String>,
    /// Questions in a viva. `None` means 6.
    pub study_viva_questions: Option<usize>,
    /// The decision model: off, local, or a remote provider.
    pub decision: crate::decide::DecisionPrefs,
    /// Project folders collapsed in the desktop sidebar, by id.
    pub collapsed_projects: Vec<String>,
    /// Where each harness was left in the desktop app (`research`,
    /// `study`): its last chat, draft, scroll and panels, so switching
    /// back, or reopening the app, returns to the same place.
    pub last_chats: std::collections::BTreeMap<String, LastChat>,
    /// Each harness's open chat tabs in the desktop app (`research`,
    /// `study`), restored on the next launch.
    pub open_tabs: std::collections::BTreeMap<String, OpenTabs>,
    /// Context compaction: off, the percent of the window that starts it
    /// (`None` means 80), and the turns kept word for word (`None` means 4).
    pub compact_off: bool,
    pub compact_at_percent: Option<u8>,
    pub compact_keep_turns: Option<usize>,
    /// Thinking effort for models that can think (`none` … `max`).
    /// `None` leaves each provider's default.
    pub effort: Option<String>,
    /// Models starred in the composer's picker, as `provider/model`.
    pub favourite_models: Vec<String>,
    /// The model the mic sends speech to, as `provider/model`. `None`
    /// picks a listed speech-to-text model (`docs/models.md`).
    pub default_stt_model: Option<String>,
    /// The model that describes pictures for a chat model that cannot
    /// see, as `provider/model`. `None` picks a listed vision model.
    pub default_vision_model: Option<String>,
    /// The model that reads text out of pictures when no vision model is
    /// chosen, as `provider/model`. `None` picks a listed OCR model.
    pub default_ocr_model: Option<String>,
    /// The desktop's panel widths in pixels, after a drag. `None` is the
    /// default width.
    pub sidebar_width: Option<u32>,
    pub right_pane_width: Option<u32>,
    /// The desktop's Appearance choices. `None` is the default for each:
    /// `dark` theme (`system`, `light`, `dark`), `lichen` accent (`slate`,
    /// `clay`, `mono`), `plex` font (`system`), `default` size (`small`,
    /// `large`).
    pub theme: Option<String>,
    pub accent: Option<String>,
    pub ui_font: Option<String>,
    pub text_size: Option<String>,
    /// Row spacing: `comfortable` (`None`) or `compact`.
    pub density: Option<String>,

    /// The first-run onboarding was finished or skipped; it shows once.
    pub onboarded: bool,
    /// The model that turns text into vectors, as `provider/model`. `None`
    /// leaves retrieval on Tern's own keyword scoring.
    pub default_embedding_model: Option<String>,
    /// A provider's server address when it is not the usual one (a local
    /// server on another port, a proxy), by provider id.
    pub base_urls: std::collections::BTreeMap<String, String>,
    /// Tern's own tools turned off in Settings › Tools, by name.
    pub tools_off: Vec<String>,
    /// Per-tool approval in Settings › Tools: `allow`, `ask`, or `deny`,
    /// by tool name. A tool not listed follows its effect class.
    pub tool_modes: std::collections::BTreeMap<String, String>,
    /// Providers the user added (OpenAI- or Anthropic-compatible), without
    /// their keys (those are in the keychain under `provider.<id>`).
    pub custom_providers: Vec<tern_providers::CustomProvider>,
    /// The default page reader: `tern` (Tern Crawl), `firecrawl`, `jina`,
    /// or `fetch`. `None` is Tern Crawl.
    pub crawler: Option<String>,
    /// Readers switched off in Settings › Search & Crawler, by id.
    pub crawlers_off: Vec<String>,
    /// The user's own system prompts: `global`, `research`, `study`.
    pub system_prompts: std::collections::BTreeMap<String, String>,
}

/// An approval choice as saved (`allow`, `ask`, `deny`).
pub fn parse_mode(s: &str) -> Option<crate::Mode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "allow" => Some(crate::Mode::Allow),
        "ask" => Some(crate::Mode::Ask),
        "deny" => Some(crate::Mode::Deny),
        _ => None,
    }
}

/// The saved word for an approval choice.
pub fn mode_word(m: crate::Mode) -> &'static str {
    match m {
        crate::Mode::Allow => "allow",
        crate::Mode::Ask => "ask",
        crate::Mode::Deny => "deny",
    }
}

impl Preferences {
    /// The saved thinking effort.
    pub fn effort(&self) -> Option<tern_providers::Effort> {
        self.effort
            .as_deref()
            .and_then(tern_providers::Effort::parse)
    }

    /// Who commits vault checkpoints.
    pub fn git_identity(&self) -> crate::git::Identity {
        crate::git::Identity {
            name: self.git_name.clone(),
            email: self.git_email.clone(),
            credit_tern: self.git_credit_tern,
        }
    }

    /// Whether motion is off: the saved choice, `TERN_REDUCED_MOTION`, or
    /// the operating system's own setting (see
    /// [`tern_platform::motion::os_reduces_motion`]).
    pub fn wants_reduced_motion(&self) -> bool {
        self.reduced_motion
            || std::env::var("TERN_REDUCED_MOTION").is_ok_and(|v| v == "1" || v == "true")
            || tern_platform::motion::os_reduces_motion()
    }

    /// In the settings folder: the data folder on Windows and macOS,
    /// `$XDG_CONFIG_HOME/tern` on Linux.
    pub fn path(paths: &AppPaths) -> PathBuf {
        paths.config_dir.join(FILE)
    }

    /// Reads the saved choices. Anything missing or malformed reads as none.
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Puts the choices that are process-wide into effect: custom
    /// providers become findable like built-ins, `read_url` uses the
    /// chosen reader, and every chat gets the user's prompts. Front ends
    /// call it after changing any of them.
    pub fn apply_runtime(&self) {
        tern_providers::custom::register_all(&self.custom_providers);
        crate::prompts::set_user(&self.system_prompts);
        crate::decide::set_jobs_off(&self.decision.jobs_off);
        let keychain = match self
            .crawler
            .as_deref()
            .and_then(crate::crawlers::Crawler::parse)
        {
            Some(c) if c.needs_key() => crate::providers::keychain().ok(),
            _ => None,
        };
        let c = crate::crawlers::pick(self.crawler.as_deref(), &self.crawlers_off, |c| {
            crate::crawlers::has_key(c, keychain.as_deref())
        });
        crate::crawlers::set(c);
    }

    /// Adds or replaces a custom provider (by id) and puts it into effect.
    pub fn upsert_custom_provider(&mut self, p: tern_providers::CustomProvider) {
        match self.custom_providers.iter_mut().find(|c| c.id == p.id) {
            Some(c) => *c = p,
            None => self.custom_providers.push(p),
        }
        tern_providers::custom::register_all(&self.custom_providers);
    }

    /// Removes a custom provider and the choices that pointed at it.
    pub fn remove_custom_provider(&mut self, id: &str) {
        self.custom_providers.retain(|c| c.id != id);
        self.base_urls.remove(id);
        self.forget_provider(id);
        tern_providers::custom::register_all(&self.custom_providers);
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

    /// The saved choices from the default data directory. The
    /// process-wide parts (custom providers, the page reader, the user's
    /// prompts, decision jobs) take effect at once.
    pub fn load() -> Self {
        let p = AppPaths::discover()
            .map(|p| Self::load_from(&Self::path(&p)))
            .unwrap_or_default();
        p.apply_runtime();
        p
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

    /// A quiz spec from the Study choices.
    pub fn quiz_spec(&self, topic: Option<String>) -> crate::study::QuizSpec {
        crate::study::QuizSpec {
            topic,
            count: self.study_quiz_count.unwrap_or(5).clamp(1, 30),
            difficulty: self
                .study_difficulty
                .as_deref()
                .and_then(crate::study::Difficulty::parse)
                .unwrap_or_default(),
            kinds: Vec::new(),
        }
    }

    /// Questions in a viva, 2 to 15.
    pub fn viva_questions(&self) -> usize {
        self.study_viva_questions.unwrap_or(6).clamp(2, 15)
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

    /// The search service chats use. SearXNG with no address saved was
    /// never set up (Tern cannot guess where it runs), so it counts as
    /// unconfigured and chats use the keyless default instead.
    pub fn effective_search(&self) -> Option<String> {
        match self.search.as_deref() {
            Some("searxng") if !self.searxng_configured() => {
                Some(tern_crawl::KEYLESS_SEARCH.to_owned())
            }
            other => other.map(str::to_owned),
        }
    }

    /// Whether a SearXNG address was ever saved.
    pub fn searxng_configured(&self) -> bool {
        self.search_url
            .as_deref()
            .is_some_and(|u| !u.trim().is_empty())
    }

    /// The saved server address for provider `id`, if one was set.
    pub fn base_url(&self, id: &str) -> Option<&str> {
        self.base_urls
            .get(id)
            .map(|u| u.trim())
            .filter(|u| !u.is_empty())
    }

    /// Saves (or, when blank, forgets) provider `id`'s server address.
    pub fn set_base_url(&mut self, id: &str, url: &str) {
        let url = url.trim();
        if url.is_empty() {
            self.base_urls.remove(id);
        } else {
            self.base_urls.insert(id.to_owned(), url.to_owned());
        }
    }

    /// Whether Tern's own tool `name` is on.
    pub fn tool_on(&self, name: &str) -> bool {
        !self.tools_off.iter().any(|t| t == name)
    }

    pub fn set_tool_on(&mut self, name: &str, on: bool) {
        self.tools_off.retain(|t| t != name);
        if !on {
            self.tools_off.push(name.to_owned());
            self.tools_off.sort();
        }
    }

    /// The approval the user chose for tool `name`, if any.
    pub fn tool_mode(&self, name: &str) -> Option<crate::Mode> {
        self.tool_modes.get(name).and_then(|m| parse_mode(m))
    }

    /// Sets tool `name`'s approval; `None` goes back to its effect class.
    pub fn set_tool_mode(&mut self, name: &str, mode: Option<crate::Mode>) {
        match mode {
            Some(m) => {
                self.tool_modes
                    .insert(name.to_owned(), mode_word(m).to_owned());
            }
            None => {
                self.tool_modes.remove(name);
            }
        }
    }

    /// Chat options that open these choices.
    pub fn chat_options(&self) -> ChatOptions {
        tern_providers::custom::register_all(&self.custom_providers);
        let base_url = self
            .provider
            .as_deref()
            .and_then(|p| self.base_url(p))
            .map(str::to_owned);
        ChatOptions {
            provider: self.provider.clone(),
            base_url,
            tools_off: self.tools_off.clone(),
            tool_modes: self
                .tool_modes
                .iter()
                .filter_map(|(k, v)| parse_mode(v).map(|m| (k.clone(), m)))
                .collect(),
            model: self.model.clone(),
            search: self.effective_search(),
            search_url: self.search_url.clone(),
            max_output_tokens: self.max_output_tokens.unwrap_or(0),
            vault: crate::vault::default_root(self).ok(),
            decision: self.decision.clone(),
            compaction: crate::compact::Compaction::from_prefs(
                self.compact_off,
                self.compact_at_percent,
                self.compact_keep_turns,
            ),
            ..ChatOptions::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_base_urls_and_onboarding_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.json");
        let mut p = Preferences::default();
        assert!(!p.onboarded);
        assert!(p.tool_on("web_search"));
        p.set_tool_on("web_search", false);
        p.set_tool_on("web_search", false);
        assert_eq!(p.tools_off, vec!["web_search".to_owned()]);
        p.set_tool_mode("vault_write", Some(crate::Mode::Allow));
        p.set_tool_mode("read_url", Some(crate::Mode::Deny));
        p.set_base_url("ollama", " http://10.0.0.2:11434 ");
        p.set_base_url("groq", "   ");
        p.onboarded = true;
        p.default_embedding_model = Some("openai/text-embedding-3-small".into());
        p.provider = Some("ollama".into());
        p.save_to(&path).unwrap();
        let back = Preferences::load_from(&path);
        assert_eq!(back, p);
        assert_eq!(back.base_url("ollama"), Some("http://10.0.0.2:11434"));
        assert_eq!(back.base_url("groq"), None);
        assert_eq!(back.tool_mode("vault_write"), Some(crate::Mode::Allow));
        assert_eq!(back.tool_mode("web_search"), None);
        let o = back.chat_options();
        assert_eq!(o.base_url.as_deref(), Some("http://10.0.0.2:11434"));
        assert_eq!(o.tools_off, vec!["web_search".to_owned()]);
        assert_eq!(o.tool_modes.get("read_url"), Some(&crate::Mode::Deny));
        p.set_tool_on("web_search", true);
        p.set_tool_mode("vault_write", None);
        assert!(p.tools_off.is_empty() && !p.tool_modes.contains_key("vault_write"));
        // An old file without the new keys still reads.
        std::fs::write(&path, r#"{"provider":"groq"}"#).unwrap();
        let old = Preferences::load_from(&path);
        assert!(!old.onboarded && old.tools_off.is_empty());
        assert_eq!(parse_mode("ASK"), Some(crate::Mode::Ask));
        assert_eq!(parse_mode("maybe"), None);
    }

    #[test]
    fn searxng_without_an_address_is_unconfigured() {
        let mut p = Preferences {
            search: Some("searxng".into()),
            ..Preferences::default()
        };
        assert!(!p.searxng_configured());
        assert_eq!(p.effective_search().as_deref(), Some("wikipedia"));
        assert_eq!(p.chat_options().search.as_deref(), Some("wikipedia"));
        p.search_url = Some("http://10.0.0.5:8888".into());
        assert_eq!(p.effective_search().as_deref(), Some("searxng"));
        p.search = Some("tavily".into());
        p.search_url = None;
        assert_eq!(p.effective_search().as_deref(), Some("tavily"));
    }

    #[test]
    fn round_trips_and_tolerates_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(FILE);
        assert_eq!(Preferences::load_from(&path), Preferences::default());
        let p = Preferences {
            provider: Some("groq".into()),
            model: Some("llama-3.3-70b-versatile".into()),
            search: Some("tavily".into()),
            search_url: Some("http://10.0.0.5:8888".into()),
            max_output_tokens: Some(4000),
            reduced_motion: true,
            vault: Some("D:/Notes".into()),
            git_name: Some("Khubaib".into()),
            git_email: None,
            git_credit_tern: true,
            deep_workers: Some(4),
            study_offline: true,
            study_quiz_count: Some(10),
            study_difficulty: Some("hard".into()),
            study_viva_questions: Some(40),
            decision: crate::decide::DecisionPrefs {
                mode: Some("remote".into()),
                provider: Some("typesafe".into()),
                ..Default::default()
            },
            collapsed_projects: vec!["p1".into()],
            last_chats: [(
                "study".to_owned(),
                LastChat {
                    chat: Some("01J".into()),
                    draft: "half".into(),
                    scroll: 120,
                    side_open: true,
                    browser_open: false,
                },
            )]
            .into(),
            effort: Some("high".into()),
            favourite_models: vec!["mistral/mistral-large-latest".into()],
            default_stt_model: Some("groq/whisper-large-v3-turbo".into()),
            default_vision_model: None,
            default_ocr_model: Some("mistral/mistral-ocr-latest".into()),
            sidebar_width: Some(300),
            right_pane_width: Some(420),
            theme: Some("light".into()),
            accent: Some("clay".into()),
            ui_font: Some("system".into()),
            text_size: Some("large".into()),
            compact_off: false,
            compact_at_percent: Some(85),
            compact_keep_turns: Some(6),
            density: Some("compact".into()),
            onboarded: true,
            default_embedding_model: None,
            base_urls: Default::default(),
            tools_off: vec!["read_url".into()],
            tool_modes: Default::default(),
            custom_providers: vec![tern_providers::CustomProvider {
                id: "box".into(),
                name: "Box".into(),
                base_url: "http://localhost:8000/v1".into(),
                ..Default::default()
            }],
            crawler: Some("jina".into()),
            crawlers_off: vec!["fetch".into()],
            system_prompts: [("global".to_owned(), "Be brief.".to_owned())].into(),
        };
        assert_eq!(p.effort(), Some(tern_providers::Effort::High));
        let c = p.chat_options().compaction;
        assert!(c.auto && (c.at - 0.85).abs() < 1e-6 && c.keep_turns == 6);
        assert_eq!(p.quiz_spec(None).count, 10);
        assert_eq!(p.quiz_spec(None).difficulty, crate::study::Difficulty::Hard);
        assert_eq!(p.viva_questions(), 15, "capped");
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
        assert_eq!(o.max_output_tokens, 0, "0 picks a length per model");
        let q = Preferences {
            search: Some("searxng".into()),
            search_url: Some("http://10.0.0.5:8888".into()),
            max_output_tokens: Some(4000),
            ..Preferences::default()
        };
        let o = q.chat_options();
        assert_eq!(o.search_url.as_deref(), Some("http://10.0.0.5:8888"));
        assert_eq!(o.max_output_tokens, 4000);
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
