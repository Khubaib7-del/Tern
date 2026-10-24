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
    SharedString, Subscription, Task, Transformation, Window, div, point, px, radians, size, svg,
};
use tern_agent::Role;
use tern_agent::attach::Source as AttachSource;
use tern_agent::{
    ApprovalAnswer, CancelFlag, ChatSession, Harness, PageRef, RawFilter, Run, RunEnd, RunStep,
    StepKind, Verdict,
};
use tern_core::{SecretString, SessionId};
use tokio::sync::{mpsc, oneshot};

use crate::assets;
use crate::bridge::{Bridge, Msg};
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
mod git_view;
mod library_view;
mod onboarding_view;
mod panels_view;
mod runs_view;
mod scout_view;
mod segmented;
mod settings_view;
mod setup_view;
mod study_view;
mod timeline_view;
mod toast_view;

use crate::layout::{Layout, Tab, Tabs};
use crate::model::{
    Activity, Connection, Entry, Model, Phase, Pick, RunView, Screen, SiteIcon, WorkingLine,
};
use crate::motion::{self, Slide, ThemeWipe};
use crate::providers::{Kind, Models, Note, ServiceRow};
use crate::theme::Palette;

pub(crate) const RAIL_W: f32 = 52.;
pub(crate) const COLUMN_MAX_W: f32 = 760.;
/// The toast when the mic is clicked and no provider lists a voice model.
pub const NO_VOICE_MODEL: &str = "No voice model available — add Groq, OpenAI or Mistral";

