//! The composer's controls, kept apart from GPUI so they can be tested
//! without a display: the + menu, the mode picker, the model picker with
//! its provider column, the effort (thinking) menu, and the search chip.
//!
//! Row of the composer, left to right: `+` · mode · `@` · `/` … model ·
//! effort · search · context ring · Send/Stop (`docs/02-ui-definition.md`).

use std::collections::HashMap;

use tern_agent::{Effort, Harness, Thinking, TurnMode};

use crate::providers::{Kind, Models, ProvidersState};

/// What a message is sent as. Research and Study each have their own set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// A normal answer; searches and reads when useful.
    #[default]
    Chat,
    /// A plan first; nothing runs until the user approves.
    Plan,
    /// Plan, then parallel workers, then one cited report.
    Deep,
    /// May use tools that change things, each still approved.
    Act,
    Tutor,
    Quiz,
    Flashcards,
    Viva,
    Explain,
}

const RESEARCH: [Mode; 4] = [Mode::Chat, Mode::Plan, Mode::Deep, Mode::Act];
const STUDY: [Mode; 5] = [
    Mode::Tutor,
    Mode::Quiz,
    Mode::Flashcards,
    Mode::Viva,
    Mode::Explain,
];

impl Mode {
    /// The modes a harness offers, in menu order.
    pub fn for_harness(h: Harness) -> &'static [Mode] {
        match h {
            Harness::Research => &RESEARCH,
            Harness::Study => &STUDY,
        }
    }

    /// The first mode of a harness.
    pub fn default_for(h: Harness) -> Self {
        Self::for_harness(h)[0]
    }

    pub fn harness(self) -> Harness {
        if RESEARCH.contains(&self) {
            Harness::Research
        } else {
            Harness::Study
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Plan => "Plan",
            Self::Deep => "Deep research",
            Self::Act => "Act",
            Self::Tutor => "Tutor",
            Self::Quiz => "Quiz",
            Self::Flashcards => "Flashcards",
            Self::Viva => "Viva",
            Self::Explain => "Explain",
        }
    }

    /// One line under the name in the menu.
    pub fn line(self) -> &'static str {
        match self {
            Self::Chat => "Fast answers; searches the web when it helps",
            Self::Plan => "Writes a plan first. Nothing runs until you approve it",
            Self::Deep => "Plans, researches the parts in parallel, writes a cited report",
            Self::Act => "May change things: vault notes, files, MCP tools. Each step asks",
            Self::Tutor => "Explains step by step and checks you follow",
            Self::Quiz => "Asks questions from your materials and marks them",
            Self::Flashcards => "Turns the topic into cards to review",
            Self::Viva => "An oral exam: one question at a time, follow-ups",
            Self::Explain => "Explains a concept plainly, with an example",
        }
    }

    /// What the chat session's tools are limited to for this mode.
    pub fn turn_mode(self) -> TurnMode {
        match self {
            Self::Plan => TurnMode::Plan,
            Self::Act => TurnMode::Act,
            _ => TurnMode::Chat,
        }
    }

    pub fn is_deep(self) -> bool {
        self == Self::Deep
    }

    /// The instruction a Study mode puts before the message, so the same
    /// chat model answers in that style.
    pub fn study_prefix(self) -> Option<&'static str> {
        match self {
            Self::Tutor => Some(
                "Tutor mode: explain step by step, check understanding with one short question at the end.",
            ),
            Self::Quiz => Some(
                "Quiz mode: ask me 3 questions on this, one at a time, and mark each answer before the next.",
            ),
            Self::Flashcards => {
                Some("Flashcards mode: write 6 to 10 flashcards on this as `Q: … / A: …` pairs.")
            }
            Self::Viva => Some(
                "Viva mode: examine me orally on this. Ask one question, wait for my answer, then follow up.",
            ),
            Self::Explain => Some(
                "Explain mode: explain this plainly for a newcomer, with one concrete example.",
            ),
            _ => None,
        }
    }
}

/// Which composer menu is open. One at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Plus,
    Mode,
    Model,
    Effort,
    Search,
}

/// A row of the + menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlusItem {
    Files,
    Images,
    Pdfs,
    Folder,
    Link,
    Resource,
    Paste,
}

