//! Continuity in the window: message IDs on the transcript, multi-select
//! and range actions, the Branch menu, background branches, handoff, the
//! highlight popover, and the side chat (docked or popped out).

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, MouseUpEvent, SharedString, Subscription, WeakEntity,
    Window, div, px, svg,
};
use tern_agent::highlight::Action;
use tern_core::SessionId;

use super::Workspace;
use crate::assets;
use crate::bridge::ContMsg;
use crate::continuity::Highlight;
use crate::model::{Entry, Screen};
use crate::theme::Palette;

/// The docked side chat's width.
pub(super) const SIDE_W: f32 = 320.;

fn icon(name: AssetIcon, color: gpui_kit::Hsla) -> AnyElement {
    gpui_kit::component::Icon::new(name)
        .xsmall()
        .text_color(color)
        .into_any_element()
}

impl Workspace {
    pub(super) fn on_cont(&mut self, m: ContMsg, window: &mut Window, cx: &mut Context<Self>) {
        match m {
            ContMsg::Ids {
                session,
                anchor,
                labels,
                next,
                removed,
                pending,
            } => {
                if self.current != Some(session) {
                    return;
                }
                let ids = &mut self.model.cont.ids;
                ids.anchor = anchor;
                ids.next = next.max(ids.next);
                if labels.iter().map(|l| l.0).eq(ids.numbers.iter().copied()) {
                    ids.labels = labels.into_iter().map(|l| l.1).collect();
                }
                ids.removed = removed;
                ids.pending = pending;
            }
            ContMsg::Ranged(session, result) => {
                let note = match result {
                    Ok(n) => n,
                    Err(e) => format!("That did not work: {e}"),
                };
                self.model.cont.ids.selected.clear();
                if self.current == Some(session) && !self.model.is_busy() {
                    self.load_chat(session, cx);
                }
                self.model.entries.push(Entry::Notice(note));
            }
            ContMsg::SubMade(id) => {
                let title = self
                    .model
                    .sessions
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.title.clone())
                    .unwrap_or_else(|| "Sub-session".into());
                if !self.model.is_busy() {
                    self.tabs.add();
                    self.tabs.set_active(id, &title);
                    self.load_chat(id, cx);
                }
            }
            ContMsg::Background(id) => {
                if !self.model.cont.background.contains(&id) {
                    self.model.cont.background.push(id);
                }
            }
            ContMsg::Reported { parent, sub, note } => {
                self.model.cont.background.retain(|s| *s != sub);
                if self.current == Some(parent) {
                    self.model.entries.push(Entry::Notice(format!(
                        "↳ A background branch reported back: {note}"
                    )));
                    if !self.model.is_busy() {
                        // Reopen so the parent's model reads the report too.
                        self.load_chat(parent, cx);
                    }
                }
            }
            ContMsg::SideOpened(r) => {
                self.model.cont.side.opening = false;
                if let Err(e) = r {
                    self.model
                        .cont
                        .side
                        .lines
                        .push(crate::continuity::SideLine {
                            question: "Open the side chat".into(),
                            answer: Some(e),
                            failed: true,
                        });
                }
            }
            ContMsg::SideAnswer(r) => self.model.cont.side.answered(r),
            ContMsg::Highlighted(r) => {
                let note = match r {
                    Ok(n) => n,
                    Err(e) => e,
                };
                match self.model.cont.highlight.as_mut() {
                    Some(h) => h.note = Some(note),
                    None => self.model.entries.push(Entry::Notice(note)),
                }
            }
            ContMsg::HandedOff(r) => match r {
                Ok((note, id)) => {
                    let saved = if note.is_empty() {
                        "Wrote the handoff packet".to_owned()
                    } else {
                        format!("Wrote the handoff packet to {note} in your vault")
                    };
                    if !self.model.is_busy() {
                        self.tabs.add();
                        self.load_chat(id, cx);
                    }
                    self.model.entries.push(Entry::Notice(format!(
                        "{saved} and started this session from it alone."
                    )));
                }
                Err(e) => self.model.entries.push(Entry::Notice(format!(
                    "Tern could not hand off that chat: {e}"
                ))),
            },
            ContMsg::Library {
                request,
                items,
                total,
                counts,
                note,
            } => {
                let l = &mut self.model.cont.library;
                l.loaded(request, items, total, counts);
                if note.is_some() {
                    l.note = note;
                }
                self.request_library_icons();
            }
        }
        let _ = window;
        cx.notify();
    }

    // ---- Message IDs and range actions ---------------------------------

    /// The `Q3` tag and select box on a user message.
    pub(super) fn message_id_tag(
        &self,
        i: usize,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (Some(label), Some(n)) = (self.model.label_of_entry(i), self.model.turn_of_entry(i))
        else {
            return div().into_any_element();
        };
        let selected = self.model.cont.ids.selected.contains(&n);
        let busy = self.model.is_busy();
        div()
            .id(("msg-id", i))
            .flex()
            .items_center()
            .gap_1()
            .text_xs()
            .font_family(assets::MONO_FONT)
            .text_color(if selected { p.accent } else { p.faint })
            .when(!busy, |d| {
                d.cursor_pointer()
                    .hover(move |d| d.text_color(p.text))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.cont.ids.toggle(n);
                        cx.notify();
                    }))
            })
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(
                    "Select for a range action: move to a sub-session or a new session, or remove",
                )
                .build(window, cx)
            })
            .child(icon(
                if selected {
                    AssetIcon::SquareCheck
                } else {
                    AssetIcon::Square
                },
                if selected { p.accent } else { p.faint },
            ))
            .child(SharedString::from(label))
            .into_any_element()
    }

    /// The Branch ▾ menu under an answer: New sub-session from here, Run
    /// the composer text in a background branch, Move to sub-session.
    pub(super) fn branch_menu(
        &self,
        i: usize,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.model.cont.ids.branch_menu != Some(i) {
            return None;
        }
        let n = self.model.turn_of_entry(i)?;
        let in_sub = self.model.cont.ids.anchor.is_some();
        let draft = !self.composer.read(cx).value().trim().is_empty();
        let item = |id: &'static str, text: String, on: bool| {
            div()
                .id((id, i))
                .px_2()
                .py_1()
                .rounded_md()
                .text_sm()
                .when(on, |d| {
                    d.cursor_pointer()
                        .text_color(p.text)
                        .hover(move |d| d.bg(p.hover))
                })
                .when(!on, |d| d.text_color(p.faint))
                .child(text)
        };
        let anchor = self.model.cont.ids.anchor.unwrap_or(n);
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .p_1()
                .w(px(300.))
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .shadow_md()
                .child(
                    item(
                        "branch-new-sub",
                        format!("New sub-session from Q{anchor}"),
                        true,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.cont.ids.branch_menu = None;
                        if let Some(s) = this.parent_of_current() {
                            this.bridge.sub_new(s, anchor);
                        }
                        cx.notify();
                    })),
                )
                .child(
                    item(
                        "branch-background",
                        if draft {
                            "Run the composer text in a background branch".into()
                        } else {
                            "Type a question, then run it in a background branch".into()
                        },
                        draft,
                    )
                    .when(draft, |d| {
                        d.on_click(cx.listener(move |this, _, window, cx| {
                            this.background_branch(anchor, window, cx);
                        }))
                    }),
                )
                .child(
                    item(
                        "branch-move",
                        if in_sub {
                            format!("Move Q{n}… to a new session")
                        } else {
                            format!("Move Q{n} to a sub-session")
                        },
                        true,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.cont.ids.branch_menu = None;
                        if let Some(s) = this.current {
                            let picked: Vec<u32> = if this.model.cont.ids.selected.is_empty() {
                                vec![n]
                            } else {
                                this.model.cont.ids.selected.iter().copied().collect()
                            };
                            this.bridge.range_move(s, picked, !in_sub);
                        }
                        cx.notify();
                    })),
                )
                .into_any_element(),
        )
    }

    /// The chat a new sub-session hangs under: the open chat, or its parent.
    fn parent_of_current(&self) -> Option<SessionId> {
        let cur = self.current?;
        Some(
            self.model
                .sessions
                .iter()
                .find(|s| s.id == cur)
                .and_then(|s| s.parent)
                .unwrap_or(cur),
        )
    }

    fn background_branch(&mut self, anchor: u32, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_owned();
        let Some(parent) = self.parent_of_current() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        self.composer
            .update(cx, |c, cx| c.set_value("", window, cx));
        self.model.cont.ids.branch_menu = None;
        self.model.entries.push(Entry::Notice(format!(
            "↳ Running “{}” in a background branch from Q{anchor}; it reports back here.",
            tern_agent::range::clip(&text, 80)
        )));
        self.bridge.sub_background(parent, anchor, text);
        cx.notify();
    }

    /// Over the composer: the selection's range actions, a waiting move's
    /// Confirm and Undo, and the chat's trash.
    pub(super) fn range_bar(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ids = &self.model.cont.ids;
        let busy = self.model.is_busy();
        let current = self.current?;
        let bar = || {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .text_sm()
        };
        if let Some((nums, _)) = &ids.pending {
            let label = tern_agent::range::label(nums);
            return Some(
                bar()
                    .child(icon(AssetIcon::ArrowRightLeft, p.accent))
                    .child(div().flex_1().child(format!(
                        "Copied {label} with its context. The originals stay until you confirm."
                    )))
                    .child(
                        Button::new("range-confirm")
                            .label("Confirm move")
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.bridge.range_confirm(current);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("range-undo")
                            .label("Undo")
                            .ghost()
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.bridge.range_undo(current);
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if !ids.selected.is_empty() {
            let picked: Vec<u32> = ids.selected.iter().copied().collect();
            let in_sub = ids.anchor.is_some();
            let (a, b, c) = (picked.clone(), picked.clone(), picked);
            return Some(
                bar()
                    .child(icon(AssetIcon::ListChecks, p.accent))
                    .child(
                        div()
                            .flex_1()
                            .child(format!("{} selected", ids.selection_label())),
                    )
                    .when(!in_sub, |d| {
                        d.child(
                            Button::new("range-to-sub")
                                .label("Move to sub-session")
                                .small()
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.bridge.range_move(current, a.clone(), true);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("range-to-new")
                            .label("Move to new session")
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.bridge.range_move(current, b.clone(), false);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("range-remove")
                            .icon(AssetIcon::Trash)
                            .label("Remove")
                            .ghost()
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.bridge.range_remove(current, c.clone());
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("range-clear")
                            .icon(AssetIcon::X)
                            .ghost()
                            .xsmall()
                            .tooltip("Clear the selection")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.cont.ids.selected.clear();
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if !ids.removed.is_empty() {
            let n = ids.removed.len();
            let list = ids
                .removed
                .iter()
                .map(|(n, t)| format!("Q{n} {t}"))
                .collect::<Vec<_>>()
                .join("\n");
            return Some(
                div()
                    .id("chat-trash")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child(icon(AssetIcon::Trash, p.faint))
                    .child(
                        div()
                            .id("chat-trash-list")
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(list.clone())
                                    .build(window, cx)
                            })
                            .child(format!(
                                "{n} removed {} in this chat's trash",
                                if n == 1 { "message" } else { "messages" }
                            )),
                    )
                    .child(
                        Button::new("chat-trash-restore")
                            .label("Restore all")
                            .ghost()
                            .xsmall()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.bridge.range_restore(current, None);
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        None
    }

    /// Writes a handoff packet for a chat and starts a new session from it.
    pub(super) fn hand_off(&mut self, id: SessionId, cx: &mut Context<Self>) {
        if self.model.is_busy() {
            return;
        }
        self.bridge.handoff(id);
        cx.notify();
    }

    // ---- Highlights ----------------------------------------------------

    /// Mouse up over answer `i`: text selected there opens the popover.
    pub(super) fn answer_mouse_up(
        &mut self,
        i: usize,
        e: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = gpui_kit::base::TextSelection::selected_text(window, cx);
        let text = text.trim();
        if text.chars().count() < 2 {
            if self
                .model
                .cont
                .highlight
                .as_ref()
                .is_some_and(|h| h.entry == i)
            {
                self.model.cont.highlight = None;
                cx.notify();
            }
            return;
        }
        self.model.cont.highlight = Some(Highlight {
            entry: i,
            text: text.to_owned(),
            at: (f32::from(e.position.x), f32::from(e.position.y)),
            note: None,
        });
        cx.notify();
    }

    /// Where a highlight came from, for notes and cards.
    fn highlight_from(&self, entry: usize) -> String {
        let title = self
            .current
            .and_then(|c| self.model.sessions.iter().find(|s| s.id == c))
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "Chat".into());
        match self.model.label_of_entry(entry) {
            Some(l) => format!("{title} · {l}"),
            None => title,
        }
    }

    fn highlight_action(&mut self, a: Action, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.model.cont.highlight.clone() else {
            return;
        };
        let from = self.highlight_from(h.entry);
        match a {
            Action::Explain => {
                self.open_side(Some(h.text.clone()), window, cx);
                self.side_ask(Action::explain_prompt(&h.text), cx);
                self.model.cont.highlight = None;
            }
            Action::AskSide => {
                self.open_side(Some(h.text.clone()), window, cx);
                self.model.cont.highlight = None;
            }
            Action::AddToNotes => {
                self.set_highlight_note("Adding to your notes…");
                self.bridge.highlight_note(h.text, from);
            }
            Action::Flashcard => {
                self.set_highlight_note("Making a flashcard…");
                self.bridge.highlight_card(h.text, from);
            }
            Action::Verify => {
                let Some(Entry::Assistant { text, pages, .. }) = self.model.entries.get(h.entry)
                else {
                    return;
                };
                let (text, pages) = (text.clone(), pages.clone());
                self.set_highlight_note("Checking it against the pages this answer read…");
                self.bridge.highlight_verify(h.text, text, pages);
            }
        }
        cx.notify();
    }

    fn set_highlight_note(&mut self, note: &str) {
        if let Some(h) = self.model.cont.highlight.as_mut() {
            h.note = Some(note.to_owned());
        }
    }

    /// The popover over selected answer text.
    pub(super) fn highlight_popover(
        &self,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.model.screen != Screen::Chat {
            return None;
        }
        let h = self.model.cont.highlight.as_ref()?;
        let actions = [
            (Action::Explain, AssetIcon::Lightbulb),
            (Action::Verify, AssetIcon::ShieldCheck),
            (Action::AddToNotes, AssetIcon::NotebookPen),
            (Action::Flashcard, AssetIcon::Layers),
            (Action::AskSide, AssetIcon::MessageCircle),
        ];
        let (x, y) = h.at;
        Some(
            div()
                .absolute()
                .left(px((x - 160.).max(8.)))
                .top(px(y + 12.))
                .flex()
                .flex_col()
                .gap_1()
                .p_1()
                .max_w(px(460.))
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .shadow_lg()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .children(actions.into_iter().map(|(a, ic)| {
                            div()
                                .id(SharedString::from(format!("hl-{}", a.label())))
                                .flex()
                                .items_center()
                                .gap_1()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .cursor_pointer()
                                .text_color(p.muted)
                                .hover(move |d| d.bg(p.hover).text_color(p.text))
                                .child(icon(ic, p.muted))
                                .child(a.label())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.highlight_action(a, window, cx)
                                }))
                        }))
                        .child(
                            Button::new("hl-close")
                                .icon(AssetIcon::X)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.cont.highlight = None;
                                    cx.notify();
                                })),
                        ),
                )
                .when_some(h.note.clone(), |d, n| {
                    d.child(div().px_2().pb_1().text_xs().text_color(p.faint).child(n))
                })
                .into_any_element(),
        )
    }

    // ---- Side chat -----------------------------------------------------

    /// Opens the side chat over the open chat, with optional focus text.
    pub(super) fn open_side(
        &mut self,
        focus: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.current else {
            return;
        };
        if self.model.cont.side.open_for(session) {
            self.bridge.side_open(session);
        }
        if focus.is_some() {
            self.model.cont.side.focus = focus;
        }
        let popped = self.model.cont.side.popped
            && self.side_window.as_ref().is_some_and(|w| {
                w.update(cx, |_, window, _| window.activate_window())
                    .is_ok()
            });
        if !popped {
            self.model.cont.side.popped = false;
            self.side_window = None;
            self.model.run_panel_open = true;
            self.side_input.update(cx, |i, cx| i.focus(window, cx));
        }
        cx.notify();
    }

    pub(super) fn side_ask(&mut self, question: String, cx: &mut Context<Self>) {
        let focus = self.model.cont.side.focus.clone();
        if self.model.cont.side.asked(&question) {
            self.bridge.side_ask(question, focus);
        }
        cx.notify();
    }

    pub(super) fn side_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.side_input.read(cx).value().trim().to_owned();
        if q.is_empty() || self.model.cont.side.busy() {
            return;
        }
        self.side_input
            .update(cx, |i, cx| i.set_value("", window, cx));
        self.side_ask(q, cx);
    }

    pub(super) fn close_side(&mut self, cx: &mut Context<Self>) {
        self.model.cont.side.close();
        if let Some(w) = self.side_window.take() {
            let _ = w.update(cx, |_, window, _| window.remove_window());
        }
        cx.notify();
    }

    /// Moves the side chat into its own window, with the tern mark at its
    /// top left.
    fn pop_out_side(&mut self, cx: &mut Context<Self>) {
        let ws = cx.entity().downgrade();
        let bounds = gpui_kit::Bounds::centered(None, gpui_kit::size(px(420.), px(560.)), cx);
        let options = gpui_kit::WindowOptions {
            window_bounds: Some(gpui_kit::WindowBounds::Windowed(bounds)),
            window_min_size: Some(gpui_kit::size(px(320.), px(360.))),
            app_id: Some(crate::APP_ID.into()),
            ..gpui_kit::component::TitleBar::window_options()
        };
        match gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| SidePopout::new(ws, window, cx))
        }) {
            Ok(handle) => {
                self.side_window = Some(handle.0);
                self.model.cont.side.popped = true;
                if self.model.browser.visible {
                    // Leave the browser where it was.
                } else {
                    self.model.run_panel_open = false;
                }
            }
            Err(e) => self.model.entries.push(Entry::Notice(format!(
                "Tern could not pop the side chat out: {e}"
            ))),
        }
        cx.notify();
    }

    /// The side chat's lines, shared by the docked pane and the popout.
    pub(super) fn side_lines(&self, p: Palette) -> AnyElement {
        let side = &self.model.cont.side;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .when(side.lines.is_empty() && !side.opening, |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(p.faint)
                        .child("Ask a quick question about this chat. It reads the thread; nothing you ask here is saved to it."),
                )
            })
            .when(side.opening, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(p.faint)
                        .child(Spinner::new().xsmall())
                        .child("Reading the thread…"),
                )
            })
            .children(side.lines.iter().enumerate().map(|(k, l)| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .self_end()
                            .px_2()
                            .py_1()
                            .rounded_lg()
                            .bg(p.hover)
                            .text_sm()
                            .child(SharedString::from(l.question.clone())),
                    )
                    .child(match &l.answer {
                        Some(a) if l.failed => div()
                            .text_sm()
                            .text_color(p.muted)
                            .child(SharedString::from(a.clone()))
                            .into_any_element(),
                        Some(a) => TextView::markdown(("side-answer", k), a.clone())
                            .selectable(true)
                            .into_any_element(),
                        None => div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .text_color(p.faint)
                            .child(Spinner::new().xsmall())
                            .child("Thinking…")
                            .into_any_element(),
                    })
            }))
            .into_any_element()
    }

    /// The docked side chat in the right pane.
    pub(super) fn side_panel(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let side = &self.model.cont.side;
        div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .bg(p.sidebar)
            .border_l_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(p.border)
                    .child(icon(AssetIcon::MessageCircle, p.muted))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Side chat"),
                    )
                    .child(div().text_xs().text_color(p.faint).child("not saved"))
                    .child(
                        Button::new("side-pop")
                            .icon(AssetIcon::PictureInPicture2)
                            .ghost()
                            .xsmall()
                            .tooltip("Pop out")
                            .on_click(cx.listener(|this, _, _, cx| this.pop_out_side(cx))),
                    )
                    .child(
                        Button::new("side-close")
                            .icon(AssetIcon::X)
                            .ghost()
                            .xsmall()
                            .tooltip("Close the side chat")
                            .on_click(cx.listener(|this, _, _, cx| this.close_side(cx))),
                    ),
            )
            .child(
                div()
                    .id("side-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(self.side_lines(p)),
            )
            .when_some(side.focus.clone(), |d, f| {
                d.child(
                    div()
                        .mx_3()
                        .pl_2()
                        .border_l_2()
                        .border_color(p.border)
                        .text_xs()
                        .text_color(p.muted)
                        .line_clamp(3)
                        .child(SharedString::from(f)),
                )
            })
            .child(
                div()
                    .p_3()
                    .child(Input::new(&self.side_input).small().disabled(side.busy())),
            )
            .into_any_element()
    }
}

