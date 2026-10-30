//! The main window: title bar with the harness switch, icon rail, chat list,
//! transcript, composer, and the run panel.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::hover_card::HoverCard;
use gpui_kit::component::input::{Input, InputEvent, InputState, TextareaState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::{TextView, TextViewMotion};
use gpui_kit::component::{Disableable as _, IconName, Selectable as _, Sizable as _, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, Entity, FontWeight, Hsla, ScrollHandle,
    SharedString, Subscription, Task, Window, div, px, svg,
};
use tern_agent::Role;
use tern_agent::attach::Source as AttachSource;
use tern_agent::verify::Stance;
use tern_agent::{
    ApprovalAnswer, CancelFlag, ChatSession, Harness, PageRef, RawFilter, Run, RunEnd, RunStep,
    StepKind, Verdict,
};
use tern_core::{SecretString, SessionId};
use tokio::sync::{mpsc, oneshot};

use crate::assets;
use crate::bridge::{Bridge, Msg};
use press::Capsule as _;
mod appearance_view;
mod browser_view;
mod cards_view;
mod commands_view;
mod composer_view;
mod continuity_view;
mod customize_view;
mod decision_view;
mod deep_view;
mod editor_view;
mod eval_view;
mod explain_view;
mod export_view;
mod files_view;
mod flow_view;
mod focus_view;
mod folders_view;
mod frame;
mod git_view;
mod intro_view;
mod library_view;
mod localmodels_view;
mod onboarding_view;
mod palette_view;
mod panels_view;
mod permissions_view;
mod pet_view;
mod plan_view;
mod press;
mod read_aloud_view;
mod runmap_view;
mod runs_view;
mod scout_view;
mod segmented;
mod settings_view;
mod setup_view;
mod study_view;
mod timeline_view;
mod toast_view;
pub(crate) mod tour;
mod work_view;
pub use pet_view::Shell;

use crate::layout::{Layout, Tab, Tabs};
use crate::model::{
    Activity, Connection, Entry, Model, Phase, Pick, RailTab, RunView, Screen, SiteIcon,
    WorkingLine,
};
use crate::motion::{self, Slide, ThemeWipe};
use crate::providers::{Kind, Models, Note, ServiceRow};
use crate::theme::Palette;

/// Rail icons that can carry the active marker (Chats, Library, Editor,
/// Browser, Git, Settings).
const RAIL_SLOTS: usize = 6;
pub(crate) const RAIL_W: f32 = 52.;
/// The right rail's width while it shows the run map.
const GRAPH_W: f32 = 440.;
/// The `@` menu's tallest; longer lists scroll.
const MENTION_MENU_MAX_H: f32 = 340.;
pub(crate) const COLUMN_MAX_W: f32 = 760.;
/// The toast when the mic is clicked and no provider lists a voice model.
pub const NO_VOICE_MODEL: &str = "No voice model available — download one in Settings › Local models, or add Groq, OpenAI or Mistral";

/// How much of an event's payload the Raw view shows when opened.
const RAW_PAYLOAD_CHARS: usize = 4_000;

/// Sets the composer's text with the caret at its end (#68). The input's
/// own `set_value` puts a multi-line input's caret at the start, so `@`
/// from the button, a picked mention or command, or a restored draft
/// left the caret before the text.
pub(crate) fn set_end(
    c: &mut TextareaState,
    text: impl AsRef<str>,
    window: &mut Window,
    cx: &mut Context<TextareaState>,
) {
    let text: String = text.as_ref().to_owned();
    let end = text.len();
    c.set_value(text, window, cx);
    c.set_selected_range(end..end, cx);
}

/// A chat whose run goes on while another chat is on screen (#71): its
/// live state and what the window holds for its run (stop, approval,
/// plan), until it is opened again. Its session comes back here when the
/// run ends.
struct BgRun {
    slot: crate::model::ChatSlot,
    cancel: CancelFlag,
    pending: Option<oneshot::Sender<ApprovalAnswer>>,
    plan_reply: Option<oneshot::Sender<tern_agent::PlanAnswer>>,
    plan_inputs: Vec<Entity<InputState>>,
    plan_dropped: HashSet<usize>,
    /// A plan that arrived while the chat was put away; shown on return.
    plan: Option<(
        tern_agent::ResearchPlan,
        oneshot::Sender<tern_agent::PlanAnswer>,
    )>,
    deep_control: tern_agent::DeepControl,
    queued: Option<String>,
    session: Option<Box<ChatSession>>,
}

/// The one window of the app.
pub struct Workspace {
    model: Model,
    bridge: Bridge,
    /// The mic, while it records.
    recording: Option<tern_agent::platform::mic::Recording>,
    session: Option<Box<ChatSession>>,
    current: Option<SessionId>,
    /// The open chats.
    tabs: Tabs,
    /// The layout at the last frame, to notice the window narrowing.
    last_layout: Option<Layout>,
    composer: Entity<TextareaState>,
    /// The one key field; only one row edits at a time.
    key_input: Entity<InputState>,
    /// The SearXNG server address on Providers.
    searx_input: Entity<InputState>,
    /// The chat each harness last had open, so switching back reopens it.
    parked: std::collections::HashMap<tern_agent::Harness, SessionId>,
    /// Where each harness was left (chat, draft, scroll, panels), kept in
    /// the preferences so it survives a restart (`crate::harness`).
    harness_memory: crate::harness::Memory,
    /// The place to put back once the harness's chat has loaded.
    pending_restore: Option<(tern_agent::Harness, tern_agent::prefs::LastChat)>,
    /// The browser's page engine (`crate::webview`).
    web: crate::webview::Shared,
    /// Reads the browser helper while it runs (`watch_web`).
    web_timer: Option<Task<()>>,
    web_ticking: bool,
    /// The run panel's Graph view: pointer and draw-in state.
    graph: runmap_view::GraphUi,
    /// Research flow v2: claim review, citation cards, splash (`flow_view`).
    flow: flow_view::FlowUi,
    /// The harness switcher's popover is open, and when it opened.
    switcher: Option<std::time::Instant>,
    /// The title field of the chat being renamed in the list.
    rename_input: Entity<InputState>,
    renaming: Option<SessionId>,
    cancel: CancelFlag,
    /// Answers whose source list is expanded, by transcript index.
    open_sources: HashSet<usize>,
    pending: Option<oneshot::Sender<ApprovalAnswer>>,
    /// The waiting deep research plan's answer channel.
    plan_reply: Option<oneshot::Sender<tern_agent::PlanAnswer>>,
    /// Chats whose runs go on while another chat is open (#71).
    background: std::collections::HashMap<SessionId, BgRun>,
    /// A put-away chat's plan to show once its chat is open again.
    replay_plan: Option<(
        tern_agent::ResearchPlan,
        oneshot::Sender<tern_agent::PlanAnswer>,
    )>,
    /// The plan card's text fields, one per sub-question.
    plan_inputs: Vec<Entity<InputState>>,
    /// Sub-questions dropped in the plan card's edit mode, by position.
    plan_dropped: HashSet<usize>,
    /// Stops one deep research worker, or the whole run.
    deep_control: tern_agent::DeepControl,
    scroll: ScrollHandle,
    ticker: Option<Task<()>>,
    /// Motion state: the side panels' slides and a theme switch in flight.
    sidebar_slide: Slide,
    panel_slide: Slide,
    wipe: Option<ThemeWipe>,
    /// Scout's rig (its mood clock), when it was last stepped, and the
    /// last beat it saw.
    scout: crate::scout::Rig,
    scout_at: Option<Instant>,
    scout_beat: usize,
    /// Scout's sprites (`crate::pet`), the state picked on Settings ›
    /// Scout (or by the tour), the preview's own clock, and whether Scout
    /// is hidden and which trim it wears (`None` follows the harness).
    pet_manifest: std::rc::Rc<crate::pet::Manifest>,
    pet_preview: Option<crate::pet::State>,
    /// The UI tour shows Scout's hover toolbar as if pointed at.
    pet_tour_toolbar: bool,
    pet_preview_anim: std::cell::RefCell<crate::pet::Animator>,
    scout_hidden: bool,
    scout_variant: Option<String>,
    /// The flashcard stack's swipe (`crate::swipe`).
    card_swipe: std::cell::RefCell<crate::swipe::Swipe>,
    /// Pressed controls' springs, for the press jiggle (`view/press.rs`).
    presses: press::Presses,
    /// Where each culled row of a long Settings page last sat
    /// (`crate::cull`).
    cull_rows: std::rc::Rc<std::cell::RefCell<crate::cull::Rows>>,
    /// Decoded thumbnails of pictures waiting in the composer, by
    /// `crate::thumbs::key` (decoding base64 every frame would be waste).
    thumbs: std::cell::RefCell<std::collections::HashMap<String, std::sync::Arc<gpui_kit::Image>>>,
    /// The empty state's mark (`crate::mark`), and when it started playing.
    mark_anim: std::rc::Rc<crate::mark::MarkAnim>,
    mark_since: Instant,
    /// The harness the empty-state mark last played for.
    mark_harness: tern_agent::Harness,
    /// Wakes Settings › Scout's preview or the empty state's mark for its
    /// next strip frame.
    pet_preview_wake: Option<Task<()>>,
    /// The approved plan's step count and its fold spring.
    plan_folded: Option<(usize, crate::motion::Spring)>,
    /// Rows that have grown in already, per list (`view/segmented.rs`).
    grown: std::cell::RefCell<segmented::Grown>,
    /// Each toast's spring, by toast id.
    toast_springs: std::cell::RefCell<std::collections::HashMap<u64, crate::motion::Presence>>,
    /// The window's height this frame, for placing overlays.
    window_h: f32,
    /// This view's own id, for measuring canvases that ask for a frame
    /// when what they measured moved (`view/frame.rs`).
    view_id: gpui_kit::EntityId,
    /// Where measured elements (tabs, rail tabs) were laid out last frame,
    /// by (group, index), for sliding underlines (`view/segmented.rs`).
    measured: Measured,
    /// Where each rail icon and the rail itself were laid out last frame
    /// (window coordinates), for the sliding active marker.
    rail_slots: std::rc::Rc<
        std::cell::RefCell<[Option<gpui_kit::Bounds<gpui_kit::Pixels>>; RAIL_SLOTS + 1]>,
    >,
    reduced_motion: bool,
    /// The Appearance choices, applied and saved on each click.
    look: crate::appearance::Appearance,
    _listener: Task<()>,
    /// The built-in browser's address bar.
    address_input: Entity<InputState>,
    /// The Git screen's text fields.
    git_inputs: git_view::GitInputs,
    /// The Editor screen's text field.
    note_input: Entity<TextareaState>,
    /// Runs due watches in the background while the window is open.
    _watch_timer: Task<()>,
    /// The Study screen's text fields.
    study_inputs: study_view::StudyInputs,
    /// Ticks the viva clock.
    study_ticker: Option<Task<()>>,
    /// Takes the keys on the flashcard review and in explainer tabs.
    keys: gpui_kit::FocusHandle,
    /// Explain in 3D tabs, by key, and the next key.
    explains: std::collections::HashMap<u32, crate::explain::ExplainState>,
    /// What each explainer was asked, for Try again.
    explain_asks: std::collections::HashMap<u32, tern_agent::explain::Ask>,
    next_explain: u32,
    /// When the explainer's tilt last eased.
    explain_at: Option<Instant>,
    /// Steps explainers on autoplay.
    explain_ticker: Option<Task<()>>,
    /// Repaints the Explain loading line while a scene builds.
    explain_loader: Option<Task<()>>,
    /// The side chat's question field.
    side_input: Entity<InputState>,
    /// The Library's search field.
    library_input: Entity<InputState>,
    /// The popped-out side chat window.
    side_window: Option<gpui_kit::AnyWindowHandle>,
    /// Customize's and the project tree's text fields.
    ext_inputs: customize_view::ExtInputs,
    /// The fields of the MCP prompt or template form, one per argument.
    arg_inputs: Vec<Entity<InputState>>,
    arg_subs: Vec<Subscription>,
    /// The model picker's search field and the + menu's Link field.
    picker_input: Entity<InputState>,
    link_input: Entity<InputState>,
    /// A message queued with Tab while a run goes; sent when it ends.
    queued: Option<String>,
    /// Text to put in the composer at the next frame: the draft of the
    /// chat just opened (chat switches run where no window is at hand).
    composer_restore: Option<String>,
    /// Redraws transcription progress (`tick_audio`).
    audio_ticker: Option<Task<()>>,
    /// When the reveal ticker last ran.
    reveal_at: Option<Instant>,
    /// The chat column's width at the last render, for the tab strip.
    centre_w: f32,
    /// Where the chat area starts from the window's left, at the last
    /// render, for placing toasts beside the message column.
    centre_x: f32,
    /// The window's width at the last render.
    window_w: f32,
    /// The right pane's width at the last render, for its header.
    side_w: f32,
    /// The browser's share of the room beside the sidebar (#68: half),
    /// whether opening it closed the sidebar (to reopen on close), and
    /// whether it showed last frame.
    browser_share: f32,
    browser_hid_sidebar: bool,
    browser_was_open: bool,
    /// The run panel's ⋯ menu of views that do not fit is open.
    run_more_open: bool,
    /// Sliding highlights and knobs, by control, and whether one moved
    /// this frame (so the frame after is drawn too).
    glides: std::cell::RefCell<std::collections::HashMap<&'static str, crate::motion::Glide>>,
    /// Profiling only (the UI tour's `perf` run): parts of Settings ›
    /// Providers left out, to measure what each costs per frame.
    /// 1 logos, 2 the model list, 4 the row buttons. Always 0 otherwise.
    pub(super) perf_strip: u8,
    anim_pending: std::cell::Cell<bool>,
    /// The effort slider's track: its left edge in the window at the last
    /// paint, and whether the knob is being dragged.
    effort_track: std::rc::Rc<std::cell::Cell<f32>>,
    /// The effort handle's fractional stop while it is dragged.
    effort_drag: Option<f32>,
    /// When the context popover opened, for its sweep-in.
    ctx_opened: std::rc::Rc<std::cell::Cell<Option<Instant>>>,
    /// The tab strip's "▾" menu of tabs that do not fit is open.
    tabs_menu_open: bool,
    /// The tab strip's + popover, with its search field, while open.
    tab_picker: Option<(Entity<InputState>, Subscription)>,
    /// The + popover's highlighted row (0 is New chat).
    tab_pick_sel: usize,
    /// When a tab-strip popover last closed on a press outside it, so the
    /// press on its own button does not reopen it at once.
    popover_closed_at: Option<Instant>,
    /// The saved chat tabs were put back after launch.
    tabs_restored: bool,
    /// The window as last saved for the next launch (`crate::restore`).
    shown_saved: crate::restore::Shown,
    /// Saves the window shortly after it last changed.
    shown_task: Option<Task<()>>,
    /// The other harness's chat tabs while this one is in front.
    parked_tabs: crate::layout::ParkedTabs,
    /// A model picked while the chat was busy, applied when it is free.
    pending_pick: Option<(&'static str, String)>,
    /// Streaming dictation while the mic records (and until its last
    /// window comes back): the state, the voice model, the speech so far
    /// (16 kHz mono), how far into the recording it has read, and the
    /// timer that sends windows.
    dictation: Option<crate::dictation::Dictation>,
    dictation_model: Option<tern_agent::media::ModelRef>,
    /// The fast English model and the Whisper one for this recording,
    /// and which takes the next window (#68: Settings › Speech ›
    /// Dictation language).
    dictation_fast: Option<tern_agent::media::ModelRef>,
    dictation_whisper: Option<tern_agent::media::ModelRef>,
    dictation_route: crate::dictation::Route,
    /// Auto dictation (#70): the dictation language is Auto, a language
    /// check is on its way, the phrase start it checked last, when, the
    /// recording's number (stale checks are dropped), and the last
    /// language heard (kept two minutes, so the next recording starts on
    /// the right model).
    dictation_auto: bool,
    dictation_checking: bool,
    dictation_check_from: Option<usize>,
    dictation_last_check: Option<Instant>,
    dictation_rec: u64,
    dictation_lang_memo: Option<(String, f32, Instant)>,
    /// Auto moved the recording to Whisper and no Whisper text is back
    /// yet: the composer says "Switching to Urdu…" (#71).
    dictation_switching: Option<String>,
    /// Where dictated text goes in the composer (follows the text).
    dictation_caret: crate::dictation::Caret,
    dictation_samples: Vec<f32>,
    dictation_pos: usize,
    dictation_task: Option<Task<()>>,
    /// The answer read aloud (#69) and the timer that follows it.
    read_aloud: Option<read_aloud_view::ReadingNow>,
    read_aloud_task: Option<Task<()>>,
    /// Wakes the window when a toast should come, go, or fade.
    toast_wake: Option<Task<()>>,
    /// The `@` menu's scroll, so the highlighted row stays in view.
    mention_scroll: ScrollHandle,
    /// The Files tab's scroll (its long trees are culled against it).
    files_scroll: ScrollHandle,
    /// Checks the Files tab's folder for changes while it shows.
    _folder_poll: Task<()>,
    /// The workbench board, checked every 120 ms (#75).
    _live_poll: Task<()>,
    /// Tool cards, Terminal and write preview state (#75).
    work: work_view::WorkUi,
    /// The timeline tick under the pointer, and a smooth scroll in flight
    /// (from, to, started).
    rail_hover: Option<usize>,
    /// The rail already opened by itself for the run under way.
    rail_auto: bool,
    /// The Ctrl+K command palette, while open.
    cmdk: Option<palette_view::CmdK>,
    rail_scroll: Option<(f32, f32, Instant)>,
    /// A chat that was shown but whose model could not be reached: picking
    /// a model reopens it instead of starting a new chat.
    resume_retry: Option<SessionId>,
    /// The scrolling child index of the first transcript entry (after the
    /// workspace chip), set as the transcript is built.
    rail_base: std::cell::Cell<usize>,
    /// Settings' search, server-address and filter fields, and its lists'
    /// scroll positions.
    settings_inputs: settings_view::SettingsInputs,
    /// Custom providers, crawlers, instructions, MCP and skill forms.
    setup_inputs: setup_view::SetupInputs,
    /// The first-run tour, while it shows.
    onboarding: Option<crate::onboarding::Onboarding>,
    /// Whether the tour was considered at launch (once per run).
    onboarding_checked: bool,
    /// The first-run intro (#76), from when it started, while it plays.
    intro: Option<Instant>,
    /// Where the cursor is over the intro, as fractions of the window.
    intro_cursor: Option<(f32, f32)>,
    /// The intro was replayed from Settings › About (#82, a temporary dev
    /// hook): the welcome tour follows it.
    intro_replay: bool,
    /// The block focus view (#76), while a block is open in it.
    focus: Option<Box<focus_view::FocusUi>>,
    /// The scripted UI tour, when `TERN_TOUR` is set (`view::tour`).
    tour: Option<tour::Tour>,
    _subscriptions: Vec<Subscription>,
}

/// Measured bounds by (group, index), shared with paint-time canvases.
type Measured = std::rc::Rc<
    std::cell::RefCell<
        std::collections::HashMap<(&'static str, usize), gpui_kit::Bounds<gpui_kit::Pixels>>,
    >,
>;

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// How long Auto dictation keeps the language it heard for the next
/// recording (#70).
const LANG_MEMO: Duration = Duration::from_secs(120);
/// Auto checks a new phrase's language at most this often (#70).
const RECHECK_EVERY: Duration = Duration::from_secs(10);

impl Workspace {
    /// What is on screen, for the frame meter (`crate::perf`).
    fn perf_tag(&self) -> String {
        match self.model.screen {
            Screen::Settings => format!("settings/{}", self.model.settings.section.id()),
            s => format!("{s:?}").to_ascii_lowercase(),
        }
    }

    pub fn new(
        bridge: Bridge,
        mut rx: mpsc::UnboundedReceiver<Msg>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // A citation chip in an answer opens its page in Tern's browser
        // (#72).
        {
            let this = cx.entity().downgrade();
            crate::richmd::set_cite_opener(move |url, window, app| {
                let url = url.to_owned();
                let _ = this.update(app, |w, cx| w.browse(&url, window, cx));
            });
        }
        // A block's Export ▾ in an answer saves through the window (#72
        // round 2): it knows the chat's folder and shows the toast.
        {
            let this = cx.entity().downgrade();
            crate::blockexport::set_saver(move |job, window, app| {
                let _ = this.update(app, |w, cx| w.save_block(job, window, cx));
            });
        }
        // A block's Expand opens it in the focus view (#76).
        {
            let this = cx.entity().downgrade();
            crate::blockexport::set_focus_opener(move |block, window, app| {
                let _ = this.update(app, |w, cx| w.open_focus(block, window, cx));
            });
        }
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .placeholder(
                    "Ask anything. / for commands and prompts, @ to reference a file, source, or resource.",
                )
                .submit_on_enter(true)
        });
        let sub = cx.subscribe_in(
            &composer,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => {
                    if this.model.slash.is_some() && !this.model.slash_rows().is_empty() {
                        this.choose_slash(None, window, cx);
                    } else if this.model.mention.is_some() {
                        this.choose_mention(None, window, cx);
                    } else {
                        this.submit(window, cx);
                    }
                }
                InputEvent::Change => {
                    this.check_long_text(window, cx);
                    let text = this.composer.read(cx).value().to_string();
                    this.model.chat_here = this.current;
                    this.model.on_composer_text(&text);
                    cx.notify();
                }
                _ => {}
            },
        );
        let key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste the API key")
        });
        let searx_input = cx.new(|cx| {
            let mut i = InputState::new(window, cx).placeholder("http://localhost:8080");
            if let Some(u) = tern_agent::Preferences::load().search_url {
                i.set_value(u, window, cx);
            }
            i
        });
        let key_sub = cx.subscribe_in(
            &key_input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save_key(window, cx);
                }
            },
        );
        let note_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(true)
                .placeholder("Write in Markdown. Link notes with [[Note name]].")
        });
        let note_sub = cx.subscribe_in(
            &note_input,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| match event {
                InputEvent::Change => this.note_edited(cx),
                InputEvent::Blur => {
                    this.save_note(cx);
                    cx.notify();
                }
                _ => {}
            },
        );
        let address_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search, or type an address like arxiv.org")
        });
        let address_sub = cx.subscribe_in(
            &address_input,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let typed = input.read(cx).value().to_string();
                    this.browse(&typed, window, cx);
                }
            },
        );
        let git_inputs = git_view::GitInputs::new(window, cx);
        let rename_input = cx.new(|cx| InputState::new(window, cx).placeholder("Chat title"));
        let rename_sub = cx.subscribe_in(
            &rename_input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.finish_rename(window, cx),
                _ => {}
            },
        );
        let side_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Ask about this chat"));
        let side_sub = cx.subscribe_in(
            &side_input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.side_submit(window, cx);
                }
            },
        );
        let library_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search the Library, or a message ID like Q12.3")
        });
        let library_sub = cx.subscribe_in(
            &library_input,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.library_typed(cx);
                }
            },
        );
        let picker_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search models"));
        let picker_sub = cx.subscribe_in(
            &picker_input,
            window,
            |this: &mut Self, input, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.model.composer.picker.query = input.read(cx).value().to_string();
                    cx.notify();
                }
            },
        );
        let link_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://… or a DOI like 10.48550/arXiv.1706.03762")
        });
        let link_sub = cx.subscribe_in(
            &link_input,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let typed = input.read(cx).value().to_string();
                    this.attach_link(&typed, window, cx);
                }
            },
        );
        let listener = cx.spawn_in(window, async move |this, cx| {
            while let Some(msg) = rx.recv().await {
                if this
                    .update_in(cx, |this, window, cx| this.on_msg(msg, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        // Open the way Tern was left: the harness, and in it the chat of
        // the tab that was in front (a deleted one falls back to the last
        // chat with messages, then a new draft).
        let shown = {
            let prefs = tern_agent::Preferences::load();
            let shown = crate::restore::Shown::from_prefs(&prefs.session);
            let start =
                crate::restore::start(prefs.open_tabs.get(crate::harness::key(shown.harness)));
            bridge.launch_start(shown.harness, start);
            shown
        };
        // The window is saved for the next launch as it closes, too.
        {
            let weak = cx.entity().downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                let _ = weak.update(cx, |this, cx| this.save_session_now(cx));
                true
            });
        }
        // The provider list decides whether the first-run tour shows.
        bridge.load_providers();
        bridge.load_watches();
        // Due watches run a minute after start, then every 15 minutes, one
        // batch at a time.
        let watch_timer = cx.spawn_in(window, async move |this, cx| {
            let mut wait = Duration::from_secs(60);
            loop {
                cx.background_executor().timer(wait).await;
                wait = Duration::from_secs(15 * 60);
                let alive = this.update(cx, |this, cx| {
                    if this.model.watches_running == 0 && !this.model.watches.is_empty() {
                        this.model.watches_running += 1;
                        this.bridge.run_watches(Vec::new());
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        // The Files tab's folder, re-read when it changes on disk (#74).
        let folder_poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(files_view::FOLDER_POLL)
                    .await;
                if this.update(cx, |this, cx| this.poll_folder(cx)).is_err() {
                    break;
                }
            }
        });
        // The workbench: commands' output, a file being written,
        // show_in_panel requests (#75).
        let live_poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(work_view::LIVE_POLL).await;
                if this.update(cx, |this, cx| this.poll_live(cx)).is_err() {
                    break;
                }
            }
        });
        tern_agent::workbench::set_settings(tern_agent::Preferences::load().workbench());
        let mut model = Model::new();
        let look = crate::appearance::Appearance::from_prefs(&tern_agent::Preferences::load());
        model.dark = look.is_dark(system_dark(window));
        crate::theme::apply_kit(model.dark, &look, Some(window), cx);
        crate::theme::set_reduced_motion(
            tern_agent::Preferences::load().wants_reduced_motion(),
            cx,
        );
        model.study = crate::study::StudyState::from_prefs(&tern_agent::Preferences::load());
        let (study_inputs, study_subs) = study_view::StudyInputs::new(window, cx);
        let (ext_inputs, ext_subs) = customize_view::ExtInputs::new(window, cx);
        let (settings_inputs, settings_subs) = settings_view::SettingsInputs::new(window, cx);
        let setup_inputs = setup_view::SetupInputs::new(window, cx);
        let harness_memory = crate::harness::Memory::from_prefs(&tern_agent::Preferences::load());
        model.projects.collapsed = tern_agent::Preferences::load()
            .collapsed_projects
            .into_iter()
            .collect();
        bridge.projects(crate::bridge::ProjectChange::Reload);
        if let Ok(v) = tern_agent::vault::Vault::open_default() {
            let root = v.root().to_path_buf();
            model.editor.attach(v);
            let refresh = crate::git::GitOp::Refresh;
            if model.git.start(&refresh) {
                bridge.git(root, refresh);
            }
        }
        {
            let prefs = tern_agent::Preferences::load();
            model.panels =
                crate::panels::Panels::from_prefs(prefs.sidebar_width, prefs.right_pane_width);
            model.composer.effort = prefs.effort();
            model.run_panel_auto = prefs.run_panel_auto.unwrap_or(true);
        }
        // The harness, Study tab, right panel and sidebar as they were.
        model.choose_harness(shown.harness);
        model.composer.harness_changed(shown.harness);
        model.study.set_tab(shown.study_tab);
        model.set_run_view(shown.right_view);
        model.sidebar_open = shown.sidebar_open;
        model.run_panel_open = shown.right_open;
        if shown.harness == tern_agent::Harness::Study {
            model.study.busy = Some("Opening your study library…".into());
            bridge.study_load();
            bridge.cards_load();
        }
        let (sidebar_open, panel_open) = (model.sidebar_open, model.run_panel_open);
        let mut this = Self {
            sidebar_slide: Slide::new(sidebar_open),
            panel_slide: Slide::new(panel_open),
            wipe: None,
            scout: crate::scout::Rig::default(),
            scout_at: None,
            scout_beat: 0,
            pet_manifest: std::rc::Rc::new(crate::pet::Manifest::embedded()),
            pet_preview: None,
            pet_tour_toolbar: false,
            pet_preview_anim: std::cell::RefCell::new(crate::pet::Animator::new(Instant::now())),
            scout_hidden: tern_agent::Preferences::load().scout_hidden,
            scout_variant: tern_agent::Preferences::load().scout_variant,
            pet_preview_wake: None,
            mark_anim: std::rc::Rc::new(crate::mark::MarkAnim::embedded()),
            mark_since: Instant::now(),
            mark_harness: tern_agent::Harness::Research,
            presses: Default::default(),
            cull_rows: Default::default(),
            thumbs: Default::default(),
            card_swipe: Default::default(),
            rail_slots: Default::default(),
            measured: Default::default(),
            view_id: cx.entity_id(),
            toast_springs: Default::default(),
            grown: Default::default(),
            plan_folded: None,
            window_h: 0.,
            reduced_motion: tern_agent::Preferences::load().wants_reduced_motion(),
            look,
            model,
            bridge,
            session: None,
            current: None,
            tabs: Tabs::default(),
            last_layout: None,
            composer,
            key_input,
            searx_input,
            parked: Default::default(),
            harness_memory: harness_memory.clone(),
            pending_restore: Some((shown.harness, harness_memory.get(shown.harness).clone())),
            switcher: None,
            web: crate::webview::Host::shared(),
            web_timer: None,
            web_ticking: false,
            graph: Default::default(),
            flow: Default::default(),
            rename_input,
            renaming: None,
            cancel: CancelFlag::default(),
            open_sources: HashSet::new(),
            pending: None,
            plan_reply: None,
            background: Default::default(),
            replay_plan: None,
            plan_inputs: Vec::new(),
            plan_dropped: HashSet::new(),
            deep_control: tern_agent::DeepControl::default(),
            scroll: ScrollHandle::new(),
            ticker: None,
            _listener: listener,
            _watch_timer: watch_timer,
            study_inputs,
            study_ticker: None,
            keys: cx.focus_handle(),
            explains: Default::default(),
            explain_asks: Default::default(),
            next_explain: 0,
            explain_at: None,
            explain_ticker: None,
            explain_loader: None,
            note_input,
            address_input,
            git_inputs,
            side_input,
            library_input,
            side_window: None,
            ext_inputs,
            arg_inputs: Vec::new(),
            arg_subs: Vec::new(),
            picker_input,
            link_input,
            recording: None,
            queued: None,
            composer_restore: None,
            audio_ticker: None,
            reveal_at: None,
            centre_w: 800.,
            centre_x: 0.,
            window_w: 0.,
            side_w: crate::panels::RIGHT_DEFAULT,
            browser_share: crate::browser::SPLIT_DEFAULT,
            browser_hid_sidebar: false,
            browser_was_open: false,
            run_more_open: false,
            glides: Default::default(),
            perf_strip: 0,
            anim_pending: Default::default(),
            effort_track: Default::default(),
            effort_drag: None,
            ctx_opened: Default::default(),
            tabs_menu_open: false,
            tab_picker: None,
            tab_pick_sel: 0,
            popover_closed_at: None,
            tabs_restored: false,
            shown_saved: shown,
            shown_task: None,
            parked_tabs: Default::default(),
            pending_pick: None,
            dictation: None,
            dictation_model: None,
            dictation_fast: None,
            dictation_whisper: None,
            // Auto's starting point (#69): the state before the first
            // recording is the one every later recording starts in.
            dictation_route: crate::dictation::Route::Detect,
            dictation_auto: false,
            dictation_checking: false,
            dictation_check_from: None,
            dictation_last_check: None,
            dictation_rec: 0,
            dictation_lang_memo: None,
            dictation_switching: None,
            dictation_caret: crate::dictation::Caret::default(),
            dictation_samples: Vec::new(),
            dictation_pos: 0,
            dictation_task: None,
            read_aloud: None,
            read_aloud_task: None,
            toast_wake: None,
            mention_scroll: ScrollHandle::new(),
            files_scroll: ScrollHandle::new(),
            _folder_poll: folder_poll,
            _live_poll: live_poll,
            work: work_view::WorkUi::default(),
            rail_hover: None,
            rail_auto: false,
            cmdk: None,
            rail_scroll: None,
            resume_retry: None,
            rail_base: Default::default(),
            settings_inputs,
            setup_inputs,
            onboarding: None,
            onboarding_checked: false,
            intro: None,
            intro_cursor: None,
            intro_replay: false,
            focus: None,
            tour: tour::enabled().then(tour::Tour::new),
            _subscriptions: {
                let mut subs = vec![
                    picker_sub,
                    link_sub,
                    sub,
                    key_sub,
                    rename_sub,
                    note_sub,
                    address_sub,
                    side_sub,
                    library_sub,
                ];
                subs.extend(study_subs);
                subs.extend(ext_subs);
                subs.extend(settings_subs);
                subs
            },
        };
        if this.tour.is_some() {
            this.start_tour(window, cx);
        }
        this
    }

    fn on_msg(&mut self, msg: Msg, window: &mut Window, cx: &mut Context<Self>) {
        if self.tour_drops(&msg) {
            return;
        }
        match msg {
            // A run's message goes to its own chat (#71): the open one, or
            // one put away while it runs.
            Msg::Chat(id, inner) => {
                if self.current != Some(id) && self.background.contains_key(&id) {
                    self.on_background(id, *inner, cx);
                } else {
                    self.on_msg(*inner, window, cx);
                }
                return;
            }
            // A file read for one chat's composer lands in that chat (#71).
            Msg::Composer(key, inner) => {
                // A new chat that got its id while the file was read.
                let key = if key.is_none()
                    && self.current.is_some()
                    && !self
                        .model
                        .drafts
                        .get(&None)
                        .is_some_and(|d| d.attaching > 0)
                {
                    self.current
                } else {
                    key
                };
                if key == self.current {
                    self.on_msg(*inner, window, cx);
                } else {
                    self.on_other_composer(key, *inner);
                }
                cx.notify();
                return;
            }
            Msg::Connected(s, recent) => {
                self.model.connected(Connection {
                    provider: s.provider_name().to_owned(),
                    model: s.model().to_owned(),
                    search: s.search_name().map(str::to_owned),
                    context_window: s.context_window(),
                });
                self.model.set_sessions(recent);
                self.model.mcp_offers = s.mcp_offers().to_vec();
                if !self.model.customize.loaded {
                    // Skills for the `/` menu.
                    self.bridge.ext_load();
                }
                if self.current.is_none() {
                    // A new chat's draft (and audio started in it) follow
                    // it now that it has an id.
                    self.model.rekey_draft(None, Some(s.session_id()));
                }
                self.current = Some(s.session_id());
                self.restore_tabs_once(s.session_id(), "New chat", s.is_draft());
                self.tabs.set_active(s.session_id(), "New chat");
                self.model.set_resume_offer(None, None);
                self.model.switching_to = None;
                self.resume_retry = None;
                self.chat_shown(&s);
                self.session = Some(s);
                // A new chat in a folder project opens in its folder.
                self.default_folder(cx);
                self.composer.update(cx, |c, cx| c.focus(window, cx));
                self.apply_restore(window, cx);
                self.apply_pending_pick();
            }
            Msg::Opening(id, turns) => {
                // The chat shows at once; its model is still connecting.
                // (On launch no chat is current yet: this one becomes it.)
                if self.current.is_none() && matches!(self.model.phase, Phase::Connecting) {
                    self.current = Some(id);
                }
                if self.current == Some(id) && self.session.is_none() {
                    self.model.restore(&turns);
                    self.focus = None;
                    self.apply_block_edits();
                    self.request_icons();
                    self.scroll.scroll_to_bottom();
                    self.apply_restore(window, cx);
                }
            }
            Msg::ResumeFailed(id, e) => {
                if self.current == Some(id) && self.session.is_none() {
                    self.resume_retry = Some(id);
                    self.model.switching_to = None;
                    self.model.connect_failed(e.clone());
                    self.model.toast_error(
                        format!("This chat is shown, but its model could not be opened: {e} Pick a model to keep going."),
                        Some(crate::toast::Retry::Connect),
                    );
                }
            }
            Msg::Dictated(seq, result, heard) => self.dictated(seq, result, heard, window, cx),
            Msg::LanguageChecked(rec, from, result) => {
                self.language_checked(rec, from, result, window, cx)
            }
            Msg::Resumed(s, turns, recent) => {
                self.resume_retry = None;
                self.model.switching_to = None;
                self.model
                    .toasts
                    .resolve(&crate::toast::Retry::Connect, Instant::now());
                self.model.connected(Connection {
                    provider: s.provider_name().to_owned(),
                    model: s.model().to_owned(),
                    search: s.search_name().map(str::to_owned),
                    context_window: s.context_window(),
                });
                self.model.restore(&turns);
                self.focus = None;
                self.apply_block_edits();
                self.model.compacted = s.compacted().cloned();
                self.model.mcp_offers = s.mcp_offers().to_vec();
                self.model.context = s.context();
                self.open_sources.clear();
                self.request_icons();
                let title = recent
                    .iter()
                    .find(|r| r.id == s.session_id())
                    .map(|r| r.title.clone())
                    .unwrap_or_else(|| "Chat".into());
                self.model.set_sessions(recent);
                self.restore_tabs_once(s.session_id(), &title, false);
                self.tabs.set_active(s.session_id(), &title);
                self.current = Some(s.session_id());
                self.model.set_resume_offer(s.checkpoint().as_ref(), None);
                self.model.cont.ids.anchor = self
                    .model
                    .sessions
                    .iter()
                    .find(|r| r.id == s.session_id())
                    .and_then(|r| r.anchor);
                self.bridge.load_ids(s.session_id());
                self.bridge.remember(s.harness(), s.session_id());
                self.model.harness_of_chat(s.harness());
                self.chat_shown(&s);
                self.session = Some(s);
                self.scroll.scroll_to_bottom();
                self.composer.update(cx, |c, cx| c.focus(window, cx));
                self.apply_restore(window, cx);
            }
            Msg::Cont(m) => self.on_cont(*m, window, cx),
            Msg::Ext(m) => match *m {
                crate::bridge::ExtMsg::Prompt(r) => self.prompt_ready(*r, window, cx),
                crate::bridge::ExtMsg::Folder(r) => self.folder_opened(r, window, cx),
                other => self.on_ext(other, cx),
            },
            Msg::Info(text) => self.model.toast_info(text),
            Msg::Doing(what) => self.model.doing = what,
            Msg::Notice(text) => {
                self.model.notice_line(text);
                // A chat that would not reopen leaves no session; open a
                // fresh one so the composer works.
                if self.session.is_none() && matches!(self.model.phase, Phase::Connecting) {
                    self.current = None;
                    self.bridge.connect();
                }
            }
            Msg::Swapped(s, failed) => {
                self.model.connected(Connection {
                    provider: s.provider_name().to_owned(),
                    model: s.model().to_owned(),
                    search: s.search_name().map(str::to_owned),
                    context_window: s.context_window(),
                });
                self.model.context = s.context();
                self.model.switching_to = None;
                if let Some(e) = failed {
                    self.model
                        .toast_error(format!("The model was not changed: {e}"), None);
                } else {
                    self.model.toast_info(format!(
                        "Now answering with {} · {}",
                        s.provider_name(),
                        s.model()
                    ));
                }
                self.session = Some(s);
            }
            Msg::Sessions(recent) => {
                for r in &recent {
                    self.tabs.retitle(r.id, &r.title);
                }
                self.model.set_sessions(recent);
            }
            Msg::ConnectFailed(e) => self.model.connect_failed(e),
            Msg::Compacted(s, result, usage) => {
                self.model.compacting = false;
                self.model.context = usage;
                match result {
                    Ok(Some(_)) => self.model.show_compaction_notice(),
                    Ok(None) => self.model.toast_info(
                        s.compact_skip()
                            .unwrap_or("Nothing to compact yet: your recent turns already fit.")
                            .to_owned(),
                    ),
                    Err(e) => self
                        .model
                        .toast_error(format!("Compacting failed: {e}"), None),
                }
                self.session = Some(s);
                self.scroll.scroll_to_bottom();
            }
            Msg::Event(e) => {
                self.model.on_event(&e);
                self.request_icons();
                // A file the agent wrote is one of the chat's outputs.
                let ev: &tern_core::EventEnvelope = &e;
                if ev.kind == "tool.executed" && ev.payload["tool"] == "file_write" {
                    let ws = self.model.workspace.as_ref().map(|(_, p)| p.clone());
                    for o in tern_agent::chat::outputs_in(std::slice::from_ref(ev), ws.as_deref()) {
                        self.model.outputs.retain(|x| x.path != o.path);
                        self.model.outputs.push(o);
                    }
                    self.model.files.reload();
                }
                // #77: a rename moves its output to the new name and the
                // Files tree drops the old entry at once.
                if ev.kind == "tool.executed" && ev.payload["tool"] == "file_rename" {
                    if let Some((_, root)) = self.model.workspace.as_ref() {
                        let a = &ev.payload["args"];
                        let at = |k: &str| {
                            let rel = a[k].as_str().unwrap_or_default().trim();
                            tern_agent::projects::plain_path(
                                &root.join(rel.strip_prefix("./").unwrap_or(rel)),
                            )
                        };
                        let (from, to) = (at("from"), at("to"));
                        tern_agent::chat::rename_output(&mut self.model.outputs, &from, &to);
                    }
                    self.model.files.reload();
                }
            }
            Msg::Icon(site, path) => self.model.icon_loaded(site, path),
            Msg::Attached(result) => self.model.attached(result),
            Msg::Preview(name, bytes) => {
                if self.model.previews.len() > 64 {
                    self.model.previews.clear();
                }
                self.model.previews.insert(name, bytes);
            }
            Msg::ReadAgain(name, result) => {
                let ok = self.model.read_again_done(&name, result);
                // A held "read it again with Granite-Docling" goes now,
                // with the new reading attached (#70); after an error it
                // waits in the composer.
                if let Some(text) = self.model.reread_send.take() {
                    self.composer
                        .update(cx, |c, cx| crate::view::set_end(c, &text, window, cx));
                    if ok && self.model.attaching == 0 {
                        self.submit(window, cx);
                    }
                }
            }
            Msg::Git(snapshot, result) => {
                if self.model.git.status().is_none() && snapshot.status.is_repo {
                    let remote = snapshot.status.remote.clone().unwrap_or_default();
                    self.git_inputs
                        .remote
                        .update(cx, |i, cx| i.set_value(remote, window, cx));
                }
                self.model.git.finished(snapshot, result);
            }
            Msg::Browsed(request, page) => {
                if self.model.browser.loaded(request, page) {
                    let address = self.model.browser.address();
                    self.address_input
                        .update(cx, |i, cx| i.set_value(address, window, cx));
                    self.request_icons();
                }
            }
            Msg::Text(call, text) => {
                self.model.on_text(call, &text);
                self.scroll.scroll_to_bottom();
            }
            Msg::Reasoning(call, text) => {
                self.model.on_reasoning(call, &text);
                self.scroll.scroll_to_bottom();
            }
            Msg::Archive(archived, deleted, note) => {
                self.model.archive.loaded(archived, deleted);
                self.model.archive.note = note;
            }
            Msg::Approval(req, reply) => {
                if self.cancel.is_cancelled() {
                    let _ = reply.send(ApprovalAnswer::Deny);
                } else {
                    self.pending = Some(reply);
                    self.model.ask_approval(req);
                    // The approval card (a diff, a command) sits at the end
                    // of the transcript: bring it into view.
                    self.scroll.scroll_to_bottom();
                }
            }
            Msg::Plan(plan, reply) => self.show_plan(plan, reply, window, cx),
            Msg::Done(s, out, usage, recent) => {
                self.model.finish_turn(&out, usage);
                self.model.show_compaction_notice();
                self.model.set_resume_offer(
                    s.checkpoint().filter(|_| out.answer.is_none()).as_ref(),
                    Some((out.state, out.reason.as_deref())),
                );
                // A new chat's tab takes the title its first answer gave it.
                for r in &recent {
                    self.tabs.retitle(r.id, &r.title);
                }
                self.model.set_sessions(recent);
                if !s.is_draft() {
                    self.bridge.remember(s.harness(), s.session_id());
                    self.model.composer.remember(&s.session_id().to_string());
                }
                // The folder keeps the mode this turn was sent in (#80); a
                // needs-Act answer does not count as using Chat there.
                if self.model.needs_act.is_none() {
                    self.remember_folder_mode(self.model.composer.mode);
                }
                self.session = Some(s);
                if self.model.reveal.is_none() {
                    self.ticker = None;
                }
                self.scroll.scroll_to_bottom();
                self.apply_pending_pick();
                // A message queued with Tab goes now.
                if let Some(q) = self.queued.take() {
                    self.composer
                        .update(cx, |c, cx| crate::view::set_end(c, q, window, cx));
                    self.submit(window, cx);
                }
            }
            Msg::Providers(rows, prefs, keychain) => {
                self.model.providers.load(rows, *prefs, keychain);
                self.consider_onboarding(window, cx);
                self.refresh_models();
                self.refresh_catalog();
            }
            Msg::Checked(kind, id, result) => {
                let ok = result.is_ok();
                self.onboarding_checked_key(kind, id, &result);
                self.model.providers.checked(kind, id, result);
                if ok {
                    self.key_input
                        .update(cx, |k, cx| k.set_value("", window, cx));
                    // A first key becomes the default on its own, so the chat
                    // can open without another click.
                    if kind == Kind::Model && self.session.is_none() && !self.model.is_busy() {
                        self.apply_choice(kind, id);
                    } else if kind == Kind::Model {
                        self.refresh_models();
                    }
                    if kind == Kind::Model {
                        self.refresh_catalog();
                    }
                }
            }
            Msg::Removed(kind, id, result) => {
                let was_default = self.model.providers.default_id(kind) == Some(id);
                self.model.providers.removed(kind, id, result);
                if was_default && !self.model.is_busy() {
                    self.reconnect();
                }
                self.refresh_models();
                self.refresh_catalog();
            }
            Msg::Models(id, result) => {
                if let Ok(list) = &result {
                    let n = list.iter().filter(|m| !m.is_retired()).count();
                    self.model.composer.models_loaded(id, n);
                    // A fresh listing also refreshes the routing's copy.
                    let c = &mut self.model.catalog;
                    match c.providers.iter_mut().find(|(p, _)| *p == id) {
                        Some(entry) => entry.1 = Ok(list.clone()),
                        None => c.providers.push((id, Ok(list.clone()))),
                    }
                }
                match &result {
                    Ok(_) => self
                        .model
                        .toasts
                        .resolve(&crate::toast::Retry::Models(id), Instant::now()),
                    // Only the picker's own tab gets a toast; it also shows
                    // the reason with Retry in the list.
                    Err(e)
                        if self.model.composer.menu.is_none()
                            && self.model.providers.default_id(Kind::Model) == Some(id) =>
                    {
                        self.model.toast_error(
                            format!("Could not list {} models: {e}", self.provider_name(id)),
                            Some(crate::toast::Retry::Models(id)),
                        );
                    }
                    Err(_) => {}
                }
                self.model.providers.models_loaded(id, result)
            }
            Msg::Catalog(c) => {
                for (id, r) in &c.providers {
                    if let Ok(list) = r {
                        let n = list.iter().filter(|m| !m.is_retired()).count();
                        self.model.composer.models_loaded(id, n);
                    }
                }
                self.model.catalog = *c;
            }
            Msg::FileTranscribed(id, result) => {
                self.model.audio_transcribed(id, result);
            }
            Msg::Decision(m) => match *m {
                crate::bridge::DecisionMsg::Loaded(rows, prefs, err) => {
                    self.model.decision.load(rows, prefs, err)
                }
                crate::bridge::DecisionMsg::Checked(id, result) => {
                    if result.is_ok() {
                        self.key_input
                            .update(cx, |k, cx| k.set_value("", window, cx));
                    }
                    self.model.decision.checked(id, result);
                }
                crate::bridge::DecisionMsg::Removed(id, result) => {
                    self.model.decision.removed(id, result)
                }
                crate::bridge::DecisionMsg::Tested(result) => {
                    self.model.decision.test = Some(match result {
                        Ok(t) => crate::decision::RowNote::Ok(t),
                        Err(e) => crate::decision::RowNote::Failed(e),
                    });
                }
            },
            Msg::Eval(m) => match *m {
                crate::bridge::EvalMsg::Loaded(runs, sel, prev) => {
                    self.model
                        .eval
                        .load(runs, sel.map(|r| *r), prev.map(|r| *r))
                }
                crate::bridge::EvalMsg::Ran(r) => self.model.eval.finished(r),
            },
            Msg::Setup(m) => self.on_setup(*m, window, cx),
            Msg::PrefsFailed(e) => self
                .model
                .toast_error(format!("Tern could not save your choice: {e}"), None),
            Msg::Study(m) => self.on_study(*m, window, cx),
            Msg::Watches(list, note, finished) => {
                self.model.watches_loaded(list);
                self.model.watches_running = self.model.watches_running.saturating_sub(finished);
                // A watch's result is told on the Watches screen and in the
                // sidebar; only errors and edits are noted in the chat.
                if let Some(n) = note
                    && finished == 0
                    && self.model.screen == Screen::Chat
                    && !self.model.is_busy()
                {
                    self.model.notice_line(n);
                }
            }
        }
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // While a run goes, Enter sends a steering note instead.
        if self.model.can_steer() {
            let text = self.composer.read(cx).value().to_string();
            if self.model.steer(&text) {
                self.composer
                    .update(cx, |c, cx| c.set_value("", window, cx));
                self.scroll.scroll_to_bottom();
            }
            cx.notify();
            return;
        }
        // In Study, a message that names a mode ("quiz me on paging",
        // "explain TLBs in 3D") switches the window to it and starts it.
        if self.model.harness == Harness::Study {
            let typed = self.composer.read(cx).value().to_string();
            if self.study_intent(&typed, window, cx) {
                self.composer
                    .update(cx, |c, cx| c.set_value("", window, cx));
                cx.notify();
                return;
            }
        }
        if self.session.is_none() {
            return;
        }
        let text = self.composer.read(cx).value().to_string();
        // "Try local model granite docling" with nothing attached: the
        // newest PDF of the chat is read again as scanned first, and the
        // message goes with that reading (#70).
        if self.model.attachments.is_empty()
            && self.model.attaching == 0
            && self.model.reread_send.is_none()
            && tern_agent::docs::asks_reread(&text)
            && let Some(name) = self.model.last_sent_pdf()
            && let Some(path) = self.model.read_again_started(&name)
        {
            self.model.reread_send = Some(text);
            self.bridge.read_again(name, path, self.current);
            self.tick_audio(cx);
            self.composer
                .update(cx, |c, cx| c.set_value("", window, cx));
            cx.notify();
            return;
        }
        let Some(text) = self.model.start_turn(&text) else {
            return;
        };
        let Some(session) = self.session.take() else {
            return;
        };
        // Asked in Study: the answer shows in the Study window's chat.
        if self.model.harness == Harness::Study {
            self.model.study.set_tab(crate::study::StudyTab::Chat);
            self.model.study.materials_open = false;
            self.model.screen = Screen::Chat;
        }
        let mut session = session;
        let mode = self.model.composer.mode;
        session.set_turn_mode(mode.turn_mode());
        {
            // The approval mode and rules as Settings › Permissions and
            // the composer left them (#75).
            let prefs = &self.model.providers.prefs;
            session.set_approval(prefs.approval(), prefs.permission_rules.clone());
            tern_agent::workbench::set_settings(prefs.workbench());
        }
        tern_agent::live::clear_preview();
        self.work.preview_path = None;
        session.set_effort(self.model.composer.effort);
        // A Study mode asks the same model to answer in its style.
        let mut text = match mode.study_prefix() {
            Some(prefix) => format!("{prefix}\n\n{text}"),
            None => text,
        };
        let images = std::mem::take(&mut self.model.outgoing_images);
        let provider = self.provider_id();
        let chat_model = self
            .model
            .connection
            .as_ref()
            .map(|c| c.model.clone())
            .unwrap_or_default();
        let sees = self
            .model
            .catalog
            .info(&provider, &chat_model)
            .sees_images()
            && !tern_agent::media::is_not_vision(&provider, &chat_model);
        // A model that cannot see never gets a picture, in this message or
        // from earlier ones (a provider refuses the whole request when any
        // message carries one).
        session.set_sees_pictures(Some(sees));
        if !sees {
            session.drop_pictures();
        }
        let mut pictures = None;
        if !images.is_empty() {
            // Reading the models folder is a few small file reads, once
            // per message with pictures.
            let local = tern_agent::local_models::LocalModels::discover()
                .ok()
                .map(|(lm, _)| lm);
            match tern_agent::media::route_images(
                &self.model.catalog,
                &provider,
                &chat_model,
                &self.model.providers.prefs,
                local.as_ref(),
            ) {
                tern_agent::media::ImageRoute::Send { fallback } => {
                    // Normalised for the provider (PNG/JPEG, 2048 px,
                    // 5 MB); one it cannot take goes as a note.
                    let mut ok = Vec::new();
                    let mut notes = Vec::new();
                    for img in &images {
                        match tern_agent::pictures::prepare(img) {
                            Ok(p) => ok.push(p),
                            Err(e) => notes.push(format!("[Tern couldn't read {}: {e}]", img.name)),
                        }
                    }
                    if !notes.is_empty() {
                        self.model.toast_info(notes.join(" "));
                        text = format!("{text}\n\n{}", notes.join("\n"));
                    }
                    session.attach_images(ok.clone());
                    pictures = Some(crate::bridge::Pictures {
                        images: ok,
                        readers: fallback,
                        sent: true,
                        chat_model,
                    });
                }
                tern_agent::media::ImageRoute::Read(readers) => {
                    pictures = Some(crate::bridge::Pictures {
                        images,
                        readers,
                        sent: false,
                        chat_model,
                    })
                }
                tern_agent::media::ImageRoute::Missing(why) => {
                    // The message still goes, with a note for the model.
                    let names: Vec<&str> = images.iter().map(|i| i.name.as_str()).collect();
                    text = format!(
                        "{text}\n\n[Tern couldn't read {}: nothing on this computer or at a configured provider can read pictures]",
                        names.join(", ")
                    );
                    self.model.toast_info(why);
                }
            }
        }
        self.model.steering = Some(session.steering());
        self.model.resume_offer = None;
        self.cancel = CancelFlag::default();
        if self.model.deep_mode {
            self.deep_control = tern_agent::DeepControl::new(self.cancel.clone());
            self.bridge.deep(session, text, self.deep_control.clone());
        } else {
            self.bridge
                .send(session, text, self.cancel.clone(), pictures);
        }
        self.composer
            .update(cx, |c, cx| c.set_value("", window, cx));
        self.scroll.scroll_to_bottom();
        self.start_ticker(cx);
        cx.notify();
    }

    /// Asks the model to go on from an answer cut off at the length limit.
    fn continue_answer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.model.can_send() {
            return;
        }
        self.model.continue_offer = false;
        let deep = self.model.deep_mode;
        self.model.deep_mode = false;
        self.composer.update(cx, |c, cx| {
            crate::view::set_end(c, tern_agent::CONTINUE_PROMPT, window, cx)
        });
        self.submit(window, cx);
        self.model.deep_mode = deep;
    }

    /// The composer follows its text: right to left (right-aligned, arrow
    /// keys swapped) when the first strong letter is Urdu, Arabic, Persian
    /// or Hebrew, as `dir="auto"` on the web; `@` references in the accent
    /// (#68). Both are no-ops while nothing changed.
    fn style_composer(&mut self, p: Palette, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().to_string();
        let rtl = crate::bidi::has_rtl(&text) && crate::bidi::is_rtl(&text);
        let style = gpui_kit::HighlightStyle {
            color: Some(p.accent),
            ..Default::default()
        };
        let spans: Vec<_> = crate::composer::mention_spans(&text)
            .into_iter()
            .map(|r| (r, style))
            .collect();
        self.composer.update(cx, |c, cx| {
            c.set_right_to_left(rtl, cx);
            c.set_text_highlights(spans, cx);
        });
    }

    /// Picks a row of the `@` menu (the highlighted one for `None`).
    fn choose_mention(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.composer.read(cx).value().to_string();
        if let Some(chosen) = self.model.choose_mention(&text, index) {
            self.composer.update(cx, |c, cx| {
                crate::view::set_end(c, chosen.text, window, cx);
                c.focus(window, cx);
            });
            if chosen.browse {
                self.pick_attachments(cx);
            }
            if let Some((server, uri)) = chosen.fetch {
                self.bridge.mcp_resource(server, uri);
            }
            if let Some(path) = chosen.attach {
                // Chosen by the user from the menu, like the file picker.
                self.attach_paths(vec![path], cx);
            }
            if let Some((id, title)) = chosen.chat {
                self.bridge.attach_chat(id, title);
            }
            if self.model.arg_form.is_some() && self.arg_inputs.is_empty() {
                self.open_arg_form(window, cx);
            }
        }
        cx.notify();
    }

    /// Up, Down, Tab, and Escape drive the `@` menu while it is open.
    fn mention_keys(
        &mut self,
        e: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.slash_keys(e, window, cx) {
            return;
        }
        if self.model.mention.is_none() {
            // Esc stops a running turn (the menus take it first when open).
            if e.keystroke.key == "escape" && self.model.is_busy() && self.model.slash.is_none() {
                cx.stop_propagation();
                self.stop(cx);
            }
            return;
        }
        match e.keystroke.key.as_str() {
            "up" => {
                self.model.move_mention(-1);
                self.scroll_menu_to_selected();
            }
            "down" => {
                self.model.move_mention(1);
                self.scroll_menu_to_selected();
            }
            "tab" => {
                cx.stop_propagation();
                self.choose_mention(None, window, cx);
                return;
            }
            "escape" => self.model.mention = None,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Scrolls the `@` menu so its highlighted row shows (its first child
    /// is the heading).
    fn scroll_menu_to_selected(&self) {
        if let Some(m) = &self.model.mention {
            self.mention_scroll.scroll_to_item(m.selected + 1);
        }
    }

    /// The `@` menu over the composer.
    fn mention_menu(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.model.mention.as_ref()?;
        let rows = self.model.mention_candidates();
        let selected = menu.selected;
        let heading = match menu.role {
            Some(Role::Prompt) => "Reference as Prompt",
            Some(Role::Context) => "Reference as Context",
            None => "Reference",
        };
        Some(
            div()
                .id("mention-menu")
                .flex()
                .flex_col()
                .gap(px(2.))
                // Long lists scroll; Up and Down keep the highlight in view.
                .max_h(px(MENTION_MENU_MAX_H))
                .overflow_y_scroll()
                .track_scroll(&self.mention_scroll)
                .p_1()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .child(
                    div()
                        .px_2()
                        .pt_1()
                        .text_xs()
                        .text_color(p.faint)
                        .child(heading),
                )
                .children(rows.into_iter().enumerate().map(|(i, row)| {
                    let lead = match &row.pick {
                        Pick::Attachment(_) => div()
                            .text_color(p.muted)
                            .child(gpui_kit::component::Icon::new(AssetIcon::Paperclip).small())
                            .into_any_element(),
                        Pick::Source(page) => self.site_icon(page, 16., p),
                        Pick::Resource(item) => self.resource_logo(item, p),
                        Pick::Browse => div()
                            .text_color(p.muted)
                            .child(gpui_kit::component::Icon::new(AssetIcon::FolderOpen).small())
                            .into_any_element(),
                        Pick::Path(_) => div()
                            .text_color(p.muted)
                            .child(gpui_kit::component::Icon::new(AssetIcon::Paperclip).small())
                            .into_any_element(),
                        Pick::File(path) => crate::fileicon::logo(
                            crate::fileicon::FileIcon::of(&path.to_string_lossy()),
                            16.,
                            p.text.l > 0.5,
                        ),
                        Pick::Folder(path) => {
                            crate::fileicon::folder_logo(&path.to_string_lossy(), 16.)
                        }
                        Pick::Chat(_) => div()
                            .text_color(p.muted)
                            .child(gpui_kit::component::Icon::new(AssetIcon::MessageSquare).small())
                            .into_any_element(),
                    };
                    div()
                        .id(("mention", i))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_sm()
                        .cursor_pointer()
                        // One highlight for the mouse and the keys: hovering
                        // a row selects it.
                        .when(i == selected, |d| d.bg(p.hover))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered
                                && let Some(m) = &mut this.model.mention
                                && m.selected != i
                            {
                                m.selected = i;
                                cx.notify();
                            }
                        }))
                        .child(lead)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(p.text)
                                .child(SharedString::from(row.label)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(row.detail)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose_mention(Some(i), window, cx);
                        }))
                }))
                .with_animation(
                    "mention-menu-in",
                    Animation::new(if self.reduced_motion {
                        Duration::ZERO
                    } else {
                        motion::UI
                    })
                    .with_easing(motion::ease),
                    |d, t| d.opacity(t),
                )
                .into_any_element(),
        )
    }

    /// Moves text over the long-paste limit out of the composer and into a
    /// `Pasted text` chip.
    fn check_long_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().to_string();
        if self.model.take_long_text(&text) {
            self.composer
                .update(cx, |c, cx| c.set_value("", window, cx));
            cx.notify();
        }
    }

    /// Opens the system picker for files (zips are files). Folders are
    /// opened in the `@` menu itself, and a picker that allows folders
    /// lets only folders be chosen on Windows.
    fn pick_attachments(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                let _ = this.update(cx, |this, cx| this.attach_paths(paths, cx));
            }
        })
        .detach();
    }

    fn attach_paths(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.note_recent_files(&paths);
        // Audio files go to the speech-to-text model; the transcript joins
        // the composer like spoken words.
        let (audio, paths): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| {
            p.file_name()
                .is_some_and(|n| tern_agent::media::is_audio_name(&n.to_string_lossy()))
        });
        for a in audio {
            let name = a
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(why) = tern_agent::media::audio_not_supported(&name) {
                self.model.toast_error(why, None);
                continue;
            }
            match self.file_stt_model() {
                Some(via) => {
                    // The length is read from the file's headers only.
                    let secs = tern_agent::media::audio_duration(&a);
                    let (id, ctl) = self.model.audio_started(name, secs, self.current);
                    self.bridge.transcribe_file(via, a, secs, id, ctl);
                    self.tick_audio(cx);
                }
                None => self.model.audio_needs_model(&name),
            }
        }
        if paths.is_empty() {
            cx.notify();
            return;
        }
        self.model.attaching_doc = match paths.as_slice() {
            [one] if self.model.attaching == 0 => one
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| tern_agent::docs::converts(n)),
            _ => None,
        };
        self.model.attaching_name = self.model.attaching_doc.clone();
        self.model.attaching += paths.len();
        self.tick_audio(cx);
        self.bridge.attach(paths, self.current);
        cx.notify();
    }

    /// Remembers attached files for the `@` menu's Recent rows, here and
    /// in the saved preferences.
    fn note_recent_files(&mut self, paths: &[std::path::PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let mut saved = tern_agent::Preferences::load();
        for p in paths {
            let p = p.display().to_string();
            self.model.providers.prefs.note_recent_file(&p);
            saved.note_recent_file(&p);
        }
        self.bridge.save_prefs(saved);
    }

    /// Redraws the transcribing chips' progress a few times a second while
    /// any audio is being transcribed, in any chat.
    fn tick_audio(&mut self, cx: &mut Context<Self>) {
        if self.audio_ticker.is_some() {
            return;
        }
        self.audio_ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let go = this.update(cx, |this, cx| {
                    // Converting chips show page progress too (#71).
                    let any = !this.model.audio_pending.is_empty()
                        || this.model.drafts.values().any(|d| !d.audio.is_empty())
                        || this.model.attaching_name.is_some();
                    if any {
                        cx.notify();
                    } else {
                        this.audio_ticker = None;
                    }
                    any
                });
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
        }));
    }

    /// Redraws while a turn runs: every 50 ms while text is being revealed
    /// at reading pace, else twice a second for the working line.
    fn start_ticker(&mut self, cx: &mut Context<Self>) {
        self.reveal_at = Some(Instant::now());
        self.ticker = Some(cx.spawn(async move |this, cx| {
            let mut quiet = 0u32;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let go = this.update(cx, |this, cx| {
                    let now = Instant::now();
                    let dt = this
                        .reveal_at
                        .map(|t| now.saturating_duration_since(t))
                        .unwrap_or_default();
                    this.reveal_at = Some(now);
                    let pacing = this.model.tick_reveal(dt, this.reduced_motion);
                    quiet += 1;
                    if pacing || quiet >= 10 {
                        quiet = 0;
                        cx.notify();
                    }
                    if pacing {
                        this.scroll.scroll_to_bottom();
                    }
                    this.model.is_busy() || pacing
                });
                if !matches!(go, Ok(true)) {
                    let _ = this.update(cx, |_, cx| cx.notify());
                    break;
                }
            }
        }));
    }

    /// A file read for a chat that is not open (#71): it waits in that
    /// chat's composer. A held "read it again with Granite-Docling"
    /// message waits there too, ready to send.
    fn on_other_composer(&mut self, key: crate::model::DraftKey, msg: Msg) {
        match msg {
            Msg::Attached(result) => self.model.in_draft(key, |m| m.attached(result)),
            Msg::ReadAgain(name, result) => {
                let held = self.model.in_draft(key, |m| {
                    m.read_again_done(&name, result);
                    m.reread_send.take()
                });
                if let Some(text) = held {
                    self.model.drafts.entry(key).or_default().text = text;
                }
                self.model.toast_info(format!(
                    "{name} is read as scanned; it waits in the chat you asked in."
                ));
            }
            Msg::Composer(_, inner) | Msg::Chat(_, inner) => self.on_other_composer(key, *inner),
            _ => {}
        }
    }

    /// Puts the open chat away while its run goes on (#71), so another
    /// chat can open and run at the same time. The run's messages land in
    /// it until it is opened again. Does nothing when the open chat is
    /// idle.
    fn park_running(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.current else {
            return;
        };
        if !self.model.is_busy() {
            return;
        }
        let slot = self.model.park_chat();
        let bg = BgRun {
            slot,
            cancel: std::mem::take(&mut self.cancel),
            pending: self.pending.take(),
            plan_reply: self.plan_reply.take(),
            plan_inputs: std::mem::take(&mut self.plan_inputs),
            plan_dropped: std::mem::take(&mut self.plan_dropped),
            plan: None,
            deep_control: std::mem::take(&mut self.deep_control),
            queued: self.queued.take(),
            session: None,
        };
        tern_agent::diag::note(&format!("chat: {id} runs on while another chat is open"));
        self.background.insert(id, bg);
        self.ticker = None;
        cx.notify();
    }

    /// Whether chat `id` has a run going out of sight (#71): put away
    /// while it ran, or a background branch. Its tab and sidebar row show
    /// a small spinner.
    pub(super) fn runs_in_background(&self, id: SessionId) -> bool {
        self.background.get(&id).is_some_and(|b| b.slot.is_busy())
            || self.model.cont.background.contains(&id)
    }

    /// Brings back chat `id` put away while it ran (#71): its transcript,
    /// its run (still going, or finished while away) and its session.
    /// Returns false when it was not put away.
    fn unpark(&mut self, id: SessionId, cx: &mut Context<Self>) -> bool {
        let Some(mut bg) = self.background.remove(&id) else {
            return false;
        };
        self.model.swap_slot(&mut bg.slot);
        self.cancel = bg.cancel;
        self.pending = bg.pending;
        self.plan_reply = bg.plan_reply;
        self.plan_inputs = bg.plan_inputs;
        self.plan_dropped = bg.plan_dropped;
        self.deep_control = bg.deep_control;
        self.queued = bg.queued;
        self.replay_plan = bg.plan;
        self.current = Some(id);
        if let Some(s) = bg.session {
            self.bridge.load_ids(s.session_id());
            self.session = Some(s);
        }
        if self.model.is_busy() || self.model.reveal.is_some() {
            self.start_ticker(cx);
        }
        self.scroll.scroll_to_bottom();
        cx.notify();
        true
    }

    /// A message from the run of chat `id`, which is put away (#71): it
    /// is applied to that chat's own state, never the open chat's.
    fn on_background(&mut self, id: SessionId, msg: Msg, cx: &mut Context<Self>) {
        let Some(mut bg) = self.background.remove(&id) else {
            return;
        };
        let title = self
            .model
            .all_sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "A chat".into());
        match msg {
            Msg::Event(e) => self.model.in_slot(&mut bg.slot, |m| m.on_event(&e)),
            Msg::Text(call, text) => self.model.in_slot(&mut bg.slot, |m| m.on_text(call, &text)),
            Msg::Reasoning(call, text) => self
                .model
                .in_slot(&mut bg.slot, |m| m.on_reasoning(call, &text)),
            Msg::Doing(what) => self.model.in_slot(&mut bg.slot, |m| m.doing = what),
            Msg::Info(text) => self.model.toast_info(text),
            Msg::Approval(req, reply) => {
                if bg.cancel.is_cancelled() {
                    let _ = reply.send(ApprovalAnswer::Deny);
                } else {
                    bg.pending = Some(reply);
                    self.model.in_slot(&mut bg.slot, |m| m.ask_approval(req));
                    self.model
                        .toast_info(format!("“{title}” needs your approval to go on."));
                }
            }
            Msg::Plan(plan, reply) => {
                if bg.cancel.is_cancelled() {
                    let _ = reply.send(tern_agent::PlanAnswer::Cancel);
                } else {
                    bg.plan = Some((plan, reply));
                    self.model
                        .toast_info(format!("“{title}” has a research plan for you to check."));
                }
            }
            Msg::Done(s, out, usage, recent) => {
                let answered = out.answer.is_some();
                self.model.in_slot(&mut bg.slot, |m| {
                    m.finish_turn(&out, usage);
                    m.show_compaction_notice();
                    m.set_resume_offer(
                        s.checkpoint().filter(|_| out.answer.is_none()).as_ref(),
                        Some((out.state, out.reason.as_deref())),
                    );
                });
                for r in &recent {
                    self.tabs.retitle(r.id, &r.title);
                }
                self.model.set_sessions(recent);
                if !s.is_draft() {
                    self.model.composer.remember(&s.session_id().to_string());
                }
                bg.session = Some(s);
                if answered {
                    self.model.toast_info(format!("“{title}” has its answer."));
                }
            }
            Msg::Compacted(s, _, usage) => {
                self.model.in_slot(&mut bg.slot, |m| {
                    m.compacting = false;
                    m.context = usage;
                });
                bg.session = Some(s);
            }
            other => {
                tern_agent::diag::note(&format!("chat: {id} put away; dropped {other:?}"));
            }
        }
        self.background.insert(id, bg);
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.cancel.cancel();
        self.model.request_stop();
        if let Some(p) = self.pending.take() {
            let _ = p.send(ApprovalAnswer::Deny);
            self.model.approval_answered();
        }
        if self.plan_reply.is_some() {
            self.answer_plan(tern_agent::PlanAnswer::Cancel, cx);
        }
        cx.notify();
    }

    fn answer(&mut self, answer: ApprovalAnswer, cx: &mut Context<Self>) {
        if let Some(p) = self.pending.take() {
            let _ = p.send(answer);
        }
        self.model.approval_answered();
        cx.notify();
    }

    /// Asks the bridge for favicons the transcript needs and lacks.
    fn request_icons(&mut self) {
        for (site, url) in self.model.wanted_icons() {
            self.bridge.favicon(site, url);
        }
    }

    /// A source's icon: its cached favicon, else the site's initial.
    fn site_icon(&self, page: &PageRef, size: f32, p: Palette) -> AnyElement {
        let initial: String = page
            .site
            .trim_start_matches("www.")
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_else(|| "·".into());
        let monogram = move || {
            div()
                .size(px(size))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_sm()
                .bg(p.hover)
                .text_color(p.muted)
                .text_size(px(size * 0.62))
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(initial.clone()))
                .into_any_element()
        };
        if let Some(service) = assets::service_for(&page.site) {
            return div()
                .size(px(size))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(service_logo(service, size * 0.85, p, p.text))
                .into_any_element();
        }
        match self.model.icons.get(&page.site) {
            Some(SiteIcon::Ready(path)) => gpui_kit::img(path.clone())
                .size(px(size))
                .flex_none()
                .rounded_sm()
                .with_fallback(monogram)
                .into_any_element(),
            _ => monogram(),
        }
    }

    /// "Read N pages" with the first icons stacked; opens into one row per page.
    fn sources(
        &self,
        i: usize,
        pages: &[PageRef],
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.open_sources.contains(&i);
        let label = if pages.len() == 1 {
            "Read 1 page".to_owned()
        } else {
            format!("Read {} pages", pages.len())
        };
        let stack = div()
            .flex()
            .items_center()
            .children(pages.iter().take(3).enumerate().map(|(k, pg)| {
                div()
                    .when(k > 0, |d| d.ml(px(-4.)))
                    .rounded_sm()
                    .border_1()
                    .border_color(p.canvas)
                    .child(self.site_icon(pg, 14., p))
            }));
        let header = div()
            .id(("sources", i))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .text_xs()
            .text_color(p.muted)
            .cursor_pointer()
            .hover(|d| d.bg(p.hover).text_color(p.text))
            .child(gpui_kit::img(assets::CRAWL_LOGO).size(px(14.)).flex_none())
            .child(stack)
            .child(label)
            .child(
                gpui_kit::component::Icon::new(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .xsmall(),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.open_sources.remove(&i) {
                    this.open_sources.insert(i);
                }
                cx.notify();
            }));
        let rows = pages.iter().enumerate().map(|(k, pg)| {
            let url = pg.url.clone();
            div()
                .id(SharedString::from(format!("source-{i}-{k}")))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py(px(5.))
                .rounded_md()
                .cursor_pointer()
                .hover(|d| d.bg(p.hover))
                .child(self.site_icon(pg, 16., p))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(p.text)
                        .child(SharedString::from(pg.label().to_owned())),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(pg.site.clone())),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.browse(&url, window, cx)))
        });
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap_1()
            .child(header)
            .when(open, |d| {
                d.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .p_1()
                        .rounded_lg()
                        .border_1()
                        .border_color(p.border)
                        .children(rows),
                )
            })
            .into_any_element()
    }

    /// Reopens a chat from the list. Ignored mid-turn or for the open chat.
    /// A chat of the other harness never opens here: the window switches
    /// harness first and opens it there.
    fn open_chat(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if let crate::harness::Open::SwitchTo(h) =
            crate::harness::open(&self.model.all_sessions, self.model.harness, id)
        {
            self.switch_harness(h, Some(id), cx);
            return;
        }
        // From an explainer, a chat that is already open is only focused.
        if self.tabs.active_tab().is_explain()
            && let Some(i) = self.tabs.list.iter().position(|t| t.id == Some(id))
            && self.current == Some(id)
        {
            self.tabs.focus(i);
            self.model.screen = Screen::Chat;
            cx.notify();
            return;
        }
        if self.current == Some(id) {
            return;
        }
        // A running chat goes on in the background (#71).
        self.park_running(cx);
        let title = self
            .model
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "Chat".into());
        if self.tabs.open(id, &title) {
            self.load_chat(id, cx);
        }
    }

    /// Opens a chat from the list in a new tab.
    fn open_in_tab(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        self.park_running(cx);
        if self.tabs.list.iter().any(|t| t.id == Some(id)) {
            self.open_chat(id, cx);
            return;
        }
        self.tabs.add();
        self.tabs.reset_active();
        self.current = None;
        self.open_chat(id, cx);
        let _ = window;
    }

    /// Adds a tab with a fresh chat.
    fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.park_running(cx);
        self.stash_tab_place(cx);
        self.tabs.add();
        self.show_tab(self.tabs.active_tab().clone(), window, cx);
        self.persist_tabs();
    }

    /// Opens one of this harness's chats in a new tab (the + popover), or
    /// focuses its tab when it is open. The chat on screen keeps its draft
    /// and scroll in its own tab.
    fn open_in_new_tab(
        &mut self,
        id: SessionId,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.park_running(cx);
        self.stash_tab_place(cx);
        if self.tabs.open_new(id, title) {
            let tab = self.tabs.active_tab().clone();
            self.show_tab(tab, window, cx);
        }
        self.persist_tabs();
        cx.notify();
    }

    /// What the window shows now, as a restart puts it back.
    fn shown(&self) -> crate::restore::Shown {
        crate::restore::Shown {
            harness: self.model.harness,
            study_tab: self.model.study.tab,
            right_view: self.model.run_panel.view,
            right_open: self.model.run_panel_open,
            sidebar_open: self.model.sidebar_open,
        }
    }

    /// Saves the window for the next launch once it has stayed the same
    /// for `restore::SAVE_AFTER`: one write after a burst of changes, none
    /// while nothing changes. Cheap enough to call every frame.
    fn save_session_soon(&mut self, cx: &mut Context<Self>) {
        if self.shown_task.is_some() || self.shown() == self.shown_saved {
            return;
        }
        self.shown_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(crate::restore::SAVE_AFTER)
                .await;
            let _ = this.update(cx, |this, _| {
                this.shown_task = None;
                let now = this.shown();
                if now != this.shown_saved {
                    this.shown_saved = now;
                    this.save_ui_prefs(|p| p.session = now.to_prefs());
                }
            });
        }));
    }

    /// Writes the whole session now, on this thread, as the window closes:
    /// the window, both harnesses' chat tabs, and where the open harness's
    /// chat was left (its draft and scroll).
    pub(super) fn save_session_now(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown();
        self.shown_saved = shown;
        let h = self.model.harness;
        let place = self.current_place(cx);
        self.harness_memory.record(h, place);
        let other = match h {
            tern_agent::Harness::Research => tern_agent::Harness::Study,
            tern_agent::Harness::Study => tern_agent::Harness::Research,
        };
        let here = self.tabs.to_saved();
        let there = self.parked_tabs.saved(other);
        let memory = self.harness_memory.clone();
        let apply = |p: &mut tern_agent::Preferences| {
            p.session = shown.to_prefs();
            memory.save_into(p);
            p.open_tabs
                .insert(crate::harness::key(h).to_owned(), here.clone());
            if let Some(t) = &there {
                p.open_tabs
                    .insert(crate::harness::key(other).to_owned(), t.clone());
            }
        };
        let mut prefs = tern_agent::Preferences::load();
        apply(&mut prefs);
        apply(&mut self.model.providers.prefs);
        if let Err(e) = prefs.save() {
            crate::crashlog::note(&format!("Could not save the session on close: {e}"));
        }
    }

    /// After launch, once this harness's chats are known: put back the chat
    /// tabs saved last time, with the chat that opened in front.
    fn restore_tabs_once(&mut self, id: SessionId, title: &str, fresh: bool) {
        if self.tabs_restored {
            return;
        }
        self.tabs_restored = true;
        let key = crate::harness::key(self.model.harness);
        let Some(saved) = tern_agent::Preferences::load().open_tabs.get(key).cloned() else {
            return;
        };
        self.tabs = Tabs::from_saved(&saved, &self.model.sessions);
        if !fresh {
            self.tabs.open(id, title);
        } else if self.tabs.active_tab().id.is_some() {
            // A fresh chat opened: it takes a fresh tab of its own.
            if let Some(i) = self.tabs.list.iter().position(|t| t.id.is_none()) {
                self.tabs.focus(i);
            } else {
                self.tabs.add();
            }
        }
    }

    /// Keeps the front tab's draft and scroll before another tab comes up.
    fn stash_tab_place(&mut self, cx: &mut Context<Self>) {
        if !self.tabs.active_tab().is_explain() {
            let place = self.current_place(cx);
            self.tabs.stash_place(place);
        }
    }

    /// Saves both harnesses' chat tabs for the next launch.
    fn persist_tabs(&mut self) {
        let h = self.model.harness;
        let here = self.tabs.to_saved();
        let other = match h {
            tern_agent::Harness::Research => tern_agent::Harness::Study,
            tern_agent::Harness::Study => tern_agent::Harness::Research,
        };
        let there = self.parked_tabs.saved(other);
        self.save_ui_prefs(|p| {
            p.open_tabs
                .insert(crate::harness::key(h).to_owned(), here.clone());
            if let Some(t) = &there {
                p.open_tabs
                    .insert(crate::harness::key(other).to_owned(), t.clone());
            }
        });
    }

    fn focus_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let from_explain = self.tabs.active_tab().is_explain();
        let to_explain = self.tabs.list.get(i).is_some_and(Tab::is_explain);
        // Explainers never interrupt a run; another chat's tab puts a
        // running chat away, still running (#71).
        if self.model.is_busy() && !from_explain && !to_explain && i != self.tabs.active {
            self.park_running(cx);
        }
        if i != self.tabs.active {
            self.stash_tab_place(cx);
        }
        if let Some(tab) = self.tabs.focus(i).cloned() {
            if from_explain && tab.id.is_none() && !tab.is_explain() && self.current.is_none() {
                // Back to the fresh chat that was on screen.
                cx.notify();
                return;
            }
            if self.model.is_busy() && !tab.is_explain() && tab.id != self.current {
                self.park_running(cx);
            }
            self.show_tab(tab, window, cx);
        }
    }

    fn close_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let closing = self.tabs.list.get(i).cloned();
        let was_explain = self.tabs.active_tab().is_explain();
        if self.model.is_busy() && i == self.tabs.active && !was_explain {
            // Closing a running chat's tab: the run goes on in the
            // background and the chat stays in the list (#71).
            self.park_running(cx);
        }
        if let Some(key) = closing.as_ref().and_then(|t| t.explain) {
            self.drop_explain(key);
        }
        if let Some(tab) = self.tabs.close(i).cloned() {
            if was_explain && tab.id == self.current && !tab.is_explain() {
                cx.notify();
                return;
            }
            self.show_tab(tab, window, cx);
        }
        self.persist_tabs();
        cx.notify();
    }

    /// Shows a tab's chat: reopens it, or starts a fresh one. An
    /// explainer's tab shows itself; the chat behind it is kept.
    fn show_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if tab.is_explain() {
            self.model.screen = Screen::Chat;
            self.keys.focus(window, cx);
            cx.notify();
            return;
        }
        match tab.id {
            Some(id) if self.current != Some(id) => {
                // Its draft and scroll come back once it has loaded.
                self.pending_restore = Some((self.model.harness, tab.place.clone()));
                self.load_chat(id, cx);
            }
            Some(_) => {}
            None => {
                self.current = None;
                self.session = None;
                self.model.entries.clear();
                self.new_chat(window, cx);
                let draft = tab.place.draft.clone();
                self.composer
                    .update(cx, |c, cx| crate::view::set_end(c, draft, window, cx));
            }
        }
        self.persist_tabs();
        cx.notify();
    }

    /// Loads a past chat into the window.
    fn load_chat(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.park_running(cx);
        self.switch_draft(Some(id), cx);
        if self.background.contains_key(&id) {
            // Put away while it ran (#71): it comes back as it is now,
            // with its own composer, without reading the log again.
            self.session = None;
            self.open_sources.clear();
            self.pending_restore = None;
            let text = self.model.restore_draft(Some(id));
            self.composer_restore = Some(text);
            self.model.screen = Screen::Chat;
            self.unpark(id, cx);
            if let Some(t) = self
                .model
                .all_sessions
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.title.clone())
            {
                self.tabs.set_active(id, &t);
            }
            return;
        }
        self.session = None;
        self.current = Some(id);
        self.open_sources.clear();
        self.model.new_chat();
        // `new_chat` empties the composer; this chat's own draft returns.
        let text = self.model.restore_draft(Some(id));
        self.composer_restore = Some(text);
        self.model.phase = Phase::Connecting;
        self.model.screen = Screen::Chat;
        self.bridge.resume(id);
        cx.notify();
    }

    /// Puts the open chat's composer (text, attachments, audio still being
    /// transcribed) away before chat `to` shows. Each chat keeps its own.
    fn switch_draft(&mut self, to: Option<SessionId>, cx: &mut Context<Self>) {
        if self.current == to {
            return;
        }
        let mut text = self.composer.read(cx).value().to_string();
        // Dictation belongs to the chat it started in (#71): leaving that
        // chat stops it, and what was heard stays in its composer.
        if self.dictation.is_some() || self.recording.is_some() {
            if let Some(rec) = self.recording.take() {
                rec.cancel();
            }
            if let Some(mut d) = self.dictation.take() {
                let heard: Vec<String> = [
                    d.take_committed(),
                    d.interim_line()
                        .map(|l| l.trim_end_matches('…').trim().to_owned()),
                ]
                .into_iter()
                .flatten()
                .filter(|t| !t.is_empty())
                .collect();
                if !heard.is_empty() {
                    if !text.is_empty() && !text.ends_with(char::is_whitespace) {
                        text.push(' ');
                    }
                    text.push_str(&heard.join(" "));
                }
            }
            self.dictation_model = None;
            self.dictation_samples = Vec::new();
            self.dictation_pos = 0;
            self.dictation_task = None;
            self.dictation_switching = None;
            self.dictation_checking = false;
            self.dictation_rec += 1;
        }
        self.model.stash_draft(self.current, text);
    }

    /// Turns a chat's row into a title field.
    fn start_rename(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(title) = self
            .model
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
        else {
            return;
        };
        self.renaming = Some(id);
        self.rename_input.update(cx, |i, cx| {
            i.set_value(title, window, cx);
            i.select_all(window, cx);
            i.focus(window, cx);
        });
        cx.notify();
    }

    /// Saves the typed title. A blank or unchanged title keeps the old one.
    fn finish_rename(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.renaming.take() else {
            return;
        };
        let typed = self.rename_input.read(cx).value().to_string();
        let old = self
            .model
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone());
        if let Some(title) = tern_agent::clean_title(&typed)
            && Some(&title) != old.as_ref()
        {
            self.model.rename(id, &title);
            self.bridge.rename(id, title);
        }
        cx.notify();
    }

    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.renaming = None;
        cx.notify();
    }

    /// Hides a chat from the list; archiving the open chat starts a new one.
    fn archive_chat(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        if self.current == Some(id) {
            if self.model.is_busy() {
                return;
            }
            self.current = None;
            self.model.entries.clear();
            self.new_chat(window, cx);
        }
        if let Some(tab) = self.tabs.forget(id).cloned()
            && tab.id.is_some()
        {
            self.show_tab(tab, window, cx);
        }
        self.model.sessions.retain(|s| s.id != id);
        self.model.all_sessions.retain(|s| s.id != id);
        self.bridge.archive(id);
        cx.notify();
    }

    fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A running chat goes on in the background (#71).
        self.park_running(cx);
        let had_messages = !self.model.entries.is_empty();
        let fresh = had_messages || self.session.is_none();
        // The old chat keeps its composer; the new one starts empty.
        if fresh {
            self.switch_draft(None, cx);
        }
        // From inside a folder project the new chat starts in that folder
        // (#74); it can still pick another under the composer.
        let folder = self.current_folder_project();
        self.model.projects.new_in = folder.clone();
        self.model.new_chat();
        self.open_sources.clear();
        if fresh {
            // A fresh session row, so the old chat keeps its own history.
            self.tabs.reset_active();
            self.session = None;
            self.current = None;
            self.model.phase = Phase::Connecting;
            match folder {
                Some(name) => self.bridge.connect_in(Some(name)),
                None => self.bridge.connect(),
            }
        }
        let text = self.model.restore_draft(self.current);
        self.composer.update(cx, |c, cx| {
            crate::view::set_end(c, text, window, cx);
            c.focus(window, cx);
        });
        cx.notify();
    }

    /// The title bar's sun and moon: flips the theme and saves it as an
    /// explicit Light or Dark choice.
    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let to = if self.model.dark {
            crate::appearance::ThemeChoice::Light
        } else {
            crate::appearance::ThemeChoice::Dark
        };
        self.set_appearance(crate::appearance::Choice::Theme(to), window, cx);
    }

    /// One click on the Appearance screen: applied at once (theme, accent,
    /// font, text size) and saved to preferences.
    pub(super) fn set_appearance(
        &mut self,
        choice: crate::appearance::Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.look.apply(choice) {
            return;
        }
        let dark = self.look.is_dark(system_dark(window));
        if dark != self.model.dark && !self.reduced_motion {
            self.wipe = Some(ThemeWipe {
                since: Instant::now(),
                to_dark: dark,
            });
        }
        self.model.dark = dark;
        crate::theme::apply_kit(dark, &self.look, Some(window), cx);
        let look = self.look;
        self.save_ui_prefs(move |p| look.save_to(p));
        window.refresh();
        cx.notify();
    }

    // ----- providers -----

    fn open_providers(&mut self, cx: &mut Context<Self>) {
        self.show_settings(crate::settings::Section::Providers);
        if !self.model.providers.loaded {
            self.bridge.load_providers();
        } else {
            self.refresh_models();
        }
        cx.notify();
    }

    /// Loads the model list of the default provider when it is not loaded.
    /// Lists every usable provider's models for the routing (the mic and
    /// pictures the chat model cannot see).
    fn refresh_catalog(&mut self) {
        // The local models too: the mic and audio files can use them.
        self.reload_local_models();
        let ids: Vec<&'static str> = self
            .model
            .providers
            .section(Kind::Model)
            .filter(|r| r.usable())
            .map(|r| r.id)
            .collect();
        if ids.is_empty() {
            self.model.catalog = Default::default();
            return;
        }
        self.bridge.catalog(ids);
    }

    /// The speech-to-text model the mic uses: a local live-dictation
    /// model when one is installed and on, else the saved provider model,
    /// else the best listed one (the chat's provider first), else a local
    /// transcription model.
    fn stt_model(&self) -> Option<tern_agent::media::ModelRef> {
        use tern_agent::local_models::Task;
        let local = &self.model.settings.local;
        local
            .transcriber(Task::Dictation)
            .or_else(|| self.remote_stt_model())
            .or_else(|| local.transcriber(Task::SpeechToText))
    }

    /// The Whisper model dictation uses for languages other than English
    /// (#68): a local Whisper model when one is installed and on, else
    /// the provider speech-to-text model (they take a language too).
    fn whisper_model(&self) -> Option<tern_agent::media::ModelRef> {
        use tern_agent::local_models::Task;
        self.model
            .settings
            .local
            .transcriber(Task::SpeechToText)
            .filter(|m| m.model.starts_with("whisper"))
            .or_else(|| self.remote_stt_model())
    }

    /// The model that transcribes attached audio files: a local
    /// transcription model when one is installed and on (it runs on this
    /// computer), else the provider model the mic would use.
    fn file_stt_model(&self) -> Option<tern_agent::media::ModelRef> {
        use tern_agent::local_models::Task;
        self.model
            .settings
            .local
            .transcriber(Task::SpeechToText)
            .or_else(|| self.remote_stt_model())
    }

    /// The provider speech-to-text model: the saved choice, else the best
    /// listed one (the chat's provider first).
    fn remote_stt_model(&self) -> Option<tern_agent::media::ModelRef> {
        let near = self.model.providers.default_id(Kind::Model);
        self.model.catalog.pick(
            tern_agent::media::Job::Transcribe,
            self.model.providers.prefs.default_stt_model.as_deref(),
            near,
        )
    }

    /// Starts recording, or stops it. Speech is transcribed while it is
    /// spoken (see [`crate::dictation`]): the open phrase shows in grey
    /// under the composer text, each finished phrase joins the text, and
    /// stopping sends the last bit and commits it. While recording, the
    /// model chip names the voice model.
    fn toggle_mic(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            self.stop_mic(window, cx);
            return;
        }
        use tern_agent::local_models::{Task, speech};
        // The first recording after launch can come before the models
        // folder was read in the background; read it now so the first
        // recording routes exactly as the later ones (#69).
        if self.model.settings.local.statuses.is_none()
            && let Ok((lm, warning)) = tern_agent::local_models::LocalModels::discover()
        {
            let st = lm.statuses(&self.model.providers.prefs.local_models);
            let root = lm.root().display().to_string();
            self.model.settings.local.loaded(st, root, warning);
        }
        let fast = self.model.settings.local.transcriber(Task::Dictation);
        let whisper = self.whisper_model();
        let language =
            speech::dictation_language(self.model.providers.prefs.dictation_language.as_deref());
        let Some((route, note)) =
            crate::dictation::Route::pick(language, fast.is_some(), whisper.is_some())
        else {
            // Nothing for the language rule: the old order (a provider
            // model, a local transcription model), else no voice model.
            let Some(via) = self.stt_model() else {
                self.model.toast_error(NO_VOICE_MODEL, None);
                cx.notify();
                return;
            };
            self.start_recording(
                via.clone(),
                None,
                Some(via),
                crate::dictation::Route::Fast,
                window,
                cx,
            );
            return;
        };
        if let Some(n) = note {
            self.model.toast_info(n);
        }
        // Auto: a language heard with confidence in the last two minutes
        // picks the model at once; the check still runs again later.
        let remembered = match (&route, &self.dictation_lang_memo) {
            (crate::dictation::Route::Detect, Some((l, p, at)))
                if at.elapsed() < LANG_MEMO && *p >= 0.8 =>
            {
                Some(l.clone())
            }
            _ => None,
        };
        let route = match &remembered {
            Some(l) => crate::dictation::Route::checked(l, fast.is_some()),
            None => route,
        };
        // Auto streams with the fast model while the language is checked.
        let via = match route {
            crate::dictation::Route::Fast | crate::dictation::Route::Detect => fast.clone(),
            _ => whisper.clone(),
        };
        let Some(via) = via else {
            self.model.toast_error(NO_VOICE_MODEL, None);
            cx.notify();
            return;
        };
        tern_agent::diag::note(&format!(
            "dictation: language={} route={route:?} via={via}{}",
            speech::language_name(language),
            remembered
                .as_deref()
                .map(|l| format!(" (kept {l} from the last recording)"))
                .unwrap_or_default()
        ));
        let auto = language.is_none();
        // Auto keeps Whisper loaded from the first second (#71), so a
        // switch to Urdu does not wait for the model to load.
        let warm = whisper.clone().filter(|w| auto && w.is_local());
        self.start_recording(via, fast, whisper, route, window, cx);
        self.dictation_auto = auto;
        if let Some(w) = warm {
            self.bridge.warm_speech(w);
        }
        if remembered.is_some() {
            self.dictation_last_check = Some(Instant::now());
        }
    }

    /// Opens the mic and starts streaming windows to `via`.
    fn start_recording(
        &mut self,
        via: tern_agent::media::ModelRef,
        fast: Option<tern_agent::media::ModelRef>,
        whisper: Option<tern_agent::media::ModelRef>,
        route: crate::dictation::Route,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match tern_agent::platform::mic::Recording::start() {
            Ok(r) => {
                self.recording = Some(r);
                let mut d = crate::dictation::Dictation::with_pace(route.pace());
                d.set_language(route_language(&route));
                self.dictation = Some(d);
                self.dictation_switching = None;
                self.dictation_model = Some(via);
                self.dictation_fast = fast;
                self.dictation_whisper = whisper;
                self.dictation_route = route;
                self.dictation_auto = false;
                self.dictation_checking = false;
                self.dictation_check_from = None;
                self.dictation_last_check = None;
                self.dictation_rec += 1;
                let cursor = self.composer.read(cx).cursor();
                self.dictation_caret = crate::dictation::Caret::start(cursor);
                self.dictation_samples.clear();
                self.dictation_pos = 0;
                self.start_dictation_timer(window, cx);
            }
            Err(e) => self.model.toast_error(e, None),
        }
        cx.notify();
    }

    /// Stops the mic; the last window is sent and committed when it comes
    /// back.
    fn stop_mic(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(rec) = self.recording.take() {
            self.pull_speech(&rec);
            rec.cancel();
        }
        if let Some(d) = self.dictation.as_mut() {
            d.stop();
        }
        self.pump_dictation(window, cx);
        cx.notify();
    }

    /// Reads the speech recorded since the last read.
    fn pull_speech(&mut self, rec: &tern_agent::platform::mic::Recording) {
        let (more, pos) = rec.speech_since(self.dictation_pos);
        self.dictation_samples.extend(more);
        self.dictation_pos = pos;
    }

    /// Every 200 ms while dictating (#71: was 400 ms): read new speech,
    /// send the next window when it is due, and end when the last one is
    /// back.
    fn start_dictation_timer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dictation_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let go = this.update_in(cx, |this, window, cx| {
                    if let Some(rec) = this.recording.take() {
                        this.pull_speech(&rec);
                        this.recording = Some(rec);
                    }
                    this.pump_dictation(window, cx)
                });
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
        }));
    }

    /// Sends the next window if one is due, moves committed text into the
    /// composer, and finishes a stopped dictation whose last window is
    /// back. Returns whether dictation goes on.
    fn pump_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.maybe_check_language();
        let Some(d) = self.dictation.as_mut() else {
            return false;
        };
        let req = d.next_request(&self.dictation_samples);
        if let Some(r) = req.filter(|r| r.quiet) {
            // No speech in the window: nothing is sent, so a model never
            // writes words for silence or noise (#70).
            d.on_result(r.seq, Ok(String::new()));
        } else if let Some(req) = req
            && let Some(via) = self.dictation_model.clone()
        {
            let end = req.end.min(self.dictation_samples.len());
            let start = req.start.min(end);
            let wav = tern_agent::platform::audio::wav(
                &self.dictation_samples[start..end],
                tern_agent::platform::audio::SPEECH_RATE,
            );
            let language = match &self.dictation_route {
                crate::dictation::Route::Whisper(l) => l.clone(),
                crate::dictation::Route::Fast | crate::dictation::Route::Detect => None,
            };
            self.bridge.dictate(via, req.seq, wav, language);
        }
        // While Auto checks the language, the fast model's text shows grey
        // and is held: Urdu heard starts the recording over on Whisper.
        let detecting = self.dictation_route == crate::dictation::Route::Detect;
        // A later phrase's check holds its text too, so a switch to Urdu
        // can hear the phrase again without English words left behind.
        if !detecting && !self.dictation_checking {
            self.commit_dictation(window, cx);
        }
        let finished = !self.dictation_checking
            && self
                .dictation
                .as_ref()
                .is_some_and(|d| d.done(self.dictation_samples.len()));
        if finished {
            if detecting {
                // Too short to check: the fast model's text stands.
                self.commit_dictation(window, cx);
            }
            // One note per recording at most (#68: "heard no speech"
            // toasts kept popping for every pause).
            if let Some(e) = self.dictation.as_mut().and_then(|d| d.error.take()) {
                self.model
                    .toast_error(format!("Some speech could not be transcribed: {e}"), None);
            } else if let Some(n) = self
                .dictation
                .as_ref()
                .and_then(|d| crate::dictation::nothing_heard_note(d, &self.dictation_samples))
            {
                self.model.toast_info(n);
            }
            self.dictation = None;
            self.dictation_model = None;
            self.dictation_samples = Vec::new();
            self.dictation_pos = 0;
            cx.notify();
            return false;
        }
        true
    }

    /// Finished phrases join the composer text (dark, editable).
    fn commit_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.dictation.as_mut().and_then(|d| d.take_committed()) else {
            return;
        };
        self.insert_dictated(&text, window, cx);
    }

    /// Puts a dictated phrase into the composer and moves the caret to
    /// its end (the UI tour streams mocked phrases through it too).
    fn insert_dictated(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        // At the end of what was dictated so far (the caret follows it),
        // or where the user put the caret during dictation (#68).
        let (now, cursor) = {
            let c = self.composer.read(cx);
            (c.value().to_string(), c.cursor())
        };
        let (joined, at) = self.dictation_caret.place(&now, cursor, text);
        self.model.chat_here = self.current;
        self.model.on_composer_text(&joined);
        self.composer.update(cx, |c, cx| {
            c.set_value(joined, window, cx);
            c.set_selected_range(at..at, cx);
        });
        cx.notify();
    }

    /// A window of dictation came back.
    fn dictated(
        &mut self,
        seq: u64,
        result: Result<String, String>,
        heard: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = heard;
        if result.is_ok() {
            self.dictation_switching = None;
        }
        if let Some(d) = self.dictation.as_mut() {
            // Auto's language comes from its own check (#70), not from
            // the windows' transcription.
            d.on_result(seq, result);
        }
        self.pump_dictation(window, cx);
    }

    /// Starts Auto's language check when it is due (#70): once on the
    /// first seconds of a recording, and again on a new phrase after a
    /// pause, at most every [`RECHECK_EVERY`], while it is on English.
    fn maybe_check_language(&mut self) {
        if self.dictation_checking || !self.dictation_auto {
            return;
        }
        let Some(d) = self.dictation.as_ref() else {
            return;
        };
        let from = match &self.dictation_route {
            crate::dictation::Route::Detect => 0,
            crate::dictation::Route::Fast
                if self
                    .dictation_last_check
                    .is_some_and(|t| t.elapsed() >= RECHECK_EVERY) =>
            {
                d.open_start()
            }
            _ => return,
        };
        if self.dictation_check_from == Some(from) {
            return;
        }
        let Some((a, b)) =
            crate::dictation::language_window(&self.dictation_samples, from, d.stopped())
        else {
            return;
        };
        self.dictation_checking = true;
        self.dictation_check_from = Some(from);
        self.dictation_last_check = Some(Instant::now());
        self.bridge.check_language(
            self.dictation_rec,
            from,
            self.dictation_samples[a..b].to_vec(),
        );
    }

    /// Auto's language check came back (#70). English keeps the fast
    /// model (and the text it held); another language goes to Whisper:
    /// from the start of the recording on the first check, or from the
    /// open phrase when the speaker switched language later.
    fn language_checked(
        &mut self,
        rec: u64,
        from: usize,
        result: Result<(String, f32, String), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if rec != self.dictation_rec {
            return;
        }
        self.dictation_checking = false;
        let fast = self.dictation_fast.is_some();
        let next = match &result {
            Ok((lang, p, model)) => {
                if *p >= 0.5 {
                    self.dictation_lang_memo = Some((lang.clone(), *p, Instant::now()));
                }
                let next = crate::dictation::Route::checked(lang, fast);
                tern_agent::diag::note(&format!(
                    "dictation: language check heard {lang} (p={p:.2}, {model}) from {:.1}s -> {next:?}",
                    from as f64 / crate::dictation::RATE as f64
                ));
                // A later phrase switches only when the check is sure.
                if self.dictation_route != crate::dictation::Route::Detect && *p < 0.6 {
                    self.dictation_route.clone()
                } else {
                    next
                }
            }
            Err(e) => {
                tern_agent::diag::note(&format!(
                    "dictation: language check failed ({e}); staying on the fast model"
                ));
                crate::dictation::Route::Fast
            }
        };
        let was = self.dictation_route.clone();
        if next != was
            && let Some(d) = self.dictation.as_mut()
        {
            match (&was, &next) {
                (crate::dictation::Route::Detect, crate::dictation::Route::Fast) => {}
                (crate::dictation::Route::Detect, _) => d.restart(next.pace()),
                // The phrase the check heard is heard again from where it
                // started: no word is lost in the switch (#71).
                (_, crate::dictation::Route::Whisper(_)) => d.rewind(from, next.pace()),
                _ => d.redo_open_window(next.pace()),
            }
            d.set_language(route_language(&next));
            if let crate::dictation::Route::Whisper(l) = &next {
                self.dictation_switching = Some(format!(
                    "Switching to {}…",
                    tern_agent::local_models::speech::language_name(l.as_deref())
                ));
            }
            if next != crate::dictation::Route::Fast {
                self.dictation_model = self
                    .dictation_whisper
                    .clone()
                    .or(self.dictation_model.take());
            }
        }
        self.dictation_route = next;
        self.pump_dictation(window, cx);
        cx.notify();
    }

    /// The open phrase's interim text, grey with a trailing ellipsis.
    fn dictation_line(&self) -> Option<String> {
        self.dictation
            .as_ref()
            .and_then(|d| d.interim_line())
            .or_else(|| self.dictation_switching.clone())
    }

    /// Esc while recording: stop and throw the audio away.
    pub(crate) fn cancel_recording(&mut self) -> bool {
        let had = self.dictation.is_some();
        self.dictation = None;
        self.dictation_model = None;
        self.dictation_task = None;
        self.dictation_samples = Vec::new();
        self.dictation_pos = 0;
        match self.recording.take() {
            Some(r) => {
                r.cancel();
                true
            }
            None => had,
        }
    }

    fn refresh_models(&mut self) {
        let Some(id) = self.model.providers.default_id(Kind::Model) else {
            self.model.providers.models = None;
            return;
        };
        if self
            .model
            .providers
            .models
            .as_ref()
            .is_some_and(|m| m.provider() == id && !matches!(m, Models::Failed(..)))
        {
            return;
        }
        if !self.model.providers.show_models(id, Instant::now()) {
            self.bridge.models(id, false);
        }
    }

    /// Applies the current choices, saving them first. The open chat
    /// switches model or search in place (same chat, same history); only
    /// with no chat open yet does a new one open.
    fn reconnect(&mut self) {
        if self.model.is_busy() {
            return;
        }
        let reopen = self.resume_retry.filter(|id| self.current == Some(*id));
        match (self.session.take(), reopen) {
            (Some(s), _) => {
                self.model.phase = Phase::Connecting;
                self.bridge.swap(s, self.model.providers.prefs.clone());
            }
            // A chat whose model could not be reached reopens with the
            // new choice, keeping its transcript.
            (None, Some(id)) => {
                self.resume_retry = None;
                self.model.phase = Phase::Connecting;
                self.bridge
                    .save_and_resume(self.model.providers.prefs.clone(), id);
            }
            (None, None) => {
                self.model.connection = None;
                self.model.new_chat();
                self.model.phase = Phase::Connecting;
                self.bridge
                    .save_and_connect(self.model.providers.prefs.clone());
            }
        }
    }

    fn apply_choice(&mut self, kind: Kind, id: &'static str) {
        if self.model.providers.choose(kind, id) {
            if kind == Kind::Model {
                self.refresh_models();
            }
            self.reconnect();
        }
    }

    fn pick_model(&mut self, model: String, cx: &mut Context<Self>) {
        if self.model.is_busy() {
            return;
        }
        self.model.providers.choose_model(&model);
        self.reconnect();
        cx.notify();
    }

    fn edit_key(
        &mut self,
        kind: Kind,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.model.providers.start_edit(kind, id);
        self.key_input.update(cx, |k, cx| {
            k.set_value("", window, cx);
            k.focus(window, cx);
        });
        cx.notify();
    }

    fn save_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((kind, id)) = self.model.providers.editing else {
            return;
        };
        if self.model.providers.is_working(kind, id) {
            return;
        }
        let key = self.key_input.read(cx).value().trim().to_owned();
        if key.is_empty() {
            return;
        }
        self.model.providers.begin(kind, id, "Testing the key…");
        self.bridge.add_key(kind, id, SecretString::new(key));
        // The field keeps no copy while the test runs.
        self.key_input
            .update(cx, |k, cx| k.set_value("", window, cx));
        cx.notify();
    }

    // ----- pieces -----

    fn title_bar(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        // The harness switcher lives at the top of the sidebar (like the
        // ChatGPT / Codex app switcher), not in the title bar.
        TitleBar::new().child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_between()
                .pr_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(self.wordmark(p))
                        // `occlude` keeps title-bar controls out of the window's
                        // drag region; on Windows a click there otherwise goes to
                        // the OS as a caption drag and the button never sees it.
                        .child(
                            div().occlude().child(
                                Button::new("toggle-sidebar")
                                    .capsule()
                                    .icon(IconName::PanelLeft)
                                    .ghost()
                                    .small()
                                    .tooltip("Show or hide chats")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.model.sidebar_open = !this.model.sidebar_open;
                                        cx.notify();
                                    })),
                            ),
                        )
                        .child(div().occlude().child(self.workspace_button(p, cx)))
                        .children(self.git_chip(p, cx))
                        .children(self.title_crumb(p)),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .occlude()
                        .when(self.model.screen == Screen::Chat, |d| {
                            d.child(self.outputs_button(p, cx))
                        })
                        .child(self.search_button(cx))
                        .child(
                            Button::new("toggle-theme")
                                .capsule()
                                .icon(if self.model.dark {
                                    IconName::Sun
                                } else {
                                    IconName::Moon
                                })
                                .ghost()
                                .small()
                                .tooltip("Light or dark")
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.toggle_theme(window, cx)
                                    }),
                                ),
                        )
                        .child(
                            Button::new("toggle-run")
                                .capsule()
                                .icon(IconName::PanelRight)
                                .ghost()
                                .small()
                                .tooltip("Show or hide the run panel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this.model.browser.visible && this.model.run_panel_open {
                                        this.model.browser.visible = false;
                                    } else {
                                        this.model.browser.visible = false;
                                        this.model.run_panel_open = !this.model.run_panel_open;
                                    }
                                    cx.notify();
                                })),
                        )
                        .children(self.maximise_button(cx)),
                ),
        )
    }

    /// The left rail: tight groups of icons 4 px apart with a short rule
    /// between groups (chats, work surfaces, tools), settings at the foot.
    fn rail(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let slots = self.rail_slots.clone();
        let view = self.view_id;
        // Records where slot `i` was laid out (the rail itself is the last
        // slot), so the marker knows where to slide next frame.
        let measured = move |i: usize, el: AnyElement| -> AnyElement {
            let slots = slots.clone();
            div()
                .relative()
                .flex_none()
                .child(el)
                .child(
                    gpui_kit::canvas(
                        move |b, window, _| {
                            if let Some(s) = slots.borrow_mut().get_mut(i)
                                && s.replace(b) != Some(b)
                            {
                                frame::next_frame_for(window, view);
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .into_any_element()
        };
        let marker = self.rail_marker(p);
        let rule = || {
            div()
                .w(px(18.))
                .h(px(1.))
                .my(px(6.))
                .bg(p.border)
                .flex_none()
        };
        div()
            .relative()
            .w(px(RAIL_W))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(crate::layout::RAIL_GAP))
            .py_2()
            .bg(p.rail)
            .child({
                let slots = self.rail_slots.clone();
                let view = self.view_id;
                gpui_kit::canvas(
                    move |b, window, _| {
                        if slots.borrow_mut()[RAIL_SLOTS].replace(b) != Some(b) {
                            frame::next_frame_for(window, view);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
            })
            .children(marker)
            .border_r_1()
            .border_color(p.border)
            .child(
                Button::new("rail-new")
                    .capsule()
                    .icon(IconName::Plus)
                    .ghost()
                    .tooltip("New chat")
                    .on_click(cx.listener(|this, _, window, cx| this.new_chat(window, cx))),
            )
            .child(measured(
                0,
                Button::new("rail-chats")
                    .capsule()
                    .icon(IconName::Inbox)
                    .ghost()
                    .tooltip("Chats")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.sidebar_open = !this.model.sidebar_open;
                        cx.notify();
                    }))
                    .into_any_element(),
            ))
            .child(measured(
                1,
                Button::new("rail-library")
                    .capsule()
                    .icon(AssetIcon::Library)
                    .ghost()
                    .when(self.model.screen == Screen::Library, |b| b.selected(true))
                    .tooltip("Library")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.model.screen == Screen::Library {
                            this.close_library(window, cx);
                        } else {
                            this.open_library(cx);
                        }
                    }))
                    .into_any_element(),
            ))
            .child(rule())
            .child(measured(
                2,
                Button::new("rail-editor")
                    .capsule()
                    .icon(IconName::FileText)
                    .ghost()
                    .when(self.model.screen == Screen::Editor, |b| b.selected(true))
                    .tooltip("Editor")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.model.screen == Screen::Editor {
                            this.close_editor(window, cx);
                        } else {
                            this.open_editor(window, cx);
                        }
                    }))
                    .into_any_element(),
            ))
            .child(measured(
                3,
                Button::new("rail-browser")
                    .capsule()
                    .icon(IconName::Globe)
                    .ghost()
                    .when(
                        self.model.browser.visible && self.model.run_panel_open,
                        |b| b.selected(true),
                    )
                    .tooltip("Browser")
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_browser(window, cx)))
                    .into_any_element(),
            ))
            .child(measured(
                4,
                Button::new("rail-git")
                    .capsule()
                    .icon(AssetIcon::GitBranch)
                    .ghost()
                    .when(self.model.screen == Screen::Git, |b| b.selected(true))
                    .tooltip("Git")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.model.screen == Screen::Git {
                            this.close_git(window, cx);
                        } else {
                            this.open_git(window, cx);
                        }
                    }))
                    .into_any_element(),
            ))
            .child(div().flex_1())
            .child(measured(
                5,
                Button::new("rail-settings")
                    .capsule()
                    .icon(IconName::Settings)
                    .ghost()
                    .when(self.model.screen == Screen::Settings, |b| b.selected(true))
                    .tooltip("Settings (Ctrl+,)")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.model.screen == Screen::Settings {
                            this.close_settings(window, cx);
                        } else {
                            let s = this.model.settings.section;
                            this.open_settings(s, window, cx);
                        }
                    }))
                    .into_any_element(),
            ))
    }

    /// Which rail icon stands for the screen showing (v2's ribbon bar).
    fn rail_active(&self) -> Option<usize> {
        match self.model.screen {
            Screen::Settings => Some(5),
            Screen::Git => Some(4),
            Screen::Editor => Some(2),
            Screen::Library => Some(1),
            _ if self.model.browser.visible && self.model.run_panel_open => Some(3),
            Screen::Chat | Screen::Watches | Screen::Archived | Screen::Study => Some(0),
        }
    }

    /// The 2 px bar on the active icon's left edge. One bar for the whole
    /// rail: it springs from icon to icon (`motion::Glide`), even down to
    /// Settings at the foot.
    fn rail_marker(&self, p: Palette) -> Option<AnyElement> {
        let i = self.rail_active()?;
        let slots = self.rail_slots.borrow();
        let rail = slots[RAIL_SLOTS]?;
        let slot = slots[i]?;
        let h = f32::from(slot.size.height) * 0.56;
        let target =
            f32::from(slot.origin.y - rail.origin.y) + (f32::from(slot.size.height) - h) / 2.;
        let y = self.glide("rail-marker", target);
        Some(
            div()
                .absolute()
                .left_0()
                .top(px(y))
                .w(px(2.))
                .h(px(h))
                .rounded_r(px(1.))
                .bg(p.accent)
                .into_any_element(),
        )
    }

    /// The lockup: the mark, TERN, and the harness in italic serif.
    fn wordmark(&self, p: Palette) -> impl IntoElement {
        let harness = if self.model.harness == Harness::Study {
            "Study"
        } else {
            "Research"
        };
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(
                svg()
                    .path(assets::MARK)
                    .size(px(15.))
                    .text_color(p.text)
                    // The launch splash flies the mark here.
                    .when(self.mark_in_flight(), |d| d.opacity(0.)),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("TERN"),
            )
            .child(
                div()
                    .font_family(assets::SERIF_FONT)
                    .italic()
                    .text_base()
                    .text_color(p.muted)
                    .child(harness),
            )
    }

    /// One chat or sub-session row in the sidebar's project tree.
    fn chat_row(&self, ix: usize, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.model.sessions[ix];
        let current = Some(s.id) == self.current;
        let id = s.id;
        let sub = s.parent.is_some();
        let planned = s.plan.is_some();
        let running = !current && self.runs_in_background(id);
        let needs_you = current && self.model.scout_mood() == crate::scout::Mood::NeedsYou;
        let age = crate::rows::age_label(&s.created_at, crate::rows::now_secs());
        if self.renaming == Some(id) {
            return div()
                .id(("chat", ix))
                .px_1()
                .py(px(2.))
                .on_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, _, cx| {
                    if e.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.cancel_rename(cx);
                    }
                }))
                .child(Input::new(&self.rename_input).small())
                .into_any_element();
        }
        let row = div()
            .id(("chat", ix))
            .group("chat-row")
            .on_drag(
                customize_view::DraggedChat {
                    id,
                    title: s.title.clone(),
                    palette: p,
                },
                |d, _, _, cx| cx.new(|_| d.clone()),
            )
            .flex()
            .items_center()
            .gap_1()
            .pl(px(if sub { 34. } else { 22. }))
            .pr_1()
            .py(px(self.look.density.row_pad() - 1.))
            .rounded_md()
            .text_sm()
            .text_color(if current { p.text } else { p.muted })
            .when(current, |d| d.bg(p.hover))
            .when(!current, |d| {
                d.cursor_pointer()
                    .hover(|d| d.bg(p.hover).text_color(p.text))
            })
            .on_click(
                cx.listener(move |this, e: &gpui_kit::ClickEvent, window, cx| {
                    if e.click_count() >= 2 {
                        this.start_rename(id, window, cx);
                    } else {
                        this.open_chat(id, cx);
                    }
                }),
            )
            .when(sub, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_color(p.faint)
                        .child(SharedString::from(match s.anchor {
                            Some(a) => format!("↳ Q{a}"),
                            None => "↳".to_owned(),
                        })),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .py(px(3.))
                    .truncate()
                    // An Urdu title reads right to left, `??` at its left
                    // end (#68).
                    .when(crate::bidi::is_rtl(&s.title), |d| d.text_right())
                    .child(SharedString::from(crate::bidi::display_line(&s.title))),
            )
            .when(running, |d| d.child(Spinner::new().xsmall()))
            // v2's one mark: ◆ when this chat is waiting on you.
            .when(needs_you, |d| {
                d.child(div().flex_none().text_xs().text_color(p.accent).child("◆"))
            })
            // The age, quiet, on the right; it gives way to the menu on
            // hover.
            .when(!running && !needs_you, |d| {
                d.children(age.map(|a| {
                    div()
                        .flex_none()
                        .text_xs()
                        .font_family(crate::assets::MONO_FONT)
                        .text_color(p.faint)
                        .group_hover("chat-row", |d| d.opacity(0.))
                        .child(SharedString::from(a))
                }))
            })
            .when(planned && !running, |d| {
                d.child(
                    Button::new(("run-sub", ix))
                        .capsule()
                        .icon(AssetIcon::Play)
                        .ghost()
                        .xsmall()
                        .tooltip("Planned: run it in the background; it reports back")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.bridge.sub_run(id);
                            this.model.cont.background.push(id);
                            cx.notify();
                        })),
                )
            })
            .child(
                div()
                    .flex()
                    .when(self.model.row_menu != Some(id), |d| {
                        d.opacity(0.).group_hover("chat-row", |d| d.opacity(1.))
                    })
                    .child(self.row_menu_button(ix, id, cx)),
            )
            .into_any_element();
        let row = self.with_row_menu(row, ix, id, sub, p, cx);
        self.grow_in("chat-rows", id.to_string(), row)
    }

    /// The harness switcher at the top of the sidebar: the harness's name
    /// as a title with a chevron; it opens a rounded popover with one row
    /// per harness (bold name, a muted line on what it is for, a check on
    /// the active one) that fades and slides in on the house curve.
    fn harness_switcher(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let active = self.model.harness;
        let (name, _) = crate::harness::label(active);
        let open = self.switcher.is_some();
        let title = div()
            .id("harness-switcher")
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_lg()
            .cursor_pointer()
            .when(open, |d| d.bg(p.hover))
            .hover(|d| d.bg(p.hover))
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.text)
                    .when(active == Harness::Study, |d| {
                        d.font_family(assets::SERIF_FONT)
                            .font_weight(FontWeight::NORMAL)
                            .text_xl()
                    })
                    .child(name),
            )
            .child(
                gpui_kit::component::Icon::new(IconName::ChevronDown)
                    .size(px(14.))
                    .text_color(p.muted),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.switcher = match this.switcher {
                    Some(_) => None,
                    None => Some(std::time::Instant::now()),
                };
                cx.notify();
            }));
        let menu = open.then(|| {
            let rows = [Harness::Research, Harness::Study].map(|h| {
                let (n, line) = crate::harness::label(h);
                let on = h == active;
                div()
                    .id(SharedString::from(format!("harness-row-{n}")))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .cursor_pointer()
                    .hover(|d| d.bg(p.hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text)
                                    .child(n),
                            )
                            .child(div().text_xs().text_color(p.muted).truncate().child(line)),
                    )
                    .when(on, |d| {
                        d.child(
                            gpui_kit::component::Icon::new(IconName::Check)
                                .size(px(15.))
                                .text_color(p.text),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.switcher = None;
                        this.choose_harness(h, cx);
                        cx.notify();
                    }))
            });
            let dur = if self.reduced_motion {
                Duration::ZERO
            } else {
                motion::UI
            };
            div()
                .id("harness-menu")
                .absolute()
                .top(px(48.))
                .left(px(8.))
                .right(px(8.))
                .p_1()
                .flex()
                .flex_col()
                .gap_0p5()
                .rounded_xl()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .shadow_lg()
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.switcher = None;
                    cx.notify();
                }))
                .children(rows)
                .with_animation(
                    "harness-menu-in",
                    Animation::new(dur).with_easing(motion::ease),
                    |d, t| d.opacity(t).mt(px((1. - t) * -6.)),
                )
                .into_any_element()
        });
        div()
            .px_2()
            .pt_2()
            .child(title)
            .children(menu.map(|m| gpui_kit::deferred(m).with_priority(2)))
            .into_any_element()
    }

    fn sidebar(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let order = crate::continuity::tree(&self.model.sessions);
        let rows = self
            .model
            .projects
            .rows(&self.model.sessions, &order)
            .into_iter()
            .flat_map(|r| self.tree_row(r, p, cx))
            .collect::<Vec<_>>();
        div()
            .w(px(self.model.panels.sidebar))
            .h_full()
            .flex()
            .flex_col()
            .relative()
            .bg(p.sidebar)
            .border_r_1()
            .border_color(p.border)
            .child(self.harness_switcher(p, cx))
            .child(self.projects_header(p, cx))
            .children(self.project_name_field(None, p))
            .child(
                div()
                    .id("chat-list")
                    .flex_1()
                    .overflow_y_scrollbar()
                    .px_2()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    // Folder projects fill the list even before their
                    // first chat (#74).
                    .when(
                        self.model.sessions.is_empty()
                            && self.model.projects.projects.iter().all(|pr| pr.row.inbox),
                        |d| {
                            d.child(
                                div()
                                    .px_2()
                                    .text_sm()
                                    .text_color(p.faint)
                                    .child("Your chats will show here."),
                            )
                        },
                    )
                    .children(rows),
            )
            .children(self.watches_section(p, cx))
            .child(self.archived_link(p, cx))
    }

    fn entry(&self, i: usize, e: &Entry, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        match e {
            Entry::User {
                text, chips, quote, ..
            } => div()
                .id(("msg", i))
                .group(SharedString::from(format!("msg-{i}")))
                .flex()
                .flex_col()
                .items_end()
                .gap_1()
                .when_some(quote.clone(), |d, q| {
                    d.child(
                        div()
                            // The quoted reply as a chip over the bubble:
                            // a reply arrow and one line of the quote.
                            .max_w(px(COLUMN_MAX_W * 0.8))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_2p5()
                            .py_1()
                            .rounded_full()
                            .border_1()
                            .border_color(p.border)
                            .bg(p.raised.opacity(0.5))
                            .text_xs()
                            .text_color(p.muted)
                            .child(div().flex_none().text_color(p.faint).child("↩"))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child(SharedString::from(q.replace('\n', " "))),
                            ),
                    )
                })
                .when(!chips.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .justify_end()
                            .gap_1()
                            .max_w(px(COLUMN_MAX_W * 0.8))
                            // A sent picture opens large (#76).
                            .children(chips.iter().map(|c| self.sent_picture(c, p, cx))),
                    )
                })
                .child(
                    div()
                        .max_w(px(COLUMN_MAX_W * 0.8))
                        .px_3()
                        .py_2()
                        .rounded_xl()
                        .bg(p.raised)
                        .child({
                            // An Urdu or Arabic message reads right to left,
                            // as rows of words from the right edge.
                            if crate::bidi::has_rtl(text) && crate::bidi::is_rtl(text) {
                                rtl_flow(crate::bidi::rtl_lines(text, false), false)
                            } else {
                                div()
                                    .child(SharedString::from(text.clone()))
                                    .into_any_element()
                            }
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .children(self.stamp(i, "msg", p))
                        .child(self.message_id_tag(i, p, cx))
                        .child(self.grouped_actions(i, false, p, cx)),
                )
                .into_any_element(),
            Entry::Assistant {
                run,
                pages,
                thought,
                ..
            } => {
                let folded = self.model.folded.contains(&i);
                let revealing = self.model.is_revealing(i);
                let shown = self.model.shown_answer(i).unwrap_or_default();
                let body = if folded {
                    // Folded: the first lines, faded.
                    let first: String = shown.lines().take(2).collect::<Vec<_>>().join(" ");
                    div()
                        .text_sm()
                        .text_color(p.muted)
                        .line_clamp(2)
                        .child(SharedString::from(first))
                        .into_any_element()
                } else {
                    div()
                        .id(("answer-body", i))
                        .on_mouse_up(
                            gpui_kit::MouseButton::Left,
                            cx.listener(move |this, e: &gpui_kit::MouseUpEvent, window, cx| {
                                this.answer_mouse_up(i, e, window, cx)
                            }),
                        )
                        .child({
                            let open = std::sync::Arc::new(self.link_opener(cx));
                            // Citation chips once the answer has landed; while
                            // it streams, `[n]` stays text so the view only
                            // re-parses its last block (#72).
                            let shown = render_quotes(shown, run);
                            let text = with_badges(i, &shown, run);
                            // `\( \)` and `\[ \]` formulas as dollar math
                            // (round 2).
                            let text = crate::richmd::prepare(&text).into_owned();
                            let text = if revealing {
                                text
                            } else {
                                let targets = cite_pages(&text, pages);
                                if let Some(chain) = run.evidence.as_deref() {
                                    crate::richmd::set_cite_cards(cite_cards(
                                        chain,
                                        &targets,
                                        pages,
                                        &self.model.icons,
                                    ));
                                }
                                crate::richmd::chips(&text, &targets)
                            };
                            bidi_markdown("answer", i, text, move |t| {
                                let open = open.clone();
                                crate::richmd::rich(
                                    t.on_link_click(move |u, e, w, a| open(u, e, w, a))
                                        .selectable(true)
                                        .motion(stream_motion()),
                                )
                            })
                        })
                        .into_any_element()
                };
                let fold = div()
                    .id(("answer-fold", i))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .text_xs()
                    .text_color(p.faint)
                    .cursor_pointer()
                    .hover(|d| d.text_color(p.text))
                    .child(
                        gpui_kit::component::Icon::new(if folded {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .xsmall(),
                    )
                    .child(if folded { "Show answer" } else { "Fold" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.toggle_fold(i);
                        cx.notify();
                    }));
                let clickable = !run.is_empty();
                let open_run = div()
                    .id(("answer-run", i))
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(p.faint)
                    .when(clickable, |d| {
                        d.cursor_pointer()
                            .hover(|d| d.text_color(p.text))
                            .child("Run")
                            .child(gpui_kit::component::Icon::new(IconName::ChevronRight).xsmall())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model.show_run_of(i);
                                cx.notify();
                            }))
                    });
                div()
                    .id(("answer", i))
                    .group(SharedString::from(format!("answer-{i}")))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(thought.as_ref().map(|th| self.thinking_done(i, th, p, cx)))
                    .children(self.rollup(Some(i), run, p, cx))
                    .children(if folded {
                        None
                    } else {
                        self.plan_cards(Some(i), run, p, cx)
                    })
                    .children(if folded {
                        None
                    } else {
                        self.tool_cards(Some(i), run, p, cx)
                    })
                    .child(body)
                    .when(revealing && !folded, |d| {
                        d.child(composer_view::show_all(p, cx))
                    })
                    .children(if folded {
                        None
                    } else {
                        self.citations(i, run, pages, p, cx)
                    })
                    .when(!pages.is_empty() && !folded, |d| {
                        d.child(self.sources(i, pages, p, cx))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(fold)
                                    .child(open_run)
                                    .children(self.stamp(i, "answer", p)),
                            )
                            .child(self.grouped_actions(i, true, p, cx)),
                    )
                    .children(self.action_menu_view(i, p, cx))
                    .children(self.branch_menu(i, p, cx))
                    .into_any_element()
            }
            Entry::Notice(t) if t == crate::model::COMPACT_NOTICE => self.compact_notice(p, cx),
            // A cut-off reply's note (#76): a short line at the cut,
            // ruled on the left so it reads as the end of that reply.
            // #79: the line wraps inside the chat column; a long reason
            // no longer runs past the composer's width.
            Entry::Notice(t) if t.starts_with("Interrupted: ") => div()
                .id(SharedString::from(format!("interrupted-{i}")))
                .w_full()
                .min_w_0()
                .flex()
                .items_start()
                .gap_2()
                .pl_2()
                .py(px(2.))
                .border_l_2()
                .border_color(p.faint)
                .text_sm()
                .text_color(p.muted)
                .child(div().flex_none().text_color(p.text).child("Interrupted"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .child(SharedString::from(
                            t.trim_start_matches("Interrupted: ").to_owned(),
                        )),
                )
                .into_any_element(),
            Entry::Notice(t) => div()
                .text_sm()
                .text_color(p.muted)
                .child(SharedString::from(t.clone()))
                .into_any_element(),
        }
    }

    /// Opens a new chat that starts with this chat's turns up to entry `i`.
    fn branch_from(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(from) = self.current else {
            return;
        };
        // A running chat goes on in the background (#71); the copy opens
        // beside it and answers on its own.
        self.park_running(cx);
        let turns = self.model.turns_through(i);
        self.tabs.add();
        self.session = None;
        self.current = None;
        self.open_sources.clear();
        self.model.new_chat();
        self.model.phase = Phase::Connecting;
        self.model.screen = Screen::Chat;
        self.bridge.branch(from, turns);
        cx.notify();
    }

    /// The quote the next message replies to, over the composer.
    fn reply_bar(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let q = self.model.reply_to.clone()?;
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_lg()
                .bg(p.canvas)
                .child(
                    div()
                        .text_color(p.muted)
                        .child(gpui_kit::component::Icon::new(AssetIcon::Reply).xsmall()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pl_2()
                        .border_l_2()
                        .border_color(p.border)
                        .text_xs()
                        .text_color(p.muted)
                        .truncate()
                        .child(SharedString::from(q)),
                )
                .child(
                    Button::new("cancel-reply")
                        .capsule()
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .tooltip("Cancel reply")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.reply_to = None;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    fn empty_state(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let offline = matches!(self.model.phase, Phase::Offline(_));
        let (headline, detail): (&str, String) = match &self.model.phase {
            Phase::Connecting => ("Connecting…", "Opening your default model.".into()),
            Phase::Offline(reason) if reason.starts_with("No provider has a key yet") => (
                "Tern needs a model first",
                "Add a key for any provider, or point Tern at Ollama or LM Studio on this computer. Groq and Google AI Studio have free keys."
                    .into(),
            ),
            Phase::Offline(reason) => ("Tern could not open a model", reason.clone()),
            _ if self.model.harness == tern_agent::Harness::Study => (
                "Start a study session",
                "Learn from your notes: Tern explains, quizzes and tests you, kept apart from Research."
                    .into(),
            ),
            _ => (
                "Start a research session",
                match self.model.connection.as_ref().and_then(|c| c.search.as_deref()) {
                    Some(s) => format!(
                        "Ask a question. Tern plans, searches with {s}, reads, and cites every claim."
                    ),
                    None => "Ask a question. Web search is off, so answers come from the model alone."
                        .into(),
                },
            ),
        };
        let ready = !offline && !matches!(self.model.phase, Phase::Connecting);
        let study = self.model.harness == tern_agent::Harness::Study;
        let chips: Vec<AnyElement> = if ready {
            crate::mark::suggestions(study)
                .iter()
                .enumerate()
                .map(|(i, &(label, prompt))| {
                    let id = SharedString::from(format!("suggest-{i}"));
                    let chip = div()
                        .id(id.clone())
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .border_1()
                        .border_color(p.border)
                        .text_sm()
                        .text_color(p.muted)
                        .cursor_pointer()
                        .hover(|d| d.bg(p.hover).text_color(p.text))
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.composer.update(cx, |c, cx| {
                                crate::view::set_end(c, prompt, window, cx);
                                c.focus(window, cx);
                            });
                            cx.notify();
                        }));
                    self.press(id, chip).into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .pt(px(110.))
            .child(self.empty_mark(p, cx))
            .child(
                div()
                    .pt_2()
                    .text_xl()
                    .font_weight(FontWeight::MEDIUM)
                    .child(headline),
            )
            .child(
                div()
                    .max_w(px(520.))
                    .text_sm()
                    .text_color(p.muted)
                    .text_center()
                    .child(detail),
            )
            .when(!chips.is_empty(), |d| {
                d.child(
                    div()
                        .pt_3()
                        .flex()
                        .flex_wrap()
                        .justify_center()
                        .gap_2()
                        .children(chips),
                )
            })
            .when(offline, |d| {
                d.child(
                    div().pt_2().child(
                        Button::new("open-providers")
                            .capsule()
                            .label("Set up a provider")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.open_providers(cx))),
                    ),
                )
            })
            .into_any_element()
    }

    /// The empty state's dithered Tern mark: its loop (wing beat, tail
    /// flick, body shiver) plays a few times when the empty state appears,
    /// on a harness switch or on hover, then rests on the still; reduced
    /// motion shows the still.
    fn empty_mark(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let m = &self.mark_anim;
        let dark = p.canvas.l < 0.5;
        let (path, index, frames) =
            m.frame(dark, self.mark_since, Instant::now(), self.reduced_motion);
        let (w, h) = m.display_size(m.frame_h);
        div()
            .id("empty-mark")
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered && !this.mark_anim.playing(this.mark_since, Instant::now()) {
                    this.mark_since = Instant::now();
                    cx.notify();
                }
            }))
            .w(px(w))
            .h(px(h))
            .flex_none()
            .overflow_hidden()
            .relative()
            .child(
                gpui_kit::img(SharedString::from(path))
                    .absolute()
                    .top_0()
                    .left(px(-(index as f32) * w))
                    .w(px(w * frames as f32))
                    .h(px(h))
                    .object_fit(gpui_kit::ObjectFit::Fill),
            )
            .into_any_element()
    }

    /// The chat. Each message is its own child of the scrolling area, so
    /// the timeline rail can find where a message is and scroll to it.
    fn transcript(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let wrap = |el: AnyElement| {
            div()
                .w_full()
                .max_w(px(COLUMN_MAX_W))
                .px_4()
                .flex_none()
                .child(el)
                .into_any_element()
        };
        let mut items: Vec<AnyElement> = Vec::new();
        // The chat's folder shows in the title bar (#75 removed the chip
        // that sat here, centred over the transcript).
        if self.model.entries.is_empty() && !self.model.is_busy() {
            items.push(wrap(self.empty_state(p, cx)));
        }
        self.rail_base.set(items.len());
        // Entries far outside the viewport are boxes of their last height
        // (`crate::cull`): a long chat lays out only what is near the
        // window on every frame, not every message it ever had.
        let chat = self
            .current
            .map(|id| id.to_string())
            .unwrap_or_else(|| "new".to_owned());
        for (i, e) in self.model.entries.iter().enumerate() {
            let key = format!("chat-{chat}-{i}");
            items.push(wrap(
                self.culled(key, &self.scroll, || self.entry(i, e, p, cx)),
            ));
        }
        // Thinking, the working rollup, and the answer so far, revealed at
        // reading pace.
        if let Some(live) = self.live_answer(p, cx) {
            items.push(wrap(live));
        }
        if let Some(card) = self.needs_you_card(p, cx) {
            items.push(wrap(card));
        }
        if let Some(card) = self.plan_card(p, cx) {
            items.push(wrap(card));
        }
        if let Some(line) = self.plan_folded_line(p) {
            items.push(wrap(line));
        }
        if let Some(line) = self.model.working_line() {
            items.push(wrap(
                working_line(&line, p, self.reduced_motion, "transcript").into_any_element(),
            ));
        }
        let scroller = div()
            .id("transcript")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .items_center()
            .gap_5()
            .pt_6()
            .pb_6()
            .children(items);
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(scroller)
            .children(self.timeline_rail(p, cx))
            .children(self.checkpoint_strip(p, cx))
            .vertical_scrollbar(&self.scroll)
    }

    /// Over the composer when a turn needs Act mode (#80): what it needs,
    /// and "Switch to Act and run", which sends the message again in Act.
    fn needs_act_card(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let n = self.model.needs_act.as_ref()?;
        if self.model.is_busy() {
            return None;
        }
        Some(
            div()
                .id("needs-act-card")
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_sm()
                                .text_color(p.text)
                                .child(SharedString::from(n.line())),
                        )
                        .child(div().text_xs().text_color(p.faint).child(
                            "Chat mode reads the folder; Act may change it, and each change still asks you first.",
                        )),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            Button::new("needs-act-dismiss")
                                .capsule()
                                .label("Dismiss")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.needs_act = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("needs-act-run")
                                .capsule()
                                .label("Switch to Act and run")
                                .primary()
                                .small()
                                .disabled(!self.model.can_send())
                                .tooltip("Switch this chat and its folder to Act, then send the message again")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.switch_to_act_and_run(window, cx)
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Over the composer after an answer stopped at the length limit.
    fn continue_bar(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.model.continue_offer || self.model.is_busy() {
            return None;
        }
        Some(
            div()
                .id("continue-bar")
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .child(
                    div()
                        .min_w_0()
                        .text_sm()
                        .text_color(p.text)
                        .child("The answer was cut off at the length limit."),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            Button::new("continue-dismiss")
                                .capsule()
                                .label("Dismiss")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.continue_offer = false;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("continue-answer")
                                .capsule()
                                .label("Continue")
                                .primary()
                                .small()
                                .disabled(!self.model.can_send())
                                .tooltip("Ask for the rest, picking up where it stopped")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.continue_answer(window, cx)
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Waiting attachments as chips: icon, name, detail, role, remove.
    fn attachment_chips(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let cards: Vec<AnyElement> = self
            .model
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| self.attachment_card(i, a, p, cx))
            .collect();
        let loading = (self.model.attaching > 0).then(|| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .text_xs()
                .text_color(p.muted)
                .child(Spinner::new().xsmall())
                .child(if let Some(doc) = &self.model.attaching_doc {
                    // Page x/y and % while a local model reads the pages
                    // (#71: Granite-Docling showed no progress).
                    match self
                        .model
                        .attaching_name
                        .as_deref()
                        .and_then(tern_agent::docs::progress)
                    {
                        Some((done, total)) if total > 0 => format!(
                            "Converting {doc} to Markdown… {}",
                            tern_agent::docs::progress_words(done, total)
                        ),
                        _ => format!("Converting {doc} to Markdown…"),
                    }
                } else if self.model.attaching == 1 {
                    "Reading 1 item".to_owned()
                } else {
                    format!("Reading {} items", self.model.attaching)
                })
        });
        // Audio files on their way to the speech-to-text model.
        let audio = self.model.audio_pending.iter().map(|job| {
            let (name, id) = (&job.name, job.id);
            let pc = job.ctl.progress();
            let doing = if pc > 0 {
                format!("transcribing… {pc}%")
            } else {
                "transcribing…".to_owned()
            };
            let detail = match job.secs {
                Some(s) => format!("Audio · {} · {doing}", tern_agent::media::clock(s)),
                None => format!("Audio · {doing}"),
            };
            div()
                .flex()
                .items_center()
                .gap_2()
                .pl_2()
                .pr_2()
                .py_1()
                .max_w(px(280.))
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .child(Spinner::new().xsmall())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(p.text)
                                .child(SharedString::from(name.clone())),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(detail)),
                        ),
                )
                .child(chip_remove(
                    ("audio-remove", id as usize),
                    "Stop transcribing and remove",
                    p,
                    cx.listener(move |this, _, _, cx| {
                        this.model.cancel_audio(id);
                        cx.notify();
                    }),
                ))
        });
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(cards)
            .children(audio)
            .children(loading)
            .children(self.cannot_see_note(p))
    }

    /// A preview of a file as a picture: a picture attachment's
    /// thumbnail, or the first page picture of a scanned PDF (#71).
    fn preview_image(&self, a: &tern_agent::Attachment) -> Option<std::sync::Arc<gpui_kit::Image>> {
        self.thumbnail(a).or_else(|| self.named_preview(&a.name))
    }

    /// The picture kept for file `name` (a scanned PDF's first page, a
    /// sent picture), decoded once.
    fn named_preview(&self, name: &str) -> Option<std::sync::Arc<gpui_kit::Image>> {
        let key = format!("preview:{name}");
        if let Some(t) = self.thumbs.borrow().get(&key) {
            return Some(t.clone());
        }
        let bytes = self.model.previews.get(name)?;
        let format = match crate::thumbs::sniff(bytes)? {
            crate::thumbs::Format::Png => gpui_kit::ImageFormat::Png,
            crate::thumbs::Format::Jpeg => gpui_kit::ImageFormat::Jpeg,
            crate::thumbs::Format::Gif => gpui_kit::ImageFormat::Gif,
            crate::thumbs::Format::Webp => gpui_kit::ImageFormat::Webp,
        };
        let image = std::sync::Arc::new(gpui_kit::Image::from_bytes(format, bytes.clone()));
        let mut cache = self.thumbs.borrow_mut();
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(key, image.clone());
        Some(image)
    }

    /// One waiting attachment as a preview card (#71): a thumbnail for a
    /// picture, the first page for a PDF or document (its picture when
    /// scanned, else its first lines), a kind tile otherwise; the file's
    /// name, pages and size; a coloured kind badge in the bottom corner;
    /// role, "Read as scanned", and remove.
    fn attachment_card(
        &self,
        i: usize,
        a: &tern_agent::Attachment,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::filetype::FileKind;
        let kind = match a.source {
            AttachSource::File if a.is_image() => FileKind::Image,
            AttachSource::Folder => FileKind::Folder,
            AttachSource::Archive => FileKind::Archive,
            AttachSource::Link => FileKind::Link,
            AttachSource::Audio(_) => FileKind::Audio,
            AttachSource::Paste => FileKind::Text,
            _ => FileKind::of(&a.name),
        };
        let prompt = a.role == Role::Prompt;
        let size = tern_agent::attach::origin(&a.name)
            .and_then(|path| std::fs::metadata(path).ok())
            .map(|m| crate::filetype::size_words(m.len()));
        // A notebook counts its cells, not the lines of its JSON.
        let base = match kind {
            FileKind::Notebook => a
                .files
                .first()
                .and_then(|f| crate::filetype::notebook_cells(&f.text))
                .map(|c| match c.len() {
                    1 => "1 cell".to_owned(),
                    n => format!("{n} cells"),
                })
                .unwrap_or_else(|| a.detail()),
            _ => a.detail(),
        };
        let detail = match size {
            Some(sz) => format!("{base} · {sz}"),
            None => base,
        };
        let preview = card_preview(
            self.preview_image(a),
            kind,
            &a.name,
            a.files.first().map(|f| f.text.as_str()),
            p,
            PREVIEW_CARD_W,
            84.,
        );
        let name = a.name.clone();
        div()
            .id(("attach-card", i))
            .flex()
            .flex_col()
            .w(px(PREVIEW_CARD_W))
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .border_color(if prompt { p.muted } else { p.border })
            .bg(p.canvas)
            .child(
                div()
                    .relative()
                    .child(preview)
                    .child(
                        div()
                            .absolute()
                            .top(px(3.))
                            .right(px(3.))
                            .rounded_md()
                            .bg(p.canvas)
                            .child(chip_remove(
                                ("attach-remove", i),
                                "Remove",
                                p,
                                cx.listener(move |this, _, _, cx| {
                                    this.model.remove_attachment(i);
                                    cx.notify();
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .px_2()
                    .py_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .child(kind_logo(kind, &a.name, 16., p))
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(p.text)
                                    .child(SharedString::from(a.name.clone())),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(detail)),
                    )
                    // Context and Read as scanned: the second goes under
                    // the first when the card is too narrow for both.
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id(("attach-role", i))
                                    .flex_none()
                                    .px_1()
                                    .rounded_sm()
                                    .text_xs()
                                    .cursor_pointer()
                                    .when(prompt, |d| d.bg(p.hover))
                                    .text_color(if prompt { p.text } else { p.faint })
                                    .hover(|d| d.text_color(p.text))
                                    .child(a.role.label())
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "Context is material to read. Prompt is your instructions. Click to switch.",
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.model.toggle_role(i);
                                        cx.notify();
                                    })),
                            )
                            .when(a.rereadable(), |d| {
                                d.child(
                                    div()
                                        .id(("attach-reread", i))
                                        .flex_none()
                                        .px_1()
                                        .rounded_sm()
                                        .text_xs()
                                        .cursor_pointer()
                                        .text_color(p.faint)
                                        .hover(|d| d.text_color(p.text).bg(p.hover))
                                        .child("Read as scanned")
                                        .tooltip(|window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                "Read again with Granite-Docling (or the local OCR model) as scanned pages, for a scan whose text layer is empty or only a watermark.",
                                            )
                                            .build(window, cx)
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(path) = this.model.read_again_started(&name) {
                                                this.bridge.read_again(name.clone(), path, this.current);
                                                this.tick_audio(cx);
                                            }
                                            cx.notify();
                                        })),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    /// Under waiting pictures, when the chat model cannot see: who will
    /// read them instead, decided for real when the message is sent.
    fn cannot_see_note(&self, p: Palette) -> Option<gpui_kit::Div> {
        if !self.model.attachments.iter().any(|a| a.is_image()) {
            return None;
        }
        let provider = self.provider_id();
        let model = self.model.connection.as_ref()?.model.clone();
        let text = crate::composer::cannot_see_note(&self.model.catalog, &provider, &model)?;
        Some(
            div()
                .w_full()
                .text_xs()
                .text_color(p.faint)
                .child(SharedString::from(text)),
        )
    }

    /// A picture attachment's thumbnail, decoded once and kept while it
    /// waits in the composer.
    fn thumbnail(&self, a: &tern_agent::Attachment) -> Option<std::sync::Arc<gpui_kit::Image>> {
        let img = a.images.first()?;
        let key = crate::thumbs::key(img);
        if let Some(t) = self.thumbs.borrow().get(&key) {
            return Some(t.clone());
        }
        let (fmt, bytes) = crate::thumbs::decode(img)?;
        let format = match fmt {
            crate::thumbs::Format::Png => gpui_kit::ImageFormat::Png,
            crate::thumbs::Format::Jpeg => gpui_kit::ImageFormat::Jpeg,
            crate::thumbs::Format::Gif => gpui_kit::ImageFormat::Gif,
            crate::thumbs::Format::Webp => gpui_kit::ImageFormat::Webp,
        };
        let image = std::sync::Arc::new(gpui_kit::Image::from_bytes(format, bytes));
        let mut cache = self.thumbs.borrow_mut();
        // Only pictures still waiting stay cached.
        if cache.len() > 32 {
            cache.clear();
        }
        cache.insert(key, image.clone());
        Some(image)
    }

    /// Voice input stays locked until a transcription model is set up.
    /// The mic: on when a speech-to-text model is listed on any configured
    /// provider. Click to record, click again (or Esc) to stop; the speech
    /// goes to that model and the transcript lands in the composer.
    fn mic_button(&self, _p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let via = self.stt_model();
        let recording = self.recording.is_some();
        let finishing = !recording && self.dictation.is_some();
        let (icon, tip, enabled): (AssetIcon, SharedString, bool) = match (&via, recording) {
            (_, true) => (
                AssetIcon::CircleStop,
                "Listening. Click to stop, Esc to throw it away".into(),
                true,
            ),
            (_, false) if finishing => (AssetIcon::Mic, "Finishing the transcript…".into(), false),
            (None, false) => (
                AssetIcon::MicOff,
                "Voice input needs a speech-to-text model: a local one (Settings › Local models) or a provider one (Groq or OpenAI Whisper, Mistral Voxtral)".into(),
                true,
            ),
            (Some(v), false) if !self.model.audio_pending.is_empty() => (
                AssetIcon::Mic,
                format!("Transcribing with {v}…").into(),
                false,
            ),
            (Some(v), false) => (
                AssetIcon::Mic,
                format!("Speak instead of typing (transcribed by {v} as you talk)").into(),
                true,
            ),
        };
        Button::new("mic")
            .capsule()
            .icon(icon)
            .ghost()
            .small()
            .selected(recording)
            .disabled(!enabled)
            .tooltip(tip)
            .on_click(cx.listener(|this, _, window, cx| this.toggle_mic(window, cx)))
    }

    /// The composer's context ring: fills as the window does, turns amber
    /// past 70% and red past 90%. Hover for exact numbers.
    fn context_ring(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let fraction = self.model.context_fraction();
        let dark = self.model.dark;
        let level = self.model.context_level();
        let full_color = crate::gauge::fullness(level, dark);
        let label = self.model.context_label();
        let b = self.model.context_breakdown();
        let reduced = self.reduced_motion;
        // The composer's ring: a mini version of the popover's donut, its
        // used arcs in the fullness colour.
        let mini: Vec<(f32, f32, Hsla)> = crate::gauge::arcs(
            &b.rows
                .iter()
                .filter(|r| r.label != "Free")
                .map(|r| r.share)
                .collect::<Vec<_>>(),
            12.,
        )
        .into_iter()
        .map(|(s, w)| (s, w, full_color))
        .collect();
        let trigger = div()
            .id("context-ring")
            .flex()
            .items_center()
            .gap_1()
            .px_1()
            .rounded_md()
            .text_xs()
            .text_color(p.faint)
            .hover(|d| d.bg(p.hover).text_color(p.text))
            .child(donut(14., 2.5, mini, p.border))
            .child(match fraction {
                Some(f) => format!("{:.0}%", f * 100.),
                None => label.clone(),
            });
        // "New chat with summary": a handoff packet from this chat starts a
        // fresh one (tern_agent::handoff), only once the chat is saved.
        let chat = self
            .session
            .as_ref()
            .filter(|s| !s.is_draft())
            .map(|s| s.session_id());
        let compacting = self.model.compacting;
        let busy = self.model.is_busy();
        let me = cx.entity().downgrade();
        let opened = self.ctx_opened.clone();
        let opened_set = self.ctx_opened.clone();
        let accent = p.accent;
        let ws = cx.entity_id();
        HoverCard::new("context-card")
            .anchor(gpui_kit::Anchor::BottomRight)
            .open_delay(Duration::from_millis(150))
            .trigger(trigger)
            .on_open_change(move |open, _, _| {
                opened_set.set(open.then(Instant::now));
            })
            .content(move |_, window, _| {
                let t = opened
                    .get()
                    .map_or(crate::gauge::SWEEP, |at| at.elapsed());
                if !reduced && t < crate::gauge::SWEEP + Duration::from_millis(30) {
                    frame::next_frame_for(window, ws);
                }
                let used: Vec<&crate::model::ContextRow> =
                    b.rows.iter().filter(|r| r.label != "Free").collect();
                let colors: Vec<Hsla> = used
                    .iter()
                    .map(|r| {
                        crate::gauge::Part::of_label(r.label)
                            .map_or(p.muted, |part| part.color(dark, accent))
                    })
                    .collect();
                let shares: Vec<f32> = used.iter().map(|r| r.share).collect();
                let n = shares.iter().filter(|s| **s > 0.).count();
                let mut k = 0;
                let arcs: Vec<(f32, f32, Hsla)> =
                    crate::gauge::arcs(&shares, crate::gauge::GAP_DEG)
                        .into_iter()
                        .zip(colors.iter())
                        .map(|((start, sweep), &c)| {
                            if sweep <= 0. {
                                return (start, 0., c);
                            }
                            let grow = crate::gauge::sweep_in(k, n, t, reduced);
                            k += 1;
                            (start, sweep * grow, c)
                        })
                        .collect();
                let pct = fraction.map(|f| crate::gauge::count_up(f * 100., t, reduced));
                let gauge = div()
                    .relative()
                    .size(px(crate::gauge::SIZE))
                    .flex_none()
                    .child(donut(
                        crate::gauge::SIZE,
                        crate::gauge::RING,
                        arcs,
                        p.border,
                    ))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(full_color)
                                    .child(match pct {
                                        Some(v) => format!("{v:.0}%"),
                                        None => "–".into(),
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(p.muted)
                                    .child(SharedString::from(b.header.clone())),
                            ),
                    );
                let rows = div().flex_1().min_w_0().flex().flex_col().gap_1().children(
                    b.rows.iter().map(|r| {
                        let free = r.label == "Free";
                        let dot = crate::gauge::Part::of_label(r.label)
                            .map(|part| part.color(dark, accent));
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(7.))
                                    .rounded_full()
                                    .flex_none()
                                    .bg(match dot {
                                        Some(c) => c,
                                        None if free => p.border,
                                        None => p.muted,
                                    }),
                            )
                            .child(div().flex_1().min_w_0().truncate().text_color(p.muted).child(r.label))
                            .child(
                                div()
                                    .text_color(p.text)
                                    .child(SharedString::from(crate::model::grouped(r.tokens))),
                            )
                    }),
                );
                div()
                    .w(px(320.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .text_xs()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text)
                            .child("Context"),
                    )
                    .child(div().flex().items_center().gap_3().child(gauge).child(rows))
                    .child(
                        div()
                            .text_color(p.faint)
                            .child(SharedString::from(b.note.clone())),
                    )
                    .children(chat.map(|id| {
                        let me = me.clone();
                        let me2 = me.clone();
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .child(
                                Button::new("context-compact").capsule()
                                    .label(if compacting { "Compacting…" } else { "Compact now" })
                                    .ghost()
                                    .xsmall()
                                    .disabled(compacting || busy)
                                    .tooltip("Fold older turns into a summary; the last turns stay word for word")
                                    .on_click(move |_, _, cx| {
                                        let _ = me2.update(cx, |this, cx| this.compact_now(cx));
                                    }),
                            )
                            .child(
                                Button::new("context-handoff").capsule()
                                    .label("New chat with summary")
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Write a summary of this chat and continue in a fresh one")
                                    .on_click(move |_, _, cx| {
                                        let _ = me.update(cx, |this, cx| this.hand_off(id, cx));
                                    }),
                            )
                    }))
                    .with_animation(
                        "context-card-in",
                        Animation::new(if reduced { Duration::ZERO } else { motion::UI })
                            .with_easing(motion::ease),
                        |d, t| d.opacity(t).mt(px((1. - t) * 6.)),
                    )
            })
    }

    /// "Compact now" in the context popover.
    pub(super) fn compact_now(&mut self, cx: &mut Context<Self>) {
        if self.model.is_busy() || self.model.compacting {
            return;
        }
        let Some(session) = self.session.take() else {
            return;
        };
        if session.is_draft() {
            self.session = Some(session);
            return;
        }
        self.model.compacting = true;
        self.bridge.compact(session);
        cx.notify();
    }

    /// The inline line after a compaction: "Compacted earlier messages ·
    /// View summary", opening the summary under it.
    fn compact_notice(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.model.compact_open;
        let summary = self
            .model
            .compacted
            .as_ref()
            .map(|c| (c.summary.clone(), c.turns.len()));
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .py_1()
            .text_xs()
            .text_color(p.faint)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().h(px(1.)).bg(p.border))
                    .child(crate::model::COMPACT_NOTICE)
                    .child("·")
                    .child(
                        div()
                            .id("compact-view")
                            .cursor_pointer()
                            .text_color(p.muted)
                            .hover(|d| d.text_color(p.text).underline())
                            .child(if open { "Hide summary" } else { "View summary" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.compact_open = !this.model.compact_open;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1().h(px(1.)).bg(p.border)),
            )
            .when_some(summary.filter(|_| open), |d, (text, n)| {
                d.child(
                    div()
                        .p_3()
                        .rounded_lg()
                        .bg(p.raised)
                        .text_sm()
                        .text_color(p.muted)
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_xs().text_color(p.faint).child(format!(
                            "Earlier in this chat · {n} turns folded; they stay in the transcript"
                        )))
                        .child(SharedString::from(text)),
                )
            })
            .into_any_element()
    }

    fn section(
        &self,
        kind: Kind,
        title: &'static str,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows: Vec<AnyElement> = self
            .model
            .providers
            .section(kind)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .map(|r| {
                let key = format!("{:?}-{}", r.kind, r.id);
                let handle = self.settings_page_scroll();
                self.culled(key, &handle, || self.service_row(r, p, cx))
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_xs().text_color(p.faint).child(title))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded_xl()
                    .border_1()
                    .border_color(p.border)
                    .children(rows),
            )
    }

    /// A row of a long scrolling page: built by `build` when it is near
    /// the viewport of `handle`'s scroll area, else an empty box of its
    /// last height (`crate::cull`). Either way it records where it sits.
    pub(super) fn culled(
        &self,
        key: String,
        handle: &ScrollHandle,
        build: impl FnOnce() -> AnyElement,
    ) -> AnyElement {
        let view = handle.bounds();
        let off = f32::from(handle.offset().y);
        let boxed = self.cull_rows.borrow().hidden(
            &key,
            off,
            f32::from(view.origin.y),
            f32::from(view.size.height),
        );
        let inner = match boxed {
            Some(h) => div().w_full().h(px(h)).into_any_element(),
            None => build(),
        };
        let rows = self.cull_rows.clone();
        let handle = handle.clone();
        div()
            .relative()
            .w_full()
            .child(inner)
            .child(
                gpui_kit::canvas(
                    move |b, _, _| {
                        let off = f32::from(handle.offset().y);
                        rows.borrow_mut().set(
                            &key,
                            crate::cull::Spot::at(
                                f32::from(b.origin.y),
                                f32::from(b.size.height),
                                off,
                            ),
                        );
                    },
                    |_, _, _, _| (),
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    fn service_row(&self, r: ServiceRow, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.providers;
        let busy_turn = self.model.is_busy();
        let working = st.is_working(r.kind, r.id);
        let is_default = st.is_default(&r);
        let editing = st.is_editing(r.kind, r.id);
        let (kind, id) = (r.kind, r.id);
        let key_url = r.key_url;
        let bid = |what: &'static str| SharedString::from(format!("{}-{:?}-{}", what, kind, id));

        let note = st.note(kind, id).cloned();
        let mut info = div()
            .flex()
            .flex_col()
            .min_w_0()
            .flex_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(r.name),
                    )
                    .when(is_default, |d| {
                        d.child(
                            div()
                                .px_1p5()
                                .rounded_md()
                                .border_1()
                                .border_color(p.border)
                                .text_xs()
                                .text_color(p.muted)
                                .child("Default"),
                        )
                    }),
            )
            .child(div().text_xs().text_color(p.faint).child(r.status()));
        if let Some(n) = note {
            let text = match n {
                Note::Working(t) | Note::Ok(t) => t,
                Note::Failed(t) => format!("Failed: {t}"),
            };
            info = info.child(
                div()
                    .text_xs()
                    .text_color(p.muted)
                    .child(SharedString::from(text)),
            );
        }

        let mut actions = div().flex().items_center().gap_1().flex_shrink_0();
        let tip = |t: &'static str| Some(SharedString::from(t));
        if working {
            actions = actions.child(Spinner::new().small().color(p.muted));
        } else if r.needs_key() && !r.key_saved {
            if !editing {
                actions = actions.child(pill(
                    bid("add"),
                    "Add",
                    PillLook::Solid,
                    false,
                    None,
                    p,
                    cx.listener(move |this, _, window, cx| this.edit_key(kind, id, window, cx)),
                ));
            }
        } else {
            let can_choose = st.can_choose(&r);
            if !is_default {
                actions = actions.child(pill(
                    bid("use"),
                    "Use",
                    PillLook::Solid,
                    busy_turn || !can_choose,
                    tip(if can_choose {
                        "Use this one for new answers in this chat"
                    } else {
                        "Test the server address first"
                    }),
                    p,
                    cx.listener(move |this, _, _, cx| {
                        this.apply_choice(kind, id);
                        cx.notify();
                    }),
                ));
            }
            actions = actions.child(pill(
                bid("test"),
                "Test",
                PillLook::Ghost,
                false,
                None,
                p,
                cx.listener(move |this, _, _, cx| {
                    this.model.providers.begin(kind, id, "Checking…");
                    if kind == Kind::Search && id == "searxng" {
                        let typed = this.searx_input.read(cx).value().to_string();
                        let url = this.model.providers.set_search_url(&typed);
                        this.model.providers.begin(kind, id, "Checking…");
                        this.bridge.save_prefs(this.model.providers.prefs.clone());
                        this.bridge.test_search_at(id, url);
                    } else {
                        this.bridge.test(kind, id);
                    }
                    cx.notify();
                }),
            ));
            if kind == Kind::Model {
                actions = actions.child(pill(
                    bid("address"),
                    "Address",
                    PillLook::Ghost,
                    false,
                    tip("The server address, for a local server elsewhere or a proxy"),
                    p,
                    cx.listener(move |this, _, window, cx| this.edit_base_url(id, window, cx)),
                ));
            }
            if kind == Kind::Model && tern_agent::providers::custom::is_custom(id) {
                // Asks twice; the armed button is this provider's own.
                let confirm = self.model.settings.confirm == Some(id);
                actions = actions.child(pill(
                    bid("delete"),
                    if confirm {
                        "Click again to delete"
                    } else {
                        "Delete"
                    },
                    PillLook::Ghost,
                    busy_turn && is_default,
                    tip("Delete this custom provider and its key"),
                    p,
                    cx.listener(move |this, _, _, cx| {
                        if this.model.settings.confirm(id) {
                            this.model.providers.begin(kind, id, "Deleting…");
                            this.bridge.custom_remove(id.to_owned());
                        }
                        cx.notify();
                    }),
                ));
            } else if r.key_saved {
                actions = actions.child(pill(
                    bid("remove"),
                    "Remove",
                    PillLook::Ghost,
                    busy_turn && is_default,
                    None,
                    p,
                    cx.listener(move |this, _, _, cx| {
                        this.model.providers.begin(kind, id, "Removing…");
                        this.bridge.remove(kind, id);
                        cx.notify();
                    }),
                ));
            }
        }

        if self.perf_strip & 4 != 0 {
            actions = div();
        }
        let mut row = div()
            .flex()
            .flex_col()
            .gap_2()
            .px_3()
            .py_2p5()
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(self.perf_strip & 1 == 0, |d| d.child(logo_tile(&r, p)))
                    .child(info)
                    .child(actions),
            );

        if kind == Kind::Search && id == "searxng" {
            row = row.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pl(px(40.))
                    .child(div().text_xs().text_color(p.faint).child("Server address"))
                    .child(Input::new(&self.searx_input))
                    .child(div().text_xs().text_color(p.faint).child(
                        "Tern asks for JSON results, so its settings.yml needs `formats: [html, json]` under `search:`.",
                    )),
            );
        }
        if editing {
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl(px(40.))
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.key_input).mask_toggle()),
                    )
                    .child(
                        Button::new(bid("save"))
                            .capsule()
                            .label("Save")
                            .small()
                            .disabled(working)
                            .on_click(cx.listener(|this, _, window, cx| this.save_key(window, cx))),
                    )
                    .when(!key_url.is_empty(), |d| {
                        d.child(
                            Button::new(bid("get"))
                                .capsule()
                                .label("Get a key")
                                .ghost()
                                .small()
                                .on_click(move |_, _, cx| cx.open_url(key_url)),
                        )
                    })
                    .child(
                        Button::new(bid("cancel"))
                            .capsule()
                            .label("Cancel")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.providers.cancel_edit();
                                cx.notify();
                            })),
                    ),
            );
        }

        if kind == Kind::Model && self.model.settings.editing_url == Some(id) {
            row = row.child(self.base_url_row(id, p, cx));
        }
        if kind == Kind::Model && is_default && self.perf_strip & 2 == 0 {
            row = row.child(self.provider_model_list(p, cx));
        }
        row.into_any_element()
    }

    /// The run panel's Auto-open switch (and Settings › Appearance),
    /// remembered in preferences.
    pub(crate) fn set_run_panel_auto(&mut self, on: bool) {
        self.model.run_panel_auto = on;
        self.save_ui_prefs(|p| p.run_panel_auto = Some(on));
    }

    /// The run panel: one run in three views (Summary, Detailed, Raw).
    fn run_panel(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let run = self.model.panel_run();
        let view = self.model.run_panel.view;
        // The rail's tabs (v2 s05) as a sliding segmented control sized to
        // the pane: what does not fit goes in a ⋯ menu, so nothing runs
        // past the edge. Plan and Sources hold several views each; a
        // second, smaller switch picks between them.
        let tab = RailTab::of(view);
        let titles: Vec<String> = RailTab::ALL.iter().map(|t| t.title(run)).collect();
        let labels: Vec<&str> = titles.iter().map(String::as_str).collect();
        let active = RailTab::ALL.iter().position(|&t| t == tab).unwrap_or(0);
        let (show_label, shown, hidden) = crate::panels::run_header_fit(
            &labels,
            self.side_w - crate::panels::AUTO_OPEN_W,
            active,
        );
        let segs: Vec<segmented::Segment> = shown
            .iter()
            .map(|&i| segmented::Segment::text(titles[i].clone()))
            .collect();
        let picked = shown.iter().position(|&i| i == active).unwrap_or(0);
        let shown_tabs: Vec<RailTab> = shown.iter().map(|&i| RailTab::ALL[i]).collect();
        let first_views: Vec<RunView> = RailTab::ALL
            .iter()
            .map(|t| t.views(run).first().copied().unwrap_or(RunView::Summary))
            .collect();
        let shown_first: Vec<RunView> = shown.iter().map(|&i| first_views[i]).collect();
        let tabs = self.segmented(
            "run-views",
            &segs,
            picked,
            segmented::Look {
                underline: true,
                ..Default::default()
            },
            p,
            std::rc::Rc::new(move |this: &mut Self, i, _, _| {
                if let (Some(&t), Some(&v)) = (shown_tabs.get(i), shown_first.get(i)) {
                    if RailTab::of(this.model.run_panel.view) != t {
                        this.model.set_run_view(v);
                    }
                    this.run_more_open = false;
                }
            }),
            cx,
        );
        let subs = tab.views(run);
        let sub_switch = (subs.len() > 1).then(|| {
            let sub_segs: Vec<segmented::Segment> = subs
                .iter()
                .map(|v| segmented::Segment::text(v.label()))
                .collect();
            let sub_at = subs.iter().position(|&v| v == view).unwrap_or(0);
            let sub_views = subs.clone();
            self.segmented(
                "run-subviews",
                &sub_segs,
                sub_at,
                segmented::Look {
                    small: true,
                    ..Default::default()
                },
                p,
                std::rc::Rc::new(move |this: &mut Self, i, _, _| {
                    if let Some(&v) = sub_views.get(i) {
                        this.model.set_run_view(v);
                    }
                }),
                cx,
            )
        });
        let hidden_first: Vec<(RailTab, RunView)> = hidden
            .iter()
            .map(|&i| (RailTab::ALL[i], first_views[i]))
            .collect();
        let more = (!hidden.is_empty()).then(|| {
            div()
                .relative()
                .flex_none()
                .child(
                    Button::new("run-views-more")
                        .capsule()
                        .icon(IconName::Ellipsis)
                        .ghost()
                        .xsmall()
                        .selected(self.run_more_open)
                        .tooltip("More views")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run_more_open = !this.run_more_open;
                            cx.notify();
                        })),
                )
                .when(self.run_more_open, |d| {
                    d.child(
                        gpui_kit::deferred(
                            div()
                                .id("run-views-menu")
                                .absolute()
                                .top(px(26.))
                                .right_0()
                                .w(px(140.))
                                .p_1()
                                .flex()
                                .flex_col()
                                .rounded_lg()
                                .border_1()
                                .border_color(p.border)
                                .bg(p.canvas)
                                .shadow_lg()
                                .occlude()
                                .children(hidden_first.into_iter().map(|(t, v)| {
                                    div()
                                        .id(SharedString::from(format!(
                                            "run-view-more-{}",
                                            t.label()
                                        )))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .cursor_pointer()
                                        .text_color(p.muted)
                                        .hover(|d| d.bg(p.hover).text_color(p.text))
                                        .child(t.label())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.model.set_run_view(v);
                                            this.run_more_open = false;
                                            cx.notify();
                                        }))
                                }))
                                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                    this.run_more_open = false;
                                    cx.notify();
                                })),
                        )
                        .with_priority(1),
                    )
                })
        });
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_1()
            .min_w_0()
            .when(show_label, |d| {
                d.child(div().flex_none().text_xs().text_color(p.faint).child("Run"))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .child(tabs)
                    .children(more)
                    .child(
                        Button::new("run-auto-open")
                            .capsule()
                            .label("Auto-open")
                            .ghost()
                            .xsmall()
                            .selected(self.model.run_panel_auto)
                            .tooltip(if self.model.run_panel_auto {
                                "Opens by itself for deep research. Click to keep it closed until you open it."
                            } else {
                                "Stays closed until you open it. Click to open it by itself for deep research."
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_run_panel_auto(!this.model.run_panel_auto);
                                cx.notify();
                            })),
                    ),
            );
        let status: Option<AnyElement> = if let Some(line) = self.model.working_line() {
            Some(working_line(&line, p, self.reduced_motion, "panel").into_any_element())
        } else {
            run.end.as_ref().map(|end| {
                let took = run.summary().took.as_secs_f32();
                let (icon, text) = match end {
                    RunEnd::Answered => (AssetIcon::CircleCheck, format!("Answered in {took:.1}s")),
                    RunEnd::Stopped(why) => (AssetIcon::CircleStop, why.clone()),
                };
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .text_sm()
                    .text_color(p.muted)
                    .child(
                        div()
                            .pt(px(2.))
                            .child(gpui_kit::component::Icon::new(icon).small()),
                    )
                    .child(div().flex_1().min_w_0().child(text))
                    .into_any_element()
            })
        };
        let pinned = self.model.panel_is_pinned().then(|| {
            div()
                .id("run-latest")
                .flex()
                .items_center()
                .justify_between()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(p.raised)
                .text_xs()
                .text_color(p.muted)
                .child("An earlier answer")
                .child(
                    div()
                        .cursor_pointer()
                        .text_color(p.text)
                        .hover(|d| d.underline())
                        .child("Show latest"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model.run_panel.pinned = None;
                    this.model.run_panel.open.clear();
                    cx.notify();
                }))
        });
        let body: AnyElement = if view == RunView::Context {
            self.run_context(p)
        } else if view == RunView::Files {
            self.files_panel(p, cx)
        } else if view == RunView::Terminal {
            self.terminal_panel(p, cx)
        } else if run.is_empty() {
            div()
                .text_sm()
                .text_color(p.faint)
                .child("Searches, pages read, tool calls, and approvals show here.")
                .into_any_element()
        } else {
            match view {
                RunView::Summary => self.run_summary(run, p, cx),
                RunView::Detailed => self.run_detailed(run, p),
                RunView::Raw => self.run_raw(run, p, cx),
                RunView::Report => self.run_report(run, p, cx),
                RunView::Workers => self.run_workers(run, p, cx),
                RunView::Ledger => self.run_ledger(run, p, cx),
                RunView::Decisions => self.run_decisions(run, p),
                RunView::Graph => self.run_graph(run, p, cx),
                RunView::Context => self.run_context(p),
                RunView::Files => self.files_panel(p, cx),
                RunView::Tools => self.run_tools(run, p, cx),
                RunView::Terminal => self.terminal_panel(p, cx),
            }
        };
        div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .bg(p.sidebar)
            .border_l_1()
            .border_color(p.border)
            .child(header)
            .children(sub_switch)
            .children(pinned)
            .children(status)
            .child(if view == RunView::Files {
                // The Files tab scrolls itself (its tree is culled).
                div().flex_1().min_h_0().child(body).into_any_element()
            } else {
                div()
                    .id("run-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(body)
                    .into_any_element()
            })
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(self.model.context_label()),
            )
    }

    /// The Context tab: what fills the window, one bar per kind of
    /// content, with how it was counted.
    fn run_context(&self, p: Palette) -> AnyElement {
        let b = self.model.context_breakdown();
        let fill = crate::gauge::fullness(b.level, self.model.dark);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(div().text_sm().text_color(p.text).child(b.header.clone()))
                    .child(div().text_xs().text_color(p.muted).child(b.percent.clone())),
            )
            .children(b.rows.iter().map(|r| {
                let free = r.label == "Free";
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .text_color(p.muted)
                            .child(r.label)
                            .child(crate::model::compact(r.tokens)),
                    )
                    .child(
                        div().h(px(4.)).w_full().rounded_full().bg(p.border).child(
                            div()
                                .h_full()
                                .rounded_full()
                                .w(gpui_kit::relative(r.share.clamp(0., 1.)))
                                .bg(if free { p.faint } else { fill }),
                        ),
                    )
            }))
            .child(
                div()
                    .pt_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child(b.note.clone()),
            )
            .into_any_element()
    }

    /// The rollup line, the pages read, and anything that went wrong.
    fn run_summary(&self, run: &Run, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let line = run.summary().line();
        let pages = run.pages();
        let section = |title: &'static str| div().pt_2().text_xs().text_color(p.faint).child(title);
        let page_rows = pages.iter().enumerate().map(|(k, pg)| {
            let url = pg.url.clone();
            div()
                .id(SharedString::from(format!("run-page-{k}")))
                .flex()
                .items_center()
                .gap_2()
                .px_1()
                .py(px(4.))
                .rounded_md()
                .cursor_pointer()
                .hover(|d| d.bg(p.hover))
                .child(self.site_icon(pg, 14., p))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .child(SharedString::from(pg.label().to_owned())),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.browse(&url, window, cx)))
        });
        let problems: Vec<_> = run
            .problems()
            .filter(|s| s.event != "turn.finished")
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .when(!line.is_empty(), |d| {
                d.child(div().text_sm().text_color(p.text).child(line))
            })
            .when(!pages.is_empty(), |d| {
                d.child(section("Pages read")).children(page_rows)
            })
            .children(self.run_extras(run, p))
            .when(!problems.is_empty(), |d| {
                d.child(section("Problems"))
                    .children(problems.into_iter().map(|s| step_line(s, p, false)))
            })
            .into_any_element()
    }

    /// The run report: the verdict tally, each claim with its badge,
    /// evidence, and source, and the saved files.
    fn run_report(&self, run: &Run, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        if run.checks.is_empty() && run.report.is_none() {
            let why = if run.end.is_some() {
                "This answer was not checked. Answers are checked when they finish."
            } else {
                "Claims are checked against their sources when the answer is done."
            };
            return div()
                .text_sm()
                .text_color(p.faint)
                .child(why)
                .into_any_element();
        }
        let section = |title: &'static str| div().pt_2().text_xs().text_color(p.faint).child(title);
        let tally = run.tally();
        let counts = div().flex().flex_wrap().gap_3().children(
            Verdict::ALL.iter().filter(|v| tally.get(**v) > 0).map(|v| {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(p.muted)
                    .child(verdict_badge(*v, p))
                    .child(SharedString::from(format!(
                        "{} {}",
                        tally.get(*v),
                        v.label().to_lowercase()
                    )))
            }),
        );
        let review = self.review_bar(run, p, cx);
        let export = run.report.is_some().then(|| self.export_buttons(cx));
        let pages = run.pages();
        let focus = self.model.run_panel.claim;
        let claims = run.checks.iter().map(|c| {
            let focused = focus == Some(c.claim);
            let rejected = self.claim_rejected(run, c.claim);
            let actions = self.claim_actions(run, c.claim, p, cx);
            let source = c.evidence.as_ref().map(|ev| {
                let page = pages
                    .iter()
                    .find(|pg| pg.url == ev.url)
                    .cloned()
                    .unwrap_or_else(|| PageRef {
                        title: ev.title.clone(),
                        url: ev.url.clone(),
                        site: ev.site.clone(),
                        builtin: None,
                        favicon: None,
                    });
                let url = ev.url.clone();
                let where_ = match ev.page {
                    Some(n) => format!("[{}] {} · page {n}", ev.source, page.label()),
                    None => format!("[{}] {}", ev.source, page.label()),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .pl_2()
                            .border_l_2()
                            .border_color(p.border)
                            .text_xs()
                            .text_color(p.muted)
                            .child(SharedString::from(format!("“{}”", ev.quote))),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("claim-src-{}", c.claim)))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_1()
                            .py(px(2.))
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|d| d.bg(p.hover))
                            .child(self.site_icon(&page, 14., p))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(p.muted)
                                    .child(SharedString::from(where_)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.browse(&url, window, cx)
                            })),
                    )
            });
            let evidence = run
                .evidence
                .as_deref()
                .and_then(|chain| chain.claim(c.claim).map(|ev| (chain, ev)))
                .map(|(chain, ev)| self.claim_evidence(chain, ev, &pages, p, cx));
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(if focused { p.muted } else { p.border })
                .when(focused, |d| d.bg(p.raised))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .text_color(p.muted)
                        .child(verdict_badge(c.verdict, p))
                        .child(c.verdict.label())
                        .child(div().text_color(p.faint).child(SharedString::from(format!(
                            "Claim {}{}",
                            c.claim,
                            if c.method == tern_agent::verify::Method::Model {
                                " · judged by the model"
                            } else {
                                ""
                            }
                        )))),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(if rejected { p.faint } else { p.text })
                        .when(rejected, |d| d.line_through())
                        .child(SharedString::from(c.text.clone())),
                )
                .children(source)
                .children(evidence)
                .children(actions)
                .when(!c.note.is_empty(), |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(c.note.clone())),
                    )
                })
        });
        let files = run.report.clone().map(|f| {
            // A bare path is not a link on Linux or macOS.
            let md = tern_agent::platform::file_url(&f.markdown.to_string_lossy());
            let json = tern_agent::platform::file_url(&f.json.to_string_lossy());
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(section("Saved report"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new("report-open-md")
                                .capsule()
                                .label("Open report")
                                .ghost()
                                .xsmall()
                                .tooltip("Open the Markdown report")
                                .on_click(move |_, _, cx| cx.open_url(&md)),
                        )
                        .child(
                            Button::new("report-open-json")
                                .capsule()
                                .label("JSON")
                                .ghost()
                                .xsmall()
                                .tooltip("Open the report as JSON")
                                .on_click(move |_, _, cx| cx.open_url(&json)),
                        ),
                )
                .children(export)
                .child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .truncate()
                        .child(SharedString::from(
                            f.markdown
                                .parent()
                                .map(|d| d.to_string_lossy().to_string())
                                .unwrap_or_default(),
                        )),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(review)
            .when(!run.checks.is_empty(), |d| d.child(counts))
            .children(run.evidence.as_deref().and_then(|chain| {
                let line = chain.tally().line();
                (!line.is_empty()).then(|| {
                    let bad = chain.invalid_cites();
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(if bad > 0 {
                            format!(
                                "Across sources: {line} · {bad} citation{} Tern could not back",
                                if bad == 1 { "" } else { "s" }
                            )
                        } else {
                            format!("Across sources: {line}")
                        }))
                })
            }))
            .when(run.checks.is_empty(), |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(p.faint)
                        .child("The answer makes no cited claims."),
                )
            })
            .children(claims)
            .children(files)
            .into_any_element()
    }

    /// A claim's evidence in the Report: its stance across sources with
    /// the agreement bar, one card per source (quote, locator, logo,
    /// stance), citations Tern could not back, and its counter-search.
    fn claim_evidence(
        &self,
        chain: &tern_agent::verify::Chain,
        ev: &tern_agent::verify::ClaimEvidence,
        pages: &[PageRef],
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use tern_agent::verify::CiteStatus;
        let head = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(stance_chip(ev.stance, p))
            .child(agreement_bar(&ev.agreement, p))
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(if ev.agreement.few() {
                        format!("{} · few sources", ev.agreement.line())
                    } else {
                        ev.agreement.line()
                    })),
            );
        let cards = ev
            .cards
            .iter()
            .take(EVIDENCE_CARDS)
            .enumerate()
            .filter_map(|(i, k)| {
                let q = k.quote.as_ref().and_then(|id| chain.quote(id))?;
                let page = pages
                    .iter()
                    .find(|pg| pg.url == q.url)
                    .cloned()
                    .unwrap_or_else(|| PageRef {
                        title: q.title.clone(),
                        url: q.url.clone(),
                        site: q.site.clone(),
                        builtin: None,
                        favicon: None,
                    });
                let loc = q.locator();
                let mut where_ = if k.source > 0 {
                    format!("[{}] {}", k.source, page.label())
                } else {
                    page.label()
                };
                if !loc.is_empty() {
                    where_ = format!("{where_} · {loc}");
                }
                if k.counter {
                    where_ = format!("{where_} · counter-search");
                }
                let url = if q.anchor.is_empty() {
                    q.url.clone()
                } else {
                    q.anchor.clone()
                };
                Some(
                    div()
                        .id(SharedString::from(format!("ev-card-{}-{i}", ev.claim)))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .px_1()
                        .py(px(3.))
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|d| d.bg(p.hover))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .w(px(14.))
                                        .flex_none()
                                        .text_xs()
                                        .text_color(stance_ink(k.stance, p))
                                        .child(SharedString::from(k.stance.glyph())),
                                )
                                .child(self.site_icon(&page, 14., p))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(p.muted)
                                        .child(SharedString::from(where_)),
                                ),
                        )
                        .child(
                            div()
                                .pl(px(22.))
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(format!("“{}”", q.text))),
                        )
                        .when(!k.note.is_empty() && k.stance != Stance::Supports, |d| {
                            d.child(
                                div()
                                    .pl(px(22.))
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(k.note.clone())),
                            )
                        })
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.browse(&url, window, cx)),
                        ),
                )
            });
        let unbacked = ev.cites.iter().filter(|b| !b.status.renders()).map(|b| {
            let what = if b.status == CiteStatus::Forged {
                b.token.clone()
            } else {
                format!("[{}]", b.n)
            };
            div()
                .text_xs()
                .text_color(p.muted)
                .child(SharedString::from(format!("{what} · {}", b.status.label())))
        });
        let counter = ev
            .counter
            .as_ref()
            .filter(|cs| cs.ran || !cs.note.is_empty())
            .map(|cs| {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child(gpui_kit::component::Icon::new(IconName::Search).xsmall())
                    .child(SharedString::from(format!(
                        "Counter-searched “{}” · {}",
                        cs.query, cs.note
                    )))
            });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .pt_1()
            .child(head)
            .children(cards)
            .children(unbacked)
            .children(counter)
            .into_any_element()
    }

    /// Each step as one line, verb first.
    fn run_detailed(&self, run: &Run, p: Palette) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(run.detailed().map(|s| step_line(s, p, true)))
            .into_any_element()
    }

    /// Every event as logged, filterable, each row opening to its payload.
    fn run_raw(&self, run: &Run, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.model.run_panel.filter;
        let chips = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(RawFilter::ALL.map(|f| {
                let selected = f == filter;
                div()
                    .id(SharedString::from(format!("raw-filter-{}", f.label())))
                    .px_2()
                    .py(px(1.))
                    .rounded_md()
                    .border_1()
                    .border_color(if selected { p.muted } else { p.border })
                    .text_xs()
                    .cursor_pointer()
                    .text_color(if selected { p.text } else { p.muted })
                    .hover(|d| d.text_color(p.text))
                    .child(f.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.set_raw_filter(f);
                        cx.notify();
                    }))
            }));
        let open = &self.model.run_panel.open;
        let rows: Vec<AnyElement> = run
            .raw(filter)
            .enumerate()
            .map(|(k, s)| {
                let is_open = open.contains(&k);
                div()
                    .id(SharedString::from(format!("raw-{k}")))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(p.border)
                    .cursor_pointer()
                    .hover(|d| d.bg(p.hover))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .text_xs()
                            .font_family("monospace")
                            .child(
                                div()
                                    .truncate()
                                    .text_color(p.text)
                                    .child(SharedString::from(format!("#{} {}", s.seq, s.event))),
                            )
                            .child(div().flex_none().text_color(p.faint).child(offset(s))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.muted)
                            .child(SharedString::from(s.summary.clone())),
                    )
                    .when(is_open, |d| {
                        d.child(
                            div()
                                .p_2()
                                .rounded_sm()
                                .bg(p.canvas)
                                .text_xs()
                                .font_family("monospace")
                                .text_color(p.muted)
                                .child(SharedString::from(s.payload_text(RAW_PAYLOAD_CHARS))),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.toggle_raw(k);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(chips)
            .when(rows.is_empty(), |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(p.faint)
                        .child("Nothing of this kind in this run."),
                )
            })
            .children(rows)
            .into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let build_from = Instant::now();
        self.tick_intro(window, cx);
        if let Some((plan, reply)) = self.replay_plan.take() {
            self.show_plan(plan, reply, window, cx);
        }
        if let Some(text) = self.composer_restore.take() {
            self.composer
                .update(cx, |c, cx| crate::view::set_end(c, text, window, cx));
        }
        self.save_session_soon(cx);
        self.tick_scout(window, cx);
        self.tick_explain(window, cx);
        self.sync_web(window, cx);
        // The Graph view draws in each time it opens, and again when the
        // chat's graph changes (another chat, new sources or claims).
        if self.model.run_panel.view == RunView::Graph && self.model.run_panel_open {
            let (_, _, changed) = self.graph_data();
            if changed {
                self.graph.opened = Some(Instant::now());
                self.graph.hover = None;
            } else {
                self.graph.opened.get_or_insert_with(Instant::now);
            }
        } else {
            self.graph.opened = None;
        }
        self.tick_toasts(window, cx);
        self.tick_rail_scroll(window, cx);
        let now = Instant::now();
        // A flashcard turning over draws every frame until it lands.
        if self.model.screen == Screen::Study
            && self
                .model
                .study
                .review
                .as_ref()
                .is_some_and(|r| r.turning(now, self.reduced_motion))
        {
            frame::next_frame(window, cx);
        }
        self.window_h = f32::from(window.viewport_size().height);
        let layout = Layout::for_width(f32::from(window.viewport_size().width));
        if Layout::sidebar_squeezed(self.last_layout, layout) {
            self.model.sidebar_open = false;
        }
        if self.last_layout.is_some_and(|l| l.panel_beside) && !layout.panel_beside {
            self.model.run_panel_open = false;
        }
        // The rail opens by itself once a run has something to show
        // (sources, checked claims), once per run, and only where it fits
        // beside the chat; closing it keeps it closed for that run.
        if !self.model.is_busy() {
            self.rail_auto = false;
        } else if !self.rail_auto
            && layout.panel_beside
            && self.model.screen == Screen::Chat
            && self.model.run_panel_opens_itself()
        {
            let run = self.model.panel_run();
            if !run.checks.is_empty() || !run.ledger().sources.is_empty() {
                self.rail_auto = true;
                self.model.run_panel_open = true;
            }
        }
        self.last_layout = Some(layout);
        let reduced = self.reduced_motion;
        if self.wipe.is_some_and(|w| w.done(now)) {
            self.wipe = None;
        }
        let p = match &self.wipe {
            Some(w) => w.palette(now),
            None => Palette::get(self.model.dark),
        };
        self.style_composer(p, cx);
        // A page opening shows chat | page half and half with the sidebar
        // folded away (back when the page closes), on the Page tab (#68).
        let browser_open = self.model.browser.visible
            && self.model.run_panel_open
            && self.model.screen != Screen::Settings;
        if browser_open != self.browser_was_open {
            if browser_open {
                self.browser_share = crate::browser::SPLIT_DEFAULT;
                self.model.browser.view = crate::browser::PaneView::Page;
                if self.model.sidebar_open {
                    self.model.sidebar_open = false;
                    self.browser_hid_sidebar = true;
                }
            } else if std::mem::take(&mut self.browser_hid_sidebar) {
                self.model.sidebar_open = true;
            }
            self.browser_was_open = browser_open;
        }
        // Settings has its own section list: the chat list slides away
        // while it shows and comes back after.
        let sidebar = self.sidebar_slide.frame(
            self.model.sidebar_open && self.model.screen != Screen::Settings,
            now,
            reduced,
        );
        // Settings is a screen of its own (v2 s16): the right rail slides
        // away too, so nothing beside it re-renders while it scrolls.
        let panel = self.panel_slide.frame(
            self.model.run_panel_open && self.model.screen != Screen::Settings,
            now,
            reduced,
        );
        // The theme wipe's covers are drawn by the window's root over the
        // cached workspace (`Shell`), so the wipe's frames never redraw
        // the workspace: it draws once in the new theme.
        if self.sidebar_slide.moving() || self.panel_slide.moving() {
            frame::next_frame(window, cx);
        }
        // The right pane: the browser when it is showing, else the docked
        // side chat, else the run panel.
        let side_docked = self.model.cont.side.open && !self.model.cont.side.popped;
        let viewport_w = f32::from(window.viewport_size().width);
        let panels = self.model.panels;
        let right_w = panels.right;
        let sidebar_w = panels.sidebar;
        let sidebar = sidebar.filter(|_| panels.shows_sidebar());
        let panel = panel.filter(|_| panels.shows_right());
        let beside_sidebar = if layout.sidebar_beside {
            sidebar.map_or(0., |f| sidebar_w * f)
        } else {
            0.
        };
        let want = if self.model.browser.visible {
            // Half of what is right of the rail and the sidebar: with the
            // sidebar open, chat and page still get equal halves.
            let handle = if beside_sidebar > 0. {
                crate::panels::HANDLE_W
            } else {
                0.
            };
            crate::browser::split_width(
                viewport_w - RAIL_W - beside_sidebar - handle,
                self.browser_share,
            )
        } else if side_docked {
            continuity_view::SIDE_W.max(right_w)
        } else if self.model.run_panel.view == RunView::Graph && self.model.screen != Screen::Study
        {
            // The run map needs room: the rail widens for Graph (v2 s05).
            GRAPH_W.max(right_w)
        } else {
            right_w
        };
        // One row that never runs past the window's edge: the pane sits
        // beside the chat only when both fit, else it floats over it.
        let frame = crate::panels::Panels::frame(
            viewport_w,
            RAIL_W,
            beside_sidebar,
            want,
            layout.panel_beside,
        );
        let pane_beside = frame.right_beside;
        let side_w = if panels.focus == crate::panels::Focus::RightMax {
            (viewport_w - RAIL_W).max(0.)
        } else {
            frame.right
        };
        self.side_w = side_w;
        self.window_w = viewport_w;
        self.centre_x = RAIL_W + beside_sidebar;
        // What the chat column gets, for the tab strip's overflow.
        self.centre_w = (viewport_w
            - RAIL_W
            - beside_sidebar
            - if pane_beside {
                panel.map_or(0., |f| (side_w + crate::panels::HANDLE_W) * f)
            } else {
                0.
            })
        .max(0.);
        let side_pane = |this: &Self, cx: &mut Context<Self>| {
            if this.model.browser.visible {
                this.browser_pane(p, cx)
            } else if side_docked {
                this.side_panel(p, cx)
            } else if this.model.harness == tern_agent::Harness::Study
                && this.model.screen == Screen::Study
            {
                // v2 s14: in Study the rail shows decks, what is due and
                // recall, not a research run.
                this.study_rail(p, cx)
            } else {
                this.run_panel(p, cx).into_any_element()
            }
        };
        let mut side = panel.map(|_| side_pane(self, cx));
        let screen_id: &'static str = match self.model.screen {
            Screen::Chat => "screen-chat",
            Screen::Settings => "screen-settings",
            Screen::Editor => "screen-editor",
            Screen::Git => "screen-git",
            Screen::Watches => "screen-watches",
            Screen::Study => "screen-study",
            Screen::Library => "screen-library",
            Screen::Archived => "screen-archived",
        };
        let root = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.canvas)
            .text_color(p.text)
            // First in the tree: prepaints once the workspace is laid out
            // (`crate::perf`'s layout phase ends here).
            .child(
                gpui_kit::canvas(|_, _, _| crate::perf::prepaint_start(), |_, _, _, _| ())
                    .absolute()
                    .size_0(),
            )
            .when(panels.dragging().is_some(), |d| d.cursor_col_resize())
            .on_mouse_move(
                cx.listener(|this, e: &gpui_kit::MouseMoveEvent, window, cx| {
                    this.panel_drag_move(f32::from(e.position.x), window, cx);
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &gpui_kit::MouseUpEvent, _, cx| this.panel_drag_end(cx)),
            )
            .capture_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, window, cx| {
                if !this.focus_keys(e, window, cx)
                    && !this.palette_keys(e, window, cx)
                    && !this.needs_you_keys(e, window, cx)
                    && !this.settings_keys(e, window, cx)
                    && !this.scout_keys(e, cx)
                {
                    this.panel_keys(e, cx);
                }
            }))
            .child(self.title_bar(p, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(self.rail(p, cx))
                    .when_some(sidebar.filter(|_| layout.sidebar_beside), |d, f| {
                        d.child(sliding(self.sidebar(p, cx), sidebar_w, f, false))
                            .when(f >= 1., |d| {
                                d.child(self.drag_handle(crate::panels::Edge::Sidebar, p, cx))
                            })
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            // The chat gives way rather than pushing the
                            // right pane off the window.
                            .overflow_hidden()
                            .when(!panels.shows_centre(), |d| d.hidden())
                            .map(|d| match self.model.screen {
                                Screen::Chat
                                    if self.model.harness == Harness::Study
                                        && self.active_explain().is_none() =>
                                {
                                    d.child(self.study_window(p, window, cx))
                                }
                                Screen::Chat => match self.active_explain() {
                                    Some(key) => d
                                        .children(self.tab_strip(p, cx))
                                        .child(self.explain_view(key, p, cx)),
                                    None => d
                                        .children(self.tab_strip(p, cx))
                                        .child(self.transcript(p, cx))
                                        .children(self.range_bar(p, cx).map(|b| {
                                            div()
                                                .w_full()
                                                .max_w(px(COLUMN_MAX_W))
                                                .mx_auto()
                                                .px_4()
                                                .pb_1()
                                                .child(b)
                                        }))
                                        .child(self.composer(p, cx))
                                        .children(self.folder_tray(p, cx)),
                                },
                                Screen::Settings => d.child(self.settings_screen(p, window, cx)),
                                Screen::Editor => d.child(self.editor_screen(p, cx)),
                                Screen::Git => d.child(self.git_screen(p, cx)),
                                Screen::Watches => d.child(self.watches_screen(p, cx)),
                                Screen::Study => d.child(self.study_window(p, window, cx)),
                                Screen::Library => d.child(self.library_screen(p, cx)),
                                Screen::Archived => d.child(self.archived_screen(p, cx)),
                            })
                            .with_animation(
                                screen_id,
                                Animation::new(if reduced { Duration::ZERO } else { motion::UI })
                                    .with_easing(motion::ease),
                                |d, t| d.opacity(t),
                            ),
                    )
                    .when_some(panel.filter(|_| pane_beside), |d, f| {
                        d.when(f >= 1. && panels.shows_centre(), |d| {
                            d.child(self.drag_handle(crate::panels::Edge::Right, p, cx))
                        })
                        .children(side.take().map(|e| sliding(e, side_w, f, true)))
                    })
                    // Too narrow to sit beside the chat: the panels float over it.
                    .when_some(sidebar.filter(|_| !layout.sidebar_beside), |d, f| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(px(RAIL_W))
                                .shadow_lg()
                                .child(sliding(self.sidebar(p, cx), sidebar_w, f, false)),
                        )
                    })
                    .when_some(panel.filter(|_| !pane_beside), |d, f| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .shadow_lg()
                                .children(side.take().map(|e| sliding(e, side_w, f, true))),
                        )
                    }),
            )
            .children(self.highlight_popover(p, cx))
            .children(self.undo_toast(p, cx))
            .children(self.toasts(p, cx))
            .children(self.onboarding_modal(p, window, cx))
            .children(self.palette_overlay(p, cx))
            .children(self.focus_overlay(p, window, cx))
            .children(self.splash(p, window))
            .children(self.intro_overlay(window, cx));
        // A sliding highlight or knob still moving draws the next frame.
        if self.anim_pending.take() {
            frame::next_frame(window, cx);
        }
        crate::perf::workspace(&self.perf_tag(), build_from.elapsed());
        root
    }
}

