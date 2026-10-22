//! The main window: title bar with the harness switch, icon rail, chat list,
//! transcript, composer, and the run panel.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui_kit::ExternalPaths;
use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::hover_card::HoverCard;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::progress::ProgressCircle;
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
    StepKind,
};
use tern_core::{SecretString, SessionId};
use tokio::sync::{mpsc, oneshot};

use crate::assets;
use crate::bridge::{Bridge, Msg};
mod appearance_view;
mod browser_view;
mod editor_view;
mod git_view;
mod scout_view;

use crate::layout::{Layout, Tab, Tabs};
use crate::model::{
    Activity, Connection, ContextLevel, Entry, Model, Phase, Pick, RunView, Screen, SiteIcon,
    WorkingLine,
};
use crate::motion::{self, Slide, ThemeWipe};
use crate::providers::{Kind, Models, Note, ServiceRow};
use crate::theme::Palette;

const RAIL_W: f32 = 52.;
const SIDEBAR_W: f32 = 264.;
const RUN_PANEL_W: f32 = 300.;
const COLUMN_MAX_W: f32 = 760.;
/// How much of an event's payload the Raw view shows when opened.
const RAW_PAYLOAD_CHARS: usize = 4_000;

/// The one window of the app.
pub struct Workspace {
    model: Model,
    bridge: Bridge,
    session: Option<Box<ChatSession>>,
    current: Option<SessionId>,
    /// The open chats.
    tabs: Tabs,
    /// The layout at the last frame, to notice the window narrowing.
    last_layout: Option<Layout>,
    composer: Entity<TextareaState>,
    /// The one key field; only one row edits at a time.
    key_input: Entity<InputState>,
    /// The title field of the chat being renamed in the list.
    rename_input: Entity<InputState>,
    renaming: Option<SessionId>,
    cancel: CancelFlag,
    /// Answers whose source list is expanded, by transcript index.
    open_sources: HashSet<usize>,
    pending: Option<oneshot::Sender<ApprovalAnswer>>,
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
    _listener: Task<()>,
    /// The built-in browser's address bar.
    address_input: Entity<InputState>,
    /// The Git screen's text fields.
    git_inputs: git_view::GitInputs,
    /// The Editor screen's text field.
    note_input: Entity<TextareaState>,
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
                    "Ask anything. @ to reference a file or source, Shift+Enter for a new line.",
                )
                .submit_on_enter(true)
        });
        let sub = cx.subscribe_in(
            &composer,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift: false, .. } => {
                    if this.model.mention.is_some() {
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
        let address_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Address, like arxiv.org"));
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
        bridge.connect();
        let mut model = Model::new();
        if let Ok(v) = tern_agent::vault::Vault::open_default() {
            let root = v.root().to_path_buf();
            model.editor.attach(v);
            let refresh = crate::git::GitOp::Refresh;
            if model.git.start(&refresh) {
                bridge.git(root, refresh);
            }
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
            model,
            bridge,
            session: None,
            current: None,
            tabs: Tabs::default(),
            last_layout: None,
            composer,
            key_input,
            rename_input,
            renaming: None,
            cancel: CancelFlag::default(),
            open_sources: HashSet::new(),
            pending: None,
            scroll: ScrollHandle::new(),
            ticker: None,
            _listener: listener,
            note_input,
            address_input,
            git_inputs,
            _subscriptions: vec![sub, key_sub, rename_sub, note_sub, address_sub],
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
                self.current = Some(s.session_id());
                self.tabs.set_active(s.session_id(), "New chat");
                self.session = Some(s);
                self.composer.update(cx, |c, cx| c.focus(window, cx));
            }
            Msg::Resumed(s, turns, recent) => {
                self.model.connected(Connection {
                    provider: s.provider_name().to_owned(),
                    model: s.model().to_owned(),
                    search: s.search_name().map(str::to_owned),
                    context_window: s.context_window(),
                });
                self.model.restore(&turns);
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
                self.session = Some(s);
                self.scroll.scroll_to_bottom();
                self.composer.update(cx, |c, cx| c.focus(window, cx));
            }
            Msg::Notice(text) => {
                self.model.entries.push(Entry::Notice(text));
                // A chat that would not reopen leaves no session; open a
                // fresh one so the composer works.
                if self.session.is_none() && matches!(self.model.phase, Phase::Connecting) {
                    self.current = None;
                    self.bridge.connect();
                }
            }
            Msg::Sessions(recent) => {
                for r in &recent {
                    self.tabs.retitle(r.id, &r.title);
                }
                self.model.sessions = recent;
            }
            Msg::ConnectFailed(e) => self.model.connect_failed(e),
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
            Msg::Approval(req, reply) => {
                if self.cancel.is_cancelled() {
                    let _ = reply.send(ApprovalAnswer::Deny);
                } else {
                    self.pending = Some(reply);
                    self.model.ask_approval(req);
                }
            }
            Msg::Done(s, out, usage, recent) => {
                self.model.finish_turn(&out, usage);
                self.model.sessions = recent;
                self.session = Some(s);
                self.ticker = None;
                self.scroll.scroll_to_bottom();
            }
            Msg::Providers(rows, prefs, keychain) => {
                self.model.providers.load(rows, prefs, keychain);
                self.refresh_models();
            }
            Msg::Checked(kind, id, result) => {
                let ok = result.is_ok();
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
                }
            }
            Msg::Removed(kind, id, result) => {
                let was_default = self.model.providers.default_id(kind) == Some(id);
                self.model.providers.removed(kind, id, result);
                if was_default && !self.model.is_busy() {
                    self.reconnect();
                }
                self.refresh_models();
            }
            Msg::Models(id, result) => self.model.providers.models_loaded(id, result),
            Msg::PrefsFailed(e) => self.model.entries.push(Entry::Notice(format!(
                "Tern could not save your choice: {e}"
            ))),
        }
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.cancel = CancelFlag::default();
        self.bridge.send(session, text, self.cancel.clone());
        self.composer
            .update(cx, |c, cx| c.set_value("", window, cx));
        self.scroll.scroll_to_bottom();
        self.start_ticker(cx);
        cx.notify();
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
        if self.model.mention.is_none() {
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
                    let icon = match &row.pick {
                        Pick::Attachment(_) => AssetIcon::Paperclip,
                        Pick::Source(_) => AssetIcon::Globe,
                        Pick::Browse => AssetIcon::FolderOpen,
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
                        .child(
                            div()
                                .text_color(p.muted)
                                .child(gpui_kit::component::Icon::new(icon).small()),
                        )
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
        if paths.is_empty() {
            return;
        }
        self.model.attaching += paths.len();
        self.bridge.attach(paths);
        cx.notify();
    }

    /// Redraws the working line twice a second while a turn runs.
    fn start_ticker(&mut self, cx: &mut Context<Self>) {
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let busy = this.update(cx, |this, cx| {
                    cx.notify();
                    this.model.is_busy()
                });
                if !matches!(busy, Ok(true)) {
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
        if self.model.is_busy() {
            return;
        }
        if let Some(tab) = self.tabs.focus(i).cloned() {
            self.show_tab(tab, window, cx);
        }
    }

    fn close_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.is_busy() && i == self.tabs.active {
            return;
        }
        if let Some(tab) = self.tabs.close(i).cloned() {
            self.show_tab(tab, window, cx);
        }
        cx.notify();
    }

    /// Shows a tab's chat: reopens it, or starts a fresh one.
    fn show_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
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

    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.dark = !self.model.dark;
        if !self.reduced_motion {
            let at = window.mouse_position();
            self.wipe = Some(ThemeWipe {
                since: Instant::now(),
                origin: (f32::from(at.x), f32::from(at.y)),
                to_dark: self.model.dark,
            });
        }
        crate::theme::apply_kit(self.model.dark, Some(window), cx);
        cx.notify();
    }

    // ----- providers -----

    fn open_providers(&mut self, cx: &mut Context<Self>) {
        self.model.screen = Screen::Providers;
        if !self.model.providers.loaded {
            self.bridge.load_providers();
        } else {
            self.refresh_models();
        }
        cx.notify();
    }

    fn close_providers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.screen = Screen::Chat;
        self.model.providers.cancel_edit();
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    /// Loads the model list of the default provider when it is not loaded.
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
        self.model.providers.models_loading(id);
        self.bridge.models(id);
    }

    /// Opens a fresh chat with the current choices, saving them first.
    fn reconnect(&mut self) {
        if self.model.is_busy() {
            return;
        }
        self.session = None;
        self.model.connection = None;
        self.model.new_chat();
        self.model.phase = Phase::Connecting;
        self.bridge
            .save_and_connect(self.model.providers.prefs.clone());
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
        let pill = |id: &'static str, label: &'static str, h: Harness, cx: &mut Context<Self>| {
            let selected = harness == h;
            div()
                .id(id)
                .px_3()
                .py(px(3.))
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(if selected { p.text } else { p.muted })
                .flex()
                .items_center()
                .gap_1p5()
                .when(selected, |d| d.bg(p.hover))
                .hover(|d| d.text_color(p.text))
                .when(selected, |d| {
                    let glyph = if h == Harness::Study {
                        "study"
                    } else {
                        "research"
                    };
                    d.children(
                        assets::glyph(glyph)
                            .map(|path| svg().path(path).size(px(12.)).text_color(p.accent)),
                    )
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.choose_harness(h);
                    cx.notify();
                }))
        };
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
                        .children(self.git_chip(p, cx)),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .occlude()
                        .p(px(2.))
                        .rounded_lg()
                        .border_1()
                        .border_color(p.border)
                        .child(pill("harness-research", "Research", Harness::Research, cx))
                        .child(pill("harness-study", "Study", Harness::Study, cx)),
                )
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
                        ),
                ),
        )
    }

    fn rail(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let later = |id: &'static str, icon: IconName, tip: &'static str| {
            Button::new(id)
                .icon(icon)
                .ghost()
                .disabled(true)
                .tooltip(tip)
        };
        div()
            .w(px(RAIL_W))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .py_2()
            .bg(p.rail)
            .border_r_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
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
                    .child(later(
                        "rail-library",
                        IconName::BookOpen,
                        "Library (coming later)",
                    ))
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
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_browser(window, cx)),
                            ),
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
                    ),
            )
            .child(
                Button::new("rail-appearance")
                    .icon(IconName::Palette)
                    .ghost()
                    .when(self.model.screen == Screen::Appearance, |b| {
                        b.selected(true)
                    })
                    .tooltip("Appearance")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.model.screen == Screen::Appearance {
                            this.close_appearance(window, cx);
                        } else {
                            this.open_appearance(cx);
                        }
                    })),
            )
            .child(
                Button::new("rail-settings")
                    .icon(IconName::Settings)
                    .ghost()
                    .when(self.model.screen == Screen::Providers, |b| b.selected(true))
                    .tooltip("Providers and keys")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.model.screen == Screen::Providers {
                            this.close_providers(window, cx);
                        } else {
                            this.open_providers(cx);
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

    fn sidebar(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.model.is_busy();
        let rows: Vec<_> = self
            .model
            .sessions
            .iter()
            .enumerate()
            .map(|(ix, s)| {
                let current = Some(s.id) == self.current;
                let id = s.id;
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
                div()
                    .id(("chat", ix))
                    .group("chat-row")
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl_2()
                    .pr_1()
                    .py(px(3.))
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .py(px(3.))
                            .truncate()
                            .child(SharedString::from(s.title.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .opacity(0.)
                            .group_hover("chat-row", |d| d.opacity(1.))
                            .child(
                                Button::new(("rename", ix))
                                    .icon(gpui_kit::assets::IconName::Pencil)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Rename chat")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.start_rename(id, window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("open-tab", ix))
                                    .icon(gpui_kit::assets::IconName::SquarePlus)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Open in new tab")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.open_in_tab(id, window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("archive", ix))
                                    .icon(gpui_kit::assets::IconName::Archive)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Archive chat")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.archive_chat(id, window, cx);
                                    })),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .w(px(SIDEBAR_W))
            .h_full()
            .flex()
            .flex_col()
            .bg(p.sidebar)
            .border_r_1()
            .border_color(p.border)
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_2()
                    .text_xs()
                    .text_color(p.faint)
                    .child("Chats"),
            )
            .child(
                div()
                    .id("chat-list")
                    .flex_1()
                    .overflow_y_scroll()
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
            .child(self.scout_corner(p))
    }

    fn entry(&self, i: usize, e: &Entry, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        match e {
            Entry::User { text, chips, quote } => div()
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
                .child(self.message_actions(i, false, p, cx))
                .into_any_element(),
            Entry::Assistant {
                text,
                run,
                took,
                pages,
            } => {
                let mut meta = format!("Worked for {}s", took.as_secs().max(1));
                let steps = run.step_count();
                if steps > 0 {
                    meta.push_str(&format!(
                        " · {} step{}",
                        steps,
                        if steps == 1 { "" } else { "s" }
                    ));
                }
                let clickable = !run.is_empty();
                let meta = div()
                    .id(("answer-run", i))
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child(meta)
                    .when(clickable, |d| {
                        d.cursor_pointer()
                            .hover(|d| d.text_color(p.text))
                            .child(gpui_kit::component::Icon::new(IconName::ChevronRight).xsmall())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model.show_run_of(i);
                                cx.notify();
                            }))
                    });
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        TextView::markdown(("answer", i), text.clone())
                            .on_link_click(self.link_opener(cx))
                            .selectable(true)
                            .motion(stream_motion()),
                    )
                    .when(!pages.is_empty(), |d| {
                        d.child(self.sources(i, pages, p, cx))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(meta)
                            .child(self.message_actions(i, true, p, cx)),
                    )
                    .into_any_element()
            }
            Entry::Notice(t) => div()
                .text_sm()
                .text_color(p.muted)
                .child(SharedString::from(t.clone()))
                .into_any_element(),
        }
    }

    /// Copy, Reply, and (on answers) Branch, as small quiet icons.
    fn message_actions(
        &self,
        i: usize,
        answer: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let copied = self.model.copied == Some(i);
        let busy = self.model.is_busy();
        let action = move |id: &'static str, icon: AssetIcon, tip: &'static str| {
            div()
                .id((id, i))
                .p(px(3.))
                .rounded_md()
                .cursor_pointer()
                .text_color(p.faint)
                .hover(move |d| d.text_color(p.text).bg(p.hover))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                })
                .child(gpui_kit::component::Icon::new(icon).xsmall())
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(
                action(
                    "copy",
                    if copied {
                        AssetIcon::Check
                    } else {
                        AssetIcon::Copy
                    },
                    "Copy",
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(t) = this.model.entry_text(i) {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(t.to_owned()));
                        this.model.copied = Some(i);
                        cx.notify();
                    }
                })),
            )
            .child(
                action("reply", AssetIcon::Reply, "Reply").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.model.reply_to_entry(i);
                        this.composer.update(cx, |c, cx| c.focus(window, cx));
                        cx.notify();
                    },
                )),
            )
            .when(answer, |d| {
                let saved = self.model.saved_answer == Some(i);
                d.child(
                    action(
                        "save-note",
                        if saved {
                            AssetIcon::Check
                        } else {
                            AssetIcon::NotebookPen
                        },
                        "Save to vault",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.save_answer(i, cx))),
                )
            })
            .when(answer && !busy, |d| {
                d.child(
                    action("branch", AssetIcon::GitBranch, "Branch into a new chat")
                        .on_click(cx.listener(move |this, _, _, cx| this.branch_from(i, cx))),
                )
            })
    }

    /// The open chats as tabs, shown once there is more than one.
    fn tab_strip(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.tabs.list.len() < 2 {
            return None;
        }
        let busy = self.model.is_busy();
        let active = self.tabs.active;
        Some(
            div()
                .id("tabs")
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(p.border)
                .overflow_x_scroll()
                .children(self.tabs.list.iter().enumerate().map(|(i, t)| {
                    let on = i == active;
                    div()
                        .id(("tab", i))
                        .group("tab")
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .max_w(px(200.))
                        .pl_2()
                        .pr_1()
                        .py(px(3.))
                        .rounded_md()
                        .text_sm()
                        .cursor_pointer()
                        .text_color(if on { p.text } else { p.muted })
                        .when(on, |d| d.bg(p.raised).border_1().border_color(p.border))
                        .when(!on, |d| d.hover(move |d| d.bg(p.hover)))
                        .when(busy && !on, |d| d.opacity(0.5))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(SharedString::from(t.title.clone())),
                        )
                        .child(
                            Button::new(("close-tab", i))
                                .icon(IconName::Close)
                                .ghost()
                                .xsmall()
                                .tooltip("Close tab")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_tab(i, window, cx);
                                })),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.focus_tab(i, window, cx);
                        }))
                }))
                .child(
                    Button::new("add-tab")
                        .icon(IconName::Plus)
                        .ghost()
                        .xsmall()
                        .disabled(busy)
                        .tooltip("New tab")
                        .on_click(cx.listener(|this, _, window, cx| this.add_tab(window, cx))),
                )
                .into_any_element(),
        )
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
            _ => (
                "What are we researching?",
                "Ask a question. Tern searches, reads, and cites when a search service is set up."
                    .into(),
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

    fn transcript(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let mut column = div()
            .w_full()
            .max_w(px(COLUMN_MAX_W))
            .flex()
            .flex_col()
            .gap_5()
            .py_6()
            .px_4();
        if self.model.entries.is_empty() && !self.model.is_busy() {
            column = column.child(self.empty_state(p, cx));
        }
        for (i, e) in self.model.entries.iter().enumerate() {
            column = column.child(self.entry(i, e, p, cx));
        }
        if let Some(draft) = self.model.draft() {
            // Same id the finished answer gets, so the text does not redraw
            // when the stream ends.
            let i = self.model.entries.len();
            column = column.child(
                TextView::markdown(("answer", i), draft.to_owned())
                    .selectable(true)
                    .motion(stream_motion()),
            );
        }
        if let Some(card) = self.approval_card(p, cx) {
            column = column.child(card);
        }
        if let Some(line) = self.model.working_line() {
            column = column.child(working_line(&line, p, self.reduced_motion, "transcript"));
        }
        div()
            .id("transcript")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .items_center()
            .child(column)
    }

    fn composer(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.model.is_busy();
        let status = match &self.model.connection {
            Some(c) => {
                let mut s = format!("{} · {}", c.provider, c.model);
                if let Some(search) = &c.search {
                    s.push_str(&format!(" · web: {search}"));
                }
                s
            }
            None => match &self.model.phase {
                Phase::Offline(_) => "No model connected".into(),
                _ => "Connecting…".into(),
            },
        };
        let action = if busy {
            Button::new("stop")
                .icon(IconName::Square)
                .small()
                .tooltip("Stop")
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            Button::new("send")
                .icon(IconName::ArrowUp)
                .small()
                .disabled(!self.model.can_send())
                .tooltip("Send (Enter)")
                .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
        };
        div().w_full().flex().justify_center().px_4().pb_4().child(
            div()
                .id("composer-box")
                .w_full()
                .max_w(px(COLUMN_MAX_W))
                .flex()
                .flex_col()
                .gap_2()
                .p_2()
                .rounded_xl()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .drag_over::<ExternalPaths>(move |style, _, _, _| {
                    style.border_color(p.muted).bg(p.hover)
                })
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    this.attach_paths(paths.paths().to_vec(), cx);
                }))
                .capture_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, window, cx| {
                    this.mention_keys(e, window, cx);
                }))
                .children(self.mention_menu(p, cx))
                .children(self.reply_bar(p, cx))
                .when(
                    !self.model.attachments.is_empty() || self.model.attaching > 0,
                    |d| d.child(self.attachment_chips(p, cx)),
                )
                .child(
                    Textarea::new(&self.composer)
                        .appearance(false)
                        .disabled(!self.model.is_ready() && !busy),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .min_w_0()
                                .child(
                                    Button::new("attach")
                                        .icon(AssetIcon::Paperclip)
                                        .ghost()
                                        .small()
                                        .tooltip("Attach files, folders, or .zip")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.pick_attachments(cx)),
                                        ),
                                )
                                .child(self.mic_button(p)),
                        )
                        .child(
                            div()
                                .id("composer-model")
                                .min_w_0()
                                .px_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(p.faint)
                                .truncate()
                                .cursor_pointer()
                                .hover(|d| d.text_color(p.text).bg(p.hover))
                                .child(status)
                                .on_click(cx.listener(|this, _, _, cx| this.open_providers(cx))),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(self.context_ring(p))
                                .child(action),
                        ),
                ),
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
                    AttachSource::File => AssetIcon::FileText,
                    AttachSource::Folder => AssetIcon::Folder,
                    AttachSource::Archive => AssetIcon::FileArchive,
                    AttachSource::Paste => AssetIcon::ClipboardPaste,
                    AttachSource::Link => AssetIcon::Link,
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
    fn mic_button(&self, _p: Palette) -> impl IntoElement {
        Button::new("mic")
            .icon(AssetIcon::MicOff)
            .ghost()
            .small()
            .disabled(true)
            .tooltip("Voice input needs a transcription model. Set one in Settings › Voice.")
    }

    /// The composer's context ring: fills as the window does, turns amber
    /// past 70% and red past 90%. Hover for exact numbers.
    fn context_ring(&self, p: Palette) -> impl IntoElement {
        let fraction = self.model.context_fraction();
        let color = match self.model.context_level() {
            ContextLevel::Fine => p.accent,
            ContextLevel::Filling => gpui_kit::hsla(38. / 360., 0.85, 0.55, 1.),
            ContextLevel::Full => gpui_kit::hsla(0., 0.75, 0.58, 1.),
        };
        let label = self.model.context_label();
        let details = self.model.context_details();
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
            .child(
                ProgressCircle::new("context-ring-arc")
                    .value(fraction.unwrap_or(0.) * 100.)
                    .color(color)
                    .accessibility_label(label.clone())
                    .size(px(14.)),
            )
            .child(match fraction {
                Some(f) => format!("{:.0}%", f * 100.),
                None => label.clone(),
            });
        HoverCard::new("context-card")
            .anchor(gpui_kit::Anchor::BottomRight)
            .open_delay(Duration::from_millis(150))
            .trigger(trigger)
            .content(move |_, _, _| {
                div()
                    .w(px(260.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_xs()
                    .children(details.iter().map(|(k, v)| {
                        div()
                            .flex()
                            .justify_between()
                            .gap_3()
                            .child(div().text_color(p.muted).child(*k))
                            .child(
                                div()
                                    .text_color(p.text)
                                    .child(SharedString::from(v.clone())),
                            )
                    }))
            })
    }

    fn providers_screen(
        &self,
        p: Palette,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let st = &self.model.providers;
        let mut column = div()
            .w_full()
            .max_w(px(COLUMN_MAX_W))
            .flex()
            .flex_col()
            .gap_4()
            .py_6()
            .px_4()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Providers"),
                            )
                            .child(div().text_sm().text_color(p.muted).child(
                                "Bring your own keys. Tern tests a key before saving it, and keeps it only in Windows Credential Manager. Changing the default opens a new chat.",
                            )),
                    )
                    .child(
                        Button::new("providers-done")
                            .label("Back to chat")
                            .ghost()
                            .small()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_providers(window, cx)),
                            ),
                    ),
            );
        if let Some(e) = &st.keychain_error {
            column = column.child(
                div()
                    .text_sm()
                    .text_color(p.muted)
                    .child(format!("The keychain cannot be read on this computer: {e}")),
            );
        }
        if !st.loaded {
            column = column.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(p.muted)
                    .child(Spinner::new().small().color(p.muted))
                    .child("Reading the keychain…"),
            );
        } else {
            column = column
                .child(self.section(Kind::Model, "Models", p, cx))
                .child(self.section(Kind::Search, "Web search", p, cx));
        }
        div()
            .id("providers")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .child(column)
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
            if !is_default {
                actions = actions.child(
                    Button::new(bid("use"))
                        .label("Use")
                        .small()
                        .disabled(busy_turn)
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
                        this.bridge.test(kind, id);
                        cx.notify();
                    })),
            );
            if r.key_saved {
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

        if kind == Kind::Model && is_default {
            row = row.child(self.model_picker(p, cx));
        }
        row.into_any_element()
    }

    fn model_picker(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.providers;
        let chosen = st
            .chosen_model()
            .map(str::to_owned)
            .or_else(|| self.model.connection.as_ref().map(|c| c.model.clone()));
        let body: AnyElement = match &st.models {
            None | Some(Models::Loading(_)) => div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(p.muted)
                .child(Spinner::new().small().color(p.muted))
                .child("Listing models…")
                .into_any_element(),
            Some(Models::Failed(_, e)) => div()
                .text_xs()
                .text_color(p.muted)
                .child(SharedString::from(format!("Could not list models: {e}")))
                .into_any_element(),
            Some(Models::Loaded(_, list)) if list.is_empty() => div()
                .text_xs()
                .text_color(p.muted)
                .child("This provider lists no chat models.")
                .into_any_element(),
            Some(Models::Loaded(_, list)) => {
                let busy = self.model.is_busy();
                div()
                    .id("model-list")
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .children(list.iter().enumerate().map(|(i, m)| {
                        let selected = chosen.as_deref() == Some(m.id.as_str());
                        let id = m.id.clone();
                        div()
                            .id(("model", i))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .text_color(if selected { p.text } else { p.muted })
                            .when(selected, |d| d.bg(p.hover))
                            .hover(|d| d.bg(p.hover).text_color(p.text))
                            .child(div().truncate().child(SharedString::from(m.id.clone())))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .when_some(m.context_window, |d, w| {
                                        d.child(format!("{} context", crate::model::compact(w)))
                                    })
                                    .when(selected, |d| {
                                        d.child(
                                            gpui_kit::component::Icon::new(IconName::Check).small(),
                                        )
                                    }),
                            )
                            .when(!busy && !selected, |d| {
                                d.on_click(cx.listener(move |this, _, _, cx| {
                                    this.pick_model(id.clone(), cx)
                                }))
                            })
                    }))
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .pl(px(40.))
            .child(div().text_xs().text_color(p.faint).child("Model"))
            .child(body)
            .into_any_element()
    }

    /// The run panel: one run in three views (Summary, Detailed, Raw).
    fn run_panel(&self, p: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let run = self.model.panel_run();
        let view = self.model.run_panel.view;
        let tabs = div()
            .flex()
            .items_center()
            .gap_1()
            .children(RunView::ALL.map(|v| {
                let selected = v == view;
                div()
                    .id(SharedString::from(format!("run-view-{}", v.label())))
                    .px_2()
                    .py(px(2.))
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(if selected { p.text } else { p.muted })
                    .when(selected, |d| d.bg(p.hover))
                    .hover(|d| d.text_color(p.text))
                    .child(v.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.set_run_view(v);
                        cx.notify();
                    }))
            }));
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .child(div().text_xs().text_color(p.faint).child("Run"))
            .child(tabs);
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
            }
        };
        div()
            .w(px(RUN_PANEL_W))
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
                    .overflow_y_scroll()
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
            .when(!problems.is_empty(), |d| {
                d.child(section("Problems"))
                    .children(problems.into_iter().map(|s| step_line(s, p, false)))
            })
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
        let now = Instant::now();
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
        let sidebar = self
            .sidebar_slide
            .frame(self.model.sidebar_open, now, reduced);
        let panel = self
            .panel_slide
            .frame(self.model.run_panel_open, now, reduced);
        if self.wipe.is_some() || self.sidebar_slide.moving() || self.panel_slide.moving() {
            window.request_animation_frame();
        }
        let ripple = self.wipe.map(|w| {
            let (grow, strength) = w.circle(now);
            let view = window.viewport_size();
            let diag = f32::from(view.width).hypot(f32::from(view.height));
            let r = diag * grow;
            div()
                .absolute()
                .left(px(w.origin.0 - r))
                .top(px(w.origin.1 - r))
                .size(px(r * 2.))
                .rounded_full()
                .bg(p.text.opacity(strength * 0.25))
        });
        // The right pane: the browser when it is showing, else the run panel.
        let side_w = if self.model.browser.visible {
            browser_view::BROWSER_W
        } else {
            RUN_PANEL_W
        };
        let side_pane = |this: &Self, cx: &mut Context<Self>| {
            if this.model.browser.visible {
                this.browser_pane(p, cx)
            } else {
                this.run_panel(p, cx).into_any_element()
            }
        };
        let mut side = panel.map(|_| side_pane(self, cx));
        let screen_id: &'static str = match self.model.screen {
            Screen::Chat => "screen-chat",
            Screen::Providers => "screen-providers",
            Screen::Editor => "screen-editor",
            Screen::Git => "screen-git",
            Screen::Appearance => "screen-appearance",
        };
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.canvas)
            .text_color(p.text)
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
                        d.child(sliding(self.sidebar(p, cx), SIDEBAR_W, f, false))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .map(|d| match self.model.screen {
                                Screen::Chat => d
                                    .children(self.tab_strip(p, cx))
                                    .child(self.transcript(p, cx))
                                    .child(self.composer(p, cx)),
                                Screen::Providers => d.child(self.providers_screen(p, window, cx)),
                                Screen::Editor => d.child(self.editor_screen(p, cx)),
                                Screen::Git => d.child(self.git_screen(p, cx)),
                                Screen::Appearance => d.child(self.appearance_screen(p, cx)),
                            })
                            .with_animation(
                                screen_id,
                                Animation::new(if reduced { Duration::ZERO } else { motion::UI })
                                    .with_easing(motion::ease),
                                |d, t| d.opacity(t),
                            ),
                    )
                    .when_some(panel.filter(|_| layout.panel_beside), |d, f| {
                        d.children(side.take().map(|e| sliding(e, side_w, f, true)))
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
                                .child(sliding(self.sidebar(p, cx), SIDEBAR_W, f, false)),
                        )
                    })
                    .when_some(panel.filter(|_| !layout.panel_beside), |d, f| {
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
            .children(ripple)
    }
}

/// The logo square at the start of a row. Until the service glyph set is
/// approved, known services show their initial on a neutral tile.
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
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.muted)
        .child(r.monogram())
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

/// The live line while a turn runs: the tern mark moving with the work, a
/// shimmering phrase, and elapsed seconds (`docs/02-ui-definition.md` §2.7).
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
        Activity::Reading => base
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
