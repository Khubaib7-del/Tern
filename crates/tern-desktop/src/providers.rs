//! What the Providers screen shows, kept apart from GPUI so it can be tested
//! without a display.
//!
//! Two sections: model providers and web search services. Each row is
//! a catalog entry with whether this machine holds a key for it. Keys never
//! pass through this state: the view hands the typed key straight to the
//! bridge, which tests it and saves it to the OS keychain.

use std::collections::HashMap;

use tern_agent::Preferences;
use tern_agent::providers::{ModelInfo, ModelKind, ProviderRow};
use tern_agent::search::SearchRow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Model,
    Search,
}

/// One provider or search service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceRow {
    pub kind: Kind,
    pub id: &'static str,
    pub name: &'static str,
    /// Runs on this machine (Ollama, LM Studio), is self-hosted (SearXNG),
    /// or needs no key at all (Wikipedia).
    pub local: bool,
    /// A public service that needs no key (Wikipedia search).
    pub keyless: bool,
    pub key_saved: bool,
    pub key_url: &'static str,
}

impl ServiceRow {
    pub fn needs_key(&self) -> bool {
        !self.local
    }

    /// Whether a chat can use this row now.
    pub fn usable(&self) -> bool {
        self.local || self.key_saved
    }

    /// The letter drawn in the row's logo tile until the service glyphs land.
    pub fn monogram(&self) -> String {
        self.name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default()
    }

    /// The second line under the name.
    pub fn status(&self) -> &'static str {
        match (self.kind, self.local, self.key_saved) {
            (Kind::Model, true, _) => "Runs on this computer, no key needed",
            (Kind::Search, true, _) if self.keyless => {
                "Free, no key needed · searches Wikipedia articles only"
            }
            (Kind::Search, true, _) => "Self-hosted, no key needed · set its address and Test",
            (_, false, true) => "Key saved",
            (_, false, false) => "No key yet",
        }
    }
}

pub fn rows_from(models: Vec<ProviderRow>, search: Vec<SearchRow>) -> Vec<ServiceRow> {
    let models = models.into_iter().map(|r| ServiceRow {
        kind: Kind::Model,
        id: r.id,
        name: r.name,
        local: r.local,
        keyless: false,
        key_saved: r.key_saved,
        key_url: r.key_url,
    });
    let search = search.into_iter().map(|r| ServiceRow {
        kind: Kind::Search,
        id: r.id,
        name: r.name,
        local: r.self_hosted || r.keyless,
        keyless: r.keyless,
        key_saved: r.key_saved,
        key_url: r.key_url,
    });
    models.chain(search).collect()
}

/// The last thing that happened to a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    Working(String),
    Ok(String),
    Failed(String),
}

/// A model the picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    pub context_window: Option<u64>,
}