/// How a [`pill`] looks: filled like the kit's default button, or bare
/// like its ghost one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PillLook {
    Solid,
    Ghost,
}

/// A small capsule button built from one plain div: the label, the
/// capsule's hover and press tints, an optional tooltip (built only on
/// hover) and `on_click` unless `disabled`. The settings rows use it in
/// place of the kit's `Button`, which sets up a focus handle, keyed state
/// and its own style layers per button on every render; a provider row
/// has up to four of them and Settings › Providers redraws on every
/// scroll and theme-wipe frame (`crate::perf`).
pub(crate) fn pill(
    id: SharedString,
    label: impl Into<SharedString>,
    look: PillLook,
    disabled: bool,
    tip: Option<SharedString>,
    p: Palette,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let (bg, border) = match look {
        PillLook::Solid => (p.raised, p.border),
        PillLook::Ghost => (gpui_kit::transparent_black(), gpui_kit::transparent_black()),
    };
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h_6()
        .px_2()
        .rounded_full()
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_sm()
        .line_height(gpui_kit::relative(1.25))
        .text_color(p.text)
        .child(label.into())
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| {
            d.hover(|s| s.bg(p.hover))
                .active(|s| s.bg(p.border))
                .on_click(on_click)
        })
        .when_some(tip, |d, tip| {
            d.tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
        })
        .into_any_element()
}

