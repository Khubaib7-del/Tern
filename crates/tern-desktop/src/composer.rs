//! The composer's controls, kept apart from GPUI so they can be tested
//! without a display: the + menu, the mode picker, the model picker with
//! its provider column, the effort (thinking) menu, and the search chip.
//!
//! Row of the composer, left to right: `+` · mode · `@` · `/` … model ·
//! effort · search · context ring · Send/Stop (`docs/02-ui-definition.md`).

use std::collections::HashMap;

use tern_agent::{Effort, Harness, Thinking, TurnMode};

use tern_agent::providers::Category;

use crate::providers::{Kind, ModelChoice, Models, ProvidersState};

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
    /// The provider's name for it ("Mistral Small 4"), when it differs
    /// from the id.
    pub label: Option<String>,
    /// The chat's model now: drawn with a tick.
    pub current: bool,
    /// Starred: listed first in its section, its star always shown.
    pub favourite: bool,
    /// "128k", muted on the right; `None` when no size is known or the
    /// model does not chat.
    pub context: Option<String>,
    /// The section it is listed under.
    pub category: Category,
    /// Small capability badges ("vision", "tools", "speech to text").
    pub badges: Vec<&'static str>,
    /// Whether it can be the chat model.
    pub chat: bool,
    /// Other names the provider takes for it (`mistral-large-latest`).
    pub aliases: Vec<String>,
    /// Deprecated or retired (shown only with "Show retired").
    pub retired: bool,
}

/// What clicking a model in the picker does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickOutcome {
    /// It becomes the chat's model.
    Chat,
    /// A speech-to-text model: it becomes the mic's model; the chat model
    /// stays. The notice says so.
    Mic(String),
    /// An OCR model: it reads attachments the chat model cannot see.
    Ocr(String),
    /// It cannot chat and Tern has no job for it; the chat model stays.
    Refused(String),
}

/// What picking `info` from the picker should do.
pub fn pick_outcome(provider: &str, info: &tern_agent::providers::ModelInfo) -> PickOutcome {
    use tern_agent::media::{Job, can_do};
    let Some(reason) = info.not_chat_reason() else {
        return PickOutcome::Chat;
    };
    if can_do(Job::Transcribe, provider, info) {
        PickOutcome::Mic(reason)
    } else if can_do(Job::Ocr, provider, info) {
        PickOutcome::Ocr(reason)
    } else {
        PickOutcome::Refused(reason)
    }
}

/// The picker popover's width, its most height, and its provider column.
pub const PICKER_W: f32 = 560.;
pub const PICKER_MAX_H: f32 = 360.;
pub const PICKER_LEFT_W: f32 = 148.;

/// The toast for picking a speech-to-text model: it becomes the mic's
/// model (the chat keeps its model).
pub fn mic_pick_note(model: &str, provider: &str) -> String {
    format!("Voice input now uses {model} ({provider}). The chat keeps its model.")
}

/// The toast for picking an OCR model.
pub fn ocr_pick_note(model: &str, provider: &str) -> String {
    format!("{model} ({provider}) now reads documents and pictures. The chat keeps its model.")
}

/// What a model pick does while the chat cannot switch yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickTiming {
    /// Switch now.
    Now,
    /// An answer is running: switch when it ends.
    AfterAnswer,
    /// Another switch is landing: switch right after it.
    AfterSwitch,
}

/// When a picked model can take over: now, after the running answer, or
/// after the switch already under way.
pub fn pick_timing(busy: bool, connecting: bool) -> PickTiming {
    if busy {
        PickTiming::AfterAnswer
    } else if connecting {
        PickTiming::AfterSwitch
    } else {
        PickTiming::Now
    }
}

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
    /// Lists deprecated and retired models too.
    pub show_retired: bool,
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
    /// The menu a press outside just closed, and when: the same press
    /// landing on that menu's own chip must not open it again.
    pub closed_outside: Option<(Menu, std::time::Instant)>,
}

/// A chip click this soon after a press outside closed its menu belongs to
/// that same press.
pub const REOPEN_GUARD: std::time::Duration = std::time::Duration::from_millis(350);