/// The model list of the default provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Models {
    Loading(&'static str),
    Loaded(&'static str, Vec<ModelChoice>),
    Failed(&'static str, String),
}

impl Models {
    pub fn provider(&self) -> &'static str {
        match self {
            Self::Loading(p) | Self::Loaded(p, _) | Self::Failed(p, _) => p,
        }
    }
}

#[derive(Debug, Default)]
pub struct ProvidersState {
    pub loaded: bool,
    pub rows: Vec<ServiceRow>,
    pub prefs: Preferences,
    /// Why the keychain cannot be read, when it cannot.
    pub keychain_error: Option<String>,
    /// The row whose key field is open.
    pub editing: Option<(Kind, &'static str)>,
    notes: HashMap<(Kind, &'static str), Note>,
    pub models: Option<Models>,
}

impl ProvidersState {
    pub fn load(&mut self, rows: Vec<ServiceRow>, prefs: Preferences, keychain: Option<String>) {
        self.rows = rows;
        self.prefs = prefs;
        self.keychain_error = keychain;
        self.loaded = true;
    }

    pub fn section(&self, kind: Kind) -> impl Iterator<Item = &ServiceRow> {
        self.rows.iter().filter(move |r| r.kind == kind)
    }

    fn row_mut(&mut self, kind: Kind, id: &str) -> Option<&mut ServiceRow> {
        self.rows.iter_mut().find(|r| r.kind == kind && r.id == id)
    }

    /// The row a new chat uses: the saved choice, else the first remote
    /// row with a key (the same rule `ChatSession::open` follows).
    pub fn default_id(&self, kind: Kind) -> Option<&'static str> {
        let search = self.prefs.effective_search();
        let chosen = match kind {
            Kind::Model => self.prefs.provider.as_deref(),
            Kind::Search => search.as_deref(),
        };
        if let Some(id) = chosen {
            return self.section(kind).find(|r| r.id == id).map(|r| r.id);
        }
        self.section(kind)
            .find(|r| !r.local && r.key_saved)
            // With no search key, the keyless search (the one chats use).
            .or_else(|| match kind {
                Kind::Search => self.section(kind).find(|r| r.keyless),
                Kind::Model => None,
            })
            .map(|r| r.id)
    }

    /// Whether the last Test of this row passed in this window.
    pub fn tested_ok(&self, kind: Kind, id: &str) -> bool {
        matches!(self.note(kind, id), Some(Note::Ok(_)))
    }

    /// Whether "Use" is open for a row: a self-hosted search server must
    /// pass a Test at its address first.
    pub fn can_choose(&self, row: &ServiceRow) -> bool {
        if row.kind == Kind::Search && row.local && !row.keyless {
            return self.tested_ok(row.kind, row.id);
        }
        row.usable()
    }

    /// The SearXNG address typed in, kept with the choices. Blank means
    /// the default (`http://localhost:8080`).
    pub fn set_search_url(&mut self, url: &str) -> Option<String> {
        let url = url.trim().trim_end_matches('/');
        let url = if url.is_empty() {
            None
        } else if url.contains("://") {
            Some(url.to_owned())
        } else {
            Some(format!("http://{url}"))
        };
        if self.prefs.search_url != url {
            // A new address has not been tested yet.
            self.notes.remove(&(Kind::Search, "searxng"));
        }
        self.prefs.search_url = url.clone();
        url
    }

    pub fn is_default(&self, row: &ServiceRow) -> bool {
        self.default_id(row.kind) == Some(row.id)
    }

    pub fn note(&self, kind: Kind, id: &str) -> Option<&Note> {
        self.notes
            .iter()
            .find(|((k, i), _)| *k == kind && *i == id)
            .map(|(_, n)| n)
    }

    pub fn is_working(&self, kind: Kind, id: &str) -> bool {
        matches!(self.note(kind, id), Some(Note::Working(_)))
    }

    pub fn start_edit(&mut self, kind: Kind, id: &'static str) {
        self.editing = Some((kind, id));
    }

    pub fn cancel_edit(&mut self) {
        self.editing = None;
    }

    pub fn is_editing(&self, kind: Kind, id: &str) -> bool {
        matches!(self.editing, Some((k, i)) if k == kind && i == id)
    }

    pub fn begin(&mut self, kind: Kind, id: &'static str, label: &str) {
        self.notes
            .insert((kind, id), Note::Working(label.to_owned()));
    }

    /// A key was tested. `saved` says whether it was a new key (add) or the
    /// saved one (test).
    pub fn checked(&mut self, kind: Kind, id: &'static str, result: Result<String, String>) {
        let note = match result {
            Ok(text) => {
                if let Some(r) = self.row_mut(kind, id) {
                    if !r.local {
                        r.key_saved = true;
                    }
                }
                if self.is_editing(kind, id) {
                    self.editing = None;
                }
                Note::Ok(text)
            }
            Err(e) => Note::Failed(e),
        };
        self.notes.insert((kind, id), note);
    }

    pub fn removed(&mut self, kind: Kind, id: &'static str, result: Result<bool, String>) {
        let note = match result {
            Ok(_) => {
                if let Some(r) = self.row_mut(kind, id) {
                    r.key_saved = false;
                }
                match kind {
                    Kind::Model => {
                        self.prefs.forget_provider(id);
                        if self.models.as_ref().is_some_and(|m| m.provider() == id) {
                            self.models = None;
                        }
                    }
                    Kind::Search => self.prefs.forget_search(id),
                }
                Note::Ok("Key removed from the keychain".into())
            }
            Err(e) => Note::Failed(e),
        };
        self.notes.insert((kind, id), note);
    }

    /// Makes a row the default. Returns false when it cannot be used yet.
    pub fn choose(&mut self, kind: Kind, id: &'static str) -> bool {
        let Some(row) = self.section(kind).find(|r| r.id == id).cloned() else {
            return false;
        };
        if !self.can_choose(&row) {
            return false;
        }
        match kind {
            Kind::Model => self.prefs.choose_provider(id),
            Kind::Search => self.prefs.search = Some(id.to_owned()),
        }
        true
    }

    pub fn models_loading(&mut self, provider: &'static str) {
        self.models = Some(Models::Loading(provider));
    }

    /// The listing for a provider. Only chat models are offered, unless the
    /// listing labels none as chat, in which case all are.
    pub fn models_loaded(
        &mut self,
        provider: &'static str,
        result: Result<Vec<ModelInfo>, String>,
    ) {
        self.models = Some(match result {
            Ok(list) => {
                let any_chat = list.iter().any(|m| m.kind == ModelKind::Chat);
                let mut choices: Vec<ModelChoice> = list
                    .into_iter()
                    .filter(|m| !any_chat || m.kind == ModelKind::Chat)
                    .map(|m| ModelChoice {
                        id: m.id,
                        context_window: m.context_window,
                    })
                    .collect();
                choices.sort_by(|a, b| a.id.cmp(&b.id));
                choices.dedup_by(|a, b| a.id == b.id);
                Models::Loaded(provider, choices)
            }
            Err(e) => Models::Failed(provider, e),
        });
    }

    /// The model a new chat uses on the default provider: the saved choice,
    /// else the first in the listing order `ChatSession` uses.
    pub fn chosen_model(&self) -> Option<&str> {
        self.prefs.model.as_deref()
    }

    pub fn choose_model(&mut self, id: &str) {
        self.prefs.model = Some(id.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: Kind, id: &'static str, local: bool, key: bool) -> ServiceRow {
        ServiceRow {
            kind,
            id,
            name: id,
            local,
            keyless: false,
            key_saved: key,
            key_url: "",
        }
    }

    fn state() -> ProvidersState {
        let mut s = ProvidersState::default();
        s.load(
            vec![
                row(Kind::Model, "openai", false, false),
                row(Kind::Model, "groq", false, true),
                row(Kind::Model, "ollama", true, false),
                row(Kind::Search, "tavily", false, false),
                row(Kind::Search, "searxng", true, false),
            ],
            Preferences::default(),
            None,
        );
        s
    }

    #[test]
    fn default_follows_the_first_saved_key_until_a_choice_is_made() {
        let mut s = state();
        assert_eq!(s.default_id(Kind::Model), Some("groq"));
        assert_eq!(s.default_id(Kind::Search), None);
        assert!(!s.choose(Kind::Model, "openai"), "no key yet");
        assert!(s.choose(Kind::Model, "ollama"));
        assert_eq!(s.default_id(Kind::Model), Some("ollama"));
        assert!(
            !s.choose(Kind::Search, "searxng"),
            "a self-hosted server must pass Test first"
        );
        s.set_search_url("10.0.0.5:8888/");
        assert_eq!(s.prefs.search_url.as_deref(), Some("http://10.0.0.5:8888"));
        s.checked(Kind::Search, "searxng", Err("did not answer".into()));
        assert!(!s.choose(Kind::Search, "searxng"));
        s.checked(Kind::Search, "searxng", Ok("Working · 3 results".into()));
        assert!(s.choose(Kind::Search, "searxng"));
        assert_eq!(s.prefs.search.as_deref(), Some("searxng"));
        s.set_search_url("http://10.0.0.6:8080");
        assert!(
            !s.tested_ok(Kind::Search, "searxng"),
            "a new address is untested"
        );
    }

    #[test]
    fn with_no_search_key_the_keyless_search_is_the_default() {
        let mut s = state();
        let mut w = row(Kind::Search, "wikipedia", true, false);
        w.keyless = true;
        s.rows.push(w.clone());
        assert_eq!(s.default_id(Kind::Search), Some("wikipedia"));
        assert_eq!(
            w.status(),
            "Free, no key needed · searches Wikipedia articles only"
        );
        assert!(s.choose(Kind::Search, "wikipedia"), "needs no Test");
    }

    #[test]
    fn a_working_key_closes_the_field_and_marks_the_row() {
        let mut s = state();
        s.start_edit(Kind::Model, "openai");
        s.begin(Kind::Model, "openai", "Testing the key");
        assert!(s.is_working(Kind::Model, "openai"));
        s.checked(Kind::Model, "openai", Err("401: key refused".into()));
        assert!(s.is_editing(Kind::Model, "openai"), "stays open to retry");
        assert_eq!(
            s.note(Kind::Model, "openai"),
            Some(&Note::Failed("401: key refused".into()))
        );
        s.checked(Kind::Model, "openai", Ok("Works, 40 models".into()));
        assert!(s.editing.is_none());
        assert!(
            s.section(Kind::Model)
                .any(|r| r.id == "openai" && r.key_saved)
        );
        assert!(!s.is_working(Kind::Model, "openai"));
    }

    #[test]
    fn removing_the_default_forgets_it() {
        let mut s = state();
        assert!(s.choose(Kind::Model, "groq"));
        s.choose_model("llama");
        s.models_loading("groq");
        s.removed(Kind::Model, "groq", Ok(true));
        assert_eq!(s.prefs.provider, None);
        assert_eq!(s.prefs.model, None);
        assert_eq!(s.models, None);
        assert_eq!(s.default_id(Kind::Model), None);
    }

    #[test]
    fn picker_offers_chat_models_sorted() {
        let mut s = state();
        let mut embed = ModelInfo::new("text-embedding-3-small");
        embed.kind = ModelKind::Embedding;
        let mut b = ModelInfo::new("b-chat");
        b.kind = ModelKind::Chat;
        b.context_window = Some(128_000);
        let mut a = ModelInfo::new("a-chat");
        a.kind = ModelKind::Chat;
        s.models_loaded("groq", Ok(vec![embed, b, a]));
        let Some(Models::Loaded("groq", list)) = &s.models else {
            panic!("{:?}", s.models)
        };
        let ids: Vec<_> = list.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a-chat", "b-chat"]);
        assert_eq!(list[1].context_window, Some(128_000));

        let mut other = ModelInfo::new("mystery");
        other.kind = ModelKind::Other;
        s.models_loaded("groq", Ok(vec![other]));
        assert!(matches!(&s.models, Some(Models::Loaded(_, l)) if l.len() == 1));
    }

    #[test]
    fn rows_say_what_they_need() {
        let s = state();
        let get = |id| s.rows.iter().find(|r| r.id == id).unwrap();
        assert_eq!(get("openai").status(), "No key yet");
        assert_eq!(get("groq").status(), "Key saved");
        assert_eq!(
            get("ollama").status(),
            "Runs on this computer, no key needed"
        );
        assert_eq!(
            get("searxng").status(),
            "Self-hosted, no key needed · set its address and Test"
        );
        assert!(get("ollama").usable() && !get("openai").usable());
        let mut lm = row(Kind::Model, "lmstudio", true, false);
        lm.name = "LM Studio";
        assert_eq!(lm.monogram(), "L");
    }
}