/// The logo square at the start of a provider or search row: the
/// service's mark on a neutral tile, like a dock icon.
fn logo_tile(r: &ServiceRow, p: Palette) -> impl IntoElement {
    div()
        .size(px(28.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .border_1()
        .border_color(p.border)
        .bg(p.raised)
        .child(brand_logo(r.id, 16., p))
}

/// A provider's or search service's logo at `size` px: colour marks in
/// their own colours, one-colour marks in the text colour, and the
/// initial for a service Tern has no mark for (`crate::brands`).
pub(crate) fn brand_logo(id_or_name: &str, size: f32, p: Palette) -> AnyElement {
    match crate::brands::logo(id_or_name) {
        crate::brands::Logo::Mark(path, crate::brands::Tint::Original) => gpui_kit::img(path)
            .size(px(size))
            .flex_none()
            .into_any_element(),
        crate::brands::Logo::Mark(path, crate::brands::Tint::Mono) => svg()
            .path(path)
            .size(px(size))
            .flex_none()
            .text_color(mono_ink(p))
            .into_any_element(),
        crate::brands::Logo::Initial(letter) => div()
            .size(px(size + 2.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .bg(p.accent.opacity(0.18))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(p.accent)
            .child(SharedString::from(letter))
            .into_any_element(),
    }
}

/// A service's logo (a source site, an MCP server, a provider): the
/// coloured brand mark when Tern ships one, else the one-colour service
/// glyph in `ink`.
pub(crate) fn service_logo(name: &str, size: f32, p: Palette, ink: gpui_kit::Hsla) -> AnyElement {
    if matches!(crate::brands::logo(name), crate::brands::Logo::Mark(..)) {
        brand_logo(name, size, p)
    } else {
        svg()
            .path(crate::assets::service_path(name))
            .size(px(size))
            .flex_none()
            .text_color(ink)
            .into_any_element()
    }
}

/// The ink for a one-colour brand mark: white on a dark theme, near
/// black on a light one, at full strength so the mark never reads grey.
pub(crate) fn mono_ink(p: Palette) -> gpui_kit::Hsla {
    if p.canvas.l < 0.5 {
        gpui_kit::hsla(0., 0., 1., 1.)
    } else {
        gpui_kit::hsla(0., 0., 0.07, 1.)
    }
}

/// A message's Markdown as one text view; with right-to-left paragraphs
/// (Urdu, Arabic), a column of views by direction, the right-to-left ones
/// right-aligned (`crate::bidi`). `style` sets each view up the same way.
pub(crate) fn bidi_markdown(
    key: &'static str,
    i: usize,
    markdown: String,
    style: impl Fn(TextView) -> TextView,
) -> AnyElement {
    let blocks = crate::bidi::blocks(&markdown);
    if blocks.len() == 1 && !blocks[0].rtl {
        return style(TextView::markdown((key, i), markdown)).into_any_element();
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .children(blocks.into_iter().enumerate().map(|(k, b)| {
            if b.rtl {
                rtl_flow(crate::bidi::rtl_lines(&b.text, true), true)
            } else {
                style(TextView::markdown(
                    SharedString::from(format!("{key}-{i}-{k}")),
                    b.text,
                ))
                .into_any_element()
            }
        }))
        .into_any_element()
}

/// Right-to-left lines as rows of words flowing from the right edge and
/// wrapping there (`crate::bidi::RtlLine` says why GPUI's own wrapping
/// cannot). `fill`: each row spans the column, so a short line still sits
/// at its right edge; off in a bubble, which takes the width of its text.
pub(crate) fn rtl_flow(lines: Vec<crate::bidi::RtlLine>, fill: bool) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .when(fill, |d| d.w_full())
        .gap_1()
        .children(lines.into_iter().map(move |l| {
            div()
                .when(fill, |d| d.w_full())
                .when(l.starts_paragraph, |d| d.mt_2())
                .when(l.heading > 0, |d| {
                    d.font_weight(FontWeight::SEMIBOLD)
                        .when(l.heading <= 2, |d| d.text_lg())
                })
                .flex()
                .flex_row_reverse()
                .flex_wrap()
                .gap_x(px(5.))
                .children(
                    l.marker
                        .map(|m| div().flex_none().child(SharedString::from(m))),
                )
                .children(l.words.into_iter().map(|w| {
                    div()
                        .flex_none()
                        .child(SharedString::from(crate::bidi::rtl_word(&w)))
                }))
        }))
        .into_any_element()
}

/// New words fade in as they stream, a few milliseconds apart.
fn stream_motion() -> TextViewMotion {
    TextViewMotion::default()
        .with_stream_fade(Duration::from_millis(240))
        .with_stream_fade_stagger(Duration::from_millis(16))
}

/// One step as a line: its icon, the verb, what it acted on, and a note.
/// Problems keep the same colours (no hue before ADR 0002) but get a
/// distinct icon and full-strength text.
fn step_line(s: &RunStep, p: Palette, with_time: bool) -> AnyElement {
    let icon = match s.kind {
        _ if !s.ok => AssetIcon::CircleX,
        StepKind::Turn => AssetIcon::CircleCheck,
        StepKind::Model => AssetIcon::Sparkles,
        StepKind::Search => AssetIcon::Search,
        StepKind::Read => AssetIcon::Globe,
        StepKind::Tool => AssetIcon::Wrench,
        StepKind::Policy => AssetIcon::ShieldCheck,
        StepKind::Approval => AssetIcon::UserCheck,
        StepKind::Problem => AssetIcon::TriangleAlert,
        StepKind::Plan => AssetIcon::ListChecks,
        StepKind::Worker => AssetIcon::Workflow,
        StepKind::Steer => AssetIcon::Compass,
        StepKind::Decision => AssetIcon::Signpost,
    };
    div()
        .flex()
        .items_start()
        .gap_2()
        .text_sm()
        .child(
            div()
                .pt(px(2.))
                .text_color(if s.ok { p.faint } else { p.text })
                .child(gpui_kit::component::Icon::new(icon).small()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.text)
                                .child(s.verb),
                        )
                        .when(!s.target.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(p.muted)
                                    .child(SharedString::from(s.target.clone())),
                            )
                        })
                        .when(with_time, |d| {
                            d.child(
                                div()
                                    .ml_auto()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(offset(s)),
                            )
                        }),
                )
                .when(!s.note.is_empty(), |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(if s.ok { p.faint } else { p.muted })
                            .child(SharedString::from(s.note.clone())),
                    )
                }),
        )
        .into_any_element()
}

