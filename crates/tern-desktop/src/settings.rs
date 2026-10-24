//! The Settings screen's model: its sections, the search over every
//! setting, when the section list folds to icons, and the per-role default
//! models. GPUI-free so all of it is unit tested; `view::settings_view`
//! draws it.
//!
//! Settings replaces the scattered pages that came before it (Providers,
//! Customize, Appearance, Decision, Evaluation, the Study settings card):
//! each became a section here and keeps everything it did.

use tern_agent::Preferences;
use tern_agent::providers::{Capability, ModelInfo};

/// One page of Settings, in the order of the section list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Section {
    /// Model providers: keys, server addresses, Test, logos.
    #[default]
    Providers,
    /// Every listed model by capability, and the default model per role.
    Models,
    /// Tern's own tools: on or off, and when each asks first.
    Tools,
    /// The decision model: off, local, Jev, SubQ Snap, Voyage.
    Decision,
    /// Evaluation runs and suites.
    Evaluation,
    /// MCP servers, skills, and the effective instructions.
    Extensions,
    /// Web search services and Tern Crawl.
    Search,
    /// Theme, accent, font, size, density, motion, glyphs.
    Appearance,
    /// Quiz, viva, and offline Study choices.
    Study,
    /// What leaves the machine, where data is kept, deleting it.
    Privacy,
    /// Version, links, and onboarding again.
    About,
}

impl Section {
    pub const ALL: [Self; 11] = [
        Self::Providers,
        Self::Models,
        Self::Tools,
        Self::Decision,
        Self::Evaluation,
        Self::Extensions,
        Self::Search,
        Self::Appearance,
        Self::Study,
        Self::Privacy,
        Self::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Providers => "Providers",
            Self::Models => "Models",
            Self::Tools => "Tools",
            Self::Decision => "Decision model",
            Self::Evaluation => "Evaluation",
            Self::Extensions => "MCP & Skills",
            Self::Search => "Search & Crawler",
            Self::Appearance => "Appearance",
            Self::Study => "Study",
            Self::Privacy => "Privacy & data",
            Self::About => "About",
        }
    }

    /// The one line under the page title.
    pub fn blurb(self) -> &'static str {
        match self {
            Self::Providers => {
                "Bring your own keys. Tern tests a key before saving it and keeps it only in your system keychain."
            }
            Self::Models => {
                "Every model your providers list, by what it can do, and which one Tern uses for each job."
            }
            Self::Tools => "Tern's own tools: turn one off, or choose whether it asks you first.",
            Self::Decision => {
                "A small model that ranks results, picks the first tool, and pre-scores claims."
            }
            Self::Evaluation => "Scores per suite, failures, and the change since the last run.",
            Self::Extensions => {
                "MCP servers Tern may call, skills it loads when a task needs them, and what the model is told."
            }
            Self::Search => "Where web searches go, and how Tern Crawl reads the pages it finds.",
            Self::Appearance => "Theme, accent, type, density, and motion.",
            Self::Study => "How quizzes and vivas are made, and Study without a model.",
            Self::Privacy => {
                "What leaves this computer, where Tern keeps your data, and deleting it."
            }
            Self::About => "Version, links, and the first-run tour.",
        }
    }

    /// A stable id for element ids.
    pub fn id(self) -> &'static str {
        match self {
            Self::Providers => "providers",
            Self::Models => "models",
            Self::Tools => "tools",
            Self::Decision => "decision",
            Self::Evaluation => "evaluation",
            Self::Extensions => "extensions",
            Self::Search => "search",
            Self::Appearance => "appearance",
            Self::Study => "study",
            Self::Privacy => "privacy",
            Self::About => "about",
        }
    }
}

/// One searchable setting: where it lives, its name, and other words for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub section: Section,
    pub label: &'static str,
    pub keywords: &'static str,
}

const fn e(section: Section, label: &'static str, keywords: &'static str) -> Entry {
    Entry {
        section,
        label,
        keywords,
    }
}