impl PlusItem {
    pub const ALL: [PlusItem; 7] = [
        Self::Files,
        Self::Images,
        Self::Pdfs,
        Self::Folder,
        Self::Link,
        Self::Resource,
        Self::Paste,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Images => "Images",
            Self::Pdfs => "PDFs",
            Self::Folder => "Folder or .zip",
            Self::Link => "Link (URL or DOI)",
            Self::Resource => "@ Resource",
            Self::Paste => "Paste from clipboard",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Files => "Any text, code, or document",
            Self::Images => "PNG, JPEG, GIF, WebP; read by models that can see",
            Self::Pdfs => "Text is pulled out page by page",
            Self::Folder => "Every readable file inside",
            Self::Link => "Read through the guarded reader",
            Self::Resource => "Sources, notes, and MCP resources",
            Self::Paste => "Text or a picture on the clipboard",
        }
    }

    /// File extensions the picker keeps (lower case). Empty means any file.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Images => &["png", "jpg", "jpeg", "gif", "webp"],
            Self::Pdfs => &["pdf"],
            Self::Folder => &["zip"],
            _ => &[],
        }
    }

    /// Opens the OS picker (and whether it picks folders too).
    pub fn picker(self) -> Option<bool> {
        match self {
            Self::Files | Self::Images | Self::Pdfs => Some(false),
            Self::Folder => Some(true),
            _ => None,
        }
    }

    /// Keeps the picked paths this item asked for; other kinds are dropped
    /// and counted so the view can say so.
    pub fn keep(self, paths: Vec<std::path::PathBuf>) -> (Vec<std::path::PathBuf>, usize) {
        let exts = self.extensions();
        if exts.is_empty() {
            return (paths, 0);
        }
        let mut dropped = 0;
        let kept = paths
            .into_iter()
            .filter(|p| {
                let ok = p.is_dir()
                    || p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| exts.contains(&e.to_ascii_lowercase().as_str()));
                if !ok {
                    dropped += 1;
                }
                ok
            })
            .collect();
        (kept, dropped)
    }
}

/// A rough guide for the picker's hover card, read from the model's name:
/// small and fast, or large and capable. Bars are 1 to 5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelHint {
    pub speed: u8,
    pub intelligence: u8,
}

pub fn model_hint(id: &str) -> ModelHint {
    let m = id.to_ascii_lowercase();
    let has = |ks: &[&str]| ks.iter().any(|k| m.contains(k));
    let small = has(&[
        "nano", "mini", "lite", "flash", "haiku", "8b", "3b", "1b", "small", "instant", "tiny",
    ]);
    let large = has(&[
        "opus",
        "pro",
        "large",
        "405b",
        "70b",
        "gpt-5",
        "o3",
        "sonnet",
        "grok-4",
        "reasoner",
        "magistral-medium",
    ]);
    let (speed, intelligence) = match (small, large) {
        (true, false) => (5, 2),
        (false, true) => (2, 5),
        (true, true) => (4, 3),
        (false, false) => (3, 3),
    };
    ModelHint {
        speed,
        intelligence,
    }
}

/// A provider in the picker's left column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderEntry {
    pub id: &'static str,
    pub name: &'static str,
    /// Models listed, once loaded.
    pub count: Option<usize>,
    pub current: bool,
}

/// One line in the picker's model list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerRow {
    pub id: String,
    /// The chat's model now: drawn with a tick.
    pub current: bool,
    /// Starred: listed first, its star always shown (others on hover).
    pub favourite: bool,
    /// "128k", muted on the right; `None` when no size is known.
    pub context: Option<String>,
}

/// The picker popover's width, its most height, and its provider column.
pub const PICKER_W: f32 = 460.;
pub const PICKER_MAX_H: f32 = 360.;
pub const PICKER_LEFT_W: f32 = 148.;

/// The popover's size in a window: the design size, shrunk to leave a
/// 16 px margin on each side of a small window.
pub fn picker_size(window_w: f32, window_h: f32) -> (f32, f32) {
    let w = PICKER_W.min((window_w - 32.).max(240.));
    let h = PICKER_MAX_H.min((window_h - 160.).max(200.));
    (w, h)
}

/// The model picker's own state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelPicker {
    /// The provider whose models the right column shows.
    pub provider: Option<&'static str>,
    pub query: String,
    /// The model under the pointer, for the hover card.
    pub hovered: Option<String>,
}

/// The whole composer's state.
#[derive(Clone, Debug, Default)]
pub struct Composer {
    pub menu: Option<Menu>,
    pub mode: Mode,
    /// Each chat remembers its mode, by session id.
    pub modes: HashMap<String, Mode>,
    pub effort: Option<Effort>,
    pub picker: ModelPicker,
    /// The inline Link field from the + menu is open.
    pub link_open: bool,
    /// Model counts per provider, as lists load.
    pub counts: HashMap<&'static str, usize>,
    /// A one-line note under the menu ("2 files were not PDFs").
    pub note: Option<String>,
}