/// "+3.2s" from the turn's start.
fn offset(s: &RunStep) -> SharedString {
    format!("+{:.1}s", s.at.as_secs_f32()).into()
}

/// An attachment on a sent message.
/// The width of an attachment preview card in the composer (#71).
const PREVIEW_CARD_W: f32 = 148.;
/// ...and in a sent message.
const SENT_CARD_W: f32 = 112.;

/// A sent message's attachment as a small preview card (#71): its kept
/// picture, else a kind tile, with the file-type logo and the name.
fn sent_card(
    c: &tern_agent::Chip,
    image: Option<std::sync::Arc<gpui_kit::Image>>,
    p: Palette,
) -> AnyElement {
    let kind = crate::filetype::FileKind::of(&c.name);
    div()
        .flex()
        .flex_col()
        .w(px(SENT_CARD_W))
        .rounded_lg()
        .overflow_hidden()
        .border_1()
        .border_color(p.border)
        .bg(p.canvas)
        .child(card_preview(
            image,
            kind,
            &c.name,
            None,
            p,
            SENT_CARD_W,
            60.,
        ))
        .child(
            div()
                .px_2()
                .py(px(3.))
                .flex()
                .items_center()
                .gap_1()
                .min_w_0()
                .child(kind_logo(kind, &c.name, 14., p))
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(p.muted)
                        .child(SharedString::from(if c.role == Role::Prompt {
                            format!("{} · Prompt", c.name)
                        } else {
                            c.name.clone()
                        })),
                ),
        )
        .into_any_element()
}