/// Every setting the search finds.
pub const ENTRIES: &[Entry] = &[
    e(
        Section::Providers,
        "API keys",
        "key token secret keychain add replace remove",
    ),
    e(
        Section::Providers,
        "Test connection",
        "check probe works latency",
    ),
    e(
        Section::Providers,
        "Server address",
        "base url endpoint ollama lm studio local proxy port",
    ),
    e(
        Section::Providers,
        "Default provider",
        "use anthropic openai google gemini groq mistral openrouter xai deepseek",
    ),
    e(
        Section::Models,
        "Chat model",
        "default model answer writer reader role",
    ),
    e(
        Section::Models,
        "Vision model",
        "pictures images describe see",
    ),
    e(
        Section::Models,
        "Speech-to-text model",
        "mic voice dictation transcribe whisper audio",
    ),
    e(Section::Models, "OCR model", "read text documents scan pdf"),
    e(
        Section::Models,
        "Embedding model",
        "vectors embeddings retrieval similarity",
    ),
    e(
        Section::Models,
        "Model catalogue",
        "browse list capabilities context window reasoning tools",
    ),
    e(
        Section::Models,
        "Max answer length",
        "output tokens limit length",
    ),
    e(
        Section::Tools,
        "Built-in tools",
        "web search read url vault files workspace skill on off disable",
    ),
    e(
        Section::Tools,
        "Approvals",
        "ask allow deny permission confirm policy",
    ),
    e(
        Section::Decision,
        "Decision model",
        "local jev subq snap voyage rerank rank route",
    ),
    e(
        Section::Evaluation,
        "Evaluation runs",
        "eval suites scores baseline failures benchmark",
    ),
    e(
        Section::Extensions,
        "MCP servers",
        "mcp server tools trust health connect",
    ),
    e(Section::Extensions, "Skills", "skill install folder git"),
    e(
        Section::Extensions,
        "Instructions",
        "tern.md rules system prompt",
    ),
    e(
        Section::Search,
        "Search service",
        "web search wikipedia tavily brave searxng",
    ),
    e(Section::Search, "SearXNG address", "searx self-hosted url"),
    e(
        Section::Search,
        "Tern Crawl",
        "crawler snail robots pacing cache pages fetch",
    ),
    e(Section::Appearance, "Theme", "dark light system mode"),
    e(
        Section::Appearance,
        "Accent",
        "palette colour color lichen slate clay mono",
    ),
    e(Section::Appearance, "Font", "typeface plex system"),
    e(Section::Appearance, "Text size", "small large zoom"),
    e(
        Section::Appearance,
        "Density",
        "compact comfortable spacing rows",
    ),
    e(
        Section::Appearance,
        "Reduce motion",
        "animation motion wipe slide",
    ),
    e(Section::Appearance, "Glyphs", "verdict icons scout"),
    e(
        Section::Study,
        "Quiz questions",
        "quiz count difficulty easy medium hard",
    ),
    e(Section::Study, "Viva questions", "viva oral exam"),
    e(Section::Study, "Study offline", "without a model offline"),
    e(
        Section::Privacy,
        "What leaves this computer",
        "privacy network telemetry provider",
    ),
    e(
        Section::Privacy,
        "Storage location",
        "data folder path vault files where",
    ),
    e(
        Section::Privacy,
        "Delete data",
        "delete clear cache trash reset erase",
    ),
    e(Section::About, "Version", "about update release"),
    e(
        Section::About,
        "Onboarding",
        "welcome tour first run setup again",
    ),
];

/// A search result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub section: Section,
    pub label: &'static str,
}