impl Composer {
    /// Opens `menu`, or closes it when it is the open one.
    pub fn toggle(&mut self, menu: Menu) {
        self.menu = if self.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        if self.menu != Some(Menu::Model) {
            self.picker.hovered = None;
        }
        self.note = None;
    }

    pub fn close(&mut self) {
        self.menu = None;
        self.picker.hovered = None;
    }

    /// Picks a mode for the current chat (and remembers it for `chat`).
    pub fn set_mode(&mut self, mode: Mode, chat: Option<&str>) {
        self.mode = mode;
        if let Some(c) = chat {
            self.modes.insert(c.to_owned(), mode);
        }
        self.menu = None;
    }

    /// A chat opened: its remembered mode, or the harness's first.
    pub fn chat_opened(&mut self, chat: Option<&str>, harness: Harness) {
        self.mode = chat
            .and_then(|c| self.modes.get(c).copied())
            .filter(|m| m.harness() == harness)
            .unwrap_or_else(|| Mode::default_for(harness));
    }

    /// The harness changed: keep the mode if it belongs, else its first.
    pub fn harness_changed(&mut self, harness: Harness) {
        if self.mode.harness() != harness {
            self.mode = Mode::default_for(harness);
        }
    }

    /// A draft chat got its session id: it keeps the mode it was sent in.
    pub fn remember(&mut self, chat: &str) {
        self.modes.insert(chat.to_owned(), self.mode);
    }

    /// The provider column: every provider that can be used, current first.
    pub fn providers(&self, st: &ProvidersState) -> Vec<ProviderEntry> {
        let current = st.default_id(Kind::Model);
        let mut out: Vec<ProviderEntry> = st
            .section(Kind::Model)
            .filter(|r| r.usable())
            .map(|r| ProviderEntry {
                id: r.id,
                name: r.name,
                count: self.counts.get(r.id).copied(),
                current: Some(r.id) == current,
            })
            .collect();
        out.sort_by_key(|e| !e.current);
        out
    }

    /// The provider the right column shows.
    pub fn shown_provider(&self, st: &ProvidersState) -> Option<&'static str> {
        self.picker.provider.or_else(|| st.default_id(Kind::Model))
    }

    /// The models of the shown provider that match the search, favourites
    /// first. `None` while the list loads or when it failed.
    pub fn models<'a>(&self, st: &'a ProvidersState) -> Option<Vec<&'a str>> {
        let shown = self.shown_provider(st)?;
        let Some(Models::Loaded(p, list)) = &st.models else {
            return None;
        };
        if *p != shown {
            return None;
        }
        let q = self.picker.query.trim().to_ascii_lowercase();
        let favs = &st.prefs.favourite_models;
        let mut out: Vec<&str> = list
            .iter()
            .map(|m| m.id.as_str())
            .filter(|id| q.is_empty() || id.to_ascii_lowercase().contains(&q))
            .collect();
        out.sort_by_key(|id| !favs.iter().any(|f| f == &format!("{shown}/{id}")));
        Some(out)
    }

    /// The context window of a model: the listing's, else Tern's table.
    pub fn context_of(st: &ProvidersState, model: &str) -> Option<u64> {
        match &st.models {
            Some(Models::Loaded(_, list)) => list
                .iter()
                .find(|m| m.id == model)
                .and_then(|m| m.context_window),
            _ => None,
        }
        .or_else(|| tern_agent::known_context_window(model))
    }

    /// The picker's model rows: one line each, favourites first, the
    /// current model ticked, its window as a short muted size.
    pub fn picker_rows(
        &self,
        st: &ProvidersState,
        current: Option<&str>,
    ) -> Option<Vec<PickerRow>> {
        let shown = self.shown_provider(st)?;
        let current_provider = st.default_id(crate::providers::Kind::Model);
        let favs = &st.prefs.favourite_models;
        Some(
            self.models(st)?
                .into_iter()
                .map(|id| PickerRow {
                    id: id.to_owned(),
                    current: Some(shown) == current_provider && current == Some(id),
                    favourite: favs.iter().any(|f| *f == format!("{shown}/{id}")),
                    context: Self::context_of(st, id).map(crate::model::compact),
                })
                .collect(),
        )
    }

    /// A list loaded: its count joins the column.
    pub fn models_loaded(&mut self, provider: &'static str, n: usize) {
        self.counts.insert(provider, n);
    }
}