/// A card's preview area, `w` × `h`: the picture when there is one; the
/// first lines of the text as a little page for a document; else a tile
/// with the file-type logo (#72; it replaced the coloured kind badges).
fn card_preview(
    image: Option<std::sync::Arc<gpui_kit::Image>>,
    kind: crate::filetype::FileKind,
    name: &str,
    text: Option<&str>,
    p: Palette,
    w: f32,
    h: f32,
) -> AnyElement {
    let lines = text
        .filter(|_| kind.shows_page())
        .map(|t| crate::filetype::preview_lines(kind, t, 6))
        .filter(|l| !l.is_empty());
    let body = match (image, lines) {
        (Some(img), _) => div()
            .size_full()
            .child(
                gpui_kit::img(img)
                    .size_full()
                    .object_fit(gpui_kit::ObjectFit::Cover),
            )
            .into_any_element(),
        (None, Some(lines)) => div()
            .size_full()
            .p(px(6.))
            .bg(Hsla::from(gpui_kit::rgb(0xF7F6F2)))
            .flex()
            .flex_col()
            .gap(px(1.))
            .overflow_hidden()
            .children(lines.into_iter().enumerate().map(|(k, l)| {
                div()
                    .text_size(px(if k == 0 { 8.5 } else { 7. }))
                    .line_height(px(if k == 0 { 11. } else { 9. }))
                    .when(k == 0, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .text_color(gpui_kit::rgb(if k == 0 { 0x222222 } else { 0x555555 }))
                    .truncate()
                    .child(SharedString::from(l))
            }))
            .into_any_element(),
        (None, None) => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(p.raised)
            .child(kind_logo(kind, name, (h * 0.45).clamp(24., 40.), p))
            .into_any_element(),
    };
    div()
        .relative()
        .w(px(w))
        .h(px(h))
        .overflow_hidden()
        .border_b_1()
        .border_color(p.border)
        .child(body)
        .into_any_element()
}

/// The file-type logo for an attachment named `name` of kind `kind`,
/// `size` px: its real logo by name (#72); a folder its folder logo by
/// name (#73: `.git`, `src`, `docs`, …); a web page keeps its outline icon; a name with no known kind falls back to its kind's
/// logo (a pasted picture, a recording, pasted text).
fn kind_logo(kind: crate::filetype::FileKind, name: &str, size: f32, p: Palette) -> AnyElement {
    use crate::fileicon::{FileIcon, logo};
    use crate::filetype::FileKind;
    let outline = |icon: AssetIcon| {
        div()
            .flex_none()
            .size(px(size))
            .flex()
            .items_center()
            .justify_center()
            .text_color(p.muted)
            .child(gpui_kit::component::Icon::new(icon).size(px(size * 0.85)))
            .into_any_element()
    };
    match kind {
        FileKind::Folder => crate::fileicon::folder_logo(name, size),
        FileKind::Link => outline(AssetIcon::Globe),
        _ => {
            let by_name = FileIcon::of(name);
            let fallback = match kind {
                FileKind::Image => "image-raster",
                FileKind::Audio => "audio",
                FileKind::Text => "plain-text",
                FileKind::Archive => "zip-archive",
                _ => "",
            };
            let icon = if by_name.is_unknown() {
                FileIcon::named(fallback).unwrap_or(by_name)
            } else {
                by_name
            };
            logo(icon, size, p.text.l > 0.5)
        }
    }
}

/// A side panel part-way open: it keeps its full width inside a clip that
/// grows, and fades with it, so its contents never reflow mid-slide.
/// Whether the operating system shows this window dark.
fn system_dark(window: &Window) -> bool {
    matches!(
        window.appearance(),
        gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark
    )
}

fn sliding(panel: impl IntoElement, width: f32, f: f32, from_right: bool) -> impl IntoElement {
    div()
        .flex_none()
        .h_full()
        .w(px(width * f))
        .overflow_hidden()
        .opacity(f)
        .flex()
        .when(from_right, |d| d.justify_start())
        .when(!from_right, |d| d.justify_end())
        .child(div().flex_none().h_full().w(px(width)).child(panel))
}

/// A segmented donut `size` px across with a ring `ring` px wide: the
/// track all the way round, then each arc (start and sweep in degrees,
/// clockwise from twelve o'clock) in its colour (`crate::gauge`).
fn donut(size: f32, ring: f32, arcs: Vec<(f32, f32, Hsla)>, track: Hsla) -> impl IntoElement {
    gpui_kit::canvas(
        |_, _, _: &mut gpui_kit::App| (),
        move |b, _, window, _: &mut gpui_kit::App| {
            let s = f32::from(b.size.width).min(f32::from(b.size.height));
            let (cx, cy) = (
                f32::from(b.origin.x) + s / 2.,
                f32::from(b.origin.y) + s / 2.,
            );
            let r = (s - ring) / 2.;
            let stroke = |start: f32, sweep: f32| {
                let pts = crate::gauge::arc_points(cx, cy, r, start, sweep);
                let mut pb = gpui_kit::PathBuilder::stroke(px(ring));
                let (first, rest) = pts.split_first()?;
                pb.move_to(gpui_kit::point(px(first.0), px(first.1)));
                for q in rest {
                    pb.line_to(gpui_kit::point(px(q.0), px(q.1)));
                }
                pb.build().ok()
            };
            if let Some(path) = stroke(0., 360.) {
                window.paint_path(path, track);
            }
            for (start, sweep, color) in &arcs {
                if *sweep > 0.3
                    && let Some(path) = stroke(*start, *sweep)
                {
                    window.paint_path(path, *color);
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
}

/// The live line while a turn runs: the tern mark moving with the work, a
/// shimmering phrase, and elapsed seconds (`docs/02-ui-definition.md` §2.7).
/// The answer with a verdict badge after each checked claim; a badge
/// opens that claim in the run panel's Report view.
/// Which page each citation number in `text` opens: the page the claim
/// checker matched, else the answer's own link list (#72).
fn cite_pages(text: &str, pages: &[PageRef]) -> std::collections::HashMap<usize, String> {
    tern_agent::verify::cite_targets(text, pages)
        .into_iter()
        .collect()
}

/// The answer with its quote tokens (`[[q:ab12cd]]`) as `[n]`, against
/// the quotes its evidence chain holds; tokens Tern never minted are
/// dropped.
fn render_quotes(text: &str, run: &Run) -> String {
    if !text.contains("[q:") && !text.contains("[[q:") {
        return text.to_owned();
    }
    let book = tern_agent::verify::QuoteBook {
        quotes: run
            .evidence
            .as_deref()
            .map(|c| c.quotes.clone())
            .unwrap_or_default(),
    };
    tern_agent::verify::render_quotes(text, &book).text
}

/// Hover cards for the answer's citation chips: for each `[n]`, the quotes
/// Tern minted from its source that the claims rest on, with each claim's
/// stance and agreement; a citation of a page Tern never read says so.
fn cite_cards(
    chain: &tern_agent::verify::Chain,
    targets: &std::collections::HashMap<usize, String>,
    pages: &[PageRef],
    icons: &std::collections::HashMap<String, SiteIcon>,
) -> Vec<(String, crate::richmd::CiteCard)> {
    let unread = chain.unread_numbers();
    targets
        .iter()
        .map(|(n, url)| {
            let page = pages.iter().find(|p| p.url == *url);
            let site = page.map(|p| p.site.clone()).unwrap_or_else(|| {
                url.split("://")
                    .nth(1)
                    .unwrap_or(url)
                    .split('/')
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches("www.")
                    .to_owned()
            });
            let quotes: Vec<crate::richmd::CiteQuote> = chain
                .quotes_for(*n)
                .into_iter()
                .take(2)
                .map(|(ev, card, q)| crate::richmd::CiteQuote {
                    stance: (card.stance.glyph(), card.stance.label()),
                    agreement: format!(
                        "Claim {}: {} · {}",
                        ev.claim,
                        ev.stance.label().to_lowercase(),
                        ev.agreement.line()
                    ),
                    quote: q.text.clone(),
                    locator: q.locator(),
                })
                .collect();
            let note = if unread.contains(n) {
                "Tern never read this source, so this citation is not backed.".to_owned()
            } else if quotes.is_empty() {
                "No passage of this source bears on the claims that cite it.".to_owned()
            } else {
                String::new()
            };
            let icon = match icons.get(&site) {
                Some(SiteIcon::Ready(path)) => Some(path.clone()),
                _ => None,
            };
            (
                url.clone(),
                crate::richmd::CiteCard {
                    n: *n,
                    title: page.map(|p| p.title.clone()).unwrap_or_default(),
                    site,
                    icon,
                    quotes,
                    note,
                },
            )
        })
        .collect()
}

fn with_badges(entry: usize, text: &str, run: &Run) -> String {
    tern_agent::verify::annotate(text, &run.checks, |claim| {
        tern_agent::verify::claim_link(entry, claim)
    })
}

/// Source cards shown per claim in the Report.
const EVIDENCE_CARDS: usize = 4;

/// The ink for a stance: grey steps, never a hue (palette C).
fn stance_ink(s: Stance, p: Palette) -> Hsla {
    match s {
        Stance::Supports | Stance::Refutes => p.text,
        Stance::Mixed => p.muted,
        Stance::NotEnoughInfo => p.faint,
    }
}

/// A claim's stance as a chip: glyph and label, outlined.
fn stance_chip(s: Stance, p: Palette) -> AnyElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .border_1()
        .border_color(if s == Stance::Refutes {
            p.muted
        } else {
            p.border
        })
        .text_xs()
        .text_color(stance_ink(s, p))
        .child(SharedString::from(s.glyph()))
        .child(s.label())
        .into_any_element()
}

/// Width of the agreement bar.
const BAR_W: f32 = 96.;

/// The agreement bar: one stretch per side, sized by independent sources.
/// Supports is solid, mixed is mid-grey, refutes is an outline, no info is
/// the track; shape carries the meaning, the palette stays grey.
fn agreement_bar(a: &tern_agent::verify::Agreement, p: Palette) -> AnyElement {
    let total = a.sources().max(1) as f32;
    let part = |n: usize| BAR_W * n as f32 / total;
    let seg = |w: f32| div().h(px(6.)).w(px(w)).flex_none();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(1.))
        .w(px(BAR_W + 3.))
        .when(a.supports > 0, |d| {
            d.child(seg(part(a.supports)).bg(p.text))
        })
        .when(a.mixed > 0, |d| d.child(seg(part(a.mixed)).bg(p.muted)))
        .when(a.refutes > 0, |d| {
            d.child(seg(part(a.refutes)).border_1().border_color(p.text))
        })
        .when(a.nei > 0, |d| d.child(seg(part(a.nei)).bg(p.border)))
        .when(a.sources() == 0, |d| d.child(seg(BAR_W).bg(p.border)))
        .into_any_element()
}

/// A verdict as its Tern glyph (ADR 0002's verdict set). Shape carries
/// the meaning; the palette stays black, white, and grey.
fn verdict_badge(v: Verdict, p: Palette) -> AnyElement {
    let (name, color) = match v {
        Verdict::Supported => ("verdict-supported", p.text),
        Verdict::Partly => ("verdict-partial", p.muted),
        Verdict::NotSupported => ("verdict-unsupported", p.text),
        Verdict::NoSource => ("verdict-unverified", p.faint),
    };
    match assets::glyph(name) {
        Some(path) => svg()
            .path(path)
            .size(px(14.))
            .flex_none()
            .text_color(color)
            .into_any_element(),
        None => div()
            .text_xs()
            .text_color(color)
            .child(v.glyph())
            .into_any_element(),
    }
}

fn working_line(
    line: &WorkingLine,
    p: Palette,
    reduced: bool,
    at: &'static str,
) -> impl IntoElement {
    let still = reduced || matches!(line.activity, Activity::NeedsYou | Activity::Stopping);
    let phrase: AnyElement = if still {
        div()
            .text_color(if line.activity == Activity::NeedsYou {
                p.text
            } else {
                p.muted
            })
            .child(SharedString::from(line.phrase.clone()))
            .into_any_element()
    } else {
        let chars: Vec<char> = line.phrase.chars().collect();
        div()
            .flex()
            .with_animation(
                (at, 0usize),
                Animation::new(motion::SHIMMER).repeat(),
                move |d, phase| {
                    let weights = motion::shimmer(chars.len(), phase);
                    d.children(chars.iter().zip(weights).map(|(c, w)| {
                        div()
                            .text_color(motion::lerp_color(p.muted, p.text, w))
                            .child(SharedString::from(c.to_string()))
                    }))
                },
            )
            .into_any_element()
    };
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .text_color(p.muted)
        .child(working_mark(line, p, still, at))
        .child(phrase)
        .child(
            div()
                .text_color(p.faint)
                .child(format!("{}s", line.seconds)),
        )
}

/// How tall the working mark is: big enough that its wingbeats read
/// (#76; the bobbing 16 px glyph looked static).
const WORKING_MARK_H: f32 = 34.;

/// The working mark (#76): the live dithered tern from the mark rig
/// (`assets/mark`), theme-aware, flapping and gliding in a loop while the
/// turn works. Waiting for you, stopping and reduced motion show the
/// still. The frame comes from the animation's phase, so nothing asks for
/// frames from a listener.
fn working_mark(line: &WorkingLine, p: Palette, still: bool, at: &'static str) -> AnyElement {
    thread_local! {
        static ANIM: crate::mark::MarkAnim = crate::mark::MarkAnim::embedded();
    }
    let dark = p.canvas.l < 0.5;
    let (still_path, strip_path, frames, fw) = ANIM.with(|m| {
        let now = Instant::now();
        let (sp, _, _) = m.frame(dark, now, now, true);
        let (ap, _, n) = m.frame(dark, now, now, false);
        (sp, ap, n, m.display_size(WORKING_MARK_H).0)
    });
    let h = WORKING_MARK_H;
    let dim = matches!(line.activity, Activity::Stopping);
    let boxed = div()
        .id(SharedString::from(format!("working-mark-{at}")))
        .w(px(fw))
        .h(px(h))
        .flex_none()
        .overflow_hidden()
        .relative()
        .when(dim, |d| d.opacity(0.5));
    if still || frames <= 1 {
        return boxed
            .child(
                gpui_kit::img(SharedString::from(still_path))
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(fw))
                    .h(px(h))
                    .object_fit(gpui_kit::ObjectFit::Fill),
            )
            .into_any_element();
    }
    // Wingbeats and the glide (frames 0-56 of the 72-frame loop, 30 fps):
    // the rest pose at the loop's end is left out while it works.
    let flying = frames.min(57);
    let pass = Duration::from_millis(flying as u64 * 1000 / 30);
    let strip = SharedString::from(strip_path);
    boxed
        .with_animation(
            SharedString::from(format!("working-mark-{at}-fly")),
            Animation::new(pass).repeat(),
            move |d, t| {
                let i = ((t * flying as f32) as u32).min(flying - 1);
                d.child(
                    gpui_kit::img(strip.clone())
                        .absolute()
                        .top_0()
                        .left(px(-(i as f32) * fw))
                        .w(px(fw * frames as f32))
                        .h(px(h))
                        .object_fit(gpui_kit::ObjectFit::Fill),
                )
            },
        )
        .into_any_element()
}

/// A page's own scrollbar (Settings): Tern's thin thumb, shown while the
/// page scrolls, over a plain GPUI scroll div with its own handle.
pub(crate) fn page_bar(id: SharedString, handle: &gpui_kit::ScrollHandle) -> impl IntoElement {
    use gpui_kit::base::Scrollbar;
    div()
        .absolute()
        .inset_0()
        .child(Scrollbar::vertical(handle).id(id).viewport_from_layout())
}

/// A bounded inner list's scrollbar (the expanded provider's models, the
/// model catalogue, a role's choices): drawn whenever the list overflows,
/// not only while it scrolls, so the box reads as scrollable at a glance.
/// The look is still Tern's thin thumb from `theme::install_scrollbar`;
/// only the show mode differs from the app's scroll-to-reveal default.
pub(crate) fn inner_list_bar(
    id: &'static str,
    handle: &gpui_kit::UniformListScrollHandle,
) -> impl IntoElement {
    use gpui_kit::base::{Scrollbar, ScrollbarMode};
    div().absolute().inset_0().child(
        Scrollbar::vertical(handle)
            .id(id)
            .mode(ScrollbarMode::Always)
            .viewport_from_layout(),
    )
}

thread_local! {
    /// The one wheel gesture going on, and which area has it.
    static LATCH: std::cell::Cell<crate::scrollbar::WheelLatch> =
        std::cell::Cell::new(crate::scrollbar::WheelLatch::default());
}

/// Where an area is and how far it can go: `(offset, max)` in GPUI's sign
/// (0 at the top, `-max` at the bottom).
fn scroll_span<H: gpui_kit::base::ScrollbarHandle>(handle: &H) -> (f32, f32) {
    let max =
        f32::from(handle.content_size().height) - f32::from(handle.viewport_bounds().size.height);
    (f32::from(handle.offset().y), max)
}

/// Moves `handle` by `dy` at once, as GPUI's own scroll areas do (the chat
/// scrolls this way). Returns whether it moved.
fn scroll_by<H: gpui_kit::base::ScrollbarHandle>(handle: &H, dy: f32) -> bool {
    let (offset, max) = scroll_span(handle);
    match crate::scrollbar::step(offset, dy, max) {
        Some(y) => {
            handle.set_offset(gpui_kit::point(handle.offset().x, px(y)));
            true
        }
        None => false,
    }
}

/// Gives an inner scroll area (a bounded list inside a page that scrolls)
/// the wheel: put it inside the area, before the list, as an absolute
/// child, with the list's scroll handle and an id of its own, and the
/// page's handle when the page is a plain GPUI scroll div (Settings).
///
/// A wheel turn over the list scrolls the list, at once, like the chat
/// and every GPUI scroll area; the page around it stays still. A gesture
/// stays with the area it started in until the wheel rests
/// (`scrollbar::WheelLatch`): a page scrolling under a still pointer does
/// not hand the rest of the spin to a list it slides under the pointer,
/// and a list spun to its end does not jerk the page; the next gesture at
/// the list's end scrolls the page. While the page owns a gesture over a
/// list, this moves the page itself, so the list's own GPUI scrolling
/// never runs with it (both would move at once). Without a page handle
/// (an area inside the kit's own scroll area, which takes the wheel
/// first), the turn is left to it.
///
/// `view` is the view to notify (`cx.entity_id()` in its render); it is
/// passed in rather than read from GPUI's rendered-view stack
/// (`view/frame.rs`).
pub(crate) fn wheel_first<H>(
    id: &'static str,
    handle: &H,
    page: Option<&gpui_kit::ScrollHandle>,
    view: gpui_kit::EntityId,
) -> impl IntoElement
where
    H: gpui_kit::base::ScrollbarHandle + Clone,
{
    let handle = handle.clone();
    let page = page.cloned();
    gpui_kit::canvas(
        |bounds, window, _| window.insert_hitbox(bounds, gpui_kit::HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            window.on_mouse_event(move |e: &gpui_kit::ScrollWheelEvent, phase, window, cx| {
                if phase != gpui_kit::DispatchPhase::Capture || !hitbox.should_handle_scroll(window)
                {
                    return;
                }
                let d = e.delta.pixel_delta(window.line_height());
                let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                if dx.abs() > dy.abs() {
                    return;
                }
                let now = Instant::now();
                let (offset, max) = scroll_span(&handle);
                let can = crate::scrollbar::step(offset, dy, max).is_some();
                let route = LATCH.with(|l| {
                    let mut v = l.get();
                    let r = v.over_list(id, can, now);
                    l.set(v);
                    r
                });
                match route {
                    crate::scrollbar::Route::List => {
                        scroll_by(&handle, dy);
                    }
                    crate::scrollbar::Route::Hold => {}
                    crate::scrollbar::Route::Page => match &page {
                        Some(page) => {
                            scroll_by(page, dy);
                            LATCH.with(|l| {
                                let mut v = l.get();
                                v.page_took(now);
                                l.set(v);
                            });
                        }
                        None => return,
                    },
                }
                cx.stop_propagation();
                cx.notify(view);
            });
        },
    )
    .absolute()
    .inset_0()
}

/// The page's part of the wheel latch (Settings): put it after the page's
/// scrolling div, as an absolute child of the box around it, with the
/// page's scroll handle. It scrolls nothing; GPUI's own scrolling moves
/// the page, exactly as it moves the chat. It only notes that the page
/// took the turn, so a list the page slides under the pointer waits for
/// the next gesture (`wheel_first`).
pub(crate) fn page_latch(handle: &gpui_kit::ScrollHandle) -> impl IntoElement {
    let handle = handle.clone();
    gpui_kit::canvas(
        |bounds, window, _| window.insert_hitbox(bounds, gpui_kit::HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            window.on_mouse_event(move |e: &gpui_kit::ScrollWheelEvent, phase, window, _| {
                if phase != gpui_kit::DispatchPhase::Bubble || !hitbox.should_handle_scroll(window)
                {
                    return;
                }
                let d = e.delta.pixel_delta(window.line_height());
                let (offset, max) = scroll_span(&handle);
                if f32::from(d.x).abs() > f32::from(d.y).abs()
                    || crate::scrollbar::step(offset, f32::from(d.y), max).is_none()
                {
                    return;
                }
                LATCH.with(|l| {
                    let mut v = l.get();
                    v.page_took(Instant::now());
                    l.set(v);
                });
            });
        },
    )
    .absolute()
    .inset_0()
}

/// The × on a composer chip: always drawn (not only on hover), with a
/// hover background so it reads as a button. Clicks never reach the chip
/// or the composer under it.
fn chip_remove(
    id: impl Into<gpui_kit::ElementId>,
    tip: &'static str,
    p: Palette,
    on: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(20.))
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .text_color(p.muted)
        .hover(|d| d.bg(p.hover).text_color(p.text))
        .child("×")
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
        })
        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
            cx.stop_propagation()
        })
        .on_click(move |e, window, cx| {
            cx.stop_propagation();
            on(e, window, cx)
        })
}

/// The language a route's windows are transcribed in, for the dictation
/// guard (#71).
fn route_language(route: &crate::dictation::Route) -> Option<String> {
    match route {
        crate::dictation::Route::Whisper(l) => l.clone(),
        crate::dictation::Route::Fast => Some("en".into()),
        crate::dictation::Route::Detect => None,
    }
}