/// The settings matching `query`, best first: every word must appear in
/// the label, the section's title, or the keywords. A label that starts
/// with the query comes first, then label matches, then the rest.
/// An empty query finds nothing (the section list shows instead).
pub fn search(query: &str) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut scored: Vec<(u8, usize, Hit)> = ENTRIES
        .iter()
        .enumerate()
        .filter_map(|(i, en)| {
            let label = en.label.to_lowercase();
            let title = en.section.title().to_lowercase();
            let keys = en.keywords;
            let all = words
                .iter()
                .all(|w| label.contains(w) || title.contains(w) || keys.contains(w));
            if !all {
                return None;
            }
            let rank = if label.starts_with(&q) {
                0
            } else if words.iter().all(|w| label.contains(w)) {
                1
            } else if words.iter().all(|w| label.contains(w) || title.contains(w)) {
                2
            } else {
                3
            };
            Some((
                rank,
                i,
                Hit {
                    section: en.section,
                    label: en.label,
                },
            ))
        })
        .collect();
    scored.sort_by_key(|(r, i, _)| (*r, *i));
    scored.into_iter().map(|(_, _, h)| h).collect()
}

/// The sections a query touches, in list order (the section list dims the
/// others while searching).
pub fn sections_matching(query: &str) -> Vec<Section> {
    let hits = search(query);
    Section::ALL
        .into_iter()
        .filter(|s| hits.iter().any(|h| h.section == *s))
        .collect()
}

/// The section list's width with labels.
pub const NAV_W: f32 = 212.;
/// The section list folded to icons.
pub const NAV_ICONS_W: f32 = 52.;
/// The widest a settings page grows.
pub const PAGE_MAX_W: f32 = 720.;
/// Below this window width the section list folds to icons, so a page
/// keeps room at 1024 px with the rail.
pub const NAV_FOLD_BELOW: f32 = 1200.;

/// Whether the section list shows labels at this window width.
pub fn nav_labels(window_w: f32) -> bool {
    window_w >= NAV_FOLD_BELOW
}

/// The section list's width at this window width.
pub fn nav_width(window_w: f32) -> f32 {
    if nav_labels(window_w) {
        NAV_W
    } else {
        NAV_ICONS_W
    }
}

/// The Settings screen's own state.
#[derive(Clone, Debug, Default)]
pub struct SettingsState {
    pub section: Section,
    /// What the search box holds.
    pub query: String,
    /// A second click deletes (Privacy's delete buttons ask twice).
    pub confirm: Option<&'static str>,
    /// Models › catalogue: only this capability group, or every group.
    pub group: Option<Group>,
    /// Models › catalogue: what the filter box holds.
    pub filter: String,
    /// Models: the role whose model list is open.
    pub role_open: Option<Role>,
    /// Providers: the provider whose server address is being edited.
    pub editing_url: Option<&'static str>,
}

impl SettingsState {
    /// Opens a section; a pending delete confirmation is dropped.
    pub fn open(&mut self, s: Section) {
        self.section = s;
        self.confirm = None;
    }

    /// The first click on a destructive button arms it; the second, on
    /// the same button, returns `true`.
    pub fn confirm(&mut self, what: &'static str) -> bool {
        if self.confirm == Some(what) {
            self.confirm = None;
            true
        } else {
            self.confirm = Some(what);
            false
        }
    }
}

// ----- models: roles and the catalogue -----

/// A job Tern gives a model, each with its own default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Chat,
    Vision,
    SpeechToText,
    Ocr,
    Embeddings,
    Decision,
}

