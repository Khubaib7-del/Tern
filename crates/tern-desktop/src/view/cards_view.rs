//! Flashcards in the Study screen: the review (one card at a time, turned
//! over on the motion curve, answered Again, Hard, Good, or Easy with the
//! mouse or keys 1 to 4, while Scout holds up a card and reacts), and the
//! deck (filter by topic, edit, suspend, delete, write more, export to
//! Anki, CSV, or TSV, and import).

use std::path::PathBuf;
use std::time::Instant;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{Disableable as _, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, Window, div, px};
use tern_agent::cards::{self as cards, Card, CardKind, CardSpec, Format, Rating};

use super::Workspace;
use super::press::Capsule as _;
use super::study_view::{PILL, serif};
use crate::bridge::CardChange;
use crate::study::{ReviewRun, StudyView};
use crate::theme::Palette;

/// The card's full width.
const CARD_W: f32 = 560.;

/// Where file dialogs start.
pub(super) fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn due_label(c: &Card, now: i64) -> String {
    if c.suspended {
        return "Suspended".into();
    }
    match c.sched.state {
        cards::CardState::New => "New".into(),
        _ if c.sched.due <= now => "Due now".into(),
        _ => format!("Due in {}", cards::wait_label(c.sched.due - now)),
    }
}

impl Workspace {
    /// Opens a review of today's cards (on one topic, or all).
    pub(super) fn start_review(
        &mut self,
        topic: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.model
            .study
            .open_review(topic, tern_agent::study::now());
        self.keys.focus(window, cx);
        cx.notify();
    }

    pub(super) fn flip_card(&mut self, cx: &mut Context<Self>) {
        self.model.study.flip(Instant::now());
        cx.notify();
    }

    pub(super) fn rate_card(&mut self, rating: Rating, cx: &mut Context<Self>) {
        let now = tern_agent::study::now();
        if let Some((id, r)) = self.model.study.rate(rating, now) {
            self.bridge.cards_review(id, r, now);
            match r {
                Rating::Again => self.scout.react(false),
                Rating::Good | Rating::Easy => self.scout.react(true),
                Rating::Hard => {}
            }
        }
        cx.notify();
    }

    /// Space or Enter turns the card; 1 to 4 answer it.
    fn review_keys(&mut self, e: &gpui_kit::KeyDownEvent, cx: &mut Context<Self>) {
        let flipped = self.model.study.review.as_ref().is_some_and(|r| r.flipped);
        match e.keystroke.key.as_str() {
            "space" | "enter" => self.flip_card(cx),
            k if flipped => match Rating::parse(k) {
                Some(r) if k.len() == 1 && k.chars().all(|c| c.is_ascii_digit()) => {
                    self.rate_card(r, cx)
                }
                _ => return,
            },
            _ => return,
        }
        cx.stop_propagation();
    }

    pub(super) fn gen_cards(&mut self, topic: Option<String>, cx: &mut Context<Self>) {
        let s = &mut self.model.study;
        s.busy = Some("Writing flashcards from your materials…".into());
        s.notice = None;
        self.bridge.cards_gen(CardSpec {
            topic,
            ..CardSpec::default()
        });
        cx.notify();
    }