/// How much of an event's payload the Raw view shows when opened.
const RAW_PAYLOAD_CHARS: usize = 4_000;

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
    /// The title field of the chat being renamed in the list.
    rename_input: Entity<InputState>,
    renaming: Option<SessionId>,
    cancel: CancelFlag,
    /// Answers whose source list is expanded, by transcript index.
    open_sources: HashSet<usize>,
    pending: Option<oneshot::Sender<ApprovalAnswer>>,
    /// The waiting deep research plan's answer channel.
    plan_reply: Option<oneshot::Sender<tern_agent::PlanAnswer>>,
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
    /// Scout's rig, when it was last stepped, the last beat it saw, the
    /// wake-up for its next blink, and a mood picked on Appearance.
    scout: crate::scout::Rig,
    scout_at: Option<Instant>,
    scout_beat: usize,
    scout_wake: Option<Task<()>>,
    scout_preview: Option<crate::scout::Mood>,
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
    next_explain: u32,
    /// When the explainer's tilt last eased.
    explain_at: Option<Instant>,
    /// Steps explainers on autoplay.
    explain_ticker: Option<Task<()>>,
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
    /// When the reveal ticker last ran.
    reveal_at: Option<Instant>,
    /// The chat column's width at the last render, for the tab strip.
    centre_w: f32,
    /// The right pane's width at the last render, for its header.
    side_w: f32,
    /// The run panel's ⋯ menu of views that do not fit is open.
    run_more_open: bool,
    /// Sliding highlights and knobs, by control, and whether one moved
    /// this frame (so the frame after is drawn too).
    glides: std::cell::RefCell<std::collections::HashMap<&'static str, crate::motion::Glide>>,
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
    /// A model picked while the chat was busy, applied when it is free.
    pending_pick: Option<(&'static str, String)>,
    /// Streaming dictation while the mic records (and until its last
    /// window comes back): the state, the voice model, the speech so far
    /// (16 kHz mono), how far into the recording it has read, and the
    /// timer that sends windows.
    dictation: Option<crate::dictation::Dictation>,
    dictation_model: Option<tern_agent::media::ModelRef>,
    dictation_samples: Vec<f32>,
    dictation_pos: usize,
    dictation_task: Option<Task<()>>,
    /// Wakes the window when a toast should come, go, or fade.
    toast_wake: Option<Task<()>>,
    /// The timeline tick under the pointer, and a smooth scroll in flight
    /// (from, to, started).
    rail_hover: Option<usize>,
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
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl Workspace {
    pub fn new(
        bridge: Bridge,
        mut rx: mpsc::UnboundedReceiver<Msg>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
        // Reopen the last chat that has messages; a new draft otherwise.
        bridge.launch(tern_agent::Harness::Research, None);
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
        }
        let (sidebar_open, panel_open) = (model.sidebar_open, model.run_panel_open);
        Self {
            sidebar_slide: Slide::new(sidebar_open),
            panel_slide: Slide::new(panel_open),
            wipe: None,
            scout: crate::scout::Rig::default(),
            scout_at: None,
            scout_beat: 0,
            scout_wake: None,
            scout_preview: None,
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
            rename_input,
            renaming: None,
            cancel: CancelFlag::default(),
            open_sources: HashSet::new(),
            pending: None,
            plan_reply: None,
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
            next_explain: 0,
            explain_at: None,
            explain_ticker: None,
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
            reveal_at: None,
            centre_w: 800.,
            side_w: crate::panels::RIGHT_DEFAULT,
            run_more_open: false,
            glides: Default::default(),
            anim_pending: Default::default(),
            effort_track: Default::default(),
            effort_drag: None,
            ctx_opened: Default::default(),
            tabs_menu_open: false,
            pending_pick: None,
            dictation: None,
            dictation_model: None,
            dictation_samples: Vec::new(),
            dictation_pos: 0,
            dictation_task: None,
            toast_wake: None,
            rail_hover: None,
            rail_scroll: None,
            resume_retry: None,
            rail_base: Default::default(),
            settings_inputs,
            setup_inputs,
            onboarding: None,
            onboarding_checked: false,
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
        }
    }

    fn on_msg(&mut self, msg: Msg, window: &mut Window, cx: &mut Context<Self>) {
        match msg {
            Msg::Connected(s, recent) => {
                self.model.connected(Connection {
                    provider: s.provider_name().to_owned(),
                    model: s.model().to_owned(),
                    search: s.search_name().map(str::to_owned),
                    context_window: s.context_window(),
                });
                self.model.sessions = recent;
                self.model.mcp_offers = s.mcp_offers().to_vec();
                if !self.model.customize.loaded {
                    // Skills for the `/` menu.
                    self.bridge.ext_load();
                }
                self.current = Some(s.session_id());
                self.tabs.set_active(s.session_id(), "New chat");
                self.model.set_resume_offer(None, None);
                self.model.switching_to = None;
                self.resume_retry = None;
                self.chat_shown(&s);
                self.session = Some(s);
                self.composer.update(cx, |c, cx| c.focus(window, cx));
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
                    self.request_icons();
                    self.scroll.scroll_to_bottom();
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
            Msg::Dictated(seq, result) => self.dictated(seq, result, window, cx),
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
                self.tabs.set_active(s.session_id(), &title);
                self.model.sessions = recent;
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
            }
            Msg::Cont(m) => self.on_cont(*m, window, cx),
            Msg::Ext(m) => match *m {
                crate::bridge::ExtMsg::Prompt(r) => self.prompt_ready(*r, window, cx),
                other => self.on_ext(other, cx),
            },
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
                self.model.sessions = recent;
            }
            Msg::ConnectFailed(e) => self.model.connect_failed(e),
            Msg::Compacted(s, result, usage) => {
                self.model.compacting = false;
                self.model.context = usage;
                match result {
                    Ok(Some(_)) => self.model.show_compaction_notice(),
                    Ok(None) => self
                        .model
                        .toast_info("Nothing to compact yet: the latest turns stay word for word."),
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
            }
            Msg::Icon(site, path) => self.model.icon_loaded(site, path),
            Msg::Attached(result) => self.model.attached(result),
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
                self.model.sessions = recent;
                if !s.is_draft() {
                    self.bridge.remember(s.harness(), s.session_id());
                    self.model.composer.remember(&s.session_id().to_string());
                }
                self.session = Some(s);
                if self.model.reveal.is_none() {
                    self.ticker = None;
                }
                self.scroll.scroll_to_bottom();
                self.apply_pending_pick();
                // A message queued with Tab goes now.
                if let Some(q) = self.queued.take() {
                    self.composer.update(cx, |c, cx| c.set_value(q, window, cx));
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
            Msg::Transcribed(result) => {
                self.model.transcribing = false;
                match result {
                    Ok(text) => {
                        let now = self.composer.read(cx).value().to_string();
                        let joined = if now.trim().is_empty() {
                            text
                        } else {
                            format!("{} {text}", now.trim_end())
                        };
                        self.composer
                            .update(cx, |c, cx| c.set_value(joined, window, cx));
                    }
                    Err(e) => self
                        .model
                        .toast_error(format!("Voice input failed: {e}"), None),
                }
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
        if self.session.is_none() {
            return;
        }
        let text = self.composer.read(cx).value().to_string();
        let Some(text) = self.model.start_turn(&text) else {
            return;
        };
        let Some(session) = self.session.take() else {
            return;
        };
        // Asked from the Study tools: the answer shows in the Study chat.
        if self.model.screen == Screen::Study {
            self.model.screen = Screen::Chat;
        }
        let mut session = session;
        let mode = self.model.composer.mode;
        session.set_turn_mode(mode.turn_mode());
        session.set_effort(self.model.composer.effort);
        // A Study mode asks the same model to answer in its style.
        let text = match mode.study_prefix() {
            Some(prefix) => format!("{prefix}\n\n{text}"),
            None => text,
        };
        let images = std::mem::take(&mut self.model.outgoing_images);
        let mut read_first = None;
        if !images.is_empty() {
            let provider = self.provider_id();
            let chat_model = self
                .model
                .connection
                .as_ref()
                .map(|c| c.model.clone())
                .unwrap_or_default();
            match tern_agent::media::plan_images(
                &self.model.catalog,
                &provider,
                &chat_model,
                &self.model.providers.prefs,
            ) {
                tern_agent::media::ImagePlan::Send => session.attach_images(images),
                tern_agent::media::ImagePlan::Read { via, job } => {
                    read_first = Some(crate::bridge::ReadFirst {
                        via,
                        job,
                        images,
                        chat_model,
                    })
                }
                tern_agent::media::ImagePlan::Missing(why) => self.model.notice_line(why),
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
                .send(session, text, self.cancel.clone(), read_first);
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
            c.set_value(tern_agent::CONTINUE_PROMPT, window, cx)
        });
        self.submit(window, cx);
        self.model.deep_mode = deep;
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
                c.set_value(chosen.text, window, cx);
                c.focus(window, cx);
            });
            if chosen.browse {
                self.pick_attachments(cx);
            }
            if let Some((server, uri)) = chosen.fetch {
                self.bridge.mcp_resource(server, uri);
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
            "up" => self.model.move_mention(-1),
            "down" => self.model.move_mention(1),
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
                        .when(i == selected, |d| d.bg(p.hover))
                        .hover(move |d| d.bg(p.hover))
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

    /// Opens the system picker for files and folders (zips are files).
    fn pick_attachments(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: true,
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
        // Audio files go to the speech-to-text model; the transcript joins
        // the composer like spoken words.
        let (audio, paths): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| {
            p.file_name()
                .is_some_and(|n| tern_agent::media::is_audio_name(&n.to_string_lossy()))
        });
        for a in audio {
            match self.stt_model() {
                Some(via) => {
                    self.model.transcribing = true;
                    self.bridge.transcribe_file(via, a);
                }
                None => self.model.toast_error(
                    format!(
                        "{} is audio. Tern transcribes audio once a provider that lists a speech-to-text model is set up (Groq or OpenAI Whisper, Mistral Voxtral).",
                        a.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                    ),
                    None,
                ),
            }
        }
        if paths.is_empty() {
            cx.notify();
            return;
        }
        self.model.attaching += paths.len();
        self.bridge.attach(paths);
        cx.notify();
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
                .child(
                    svg()
                        .path(assets::service_path(service))
                        .size(px(size * 0.85))
                        .text_color(p.text),
                )
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
    fn open_chat(&mut self, id: SessionId, cx: &mut Context<Self>) {
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
        if self.model.is_busy() || self.current == Some(id) {
            return;
        }
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
        if self.model.is_busy() {
            return;
        }
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
        if self.model.is_busy() {
            return;
        }
        self.tabs.add();
        self.show_tab(self.tabs.active_tab().clone(), window, cx);
    }

    fn focus_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let from_explain = self.tabs.active_tab().is_explain();
        let to_explain = self.tabs.list.get(i).is_some_and(Tab::is_explain);
        // Explainers never interrupt a run; moving between them and the
        // chat on screen is always allowed.
        if self.model.is_busy() && !from_explain && !to_explain {
            return;
        }
        if let Some(tab) = self.tabs.focus(i).cloned() {
            if from_explain && tab.id.is_none() && !tab.is_explain() && self.current.is_none() {
                // Back to the fresh chat that was on screen.
                cx.notify();
                return;
            }
            if self.model.is_busy() && !tab.is_explain() && tab.id != self.current {
                return;
            }
            self.show_tab(tab, window, cx);
        }
    }

    fn close_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let closing = self.tabs.list.get(i).cloned();
        let was_explain = self.tabs.active_tab().is_explain();
        if self.model.is_busy() && i == self.tabs.active && !was_explain {
            return;
        }
        if let Some(key) = closing.as_ref().and_then(|t| t.explain) {
            self.explains.remove(&key);
        }
        if let Some(tab) = self.tabs.close(i).cloned() {
            if was_explain && tab.id == self.current && !tab.is_explain() {
                cx.notify();
                return;
            }
            self.show_tab(tab, window, cx);
        }
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
            Some(id) if self.current != Some(id) => self.load_chat(id, cx),
            Some(_) => {}
            None => {
                self.current = None;
                self.session = None;
                self.model.entries.clear();
                self.new_chat(window, cx);
            }
        }
        cx.notify();
    }

    /// Loads a past chat into the window.
    fn load_chat(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.session = None;
        self.current = Some(id);
        self.open_sources.clear();
        self.model.new_chat();
        self.model.phase = Phase::Connecting;
        self.model.screen = Screen::Chat;
        self.bridge.resume(id);
        cx.notify();
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
        self.bridge.archive(id);
        cx.notify();
    }

    fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.is_busy() {
            return;
        }
        let had_messages = !self.model.entries.is_empty();
        self.model.projects.new_in = None;
        self.model.new_chat();
        self.open_sources.clear();
        if had_messages || self.session.is_none() {
            // A fresh session row, so the old chat keeps its own history.
            self.tabs.reset_active();
            self.session = None;
            self.model.phase = Phase::Connecting;
            self.bridge.connect();
        }
        self.composer.update(cx, |c, cx| {
            c.set_value("", window, cx);
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

    /// The speech-to-text model the mic uses: the saved choice, else the
    /// best listed one (the chat's provider first).
    fn stt_model(&self) -> Option<tern_agent::media::ModelRef> {
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
        let Some(via) = self.stt_model() else {
            self.model.toast_error(NO_VOICE_MODEL, None);
            cx.notify();
            return;
        };
        match tern_agent::platform::mic::Recording::start() {
            Ok(r) => {
                self.recording = Some(r);
                self.dictation = Some(crate::dictation::Dictation::new());
                self.dictation_model = Some(via);
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

    /// Every 400 ms while dictating: read new speech, send the next window
    /// when it is due, and end when the last one is back.
    fn start_dictation_timer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dictation_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
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
        let Some(d) = self.dictation.as_mut() else {
            return false;
        };
        if let Some(req) = d.next_request(&self.dictation_samples)
            && let Some(via) = self.dictation_model.clone()
        {
            let end = req.end.min(self.dictation_samples.len());
            let start = req.start.min(end);
            let wav = tern_agent::platform::audio::wav(
                &self.dictation_samples[start..end],
                tern_agent::platform::audio::SPEECH_RATE,
            );
            self.bridge.dictate(via, req.seq, wav);
        }
        self.commit_dictation(window, cx);
        let finished = self
            .dictation
            .as_ref()
            .is_some_and(|d| d.done(self.dictation_samples.len()));
        if finished {
            if let Some(e) = self.dictation.as_mut().and_then(|d| d.error.take()) {
                self.model
                    .toast_error(format!("Some speech could not be transcribed: {e}"), None);
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
        let now = self.composer.read(cx).value().to_string();
        let joined = if now.trim().is_empty() {
            text
        } else {
            format!("{} {text}", now.trim_end())
        };
        self.model.on_composer_text(&joined);
        self.composer
            .update(cx, |c, cx| c.set_value(joined, window, cx));
        cx.notify();
    }

    /// A window of dictation came back.
    fn dictated(
        &mut self,
        seq: u64,
        result: Result<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(d) = self.dictation.as_mut() {
            d.on_result(seq, result);
        }
        self.pump_dictation(window, cx);
    }

    /// The open phrase's interim text, grey with a trailing ellipsis.
    fn dictation_line(&self) -> Option<String> {
        self.dictation.as_ref().and_then(|d| d.interim_line())
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
        let harness = self.model.harness;
        let hs = [Harness::Research, Harness::Study];
        let segs: Vec<segmented::Segment> = hs
            .iter()
            .map(|&h| segmented::Segment {
                label: if h == Harness::Study {
                    "Study"
                } else {
                    "Research"
                }
                .into(),
                glyph: assets::glyph(if h == Harness::Study {
                    "study"
                } else {
                    "research"
                }),
            })
            .collect();
        let switch = self.segmented(
            "harness",
            &segs,
            hs.iter().position(|&h| h == harness).unwrap_or(0),
            segmented::Look {
                framed: true,
                fill: false,
                small: true,
            },
            p,
            std::rc::Rc::new(move |this: &mut Self, i, _, cx| {
                this.choose_harness(hs[i], cx);
            }),
            cx,
        );
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
                        .children(self.git_chip(p, cx)),
                )
                .child(div().occlude().child(switch))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .occlude()
                        .child(
                            Button::new("toggle-theme")
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
        let rule = || {
            div()
                .w(px(18.))
                .h(px(1.))
                .my(px(6.))
                .bg(p.border)
                .flex_none()
        };
        div()
            .w(px(RAIL_W))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(crate::layout::RAIL_GAP))
            .py_2()
            .bg(p.rail)
            .border_r_1()
            .border_color(p.border)
            .child(
                Button::new("rail-new")
                    .icon(IconName::Plus)
                    .ghost()
                    .tooltip("New chat")
                    .on_click(cx.listener(|this, _, window, cx| this.new_chat(window, cx))),
            )
            .child(
                Button::new("rail-chats")
                    .icon(IconName::Inbox)
                    .ghost()
                    .tooltip("Chats")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.sidebar_open = !this.model.sidebar_open;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("rail-library")
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
                    })),
            )
            .child(rule())
            .child(
                Button::new("rail-editor")
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
                    })),
            )
            .child(
                Button::new("rail-browser")
                    .icon(IconName::Globe)
                    .ghost()
                    .when(
                        self.model.browser.visible && self.model.run_panel_open,
                        |b| b.selected(true),
                    )
                    .tooltip("Browser")
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_browser(window, cx))),
            )
            .child(
                Button::new("rail-git")
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
                    })),
            )
            .child(div().flex_1())
            .child(
                Button::new("rail-settings")
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
                    })),
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
            .child(svg().path(assets::MARK).size(px(15.)).text_color(p.text))
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
        let busy = self.model.is_busy();
        let s = &self.model.sessions[ix];
        let current = Some(s.id) == self.current;
        let id = s.id;
        let sub = s.parent.is_some();
        let planned = s.plan.is_some();
        let running = self.model.cont.background.contains(&id);
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
            .when(!current && !busy, |d| {
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
                    .child(SharedString::from(s.title.clone())),
            )
            .when(running, |d| d.child(Spinner::new().xsmall()))
            .when(planned && !running, |d| {
                d.child(
                    Button::new(("run-sub", ix))
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
        self.with_row_menu(row, ix, id, sub, p, cx)
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
            .bg(p.sidebar)
            .border_r_1()
            .border_color(p.border)
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
                    .when(self.model.sessions.is_empty(), |d| {
                        d.child(
                            div()
                                .px_2()
                                .text_sm()
                                .text_color(p.faint)
                                .child("Your chats will show here."),
                        )
                    })
                    .children(rows),
            )
            .children(self.watches_section(p, cx))
            .child(self.archived_link(p, cx))
            .child(self.scout_corner(p))
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
                            .max_w(px(COLUMN_MAX_W * 0.8))
                            .pl_2()
                            .border_l_2()
                            .border_color(p.border)
                            .text_xs()
                            .text_color(p.muted)
                            .line_clamp(2)
                            .child(SharedString::from(q)),
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
                            .children(chips.iter().map(|c| sent_chip(c, p))),
                    )
                })
                .child(
                    div()
                        .max_w(px(COLUMN_MAX_W * 0.8))
                        .px_3()
                        .py_2()
                        .rounded_xl()
                        .bg(p.raised)
                        .child(SharedString::from(text.clone())),
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
                        .child(
                            TextView::markdown(("answer", i), with_badges(i, shown, run))
                                .on_link_click(self.link_opener(cx))
                                .selectable(true)
                                .motion(stream_motion()),
                        )
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
                    .child(body)
                    .when(revealing && !folded, |d| {
                        d.child(composer_view::show_all(p, cx))
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
            Entry::Notice(t) => div()
                .text_sm()
                .text_color(p.muted)
                .child(SharedString::from(t.clone()))
                .into_any_element(),
        }
    }

    /// Opens a new chat that starts with this chat's turns up to entry `i`.
    fn branch_from(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.model.is_busy() {
            return;
        }
        let Some(from) = self.current else {
            return;
        };
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

    fn approval_card(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Phase::Approval { request, .. } = &self.model.phase else {
            return None;
        };
        let target = request.target.clone().unwrap_or_default();
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded_xl()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("Tern wants to use {}", request.tool)),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(p.muted)
                        .child(SharedString::from(request.summary.clone())),
                )
                .when(!target.is_empty(), |d| {
                    d.child(div().text_xs().text_color(p.faint).child(target))
                })
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("allow-once")
                                .label("Allow once")
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.answer(ApprovalAnswer::AllowOnce, cx)
                                })),
                        )
                        .child(
                            Button::new("allow-session")
                                .label("Allow for this chat")
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.answer(ApprovalAnswer::AllowForSession, cx)
                                })),
                        )
                        .child(Button::new("deny").label("Deny").ghost().small().on_click(
                            cx.listener(|this, _, _, cx| this.answer(ApprovalAnswer::Deny, cx)),
                        )),
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
                "What are we studying?",
                "Ask Tern to explain, quiz, or test you. This chat is kept apart from Research."
                    .into(),
            ),
            _ => (
                "What are we researching?",
                match self.model.connection.as_ref().and_then(|c| c.search.as_deref()) {
                    Some(s) => format!("Ask a question. Tern searches with {s}, reads, and cites."),
                    None => "Ask a question. Web search is off, so answers come from the model alone."
                        .into(),
                },
            ),
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .pt(px(140.))
            .child(
                div()
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
            .when(offline, |d| {
                d.child(
                    div().pt_2().child(
                        Button::new("open-providers")
                            .label("Set up a provider")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.open_providers(cx))),
                    ),
                )
            })
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
        if let Some(chip) = self.workspace_chip(p, cx) {
            items.push(wrap(chip));
        }
        if self.model.entries.is_empty() && !self.model.is_busy() {
            items.push(wrap(self.empty_state(p, cx)));
        }
        self.rail_base.set(items.len());
        for (i, e) in self.model.entries.iter().enumerate() {
            items.push(wrap(self.entry(i, e, p, cx)));
        }
        // Thinking, the working rollup, and the answer so far, revealed at
        // reading pace.
        if let Some(live) = self.live_answer(p, cx) {
            items.push(wrap(live));
        }
        if let Some(card) = self.approval_card(p, cx) {
            items.push(wrap(card));
        }
        if let Some(card) = self.plan_card(p, cx) {
            items.push(wrap(card));
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
            .vertical_scrollbar(&self.scroll)
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
                .child(if self.model.attaching == 1 {
                    "Reading 1 item".to_owned()
                } else {
                    format!("Reading {} items", self.model.attaching)
                })
        });
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(self.model.attachments.iter().enumerate().map(|(i, a)| {
                let icon = match a.source {
                    AttachSource::File if a.is_image() => AssetIcon::FileImage,
                    AttachSource::File => AssetIcon::FileText,
                    AttachSource::Folder => AssetIcon::Folder,
                    AttachSource::Archive => AssetIcon::FileArchive,
                    AttachSource::Paste => AssetIcon::ClipboardPaste,
                    AttachSource::Link => AssetIcon::Link,
                    AttachSource::Mcp => AssetIcon::Blocks,
                };
                let prompt = a.role == Role::Prompt;
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl_2()
                    .pr_1()
                    .py_1()
                    .max_w(px(280.))
                    .rounded_lg()
                    .border_1()
                    .border_color(if prompt { p.muted } else { p.border })
                    .bg(p.canvas)
                    .child(
                        div()
                            .text_color(p.muted)
                            .child(gpui_kit::component::Icon::new(icon).small()),
                    )
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
                                    .child(SharedString::from(a.name.clone())),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(a.detail())),
                            ),
                    )
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
                    .child(
                        Button::new(("attach-remove", i))
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .tooltip("Remove")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model.remove_attachment(i);
                                cx.notify();
                            })),
                    )
            }))
            .children(loading)
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
                "Voice input needs a speech-to-text model (Groq or OpenAI Whisper, Mistral Voxtral)".into(),
                true,
            ),
            (Some(v), false) if self.model.transcribing => (
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
                    window.request_animation_frame();
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
                                Button::new("context-compact")
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
                                Button::new("context-handoff")
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
            .map(|r| self.service_row(r, p, cx))
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
        if working {
            actions = actions.child(Spinner::new().small().color(p.muted));
        } else if r.needs_key() && !r.key_saved {
            if !editing {
                actions = actions.child(Button::new(bid("add")).label("Add").small().on_click(
                    cx.listener(move |this, _, window, cx| this.edit_key(kind, id, window, cx)),
                ));
            }
        } else {
            let can_choose = st.can_choose(&r);
            if !is_default {
                actions = actions.child(
                    Button::new(bid("use"))
                        .label("Use")
                        .small()
                        .disabled(busy_turn || !can_choose)
                        .tooltip(if can_choose {
                            "Use this one for new answers in this chat"
                        } else {
                            "Test the server address first"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.apply_choice(kind, id);
                            cx.notify();
                        })),
                );
            }
            actions = actions.child(
                Button::new(bid("test"))
                    .label("Test")
                    .ghost()
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
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
                    })),
            );
            if kind == Kind::Model {
                actions = actions.child(
                    Button::new(bid("address"))
                        .label("Address")
                        .ghost()
                        .small()
                        .tooltip("The server address, for a local server elsewhere or a proxy")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.edit_base_url(id, window, cx)
                        })),
                );
            }
            if kind == Kind::Model && tern_agent::providers::custom::is_custom(id) {
                // Asks twice; the armed button is this provider's own.
                let confirm = self.model.settings.confirm == Some(id);
                actions = actions.child(
                    Button::new(bid("delete"))
                        .label(if confirm { "Click again to delete" } else { "Delete" })
                        .ghost()
                        .small()
                        .disabled(busy_turn && is_default)
                        .tooltip("Delete this custom provider and its key")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.model.settings.confirm(id) {
                                this.model.providers.begin(kind, id, "Deleting…");
                                this.bridge.custom_remove(id.to_owned());
                            }
                            cx.notify();
                        })),
                );
            } else if r.key_saved {
                actions = actions.child(
                    Button::new(bid("remove"))
                        .label("Remove")
                        .ghost()
                        .small()
                        .disabled(busy_turn && is_default)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.providers.begin(kind, id, "Removing…");
                            this.bridge.remove(kind, id);
                            cx.notify();
                        })),
                );
            }
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
                    .child(logo_tile(&r, p))
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
                            .label("Save")
                            .small()
                            .disabled(working)
                            .on_click(cx.listener(|this, _, window, cx| this.save_key(window, cx))),
                    )
                    .when(!key_url.is_empty(), |d| {
                        d.child(
                            Button::new(bid("get"))
                                .label("Get a key")
                                .ghost()
                                .small()
                                .on_click(move |_, _, cx| cx.open_url(key_url)),
                        )
                    })
                    .child(
                        Button::new(bid("cancel"))
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
        if kind == Kind::Model && is_default {
            row = row.child(self.provider_model_list(p, cx));
        }
        row.into_any_element()
    }

    /// The run panel: one run in three views (Summary, Detailed, Raw).
    fn run_panel(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let run = self.model.panel_run();
        let view = self.model.run_panel.view;
        // The views as a sliding segmented control sized to the pane: what
        // does not fit goes in a ⋯ menu, so nothing runs past the edge.
        let views = RunView::for_run(run);
        let labels: Vec<&str> = views.iter().map(|v| v.label()).collect();
        let active = views.iter().position(|&v| v == view).unwrap_or(0);
        let (show_label, shown, hidden) =
            crate::panels::run_header_fit(&labels, self.side_w, active);
        let segs: Vec<segmented::Segment> = shown
            .iter()
            .map(|&i| segmented::Segment::text(labels[i]))
            .collect();
        let picked = shown.iter().position(|&i| i == active).unwrap_or(0);
        let shown_views: Vec<RunView> = shown.iter().map(|&i| views[i]).collect();
        let tabs = self.segmented(
            "run-views",
            &segs,
            picked,
            segmented::Look::default(),
            p,
            std::rc::Rc::new(move |this: &mut Self, i, _, _| {
                if let Some(&v) = shown_views.get(i) {
                    this.model.set_run_view(v);
                    this.run_more_open = false;
                }
            }),
            cx,
        );
        let more = (!hidden.is_empty()).then(|| {
            let hidden_views: Vec<RunView> = hidden.iter().map(|&i| views[i]).collect();
            div()
                .relative()
                .flex_none()
                .child(
                    Button::new("run-views-more")
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
                                .children(hidden_views.into_iter().map(|v| {
                                    div()
                                        .id(SharedString::from(format!(
                                            "run-view-more-{}",
                                            v.label()
                                        )))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .cursor_pointer()
                                        .text_color(p.muted)
                                        .hover(|d| d.bg(p.hover).text_color(p.text))
                                        .child(v.label())
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
                    .children(more),
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
        let body: AnyElement = if run.is_empty() {
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
            .children(pinned)
            .children(status)
            .child(
                div()
                    .id("run-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(body),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(self.model.context_label()),
            )
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
        let pages = run.pages();
        let focus = self.model.run_panel.claim;
        let claims = run.checks.iter().map(|c| {
            let focused = focus == Some(c.claim);
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
                        .text_color(p.text)
                        .child(SharedString::from(c.text.clone())),
                )
                .children(source)
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
                                .label("Open report")
                                .ghost()
                                .xsmall()
                                .tooltip("Open the Markdown report")
                                .on_click(move |_, _, cx| cx.open_url(&md)),
                        )
                        .child(
                            Button::new("report-open-json")
                                .label("JSON")
                                .ghost()
                                .xsmall()
                                .tooltip("Open the report as JSON")
                                .on_click(move |_, _, cx| cx.open_url(&json)),
                        ),
                )
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
            .when(!run.checks.is_empty(), |d| d.child(counts))
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
        self.tick_scout(window, cx);
        self.tick_explain(window);
        self.tick_toasts(window, cx);
        self.tick_rail_scroll(window);
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
            window.request_animation_frame();
        }
        let layout = Layout::for_width(f32::from(window.viewport_size().width));
        if Layout::sidebar_squeezed(self.last_layout, layout) {
            self.model.sidebar_open = false;
        }
        if self.last_layout.is_some_and(|l| l.panel_beside) && !layout.panel_beside {
            self.model.run_panel_open = false;
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
        // Settings has its own section list: the chat list slides away
        // while it shows and comes back after.
        let sidebar = self.sidebar_slide.frame(
            self.model.sidebar_open && self.model.screen != Screen::Settings,
            now,
            reduced,
        );
        let panel = self
            .panel_slide
            .frame(self.model.run_panel_open, now, reduced);
        if self.wipe.is_some() || self.sidebar_slide.moving() || self.panel_slide.moving() {
            window.request_animation_frame();
        }
        // The theme change: the new theme is drawn whole and the old one's
        // canvas parts from the centre out (v2 s17), no ripple.
        let ripple = self.wipe.map(|w| {
            let inset = w.inset(now);
            let width = f32::from(window.viewport_size().width);
            let cover = width * inset;
            let old = w.old_canvas();
            div()
                .absolute()
                .inset_0()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .w(px(cover))
                        .bg(old),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w(px(cover))
                        .bg(old),
                )
        });
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
            browser_view::BROWSER_W.max(right_w)
        } else if side_docked {
            continuity_view::SIDE_W.max(right_w)
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
                if !this.settings_keys(e, window, cx) {
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
                                Screen::Chat => match self.active_explain() {
                                    Some(key) => d
                                        .children(self.tab_strip(p, cx))
                                        .child(self.explain_view(key, p, cx)),
                                    None => d
                                        .children(self.tab_strip(p, cx))
                                        .children(self.study_chat_header(p, cx))
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
                                        .child(self.composer(p, cx)),
                                },
                                Screen::Settings => d.child(self.settings_screen(p, window, cx)),
                                Screen::Editor => d.child(self.editor_screen(p, cx)),
                                Screen::Git => d.child(self.git_screen(p, cx)),
                                Screen::Watches => d.child(self.watches_screen(p, cx)),
                                Screen::Study => d
                                    .child(self.study_screen(p, window, cx))
                                    // Asking from the tools goes to the Study chat.
                                    .when(
                                        self.model.study.view == crate::study::StudyView::Home,
                                        |d| d.child(self.composer(p, cx)),
                                    ),
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
            .children(ripple);
        // A sliding highlight or knob still moving draws the next frame.
        if self.anim_pending.take() {
            window.request_animation_frame();
        }
        root
    }
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

/// The ink for a one-colour brand mark: white on a dark theme, near
/// black on a light one, at full strength so the mark never reads grey.
pub(crate) fn mono_ink(p: Palette) -> gpui_kit::Hsla {
    if p.canvas.l < 0.5 {
        gpui_kit::hsla(0., 0., 1., 1.)
    } else {
        gpui_kit::hsla(0., 0., 0.07, 1.)
    }
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
fn sent_chip(c: &tern_agent::Chip, p: Palette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py(px(2.))
        .rounded_md()
        .border_1()
        .border_color(p.border)
        .text_xs()
        .text_color(p.muted)
        .child(gpui_kit::component::Icon::new(AssetIcon::Paperclip).xsmall())
        .child(SharedString::from(c.name.clone()))
        .when(c.role == Role::Prompt, |d| {
            d.child(div().text_color(p.text).child("· Prompt"))
        })
        .into_any_element()
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
fn with_badges(entry: usize, text: &str, run: &Run) -> String {
    tern_agent::verify::annotate(text, &run.checks, |claim| {
        tern_agent::verify::claim_link(entry, claim)
    })
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
    let mark_color = match line.activity {
        Activity::NeedsYou => p.text,
        Activity::Stopping => p.faint,
        _ => p.muted,
    };
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
        .child(
            div()
                .size(px(16.))
                .flex_none()
                .child(mark(line, mark_color, still, at)),
        )
        .child(phrase)
        .child(
            div()
                .text_color(p.faint)
                .child(format!("{}s", line.seconds)),
        )
}

/// The tern mark, animated by what the turn is doing. Continuous motion
/// (the glide, the wingbeats) runs only while that state lasts; the
/// one-off beats restart on each real event through the beat counter.
fn mark(line: &WorkingLine, color: Hsla, still: bool, at: &'static str) -> AnyElement {
    use std::f32::consts::PI;
    let base = svg().path(assets::MARK).size(px(16.)).text_color(color);
    if still {
        return base.into_any_element();
    }
    match line.activity {
        // Slow glide: a 2px bob every 2 s.
        Activity::Thinking => base
            .with_animation(
                (at, 1usize),
                Animation::new(Duration::from_secs(2)).repeat(),
                |s, t| {
                    let lift = (PI * (t / 0.35).min(1.0)).sin();
                    s.with_transformation(Transformation::translate(point(px(0.), px(-2. * lift))))
                },
            )
            .into_any_element(),
        // Quick wingbeats while a query or tool is in flight.
        Activity::Searching | Activity::Tool => base
            .with_animation(
                (at, 2usize),
                Animation::new(Duration::from_millis(360)).repeat(),
                |s, t| {
                    let beat = (2. * PI * t).sin().abs();
                    s.with_transformation(Transformation::scale(size(1., 1. - 0.22 * beat)))
                },
            )
            .into_any_element(),
        // Head dips once per page fetched.
        Activity::Reading | Activity::Checking => base
            .with_animation(
                SharedString::from(format!("{at}-read-{}", line.beat)),
                Animation::new(motion::MARK).with_easing(motion::ease),
                |s, t| {
                    s.with_transformation(Transformation::rotate(radians(0.28 * (PI * t).sin())))
                },
            )
            .into_any_element(),
        // Small beats in time with the streamed text.
        _ => base
            .with_animation(
                SharedString::from(format!("{at}-write-{}", line.beat)),
                Animation::new(Duration::from_millis(320)),
                |s, t| {
                    let beat = (PI * t).sin();
                    s.with_transformation(Transformation::scale(size(1., 1. - 0.14 * beat)))
                },
            )
            .into_any_element(),
    }
}