impl Role {
    pub const ALL: [Self; 6] = [
        Self::Chat,
        Self::Vision,
        Self::SpeechToText,
        Self::Ocr,
        Self::Embeddings,
        Self::Decision,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Vision => "Vision",
            Self::SpeechToText => "Speech to text",
            Self::Ocr => "OCR",
            Self::Embeddings => "Embeddings",
            Self::Decision => "Decision",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Self::Chat => "Answers, plans, and writes reports.",
            Self::Vision => "Describes pictures for a chat model that cannot see.",
            Self::SpeechToText => "Turns the mic into text.",
            Self::Ocr => "Reads the text out of pictures and documents.",
            Self::Embeddings => "Turns text into vectors for retrieval.",
            Self::Decision => "Ranks results and picks the first tool (its own page).",
        }
    }

    /// The capability a model needs for this role. `None` for the decision
    /// model, which is chosen on its own page.
    pub fn capability(self) -> Option<Capability> {
        match self {
            Self::Chat => Some(Capability::Chat),
            Self::Vision => Some(Capability::Vision),
            Self::SpeechToText => Some(Capability::SpeechToText),
            Self::Ocr => Some(Capability::Ocr),
            Self::Embeddings => Some(Capability::Embedding),
            Self::Decision => None,
        }
    }

    /// The saved default, as `provider/model`.
    pub fn saved(self, p: &Preferences) -> Option<String> {
        match self {
            Self::Chat => match (&p.provider, &p.model) {
                (Some(pr), Some(m)) => Some(format!("{pr}/{m}")),
                _ => None,
            },
            Self::Vision => p.default_vision_model.clone(),
            Self::SpeechToText => p.default_stt_model.clone(),
            Self::Ocr => p.default_ocr_model.clone(),
            Self::Embeddings => p.default_embedding_model.clone(),
            Self::Decision => None,
        }
    }

    /// Saves `key` (`provider/model`) as this role's default; `None` goes
    /// back to automatic. A key without a `/` is ignored.
    pub fn save(self, p: &mut Preferences, key: Option<&str>) {
        let key = key.map(str::trim).filter(|k| k.contains('/'));
        let owned = key.map(str::to_owned);
        match self {
            Self::Chat => {
                if let Some((pr, m)) = key.and_then(|k| k.split_once('/')) {
                    p.choose_provider(pr);
                    p.model = Some(m.to_owned());
                } else {
                    p.model = None;
                }
            }
            Self::Vision => p.default_vision_model = owned,
            Self::SpeechToText => p.default_stt_model = owned,
            Self::Ocr => p.default_ocr_model = owned,
            Self::Embeddings => p.default_embedding_model = owned,
            Self::Decision => {}
        }
    }

    /// Whether `m` can take this role.
    pub fn fits(self, m: &ModelInfo) -> bool {
        !m.is_retired() && self.capability().is_some_and(|c| m.has(c))
    }
}

/// The catalogue's capability groups, in the order shown. A model shows
/// in every group it fits, so a vision chat model is under Chat and Vision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Chat,
    Reasoning,
    Vision,
    Tools,
    Speech,
    Ocr,
    Embeddings,
    Images,
    Other,
}

impl Group {
    pub const ALL: [Self; 9] = [
        Self::Chat,
        Self::Reasoning,
        Self::Vision,
        Self::Tools,
        Self::Speech,
        Self::Ocr,
        Self::Embeddings,
        Self::Images,
        Self::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Reasoning => "Reasoning",
            Self::Vision => "Vision",
            Self::Tools => "Tool use",
            Self::Speech => "Speech",
            Self::Ocr => "OCR",
            Self::Embeddings => "Embeddings",
            Self::Images => "Image generation",
            Self::Other => "Other",
        }
    }

    /// Whether `m` belongs in this group.
    pub fn holds(self, m: &ModelInfo) -> bool {
        match self {
            Self::Chat => m.has(Capability::Chat),
            Self::Reasoning => m.has(Capability::Reasoning),
            Self::Vision => m.has(Capability::Vision),
            Self::Tools => m.has(Capability::Tools),
            Self::Speech => m.has(Capability::SpeechToText) || m.has(Capability::TextToSpeech),
            Self::Ocr => m.has(Capability::Ocr),
            Self::Embeddings => m.has(Capability::Embedding),
            Self::Images => m.has(Capability::ImageGen),
            Self::Other => Self::ALL[..8].iter().all(|g| !g.holds(m)),
        }
    }
}

/// One line of the catalogue list: a group header or a model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogRow {
    Header {
        group: Group,
        count: usize,
    },
    Model {
        group: Group,
        provider: &'static str,
        id: String,
        context: Option<u64>,
    },
}