    fn export_cards(&mut self, format: Format, cx: &mut Context<Self>) {
        let ext = match format {
            Format::Apkg => "apkg",
            Format::Csv => "csv",
            Format::Tsv => "tsv",
        };
        let topic = self.model.study.deck_topic.clone();
        let name = format!(
            "{}.{ext}",
            tern_agent::vault::clean_name(&self.model.study.deck.name)
        );
        let picked = cx.prompt_for_new_path(&home_dir(), Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = picked.await {
                let _ = this.update(cx, |this, cx| {
                    this.model.study.busy = Some(format!("Exporting ({})…", format.label()));
                    this.bridge.cards_export(path, format, topic);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn import_cards(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import cards".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    this.model.study.busy = Some("Importing cards…".into());
                    this.bridge.cards_import(path);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn edit_card(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.model.study.deck.get(&id).cloned() else {
            return;
        };
        self.study_inputs
            .card_front
            .update(cx, |i, cx| i.set_value(c.front.clone(), window, cx));
        self.study_inputs
            .card_back
            .update(cx, |i, cx| i.set_value(c.back.clone(), window, cx));
        self.model.study.editing = Some(id);
        cx.notify();
    }

    fn save_card(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.model.study.editing.take() else {
            return;
        };
        let front = self.study_inputs.card_front.read(cx).value().to_string();
        let back = self.study_inputs.card_back.read(cx).value().to_string();
        self.bridge.cards_change(CardChange::Edit(id, front, back));
        cx.notify();
    }

    // ---- Rendering ----

    pub(super) fn review_view(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.model.study;
        let now_i = tern_agent::study::now();
        let now = Instant::now();
        let title = s
            .review
            .as_ref()
            .and_then(|r| r.topic.as_ref())
            .and_then(|t| s.library.topics.iter().find(|x| &x.id == t))
            .map(|t| format!("Flashcards · {}", t.title))
            .unwrap_or_else(|| "Flashcards".into());
        let mut col = div()
            .id("review")
            .key_context("Review")
            .track_focus(&self.keys)
            .on_key_down(cx.listener(|this, e: &gpui_kit::KeyDownEvent, _, cx| {
                this.review_keys(e, cx);
            }))
            .flex()
            .flex_col()
            .gap_3()
            .child(self.back_row(title, p, cx));
        let Some(run) = &s.review else {
            return col.into_any_element();
        };
        let left = s.due(run.topic.as_deref(), now_i);
        col = col.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(format!(
                            "{} reviewed · {} left today{}",
                            run.done,
                            left,
                            match run.last {
                                Some(r) => format!(" · last: {}", r.label()),
                                None => String::new(),
                            }
                        ))),
                )
                .child(self.pet_figure(56., p)),
        );
        let Some(card) = &run.card else {
            let stats = s.deck.stats(now_i, s.offset);
            return col
                .child(
                    self.card(p)
                        .child(serif("All caught up.", p).text_xl())
                        .child(
                            div()
                                .text_sm()
                                .text_color(p.muted)
                                .child(SharedString::from(stats.line())),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .pt_2()
                                .child(
                                    Button::new("review-home")
                                        .capsule()
                                        .label("Back to Study")
                                        .small()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.model.study.go_home();
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("review-deck")
                                        .capsule()
                                        .icon(AssetIcon::WalletCards)
                                        .label("Open the deck")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.model.study.view = StudyView::Deck;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
                .into_any_element();
        };
        // The card, turning over on the motion curve.
        let t = run.turn(now, self.reduced_motion);
        let (width, back) = ReviewRun::face(t);
        let side = if back {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_sm().text_color(p.faint).child(card.question()))
                .child(div().h(px(1.)).bg(p.border))
                .child(serif(card.answer(), p).text_lg())
                .children(card.anchor.as_ref().map(|a| {
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(format!("Source: {}", a.label())))
                }))
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(serif(card.question(), p).text_xl())
        };
        let kind = match card.kind {
            CardKind::Basic => "Question",
            CardKind::Cloze => "Fill the gap",
            CardKind::Why => "Explain why",
        };
        let face = div()
            .id("flashcard")
            .w(px(CARD_W * width))
            .min_h(px(220.))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .rounded(px(PILL))
            .border_1()
            .border_color(if back { p.accent } else { p.border })
            .bg(p.canvas)
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.flip_card(cx)))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(p.faint)
                    .child(kind)
                    .child(SharedString::from(due_label(card, now_i))),
            )
            .when(width > 0.3, |d| d.child(side));
        col = col.child(div().flex().justify_center().child(face));
        if !back || !run.flipped {
            col = col.child(
                div().flex().justify_center().child(
                    Button::new("show-answer")
                        .capsule()
                        .label("Show answer  (Space)")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.flip_card(cx))),
                ),
            );
        } else {
            let hints = cards::button_hints(card, now_i);
            let buttons = Rating::ALL.into_iter().zip(hints).map(|(r, h)| {
                Button::new(SharedString::from(format!("rate-{}", r.key())))
                    .capsule()
                    .label(SharedString::from(format!(
                        "{}  {} · {h}",
                        r.key(),
                        r.label()
                    )))
                    .small()
                    .when(r == Rating::Good, |b| b.primary())
                    .when(r != Rating::Good, |b| b.ghost())
                    .on_click(cx.listener(move |this, _, _, cx| this.rate_card(r, cx)))
            });
            col = col.child(div().flex().justify_center().gap_2().children(buttons));
        }
        col.child(
            div()
                .text_xs()
                .text_color(p.faint)
                .flex()
                .justify_center()
                .child("Space turns the card · 1 Again · 2 Hard · 3 Good · 4 Easy"),
        )
        .into_any_element()
    }

    pub(super) fn deck_view(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let s = &self.model.study;
        let now = tern_agent::study::now();
        let busy = s.busy.is_some();
        let stats = s.deck.stats(now, s.offset);
        let mut col = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.back_row("Deck".into(), p, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(stats.line())),
            );
        // Filter chips: every topic with cards.
        let mut chips = vec![
            Button::new("deck-all")
                .capsule()
                .label("All")
                .ghost()
                .xsmall()
                .selected(s.deck_topic.is_none())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model.study.deck_topic = None;
                    cx.notify();
                })),
        ];
        for (i, t) in s.library.topics.iter().enumerate() {
            let n = s.deck.for_topic(&t.id).count();
            if n == 0 {
                continue;
            }
            let id = t.id.clone();
            chips.push(
                Button::new(SharedString::from(format!("deck-topic-{i}")))
                    .capsule()
                    .label(SharedString::from(format!(
                        "{} · {n}",
                        tern_agent::study::clip(&t.title, 24)
                    )))
                    .ghost()
                    .xsmall()
                    .selected(s.deck_topic.as_ref() == Some(&t.id))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.study.deck_topic = Some(id.clone());
                        cx.notify();
                    })),
            );
        }
        col = col.child(div().flex().flex_wrap().gap_1().children(chips));
        let topic = s.deck_topic.clone();
        let review_topic = topic.clone();
        let toolbar = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .child(
                Button::new("deck-review").capsule()
                    .icon(AssetIcon::Play)
                    .label(SharedString::from(format!(
                        "Review {}",
                        s.due(topic.as_deref(), now)
                    )))
                    .small()
                    .disabled(s.due(topic.as_deref(), now) == 0)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_review(review_topic.clone(), window, cx);
                    })),
            )
            .child(
                Button::new("deck-gen").capsule()
                    .icon(AssetIcon::Plus)
                    .label("Write cards")
                    .ghost()
                    .small()
                    .disabled(busy || s.library.materials.is_empty())
                    .tooltip("Write cards from your materials (this topic, or all); repeats are skipped")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.gen_cards(topic.clone(), cx);
                    })),
            )
            .child(
                Button::new("deck-import").capsule()
                    .icon(AssetIcon::Import)
                    .label("Import")
                    .ghost()
                    .small()
                    .disabled(busy)
                    .tooltip("Import cards from CSV or TSV (Anki's plain-text export works)")
                    .on_click(cx.listener(|this, _, _, cx| this.import_cards(cx))),
            )
            .children(
                [
                    (Format::Apkg, "Anki .apkg"),
                    (Format::Csv, "CSV"),
                    (Format::Tsv, "TSV"),
                ]
                .map(|(f, label)| {
                    Button::new(SharedString::from(format!("deck-export-{label}"))).capsule()
                        .icon(AssetIcon::Download)
                        .label(label)
                        .ghost()
                        .small()
                        .disabled(busy || s.deck.cards.is_empty())
                        .tooltip(match f {
                            Format::Apkg => "A package Anki opens directly: notes, tags per topic, the source in its own field, and your schedule",
                            Format::Csv => "Tern's plain CSV: front, back, kind, topic, tags, source",
                            Format::Tsv => "Text for Anki's File › Import, with its header lines",
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.export_cards(f, cx)))
                }),
            );
        col = col.child(toolbar);
        let shown = s.deck_cards();
        let mut list = self.card(p);
        if shown.is_empty() {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(p.faint)
                    .child("No cards yet. Write some from your materials, or import a CSV."),
            );
        }
        for c in shown {
            let id = c.id.clone();
            if s.editing.as_ref() == Some(&c.id) {
                list = list.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .py_2()
                        .child(div().text_xs().text_color(p.faint).child(match c.kind {
                            CardKind::Cloze => "Text (keep the {{c1::…}} gaps)",
                            _ => "Front",
                        }))
                        .child(Input::new(&self.study_inputs.card_front).small())
                        .child(div().text_xs().text_color(p.faint).child(match c.kind {
                            CardKind::Cloze => "Extra (optional)",
                            _ => "Back",
                        }))
                        .child(Input::new(&self.study_inputs.card_back).small())
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .child(
                                    Button::new("card-save")
                                        .capsule()
                                        .label("Save")
                                        .small()
                                        .on_click(cx.listener(|this, _, _, cx| this.save_card(cx))),
                                )
                                .child(
                                    Button::new("card-cancel")
                                        .capsule()
                                        .label("Cancel")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.model.study.editing = None;
                                            cx.notify();
                                        })),
                                ),
                        ),
                );
                continue;
            }
            let (edit_id, susp_id, del_id) = (id.clone(), id.clone(), id.clone());
            let suspended = c.suspended;
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .when(suspended, |d| d.opacity(0.55))
                    .child(
                        div()
                            .w(px(76.))
                            .flex_none()
                            .text_xs()
                            .text_color(p.faint)
                            .child(c.kind.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().truncate().child(c.question()))
                            .child(div().text_xs().text_color(p.faint).truncate().child(
                                SharedString::from(match &c.anchor {
                                    Some(a) => format!("{} · {}", due_label(c, now), a.label()),
                                    None => due_label(c, now),
                                }),
                            )),
                    )
                    .child(
                        Button::new(SharedString::from(format!("card-edit-{id}")))
                            .capsule()
                            .icon(AssetIcon::Pencil)
                            .ghost()
                            .xsmall()
                            .tooltip("Edit")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_card(edit_id.clone(), window, cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("card-susp-{id}")))
                            .capsule()
                            .icon(if suspended {
                                AssetIcon::Eye
                            } else {
                                AssetIcon::EyeOff
                            })
                            .ghost()
                            .xsmall()
                            .tooltip(if suspended {
                                "Bring back into review"
                            } else {
                                "Suspend: it stops coming up"
                            })
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.bridge
                                    .cards_change(CardChange::Suspend(susp_id.clone(), !suspended));
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("card-del-{id}")))
                            .capsule()
                            .icon(AssetIcon::X)
                            .ghost()
                            .xsmall()
                            .tooltip("Delete")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.bridge.cards_change(CardChange::Delete(del_id.clone()));
                                cx.notify();
                            })),
                    ),
            );
        }
        col.child(list).into_any_element()
    }
}
