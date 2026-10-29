//! The Editor screen: the vault's notes on the left, the open note on the
//! right (text or preview), its links and backlinks under it. Edits save
//! on their own a moment after typing stops.

use std::time::Duration;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, Window, div, px, relative};

use super::Workspace;
use super::press::Capsule as _;
use crate::editor::Link;
use crate::model::Screen;
use crate::theme::Palette;

/// Quiet time after the last keystroke before a note saves.
const AUTOSAVE: Duration = Duration::from_millis(700);
const LIST_W: f32 = 232.;

impl Workspace {
    pub(super) fn open_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.screen = Screen::Editor;
        self.model.editor.refresh();
        self.model.editor.check_disk();
        if self.model.editor.open.is_none()
            && let Some(first) = self.model.editor.notes.first().map(|n| n.path.clone())
        {
            self.open_note(&first, window, cx);
        }
        cx.notify();
    }

    pub(super) fn close_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_note(cx);
        if self.model.git.status().is_some() {
            self.git_op(crate::git::GitOp::Refresh, cx);
        }
        self.model.screen = Screen::Chat;
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    pub(super) fn open_note(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.save_note(cx);
        if let Ok(text) = self.model.editor.open_note(path) {
            self.note_input.update(cx, |i, cx| {
                i.set_value(text, window, cx);
                i.focus(window, cx);
            });
        }
        cx.notify();
    }

    /// Accepts or rejects the pending change to a note.
    fn answer_proposal(&mut self, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.model.editor.open.clone();
        if accept {
            match self.model.editor.accept() {
                Ok(Some((path, text))) => {
                    if open.as_deref() == Some(path.as_str()) {
                        self.note_input
                            .update(cx, |i, cx| i.set_value(text, window, cx));
                    }
                    self.model
                        .toast_info(format!("Accepted the change to {path}"));
                }
                Ok(None) => {}
                Err(e) => self
                    .model
                    .toast_error(format!("Could not write the note: {e}"), None),
            }
        } else if let Err(e) = self.model.editor.reject() {
            self.model
                .toast_error(format!("Could not restore the note: {e}"), None);
        } else {
            self.model
                .toast_info("Rejected the change; the note is as you had it");
        }
        cx.notify();
    }

    /// The pending change as a line diff with Accept and Reject.
    fn proposal_card(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let prop = self.model.editor.proposal.as_ref()?;
        let lines = crate::editor::diff(&prop.before, &prop.after);
        let added = lines
            .iter()
            .filter(|l| matches!(l, crate::editor::Line::Added(_)))
            .count();
        let removed = lines
            .iter()
            .filter(|l| matches!(l, crate::editor::Line::Removed(_)))
            .count();
        // Changed lines with one line of context each side.
        let changed: Vec<bool> = lines
            .iter()
            .map(|l| !matches!(l, crate::editor::Line::Same(_)))
            .collect();
        let near = |k: usize| {
            changed[k.saturating_sub(1)..(k + 2).min(changed.len())]
                .iter()
                .any(|c| *c)
        };
        let rows: Vec<AnyElement> = lines
            .iter()
            .enumerate()
            .filter(|(k, _)| near(*k))
            .take(200)
            .map(|(_, l)| {
                let (mark, text, bg, color) = match l {
                    crate::editor::Line::Same(t) => (" ", *t, None, p.muted),
                    crate::editor::Line::Added(t) => {
                        ("+", *t, Some(p.accent.opacity(0.12)), p.text)
                    }
                    crate::editor::Line::Removed(t) => ("−", *t, Some(p.hover), p.faint),
                };
                div()
                    .flex()
                    .gap_2()
                    .px_2()
                    .text_xs()
                    .text_color(color)
                    .when_some(bg, |d, bg| d.bg(bg))
                    .child(div().w(px(10.)).flex_none().child(mark))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(matches!(l, crate::editor::Line::Removed(_)), |d| {
                                d.line_through()
                            })
                            .child(SharedString::from(text.to_owned())),
                    )
                    .into_any_element()
            })
            .collect();
        Some(
            div()
                .id("note-proposal")
                .flex()
                .flex_col()
                .gap_2()
                .mx_4()
                .my_2()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .child(div().text_color(p.accent).child("◆"))
                        .child(SharedString::from(format!(
                            "{} · {}",
                            prop.source, prop.path
                        )))
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(format!("+{added} −{removed}"))),
                        ),
                )
                .child(
                    div()
                        .id("note-proposal-diff")
                        .max_h(px(260.))
                        .overflow_y_scrollbar()
                        .rounded_md()
                        .border_1()
                        .border_color(p.border)
                        .bg(p.canvas)
                        .py_1()
                        .children(rows),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("note-accept")
                                .capsule()
                                .label("Accept")
                                .primary()
                                .xsmall()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.answer_proposal(true, window, cx)
                                })),
                        )
                        .child(
                            Button::new("note-reject")
                                .capsule()
                                .label(if prop.on_disk {
                                    "Reject and restore mine"
                                } else {
                                    "Reject"
                                })
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.answer_proposal(false, window, cx)
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Saves the open note now, if it changed.
    pub(super) fn save_note(&mut self, cx: &mut Context<Self>) {
        let text = self.note_input.read(cx).value().to_string();
        let _ = self.model.editor.save(&text);
    }

    /// The note text changed: marks it unsaved and saves after a pause.
    pub(super) fn note_edited(&mut self, cx: &mut Context<Self>) {
        let text = self.note_input.read(cx).value().to_string();
        let generation = self.model.editor.edited(&text);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE).await;
            let _ = this.update(cx, |this, cx| {
                if this.model.editor.generation == generation {
                    this.save_note(cx);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_note(cx);
        if let Ok((_, text)) = self.model.editor.new_note() {
            self.note_input.update(cx, |i, cx| {
                i.set_value(text, window, cx);
                i.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn follow_link(&mut self, link: Link, window: &mut Window, cx: &mut Context<Self>) {
        self.save_note(cx);
        if let Ok(path) = self.model.editor.follow(&link) {
            self.open_note(&path, window, cx);
        }
    }

    /// Saves answer `i` as a new note in the vault.
    pub(super) fn save_answer(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some((q, a, sources)) = self.model.answer_for_note(i) {
            match self.model.editor.save_answer(&q, &a, &sources) {
                Ok(_) => self.model.saved_answer = Some(i),
                Err(e) if self.model.editor.proposal.is_some() => self.model.toast_info(e),
                Err(e) => self
                    .model
                    .toast_error(format!("Could not save the note: {e}"), None),
            }
        }
        cx.notify();
    }

    pub(super) fn editor_screen(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let ed = &self.model.editor;
        let open = ed.open.clone();
        let rows: Vec<AnyElement> = ed
            .notes
            .iter()
            .enumerate()
            .map(|(ix, n)| {
                let path = n.path.clone();
                let on = open.as_deref() == Some(n.path.as_str());
                div()
                    .id(("note", ix))
                    .flex()
                    .flex_col()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .when(on, |d| d.bg(p.hover))
                    .hover(move |d| d.bg(p.hover))
                    .child(
                        div()
                            .truncate()
                            .text_color(if on { p.text } else { p.muted })
                            .child(SharedString::from(n.name().to_owned())),
                    )
                    .when(!n.folder().is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .truncate()
                                .child(SharedString::from(n.folder().to_owned())),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_note(&path, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        let list = div()
            .w(px(LIST_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(p.border)
            .bg(p.sidebar)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .pt_3()
                    .pb_2()
                    .child(div().text_xs().text_color(p.faint).child("Vault"))
                    .child(
                        Button::new("new-note")
                            .capsule()
                            .icon(AssetIcon::FilePlus)
                            .ghost()
                            .xsmall()
                            .tooltip("New note")
                            .on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx))),
                    ),
            )
            .child(
                div()
                    .id("note-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .px_1()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .children(rows),
            )
            .when_some(ed.vault.as_ref(), |d, v| {
                d.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(p.border)
                        .text_xs()
                        .text_color(p.faint)
                        .truncate()
                        .child(SharedString::from(
                            tern_agent::projects::plain_path(v.root())
                                .display()
                                .to_string(),
                        )),
                )
            });

        let status = match (&ed.error, ed.dirty) {
            (Some(e), _) => format!("Not saved: {e}"),
            (None, true) => "Editing…".to_owned(),
            (None, false) => "Saved".to_owned(),
        };
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .child(SharedString::from(
                        open.clone().unwrap_or_else(|| "No note open".into()),
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_xs().text_color(p.faint).child(status))
                    .child(
                        Button::new("note-preview")
                            .capsule()
                            .icon(AssetIcon::Eye)
                            .ghost()
                            .xsmall()
                            .selected(ed.preview)
                            .tooltip("Preview")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.save_note(cx);
                                this.model.editor.preview = !this.model.editor.preview;
                                cx.notify();
                            })),
                    ),
            );

        let body = if open.is_none() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(p.muted)
                .child("Pick a note, or make a new one.")
                .into_any_element()
        } else if ed.preview {
            div()
                .id("note-preview-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .px_6()
                .py_4()
                .child(
                    TextView::markdown("note-md", self.note_input.read(cx).value().to_string())
                        .selectable(true),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .px_4()
                .py_2()
                .child(
                    Textarea::new(&self.note_input)
                        .appearance(false)
                        .h(relative(1.)),
                )
                .into_any_element()
        };

        let chips = |title: &'static str, items: Vec<(String, AnyElement)>| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .child(div().text_xs().text_color(p.faint).mr_1().child(title))
                .children(items.into_iter().map(|(_, e)| e))
        };
        let link_chips: Vec<(String, AnyElement)> = ed
            .links
            .iter()
            .enumerate()
            .map(|(ix, l)| {
                let link = l.clone();
                let missing = l.note.is_none();
                (
                    l.target.clone(),
                    div()
                        .id(("note-link", ix))
                        .px_2()
                        .py(px(1.))
                        .rounded_md()
                        .border_1()
                        .border_color(p.border)
                        .text_xs()
                        .cursor_pointer()
                        .text_color(if missing { p.faint } else { p.text })
                        .hover(move |d| d.bg(p.hover))
                        .child(SharedString::from(l.target.clone()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.follow_link(link.clone(), window, cx);
                        }))
                        .into_any_element(),
                )
            })
            .collect();
        let back_chips: Vec<(String, AnyElement)> = ed
            .backlinks
            .iter()
            .enumerate()
            .map(|(ix, n)| {
                let path = n.path.clone();
                (
                    n.path.clone(),
                    div()
                        .id(("backlink", ix))
                        .px_2()
                        .py(px(1.))
                        .rounded_md()
                        .border_1()
                        .border_color(p.border)
                        .text_xs()
                        .cursor_pointer()
                        .hover(move |d| d.bg(p.hover))
                        .child(SharedString::from(n.name().to_owned()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_note(&path, window, cx);
                        }))
                        .into_any_element(),
                )
            })
            .collect();
        let has_links = !link_chips.is_empty() || !back_chips.is_empty();
        let footer = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(p.border)
            .when(!link_chips.is_empty(), |d| {
                d.child(chips("Links", link_chips))
            })
            .when(!back_chips.is_empty(), |d| {
                d.child(chips("Linked from", back_chips))
            });

        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h_0()
            .child(list)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(header)
                    .children(self.proposal_card(p, cx))
                    .child(body)
                    .when(has_links, |d| d.child(footer)),
            )
            .into_any_element()
    }
}