/// The catalogue as a flat list for a virtualised view: each group with
/// models (or only `only`), its header, then its models by provider and
/// id. `filter` keeps models whose id or provider contains it. Retired
/// models are left out.
pub fn catalog_rows(
    models: &[(&'static str, &ModelInfo)],
    only: Option<Group>,
    filter: &str,
) -> Vec<CatalogRow> {
    let f = filter.trim().to_lowercase();
    let mut out = Vec::new();
    for g in Group::ALL {
        if only.is_some_and(|o| o != g) {
            continue;
        }
        let mut ms: Vec<(&'static str, &ModelInfo)> = models
            .iter()
            .copied()
            .filter(|(p, m)| {
                !m.is_retired()
                    && g.holds(m)
                    && (f.is_empty() || m.id.to_lowercase().contains(&f) || p.contains(&f))
            })
            .collect();
        if ms.is_empty() {
            continue;
        }
        ms.sort_by(|a, b| (a.0, &a.1.id).cmp(&(b.0, &b.1.id)));
        out.push(CatalogRow::Header {
            group: g,
            count: ms.len(),
        });
        out.extend(ms.into_iter().map(|(p, m)| CatalogRow::Model {
            group: g,
            provider: p,
            id: m.id.clone(),
            context: m.context_window,
        }));
    }
    out
}

/// The models that can take `role`, as `provider/model` keys.
pub fn role_choices<'a>(
    models: impl Iterator<Item = (&'static str, &'a ModelInfo)>,
    role: Role,
) -> Vec<String> {
    let mut v: Vec<String> = models
        .filter(|(_, m)| role.fits(m))
        .map(|(p, m)| format!("{p}/{}", m.id))
        .collect();
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, caps: &[Capability]) -> ModelInfo {
        let mut m = ModelInfo::new(id);
        m.capabilities = caps.to_vec();
        m
    }

    #[test]
    fn every_section_has_settings_to_find() {
        for s in Section::ALL {
            assert!(
                ENTRIES.iter().any(|e| e.section == s),
                "{s:?} has no searchable entry"
            );
            assert!(!s.title().is_empty() && !s.blurb().is_empty());
        }
        let mut ids: Vec<_> = Section::ALL.iter().map(|s| s.id()).collect();
        ids.dedup();
        assert_eq!(ids.len(), Section::ALL.len());
    }

    #[test]
    fn search_ranks_label_starts_first_and_needs_every_word() {
        assert!(search("").is_empty());
        assert!(search("   ").is_empty());
        let hits = search("theme");
        assert_eq!(hits[0].label, "Theme");
        assert_eq!(hits[0].section, Section::Appearance);
        // A keyword finds the setting.
        let hits = search("whisper");
        assert_eq!(hits[0].label, "Speech-to-text model");
        // Every word must match somewhere.
        assert!(
            search("searxng address")
                .iter()
                .any(|h| h.label == "SearXNG address")
        );
        assert!(search("searxng banana").is_empty());
        // Case does not matter.
        assert_eq!(search("TERN CRAWL")[0].label, "Tern Crawl");
        // The section title counts as a word source.
        assert!(
            search("privacy")
                .iter()
                .all(|h| h.section == Section::Privacy)
        );
        assert_eq!(sections_matching("key"), vec![Section::Providers]);
    }

    #[test]
    fn the_section_list_folds_to_icons_when_narrow() {
        // The four widths he tests at.
        assert!(!nav_labels(1024.));
        assert!(nav_labels(1280.));
        assert!(nav_labels(1366.));
        assert!(nav_labels(1536.));
        assert_eq!(nav_width(1024.), NAV_ICONS_W);
        // A page always keeps room for its rows beside the rail (52 px).
        for w in [1024., 1280., 1366., 1536.] {
            let page = w - 52. - nav_width(w);
            assert!(page >= 560., "{w}: {page}");
        }
    }

    #[test]
    fn deletes_ask_twice() {
        let mut s = SettingsState::default();
        assert!(!s.confirm("cache"));
        assert!(!s.confirm("chats"), "another button re-arms");
        assert!(s.confirm("chats"));
        assert_eq!(s.confirm, None);
        assert!(!s.confirm("cache"));
        s.open(Section::About);
        assert_eq!(s.confirm, None, "leaving the page disarms");
    }

    #[test]
    fn role_defaults_save_and_survive_a_restart() {
        let mut p = Preferences::default();
        for r in Role::ALL {
            assert_eq!(r.saved(&p), None);
        }
        Role::Chat.save(&mut p, Some("groq/llama-3.3-70b"));
        Role::Vision.save(&mut p, Some("openai/gpt-4o"));
        Role::SpeechToText.save(&mut p, Some("groq/whisper-large-v3"));
        Role::Ocr.save(&mut p, Some("mistral/mistral-ocr-latest"));
        Role::Embeddings.save(&mut p, Some("openai/text-embedding-3-small"));
        Role::Embeddings.save(&mut p, Some("no-slash"));
        let json = serde_json::to_string(&p).unwrap();
        let back: Preferences = serde_json::from_str(&json).unwrap();
        assert_eq!(
            Role::Chat.saved(&back).as_deref(),
            Some("groq/llama-3.3-70b")
        );
        assert_eq!(back.provider.as_deref(), Some("groq"));
        assert_eq!(Role::Vision.saved(&back).as_deref(), Some("openai/gpt-4o"));
        assert_eq!(
            Role::SpeechToText.saved(&back).as_deref(),
            Some("groq/whisper-large-v3")
        );
        assert_eq!(
            Role::Ocr.saved(&back).as_deref(),
            Some("mistral/mistral-ocr-latest")
        );
        assert_eq!(
            Role::Embeddings.saved(&back),
            None,
            "ignored key cleared it"
        );
        // Automatic again.
        let mut p = back;
        Role::Vision.save(&mut p, None);
        assert_eq!(p.default_vision_model, None);
        Role::Chat.save(&mut p, None);
        assert_eq!(p.model, None);
        assert_eq!(p.provider.as_deref(), Some("groq"), "provider stays");
    }

    #[test]
    fn the_catalogue_groups_by_capability() {
        let chat = model("a-chat", &[Capability::Chat, Capability::Vision]);
        let emb = model("embed-1", &[Capability::Embedding]);
        let stt = model("whisper", &[Capability::SpeechToText]);
        let odd = model("mystery", &[Capability::Moderation]);
        let list = [
            ("openai", &chat),
            ("openai", &emb),
            ("groq", &stt),
            ("mistral", &odd),
        ];
        let rows = catalog_rows(&list, None, "");
        let headers: Vec<Group> = rows
            .iter()
            .filter_map(|r| match r {
                CatalogRow::Header { group, .. } => Some(*group),
                CatalogRow::Model { .. } => None,
            })
            .collect();
        assert_eq!(
            headers,
            vec![
                Group::Chat,
                Group::Vision,
                Group::Speech,
                Group::Embeddings,
                Group::Other
            ]
        );
        // A chat model that sees is under both.
        let n = rows
            .iter()
            .filter(|r| matches!(r, CatalogRow::Model { id, .. } if id == "a-chat"))
            .count();
        assert_eq!(n, 2);
        // One group only, and a filter.
        let only = catalog_rows(&list, Some(Group::Embeddings), "");
        assert_eq!(only.len(), 2);
        let filtered = catalog_rows(&list, None, "GROQ");
        assert_eq!(filtered.len(), 2);
        assert!(catalog_rows(&list, None, "zzz").is_empty());
        // Role choices only list models that fit.
        assert_eq!(
            role_choices(list.iter().copied(), Role::Embeddings),
            vec!["openai/embed-1".to_owned()]
        );
        assert_eq!(
            role_choices(list.iter().copied(), Role::Vision),
            vec!["openai/a-chat".to_owned()]
        );
        assert!(role_choices(list.iter().copied(), Role::Decision).is_empty());
    }
}