/// Stars or unstars `provider/model` in the saved favourites.
pub fn toggle_favourite(favs: &mut Vec<String>, provider: &str, model: &str) -> bool {
    let key = format!("{provider}/{model}");
    if let Some(i) = favs.iter().position(|f| *f == key) {
        favs.remove(i);
        false
    } else {
        favs.push(key);
        true
    }
}

/// The effort chip: what it shows and what the menu offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffortChip {
    /// Bars lit, 0 to 5.
    pub bars: u8,
    pub label: String,
    pub tooltip: String,
    pub adjustable: bool,
    /// Whether the chip opens the effort menu. Off for models with no
    /// thinking: the chip still shows, so the control is never a mystery.
    pub enabled: bool,
}

/// The chip for the current model. Always shown; disabled when the model
/// has no thinking control.
pub fn effort_chip(thinking: Thinking, effort: Option<Effort>) -> EffortChip {
    if !thinking.visible() {
        return EffortChip {
            bars: 0,
            label: "Effort".into(),
            tooltip: "This model has no thinking control".into(),
            adjustable: false,
            enabled: false,
        };
    }
    if !thinking.adjustable() {
        return EffortChip {
            bars: 5,
            label: "Thinks".into(),
            tooltip: "This model always thinks before it answers; there is nothing to set".into(),
            adjustable: false,
            enabled: true,
        };
    }
    let e = effort.unwrap_or_default();
    let e = if e == Effort::None && !thinking.can_turn_off() {
        Effort::Low
    } else {
        e
    };
    EffortChip {
        bars: e.bars(),
        label: e.label().into(),
        tooltip: format!("Thinking effort: {} ({})", e.label(), thinking.knob(e)),
        adjustable: true,
        enabled: true,
    }
}

/// The effort menu's rows: step, the provider setting it sends, and whether
/// it can be picked.
pub fn effort_rows(thinking: Thinking) -> Vec<(Effort, String, bool)> {
    Effort::ALL
        .into_iter()
        .map(|e| {
            let ok = thinking.adjustable() && (e != Effort::None || thinking.can_turn_off());
            (e, thinking.knob(e), ok)
        })
        .collect()
}