/// The side chat in its own window: the tern mark top left, the same lines
/// and input, reading the main window's state.
pub struct SidePopout {
    ws: WeakEntity<Workspace>,
    input: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl SidePopout {
    fn new(ws: WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Ask about this chat"));
        let mut subs = vec![cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let q = input.read(cx).value().trim().to_owned();
                    if q.is_empty() {
                        return;
                    }
                    input.update(cx, |i, cx| i.set_value("", window, cx));
                    let _ = this.ws.update(cx, |w, cx| w.side_ask(q, cx));
                }
            },
        )];
        if let Some(entity) = ws.upgrade() {
            subs.push(cx.observe(&entity, |_, _, cx| cx.notify()));
        }
        Self {
            ws,
            input,
            _subs: subs,
        }
    }
}

impl Render for SidePopout {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ws) = self.ws.upgrade() else {
            return div().into_any_element();
        };
        let w = ws.read(cx);
        let p = Palette::get(w.model.dark);
        let lines = w.side_lines(p);
        let busy = w.model.cont.side.busy();
        let focus = w.model.cont.side.focus.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.canvas)
            .text_color(p.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(p.border)
                    .child(svg().path(assets::MARK).size(px(15.)).text_color(p.text))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Side chat"),
                    )
                    .child(div().text_xs().text_color(p.faint).child("not saved"))
                    .child(
                        Button::new("popout-dock")
                            .icon(AssetIcon::PanelRightClose)
                            .ghost()
                            .xsmall()
                            .tooltip("Dock in the main window")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let _ = this.ws.update(cx, |w, cx| {
                                    w.model.cont.side.popped = false;
                                    w.side_window = None;
                                    w.model.run_panel_open = true;
                                    cx.notify();
                                });
                                window.remove_window();
                            })),
                    ),
            )
            .child(
                div()
                    .id("popout-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(lines),
            )
            .when_some(focus, |d, f| {
                d.child(
                    div()
                        .mx_3()
                        .pl_2()
                        .border_l_2()
                        .border_color(p.border)
                        .text_xs()
                        .text_color(p.muted)
                        .line_clamp(3)
                        .child(SharedString::from(f)),
                )
            })
            .child(
                div()
                    .p_3()
                    .child(Input::new(&self.input).small().disabled(busy)),
            )
            .into_any_element()
    }
}
