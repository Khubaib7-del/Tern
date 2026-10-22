//! The main window: title bar with the harness switch, icon rail, chat list,
//! transcript, composer, and the run panel.

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Disableable as _, IconName, Sizable as _, Theme, ThemeMode, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, ScrollHandle, SharedString, Subscription, Task,
    Window, div, px,
};
use tern_agent::{ApprovalAnswer, CancelFlag, ChatSession, Harness};
use tern_core::SessionId;
use tokio::sync::{mpsc, oneshot};

use crate::bridge::{Bridge, Msg};
use crate::model::{Connection, Entry, Model, Phase};
use crate::theme::Palette;

const RAIL_W: f32 = 52.;
const SIDEBAR_W: f32 = 264.;
const RUN_PANEL_W: f32 = 300.;
const COLUMN_MAX_W: f32 = 760.;

/// The one window of the app.
pub struct Workspace {
    model: Model,
    bridge: Bridge,
    session: Option<Box<ChatSession>>,
    current: Option<SessionId>,
    composer: Entity<TextareaState>,
    cancel: CancelFlag,
    pending: Option<oneshot::Sender<ApprovalAnswer>>,
    scroll: ScrollHandle,
    ticker: Option<Task<()>>,
    _listener: Task<()>,
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
                .placeholder("Ask anything. Shift+Enter for a new line.")
                .submit_on_enter(true)
        });
        let sub = cx.subscribe_in(
            &composer,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.submit(window, cx);
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
        bridge.connect();
        Self {
            model: Model::new(),
            bridge,
            session: None,
            current: None,
            composer,
            cancel: CancelFlag::default(),
            pending: None,
            scroll: ScrollHandle::new(),
            ticker: None,
            _listener: listener,
            _subscriptions: vec![sub],
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
                self.session = Some(s);
                self.composer.update(cx, |c, cx| c.focus(window, cx));
            }
            Msg::ConnectFailed(e) => self.model.connect_failed(e),
            Msg::Event { kind, summary } => self.model.on_event(&kind, &summary),
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

    fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.is_busy() {
            return;
        }
        let had_messages = !self.model.entries.is_empty();
        self.model.new_chat();
        if had_messages || self.session.is_none() {
            // A fresh session row, so the old chat keeps its own history.
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
        let mode = if self.model.dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
        Theme::change(mode, Some(window), cx);
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
                .when(selected, |d| d.bg(p.hover))
                .hover(|d| d.text_color(p.text))
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
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Tern"),
                        )
                        .child(
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
                .child(
                    div()
                        .flex()
                        .gap_1()
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
                                    this.model.run_panel_open = !this.model.run_panel_open;
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
                    .child(later(
                        "rail-editor",
                        IconName::FileText,
                        "Editor (coming later)",
                    )),
            )
            .child(later(
                "rail-settings",
                IconName::Settings,
                "Settings and providers (next slice). For now: tern provider add",
            ))
    }

    fn sidebar(&self, p: Palette) -> impl IntoElement {
        let rows = self.model.sessions.iter().map(|s| {
            let current = Some(s.id) == self.current;
            div()
                .px_2()
                .py(px(6.))
                .rounded_md()
                .text_sm()
                .truncate()
                .text_color(if current { p.text } else { p.muted })
                .when(current, |d| d.bg(p.hover))
                .child(SharedString::from(s.title.clone()))
        });
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
    }

    fn entry(&self, i: usize, e: &Entry, p: Palette) -> AnyElement {
        match e {
            Entry::User(t) => div()
                .flex()
                .justify_end()
                .child(
                    div()
                        .max_w(px(COLUMN_MAX_W * 0.8))
                        .px_3()
                        .py_2()
                        .rounded_xl()
                        .bg(p.raised)
                        .child(SharedString::from(t.clone())),
                )
                .into_any_element(),
            Entry::Assistant { text, steps, took } => {
                let mut meta = format!("Worked for {}s", took.as_secs().max(1));
                if !steps.is_empty() {
                    meta.push_str(&format!(
                        " · {} step{}",
                        steps.len(),
                        if steps.len() == 1 { "" } else { "s" }
                    ));
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(TextView::markdown(("answer", i), text.clone()).selectable(true))
                    .child(div().text_xs().text_color(p.faint).child(meta))
                    .into_any_element()
            }
            Entry::Notice(t) => div()
                .text_sm()
                .text_color(p.muted)
                .child(SharedString::from(t.clone()))
                .into_any_element(),
        }
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

    fn empty_state(&self, p: Palette) -> AnyElement {
        let (headline, detail): (&str, String) = match &self.model.phase {
            Phase::Connecting => ("Connecting…", "Opening your default model.".into()),
            Phase::Offline(reason) => ("Tern needs a model first", reason.clone()),
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
            column = column.child(self.empty_state(p));
        }
        for (i, e) in self.model.entries.iter().enumerate() {
            column = column.child(self.entry(i, e, p));
        }
        if let Some(card) = self.approval_card(p, cx) {
            column = column.child(card);
        }
        if let Some(line) = self.model.working_line() {
            column = column.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(p.muted)
                    .child(Spinner::new().small().color(p.muted))
                    .child(line),
            );
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
        let context = match self.model.context_fraction() {
            Some(f) => format!("{} · {:.0}%", self.model.context_label(), f * 100.0),
            None => self.model.context_label(),
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
                .child(
                    Textarea::new(&self.composer)
                        .appearance(false)
                        .disabled(!self.model.can_send() && !busy),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_1()
                        .child(div().text_xs().text_color(p.faint).truncate().child(status))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().text_xs().text_color(p.faint).child(context))
                                .child(action),
                        ),
                ),
        )
    }

    fn run_panel(&self, p: Palette) -> impl IntoElement {
        let steps = self.model.run_steps();
        div()
            .w(px(RUN_PANEL_W))
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(p.sidebar)
            .border_l_1()
            .border_color(p.border)
            .child(div().text_xs().text_color(p.faint).child("Run"))
            .when(steps.is_empty(), |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(p.faint)
                        .child("Tool calls and approvals show here."),
                )
            })
            .children(steps.iter().map(|s| {
                div()
                    .text_sm()
                    .text_color(p.muted)
                    .child(SharedString::from(s.clone()))
            }))
            .child(
                div()
                    .mt_auto()
                    .text_xs()
                    .text_color(p.faint)
                    .child(self.model.context_label()),
            )
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::get(self.model.dark);
        div()
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
                    .flex_1()
                    .min_h_0()
                    .child(self.rail(p, cx))
                    .when(self.model.sidebar_open, |d| d.child(self.sidebar(p)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(self.transcript(p, cx))
                            .child(self.composer(p, cx)),
                    )
                    .when(self.model.run_panel_open, |d| d.child(self.run_panel(p))),
            )
    }
}