/// The search chip's label: the search service, or `web: off`.
pub fn search_label(search: Option<&str>) -> String {
    match search {
        Some(s) => s.to_owned(),
        None => "Web off".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ModelChoice, ServiceRow};

    fn st() -> ProvidersState {
        let mut st = ProvidersState::default();
        let row = |id: &'static str, name: &'static str, saved: bool, local: bool| ServiceRow {
            kind: Kind::Model,
            id,
            name,
            local,
            keyless: false,
            key_saved: saved,
            key_url: "",
        };
        st.load(
            vec![
                row("openai", "OpenAI", true, false),
                row("mistral", "Mistral", true, false),
                row("groq", "Groq", false, false),
                row("ollama", "Ollama", false, true),
            ],
            tern_agent::Preferences {
                provider: Some("mistral".into()),
                ..Default::default()
            },
            None,
        );
        st.models = Some(Models::Loaded(
            "mistral",
            [
                "ministral-8b-latest",
                "magistral-medium-latest",
                "mistral-large-latest",
            ]
            .iter()
            .map(|id| ModelChoice {
                id: (*id).into(),
                context_window: Some(128_000),
            })
            .collect(),
        ));
        st
    }

    #[test]
    fn modes_per_harness_and_per_chat() {
        assert_eq!(Mode::for_harness(Harness::Research).len(), 4);
        assert_eq!(Mode::for_harness(Harness::Study).len(), 5);
        let mut c = Composer::default();
        c.set_mode(Mode::Plan, Some("a"));
        c.chat_opened(Some("b"), Harness::Research);
        assert_eq!(c.mode, Mode::Chat, "a new chat starts in Chat");
        c.chat_opened(Some("a"), Harness::Research);
        assert_eq!(c.mode, Mode::Plan, "a chat keeps its mode");
        c.harness_changed(Harness::Study);
        assert_eq!(c.mode, Mode::Tutor);
        assert_eq!(Mode::Plan.turn_mode(), TurnMode::Plan);
        assert_eq!(Mode::Act.turn_mode(), TurnMode::Act);
        assert_eq!(Mode::Deep.turn_mode(), TurnMode::Chat);
        assert!(Mode::Deep.is_deep());
        assert!(Mode::Quiz.study_prefix().unwrap().starts_with("Quiz mode"));
        assert!(Mode::Chat.study_prefix().is_none());
        for m in Mode::for_harness(Harness::Research) {
            assert!(!m.line().is_empty());
        }
    }

    #[test]
    fn one_menu_at_a_time() {
        let mut c = Composer::default();
        c.toggle(Menu::Plus);
        assert_eq!(c.menu, Some(Menu::Plus));
        c.toggle(Menu::Model);
        assert_eq!(c.menu, Some(Menu::Model));
        c.toggle(Menu::Model);
        assert_eq!(c.menu, None);
    }

    #[test]
    fn plus_menu_items_filter_what_they_ask_for() {
        assert_eq!(PlusItem::ALL.len(), 7);
        let (kept, dropped) =
            PlusItem::Pdfs.keep(vec!["a.pdf".into(), "b.PDF".into(), "c.txt".into()]);
        assert_eq!(kept.len(), 2);
        assert_eq!(dropped, 1);
        let (kept, _) = PlusItem::Files.keep(vec!["c.txt".into()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(PlusItem::Folder.picker(), Some(true));
        assert_eq!(PlusItem::Link.picker(), None);
        assert!(PlusItem::Images.extensions().contains(&"webp"));
    }

    #[test]
    fn picker_lists_usable_providers_current_first_and_searches() {
        let st = st();
        let mut c = Composer::default();
        let cols = c.providers(&st);
        let ids: Vec<_> = cols.iter().map(|e| e.id).collect();
        assert_eq!(ids, ["mistral", "openai", "ollama"], "no key, not listed");
        assert!(cols[0].current);
        c.models_loaded("mistral", 3);
        assert_eq!(c.providers(&st)[0].count, Some(3));
        assert_eq!(c.models(&st).unwrap().len(), 3);
        c.picker.query = "MAGI".into();
        assert_eq!(c.models(&st).unwrap(), ["magistral-medium-latest"]);
        // Another provider's column waits for its list.
        c.picker.provider = Some("openai");
        assert!(c.models(&st).is_none());
        assert_eq!(
            Composer::context_of(&st, "ministral-8b-latest"),
            Some(128_000)
        );
    }

    #[test]
    fn favourites_go_first_and_toggle() {
        let mut st = st();
        assert!(toggle_favourite(
            &mut st.prefs.favourite_models,
            "mistral",
            "mistral-large-latest"
        ));
        let c = Composer::default();
        assert_eq!(c.models(&st).unwrap()[0], "mistral-large-latest");
        assert!(!toggle_favourite(
            &mut st.prefs.favourite_models,
            "mistral",
            "mistral-large-latest"
        ));
        assert!(st.prefs.favourite_models.is_empty());
    }

    #[test]
    fn hints_read_size_from_the_name() {
        assert!(model_hint("gpt-4.1-nano").speed > model_hint("claude-opus-4-1").speed);
        assert!(
            model_hint("claude-opus-4-1").intelligence
                > model_hint("gemini-2.5-flash-lite").intelligence
        );
    }

    #[test]
    fn effort_chip_follows_the_model() {
        let plain = tern_agent::thinking_support("mistral", "ministral-8b-latest");
        let chip = effort_chip(plain, Some(Effort::High));
        assert!(!chip.enabled, "shown but disabled on plain models");
        assert_eq!(chip.tooltip, "This model has no thinking control");
        assert_eq!(chip.bars, 0);
        let always = tern_agent::thinking_support("mistral", "magistral-medium-latest");
        let chip = effort_chip(always, None);
        assert!(chip.enabled && !chip.adjustable);
        assert!(effort_rows(always).iter().all(|r| !r.2));
        let o3 = tern_agent::thinking_support("openai", "o3");
        let chip = effort_chip(o3, Some(Effort::None));
        assert!(chip.enabled && chip.adjustable);
        assert_eq!(chip.label, "Low", "o3 cannot turn thinking off");
        assert_eq!(chip.bars, 1);
        let rows = effort_rows(o3);
        assert_eq!(rows.len(), 6);
        assert!(!rows[0].2 && rows[1].2);
        let claude = tern_agent::thinking_support("anthropic", "claude-sonnet-4-5");
        let chip = effort_chip(claude, Some(Effort::High));
        assert_eq!(chip.bars, 3);
        assert!(chip.tooltip.contains("budget 12,000 tokens"));
        assert!(effort_rows(claude)[0].2, "Claude can turn thinking off");
        for (prov, model) in [("google", "gemini-2.5-pro"), ("openai", "o4-mini")] {
            assert!(
                effort_chip(tern_agent::thinking_support(prov, model), None).adjustable,
                "{model}"
            );
        }
    }

    #[test]
    fn search_chip_says_off() {
        assert_eq!(search_label(None), "Web off");
        assert_eq!(search_label(Some("SearXNG")), "SearXNG");
    }
}