impl Composer {
    /// A press outside the open menu closes it (remembering which, so the
    /// chip's own click in the same press does not reopen it).
    pub fn close_outside(&mut self, now: std::time::Instant) {
        if let Some(m) = self.menu {
            self.closed_outside = Some((m, now));
            self.close();
        }
    }

    /// The chip for `menu` was clicked at `now`: opens or closes it, except
    /// that a click finishing the press that just closed it does nothing,
    /// so one click is one change.
    pub fn toggle_at(&mut self, menu: Menu, now: std::time::Instant) {
        if let Some((m, at)) = self.closed_outside.take()
            && m == menu
            && self.menu.is_none()
            && now.saturating_duration_since(at) < REOPEN_GUARD
        {
            return;
        }
        self.toggle(menu);
    }

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

    /// The models of the shown provider that match the search (id, name,
    /// or alias), by section (chat, vision, speech, OCR, image,
    /// embeddings, safety, other), favourites first in each. Deprecated
    /// and retired models only with "Show retired". `None` while the list
    /// loads or when it failed.
    #[cfg(test)]
    pub fn models<'a>(&self, st: &'a ProvidersState) -> Option<Vec<&'a str>> {
        Some(
            self.choices(st)?
                .into_iter()
                .map(|m| m.id.as_str())
                .collect(),
        )
    }

    fn choices<'a>(&self, st: &'a ProvidersState) -> Option<Vec<&'a ModelChoice>> {
        let shown = self.shown_provider(st)?;
        let Some(Models::Loaded(p, list)) = &st.models else {
            return None;
        };
        if *p != shown {
            return None;
        }
        let q = self.picker.query.trim();
        let favs = &st.prefs.favourite_models;
        let mut out: Vec<&ModelChoice> = list
            .iter()
            .filter(|m| self.picker.show_retired || !m.info.is_retired())
            .filter(|m| m.info.matches(q))
            .collect();
        out.sort_by_key(|m| {
            (
                m.info.category(),
                !favs.iter().any(|f| f == &format!("{shown}/{}", m.id)),
            )
        });
        Some(out)
    }

    /// How many of the shown provider's models are deprecated or retired.
    pub fn retired_count(&self, st: &ProvidersState) -> usize {
        match &st.models {
            Some(Models::Loaded(_, list)) => list.iter().filter(|m| m.info.is_retired()).count(),
            _ => 0,
        }
    }

    /// A listed model of the shown provider, by id.
    pub fn info_of<'a>(
        st: &'a ProvidersState,
        model: &str,
    ) -> Option<&'a tern_agent::providers::ModelInfo> {
        match &st.models {
            Some(Models::Loaded(_, list)) => list.iter().find(|m| m.id == model).map(|m| &m.info),
            _ => None,
        }
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

    /// The picker's model rows: one line each, by section, favourites
    /// first in a section, the current model ticked, its window as a short
    /// muted size, capability badges, aliases.
    pub fn picker_rows(
        &self,
        st: &ProvidersState,
        current: Option<&str>,
    ) -> Option<Vec<PickerRow>> {
        let shown = self.shown_provider(st)?;
        let current_provider = st.default_id(crate::providers::Kind::Model);
        let favs = &st.prefs.favourite_models;
        Some(
            self.choices(st)?
                .into_iter()
                .map(|m| {
                    let id = m.id.as_str();
                    let info = &m.info;
                    PickerRow {
                        id: id.to_owned(),
                        label: Some(info.label()).filter(|l| *l != id).map(str::to_owned),
                        current: Some(shown) == current_provider && current == Some(id),
                        favourite: favs.iter().any(|f| *f == format!("{shown}/{id}")),
                        context: info
                            .can_chat()
                            .then(|| Self::context_of(st, id).map(crate::model::compact))
                            .flatten(),
                        category: info.category(),
                        badges: info
                            .capabilities
                            .iter()
                            .filter(|c| **c != tern_agent::providers::Capability::Chat)
                            .map(|c| c.badge())
                            .collect(),
                        chat: info.can_chat(),
                        aliases: info.aliases.clone(),
                        retired: info.is_retired(),
                    }
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
/// it can be picked. The slider replaced the menu; kept for the tests that
/// pin which steps a model allows.
#[cfg(test)]
pub fn effort_rows(thinking: Thinking) -> Vec<(Effort, String, bool)> {
    Effort::ALL
        .into_iter()
        .map(|e| {
            let ok = thinking.adjustable() && (e != Effort::None || thinking.can_turn_off());
            (e, thinking.knob(e), ok)
        })
        .collect()
}

/// The effort slider: a compact popover over the effort chip (about
/// 300 × 110, like ChatGPT's and Codex's): the level's name on top, one
/// plain line under it, a reset at the top right, and below them a
/// rounded box holding a row of thin ticks. Ticks up to the level light
/// up warm, brightest at the handle, and a slim glowing bar handle glides
/// between the six stops.
///
/// | Size | Value |
/// | --- | --- |
/// | Popover | 300 px wide, 12 px padding |
/// | Tick box | 276 px, 11 px radius, 8 px / 12 px inner padding |
/// | Track | 252 px; the first and last stops sit 8 px in (the handle's halo) |
/// | Ticks | 41 (8 between stops), 2 px wide, 12 px tall |
/// | Handle | 4 × 22 px bar, rounded ends, halo 14 px |
pub const SLIDER_W: f32 = 300.;
pub const TRACK_W: f32 = 252.;
pub const TRACK_INSET: f32 = 8.;
/// Ticks in the row; stop `s` sits on tick `s * TICKS_PER_STOP`.
pub const TICKS: usize = 41;
pub const TICKS_PER_STOP: usize = (TICKS - 1) / LAST_STOP;
/// The last stop's index (Max); there are six stops.
pub const LAST_STOP: usize = 5;

/// What the slider can do for the current model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliderKind {
    /// No thinking control: the slider shows greyed.
    NoControl,
    /// Always thinks: one fixed state, nothing to set.
    Fixed,
    /// Movable from stop `min` (1 when thinking cannot be turned off).
    Adjustable { min: usize },
}

pub fn slider_kind(thinking: Thinking) -> SliderKind {
    if !thinking.visible() {
        SliderKind::NoControl
    } else if !thinking.adjustable() {
        SliderKind::Fixed
    } else {
        SliderKind::Adjustable {
            min: usize::from(!thinking.can_turn_off()),
        }
    }
}

/// A stop's index (None = 0 … Max = 5).
pub fn stop_of(e: Effort) -> usize {
    Effort::ALL.iter().position(|&x| x == e).unwrap_or(0)
}

pub fn effort_at(stop: usize) -> Effort {
    Effort::ALL[stop.min(LAST_STOP)]
}

/// The stop the slider shows for the chosen effort (the chip's rule: a
/// model that cannot turn thinking off shows Low for None).
pub fn shown_stop(thinking: Thinking, effort: Option<Effort>) -> usize {
    match slider_kind(thinking) {
        SliderKind::NoControl => 0,
        SliderKind::Fixed => LAST_STOP,
        SliderKind::Adjustable { min } => stop_of(effort.unwrap_or_default()).max(min),
    }
}

/// A stop's centre on a track `track_w` wide, from its left edge.
pub fn stop_x(stop: f32, track_w: f32) -> f32 {
    TRACK_INSET + stop.clamp(0., LAST_STOP as f32) * (track_w - 2. * TRACK_INSET) / LAST_STOP as f32
}

/// Tick `i`'s centre on a track `track_w` wide, from its left edge.
pub fn tick_x(i: usize, track_w: f32) -> f32 {
    stop_x(i as f32 / TICKS_PER_STOP as f32, track_w)
}

/// The pointer at `x` (from the track's left edge) as a fractional stop,
/// kept to what the model allows: where the handle is drawn while it is
/// dragged, before it snaps. `None` when the slider cannot move.
pub fn drag_stop(x: f32, track_w: f32, kind: SliderKind) -> Option<f32> {
    let SliderKind::Adjustable { min } = kind else {
        return None;
    };
    let step = (track_w - 2. * TRACK_INSET) / LAST_STOP as f32;
    Some(((x - TRACK_INSET) / step).clamp(min as f32, LAST_STOP as f32))
}

/// How one tick is drawn with the handle at fractional stop `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickLook {
    /// 0 = the dim resting grey, 1 = fully lit.
    pub heat: f32,
    /// Along the warm ramp: 0 = soft amber, 1 = the accent.
    pub warmth: f32,
}

/// Ticks that glow past the handle, fading out.
const SPILL_TICKS: f32 = 3.;

/// Tick `i`'s look with the handle at fractional stop `at`. Ticks before
/// the handle are lit, warming from amber at the far left to the accent
/// at the handle and brightest beside it; a few ticks past the handle
/// catch a fading glow; the rest rest grey. Plain arithmetic, so it runs
/// for every tick on every frame of a drag without allocating.
pub fn tick_look(i: usize, at: f32) -> TickLook {
    let h = at.clamp(0., LAST_STOP as f32) * TICKS_PER_STOP as f32;
    let i = i as f32;
    if i < h - 0.5 {
        let d = h - i;
        TickLook {
            heat: (1. - d / 14.).max(0.6),
            warmth: if h <= 0. { 1. } else { (i / h).clamp(0., 1.) },
        }
    } else {
        let d = (i - h).max(0.);
        TickLook {
            heat: (0.5 * (1. - d / SPILL_TICKS)).max(0.),
            warmth: 1.,
        }
    }
}

/// How many ticks are lit with the handle resting on `stop`.
#[cfg(test)]
pub fn lit_ticks(stop: usize) -> usize {
    (0..TICKS)
        .filter(|&i| tick_look(i, stop as f32).heat >= 0.6)
        .count()
}

/// The stop nearest `x` (from the track's left edge), kept to what the
/// model allows; `None` when the slider cannot move.
pub fn snap_stop(x: f32, track_w: f32, kind: SliderKind) -> Option<usize> {
    let SliderKind::Adjustable { min } = kind else {
        return None;
    };
    let at = drag_stop(x, track_w, kind)?.round() as usize;
    Some(at.max(min))
}

/// Left and Right keys: one stop over, kept to what the model allows.
pub fn step_stop(from: usize, right: bool, kind: SliderKind) -> Option<usize> {
    let SliderKind::Adjustable { min } = kind else {
        return None;
    };
    Some(if right {
        (from + 1).min(LAST_STOP)
    } else {
        from.saturating_sub(1).max(min)
    })
}

/// One plain line under the level's name; no provider settings.
pub fn effort_line(e: Effort) -> &'static str {
    match e {
        Effort::None => "Answers straight away",
        Effort::Low => "Thinks briefly",
        Effort::Medium => "Thinks it through",
        Effort::High => "Thinks carefully",
        Effort::XHigh => "Thinks deeply; slower",
        Effort::Max => "Thinks as long as it needs",
    }
}

/// The slider's title and line for the current model.
pub fn slider_title(thinking: Thinking, effort: Option<Effort>) -> (String, &'static str) {
    match slider_kind(thinking) {
        SliderKind::NoControl => ("Effort".into(), "This model has no thinking control"),
        SliderKind::Fixed => (
            "Always thinks".into(),
            "This model thinks before every answer",
        ),
        SliderKind::Adjustable { .. } => {
            let e = effort_at(shown_stop(thinking, effort));
            (e.label().into(), effort_line(e))
        }
    }
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

    #[test]
    fn one_click_on_the_effort_chip_is_one_change() {
        let t0 = std::time::Instant::now();
        let mut c = Composer::default();
        c.toggle_at(Menu::Effort, t0);
        assert_eq!(c.menu, Some(Menu::Effort));
        // Pressing the chip while open: the press closes it (outside the
        // slider), and the click of that same press must not reopen it.
        c.close_outside(t0 + std::time::Duration::from_millis(500));
        c.toggle_at(Menu::Effort, t0 + std::time::Duration::from_millis(560));
        assert_eq!(c.menu, None);
        // A later, separate click opens it again.
        c.toggle_at(Menu::Effort, t0 + std::time::Duration::from_secs(2));
        assert_eq!(c.menu, Some(Menu::Effort));
        // A press outside that closed another menu does not swallow this one.
        c.close_outside(t0 + std::time::Duration::from_secs(3));
        c.toggle_at(Menu::Model, t0 + std::time::Duration::from_millis(3050));
        assert_eq!(c.menu, Some(Menu::Model));
    }

    #[test]
    fn picks_wait_for_a_running_answer_or_switch() {
        assert_eq!(pick_timing(false, false), PickTiming::Now);
        assert_eq!(pick_timing(true, false), PickTiming::AfterAnswer);
        assert_eq!(pick_timing(false, true), PickTiming::AfterSwitch);
        assert!(
            mic_pick_note("whisper-large-v3", "Groq")
                .starts_with("Voice input now uses whisper-large-v3 (Groq)")
        );
    }

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
            .map(|id| {
                let mut info = tern_agent::providers::ModelInfo::new(*id);
                info.context_window = Some(128_000);
                ModelChoice::new(info)
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
    fn picker_is_a_compact_popover_with_one_line_rows() {
        let mut st = st();
        st.prefs.favourite_models = vec!["mistral/mistral-large-latest".into()];
        if let Some(Models::Loaded(_, list)) = &mut st.models {
            // A listing without a size falls back to Tern's table.
            list[1].context_window = None;
        }
        let c = Composer::default();
        let rows = c.picker_rows(&st, Some("ministral-8b-latest")).unwrap();
        assert_eq!(rows[0].id, "mistral-large-latest", "favourites first");
        assert!(rows[0].favourite && !rows[0].current);
        let current = rows.iter().find(|r| r.current).unwrap();
        assert_eq!(current.id, "ministral-8b-latest");
        assert_eq!(current.context.as_deref(), Some("128k"));
        let magistral = rows.iter().find(|r| r.id.starts_with("magistral")).unwrap();
        assert_eq!(magistral.context.as_deref(), Some("128k"), "from the table");
        assert_eq!(rows.iter().filter(|r| r.current).count(), 1);
        // The popover keeps its design size in a normal window and shrinks
        // in a small one, never past the window.
        assert_eq!(picker_size(1280., 820.), (PICKER_W, PICKER_MAX_H));
        assert!(picker_size(500., 820.).0 <= 468.);
        let (w, h) = picker_size(400., 420.);
        assert!(w <= 400. - 32. && h <= 420.);
        const { assert!(PICKER_LEFT_W < PICKER_W / 2.) };
    }

    #[test]
    fn picker_groups_every_model_and_flags_non_chat_ones() {
        let mut st = st();
        let mk = |id: &str| {
            ModelChoice::new({
                let mut m = tern_agent::providers::ModelInfo::new(id);
                m.finish("mistral");
                m
            })
        };
        let mut old = mk("mistral-ocr-2505");
        old.info.lifecycle = tern_agent::providers::Lifecycle::Deprecated;
        if let Some(Models::Loaded(_, list)) = &mut st.models {
            list.extend([
                mk("voxtral-mini-transcribe-3-0"),
                mk("mistral-ocr-4-1"),
                mk("pixtral-12b-2409"),
                mk("mistral-embed"),
                mk("mistral-small-2603"),
                old,
            ]);
        }
        let mut c = Composer::default();
        let rows = c.picker_rows(&st, None).unwrap();
        let cats: Vec<_> = rows.iter().map(|r| r.category).collect();
        let mut sorted = cats.clone();
        sorted.sort();
        assert_eq!(cats, sorted, "rows come by section");
        assert!(
            !rows.iter().any(|r| r.id == "mistral-ocr-2505"),
            "retired hidden"
        );
        assert_eq!(c.retired_count(&st), 1);
        let stt = rows.iter().find(|r| r.id.starts_with("voxtral")).unwrap();
        assert_eq!((stt.category, stt.chat), (Category::Speech, false));
        assert_eq!(stt.badges, ["speech to text"]);
        assert!(stt.context.is_none(), "no window for a non-chat model");
        let small = rows.iter().find(|r| r.id == "mistral-small-2603").unwrap();
        assert_eq!(small.label.as_deref(), Some("Mistral Small 4"));
        assert_eq!(small.category, Category::Vision);
        c.picker.query = "small 4".into();
        assert_eq!(c.models(&st).unwrap(), ["mistral-small-2603"]);
        c.picker.query.clear();
        c.picker.show_retired = true;
        assert!(c.models(&st).unwrap().contains(&"mistral-ocr-2505"));
        // Picking a non-chat model keeps the chat model and says why.
        let info = |id: &str| Composer::info_of(&st, id).unwrap().clone();
        let PickOutcome::Mic(why) = pick_outcome("mistral", &info("voxtral-mini-transcribe-3-0"))
        else {
            panic!()
        };
        assert!(
            why.contains("only transcribes speech") && why.contains("mic"),
            "{why}"
        );
        assert!(matches!(
            pick_outcome("mistral", &info("mistral-ocr-4-1")),
            PickOutcome::Ocr(_)
        ));
        assert!(matches!(
            pick_outcome("mistral", &info("mistral-embed")),
            PickOutcome::Refused(_)
        ));
        assert_eq!(
            pick_outcome("mistral", &info("pixtral-12b-2409")),
            PickOutcome::Chat
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
    fn the_effort_slider_snaps_steps_and_knows_the_model() {
        let claude = tern_agent::thinking_support("anthropic", "claude-sonnet-4-5");
        let kind = slider_kind(claude);
        assert_eq!(kind, SliderKind::Adjustable { min: 0 });
        // Each stop's own x snaps back to it, and so does anything nearer
        // it than its neighbours.
        for i in 0..=LAST_STOP {
            let x = stop_x(i as f32, TRACK_W);
            assert_eq!(snap_stop(x, TRACK_W, kind), Some(i));
            if i < LAST_STOP {
                assert_eq!(snap_stop(x + 20., TRACK_W, kind), Some(i), "right of {i}");
            }
            if i > 0 {
                assert_eq!(snap_stop(x - 20., TRACK_W, kind), Some(i), "left of {i}");
            }
        }
        assert_eq!(stop_x(0., TRACK_W), TRACK_INSET);
        assert_eq!(stop_x(5., TRACK_W), TRACK_W - TRACK_INSET);
        // Past either end it holds the end stop.
        assert_eq!(snap_stop(-50., TRACK_W, kind), Some(0));
        assert_eq!(snap_stop(9_000., TRACK_W, kind), Some(LAST_STOP));
        // Keys move one stop and stop at the ends.
        assert_eq!(step_stop(2, true, kind), Some(3));
        assert_eq!(step_stop(2, false, kind), Some(1));
        assert_eq!(step_stop(LAST_STOP, true, kind), Some(LAST_STOP));
        assert_eq!(step_stop(0, false, kind), Some(0));
        // o3 cannot turn thinking off: None is out of reach.
        let o3t = tern_agent::thinking_support("openai", "o3");
        let o3 = slider_kind(o3t);
        assert_eq!(o3, SliderKind::Adjustable { min: 1 });
        assert_eq!(snap_stop(0., TRACK_W, o3), Some(1));
        assert_eq!(step_stop(1, false, o3), Some(1));
        assert_eq!(shown_stop(o3t, Some(Effort::None)), 1);
        // No control: nothing moves, and the popover says why.
        let plain = tern_agent::thinking_support("mistral", "ministral-8b-latest");
        assert_eq!(slider_kind(plain), SliderKind::NoControl);
        assert_eq!(snap_stop(100., TRACK_W, SliderKind::NoControl), None);
        assert_eq!(step_stop(2, true, SliderKind::NoControl), None);
        assert_eq!(
            slider_title(plain, None).1,
            "This model has no thinking control"
        );
        // Always thinks: one fixed state.
        let always = tern_agent::thinking_support("mistral", "magistral-medium-latest");
        assert_eq!(slider_kind(always), SliderKind::Fixed);
        assert_eq!(snap_stop(0., TRACK_W, SliderKind::Fixed), None);
        // Titles are plain words, never the provider's setting.
        let (title, line) = slider_title(claude, Some(Effort::Low));
        assert_eq!((title.as_str(), line), ("Low", "Thinks briefly"));
        for e in Effort::ALL {
            assert!(!effort_line(e).contains(':'));
            assert_eq!(effort_at(stop_of(e)), e);
        }
        assert_eq!(effort_at(stop_of(Effort::default())), Effort::Medium);
    }

    #[test]
    fn the_effort_ticks_light_up_to_the_level() {
        // About forty thin ticks, a stop on every eighth.
        assert_eq!(TICKS, 41);
        assert_eq!(TICKS_PER_STOP, 8);
        for s in 0..=LAST_STOP {
            assert_eq!(
                tick_x(s * TICKS_PER_STOP, TRACK_W),
                stop_x(s as f32, TRACK_W),
                "stop {s} sits on a tick"
            );
        }
        assert_eq!(tick_x(0, TRACK_W), TRACK_INSET);
        assert_eq!(tick_x(TICKS - 1, TRACK_W), TRACK_W - TRACK_INSET);
        // Lit ticks per level: none for None, all forty before Max.
        let lit: Vec<usize> = (0..=LAST_STOP).map(lit_ticks).collect();
        assert_eq!(lit, vec![0, 8, 16, 24, 32, 40]);
        // Warm ramp: amber at the far left, the accent at the handle, and
        // brightest beside it.
        let at = 3.;
        let first = tick_look(0, at);
        let beside = tick_look(23, at);
        assert!(first.warmth < 0.05, "{first:?}");
        assert!(beside.warmth > 0.9, "{beside:?}");
        assert!(beside.heat > first.heat);
        assert!(beside.heat <= 1.);
        // A fading glow just past the handle, then grey.
        let past: Vec<f32> = (24..30).map(|i| tick_look(i, at).heat).collect();
        assert!(past.windows(2).all(|w| w[1] <= w[0]), "{past:?}");
        assert!(past[1] > 0. && past[1] < 0.5);
        assert_eq!(past[4], 0.);
        assert_eq!(tick_look(TICKS - 1, 0.).heat, 0.);
        // The colour moves with the handle between stops.
        assert!(tick_look(26, 3.4).heat > tick_look(26, 3.).heat);
    }

    #[test]
    fn the_effort_handle_follows_the_drag_and_snaps_on_release() {
        let kind = SliderKind::Adjustable { min: 0 };
        let step = (TRACK_W - 2. * TRACK_INSET) / LAST_STOP as f32;
        // While dragged the handle sits where the pointer is...
        let x = TRACK_INSET + 2.3 * step;
        let at = drag_stop(x, TRACK_W, kind).unwrap();
        assert!((at - 2.3).abs() < 1e-4, "{at}");
        // ...and the release snaps it to the nearest stop.
        assert_eq!(snap_stop(x, TRACK_W, kind), Some(2));
        assert_eq!(snap_stop(TRACK_INSET + 2.6 * step, TRACK_W, kind), Some(3));
        // Clamped to the ends and to what the model allows.
        assert_eq!(drag_stop(-40., TRACK_W, kind), Some(0.));
        assert_eq!(drag_stop(1e4, TRACK_W, kind), Some(LAST_STOP as f32));
        let o3 = SliderKind::Adjustable { min: 1 };
        assert_eq!(drag_stop(0., TRACK_W, o3), Some(1.));
        assert_eq!(drag_stop(50., TRACK_W, SliderKind::NoControl), None);
        assert_eq!(drag_stop(50., TRACK_W, SliderKind::Fixed), None);
    }

    #[test]
    fn search_chip_says_off() {
        assert_eq!(search_label(None), "Web off");
        assert_eq!(search_label(Some("SearXNG")), "SearXNG");
    }
}
