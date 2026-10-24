//! Checkpoints, steering, watches, and ledgers in the window: the steering
//! hint over the composer while a run goes, the Resume banner for a stopped
//! run, the run panel's Ledger view and its steering and resume lines, the
//! sidebar's Watches list, and the Watches screen with what changed.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, div, px, svg};
use tern_agent::ledger::{SourceEntry, SourceFate};
use tern_agent::run::compact;
use tern_agent::watch::{ChangedSource, Watch};
use tern_agent::{PageRef, Run};

use super::{Workspace, verdict_badge};
use crate::assets;
use crate::model::Screen;
use crate::theme::Palette;

/// Daily and weekly, the intervals the Watches screen offers.
const EVERY: [(u32, &str); 2] = [(24, "Daily"), (168, "Weekly")];

fn glyph(name: &str, size: f32, color: gpui_kit::Hsla) -> AnyElement {
    match assets::glyph(name) {
        Some(path) => svg()
            .path(path)
            .size(px(size))
            .flex_none()
            .text_color(color)
            .into_any_element(),
        None => div().size(px(size)).flex_none().into_any_element(),
    }
}

fn page_of_entry(s: &SourceEntry) -> PageRef {
    PageRef {
        title: s.title.clone(),
        url: s.url.clone(),
        site: s.site.clone(),
        builtin: None,
        favicon: None,
    }
}

fn page_of_change(s: &ChangedSource) -> PageRef {
    PageRef {
        title: s.title.clone(),
        url: s.url.clone(),
        site: s.site.clone(),
        builtin: None,
        favicon: None,
    }
}

impl Workspace {
    /// While a run goes, the composer sends steering notes; this says so.
    pub(super) fn steer_hint(&self, p: Palette) -> Option<AnyElement> {
        if !self.model.can_steer() {
            return None;
        }
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_1()
                .text_xs()
                .text_color(p.faint)
                .child(glyph("steer", 12., p.muted))
                .child(
                    div().flex_1().min_w_0().truncate().child(
                        "Enter steers the run: “focus on 2024 sources”, “drop 3”, “add: costs”, “redirect 2: …”",
                    ),
                )
                .into_any_element(),
        )
    }

    /// Over the composer when the chat's last run stopped or failed and
    /// left something to pick up: what happened, where it stands (from the
    /// workers as they are), when on the local clock, and Resume or Retry.
    /// Lines wrap, and the buttons sit on their own row, so nothing leaves
    /// the column.
    pub(super) fn resume_banner(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let offer = self.model.resume_offer.as_ref()?;
        if self.model.is_busy() {
            return None;
        }
        let action = offer.action();
        let tooltip = match offer.kind {
            tern_agent::OfferKind::Resume => {
                "Pick up from the checkpoint without paying again for finished work"
            }
            tern_agent::OfferKind::Retry => {
                "Run the failed workers again; workers that finished keep their notes"
            }
        };
        let about_search = offer.about_search;
        Some(
            div()
                .id("resume-banner")
                .flex()
                .flex_col()
                .gap_2()
                .px_2()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.canvas)
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .min_w_0()
                        .child(div().pt(px(2.)).child(glyph("checkpoint", 14., p.text)))
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
                                        .child(SharedString::from(offer.headline.clone())),
                                )
                                .child(div().text_xs().text_color(p.faint).child(
                                    SharedString::from(format!(
                                        "{} · saved {}",
                                        offer.progress, offer.saved_at
                                    )),
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .justify_end()
                        .gap_1()
                        .when(about_search, |d| {
                            d.child(
                                Button::new("resume-fix-search")
                                    .label("Fix search")
                                    .ghost()
                                    .small()
                                    .tooltip("Set up or test web search in Providers")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.open_providers(cx)),
                                    ),
                            )
                        })
                        .child(
                            Button::new("resume-dismiss")
                                .label("Dismiss")
                                .ghost()
                                .small()
                                .tooltip("Forget the stopped run")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(s) = &this.session {
                                        let _ = s.discard_checkpoint();
                                    }
                                    this.model.resume_offer = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("resume-run")
                                .label(action)
                                .primary()
                                .small()
                                .disabled(!self.model.can_send())
                                .tooltip(tooltip)
                                .on_click(cx.listener(|this, _, _, cx| this.resume_run(cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Starts resuming the chat's stopped run.
    pub(super) fn resume_run(&mut self, cx: &mut Context<Self>) {
        if !self.model.start_resume() {
            return;
        }
        let Some(session) = self.session.take() else {
            return;
        };
        self.cancel = tern_agent::CancelFlag::default();
        self.deep_control = tern_agent::DeepControl::new(self.cancel.clone());
        self.model.steering = Some(session.steering());
        self.model.run_panel_open = true;
        self.bridge.resume_run(session, self.deep_control.clone());
        self.scroll.scroll_to_bottom();
        self.start_ticker(cx);
        cx.notify();
    }

    /// Lines for the run panel's Summary: where a resumed run picked up,
    /// and each steering note with what it did.
    pub(super) fn run_extras(&self, run: &Run, p: Palette) -> Option<AnyElement> {
        if run.resumed.is_none() && run.steers.is_empty() {
            return None;
        }
        let line = |icon: &'static str, text: String, faint: bool| {
            div()
                .flex()
                .items_start()
                .gap_2()
                .py(px(3.))
                .child(div().pt(px(2.)).child(glyph(icon, 12., p.muted)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(if faint { p.faint } else { p.text })
                        .child(SharedString::from(text)),
                )
        };
        let mut col = div().flex().flex_col();
        if let Some(r) = &run.resumed {
            col = col.child(line(
                "checkpoint",
                format!("Resumed from a checkpoint · {}", r.progress),
                false,
            ));
        }
        if !run.steers.is_empty() {
            col = col.child(div().pt_2().text_xs().text_color(p.faint).child("Steering"));
            for s in &run.steers {
                let text = match s.applied {
                    None => format!("“{}” · waiting for the next safe point", s.text),
                    Some(_) => format!("“{}” · {}", s.text, s.outcome),
                };
                col = col.child(line("steer", text, s.applied == Some(false)));
            }
        }
        Some(col.into_any_element())
    }

    /// The run panel's Ledger view: every URL the run considered and what
    /// became of it, then what each lane used.
    pub(super) fn run_ledger(&self, run: &Run, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let ledger = run.ledger();
        if ledger.is_empty() {
            return div()
                .text_sm()
                .text_color(p.faint)
                .child("URLs the run considers and what each step spends show here.")
                .into_any_element();
        }
        let section = |title: &'static str| div().pt_3().text_xs().text_color(p.faint).child(title);
        let sources = ledger.sources.iter().enumerate().map(|(k, s)| {
            let url = s.url.clone();
            let pg = page_of_entry(s);
            let mut why = format!("{} · {}", s.fate.label(), s.reason.label());
            if let Some(w) = s.worker {
                why.push_str(&format!(" · worker {w}"));
            }
            if !s.detail.is_empty() && s.fate == SourceFate::Failed {
                why.push_str(&format!(" · {}", s.detail));
            }
            div()
                .id(SharedString::from(format!("ledger-src-{k}")))
                .flex()
                .items_start()
                .gap_2()
                .px_1()
                .py(px(3.))
                .rounded_md()
                .cursor_pointer()
                .hover(|d| d.bg(p.hover))
                .child(div().pt(px(1.)).child(self.site_icon(&pg, 14., p)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(if s.fate == SourceFate::Read {
                                    p.text
                                } else {
                                    p.muted
                                })
                                .child(SharedString::from(pg.label().to_owned())),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(why)),
                        ),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.browse(&url, window, cx)))
        });
        let lanes = ledger.lanes.iter().map(|l| {
            let tokens = l.tokens_in + l.tokens_out;
            let mut detail = format!(
                "{} model call{} · {} tokens",
                l.model_calls,
                if l.model_calls == 1 { "" } else { "s" },
                compact(tokens)
            );
            if let Some(b) = l.budget {
                detail.push_str(&format!(" of {}", compact(b.tokens)));
            }
            detail.push_str(&format!(
                " · {} page{} read",
                l.pages,
                if l.pages == 1 { "" } else { "s" }
            ));
            if let Some(b) = l.budget {
                detail.push_str(&format!(" (budget {})", b.pages));
            }
            detail.push_str(&format!(
                " · {} searches · {:.1}s",
                l.searches,
                l.ms as f64 / 1000.0
            ));
            div()
                .flex()
                .flex_col()
                .py(px(3.))
                .child(
                    div()
                        .text_sm()
                        .text_color(p.text)
                        .truncate()
                        .child(SharedString::from(l.label.clone())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(detail)),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .text_color(p.text)
                    .child(SharedString::from(ledger.counts().line())),
            )
            .child(section("Sources"))
            .children(sources)
            .child(section("Budget"))
            .children(lanes)
            .into_any_element()
    }

    /// The sidebar's Watches list, under the chats.
    pub(super) fn watches_section(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.model.watches.is_empty() {
            return None;
        }
        let running = self.model.watches_running > 0;
        let rows = self.model.watches.iter().enumerate().map(|(ix, w)| {
            let id = w.id.clone();
            let rm = w.id.clone();
            let selected = self.model.screen == Screen::Watches
                && self.model.watch_selected.as_ref() == Some(&w.id);
            let changed = w.last_changes.as_ref().is_some_and(|c| !c.is_empty());
            div()
                .id(("watch", ix))
                .group(SharedString::from(format!("watch-row-{ix}")))
                .flex()
                .items_start()
                .gap_2()
                .pl_2()
                .pr_1()
                .py(px(4.))
                .rounded_md()
                .cursor_pointer()
                .when(selected, |d| d.bg(p.hover))
                .hover(|d| d.bg(p.hover))
                .child(div().pt(px(2.)).child(glyph(
                    "watcher",
                    12.,
                    if changed { p.text } else { p.faint },
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_sm()
                                .truncate()
                                .text_color(p.muted)
                                .child(SharedString::from(w.question.clone())),
                        )
                        .child(
                            div()
                                .text_xs()
                                .truncate()
                                .text_color(p.faint)
                                .child(SharedString::from(w.status_line())),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .opacity(0.)
                        .group_hover(SharedString::from(format!("watch-row-{ix}")), |d| {
                            d.opacity(1.)
                        })
                        .child(
                            Button::new(("watch-row-rm", ix))
                                .icon(AssetIcon::Trash)
                                .ghost()
                                .xsmall()
                                .tooltip("Remove watch")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.remove_watch(rm.clone(), cx);
                                })),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.model.watch_selected = Some(id.clone());
                    this.model.screen = Screen::Watches;
                    cx.notify();
                }))
        });
        Some(
            div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(p.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_3()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .text_color(p.faint)
                        .child("Watches")
                        .when(running, |d| d.child(Spinner::new().xsmall().color(p.faint))),
                )
                .child(
                    div()
                        .id("watch-list")
                        .max_h(px(180.))
                        .overflow_y_scrollbar()
                        .px_2()
                        .pb_2()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    /// Puts the question before answer `i` on watch, daily.
    pub(super) fn watch_answer(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(q) = self.model.question_before(i)
            && !self.model.is_watched(&q)
        {
            let deep = self.model.entries.get(i).is_some_and(
                |e| matches!(e, crate::model::Entry::Assistant { run, .. } if run.deep.is_some()),
            );
            self.bridge.add_watch(q, 24, deep);
        }
        cx.notify();
    }

    /// The Watches screen: every watch, and what changed for the one picked.
    pub(super) fn watches_screen(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let running = self.model.watches_running > 0;
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Watches"),
                    )
                    .child(div().text_xs().text_color(p.faint).child(
                        "Tern re-runs these while the app is open and shows what changed. Nothing runs while it is closed.",
                    )),
            )
            .child(
                Button::new("watch-run-due")
                    .label(if running { "Running…" } else { "Run due now" })
                    .ghost()
                    .small()
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.watches_running += 1;
                        this.bridge.run_watches(Vec::new());
                        cx.notify();
                    })),
            );
        let mut body = div().flex().flex_col().gap_3().pt_4();
        if self.model.watches.is_empty() {
            body = body.child(div().text_sm().text_color(p.faint).child(
                "Nothing is on watch yet. Use the eye under an answer to watch its question.",
            ));
        }
        for w in &self.model.watches {
            body = body.child(self.watch_card(w, running, p, cx));
        }
        div()
            .id("watches-screen")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(super::COLUMN_MAX_W))
                    .px_4()
                    .py_6()
                    .child(header)
                    .child(body),
            )
            .into_any_element()
    }

    fn watch_card(
        &self,
        w: &Watch,
        running: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = w.id.clone();
        let selected = self.model.watch_selected.as_ref() == Some(&w.id);
        let mut meta = format!(
            "{}{} · {}",
            w.every_label(),
            if w.deep { " · deep research" } else { "" },
            w.next_due_text()
        );
        if let Some(t) = &w.last_tally {
            meta.push_str(&format!(" · {t}"));
        }
        let every = EVERY.iter().map(|&(hours, label)| {
            let id = id.clone();
            Button::new(SharedString::from(format!("watch-every-{}-{hours}", w.id)))
                .label(label)
                .ghost()
                .xsmall()
                .selected(w.every_hours == hours)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.bridge.set_watch_every(id.clone(), hours);
                    cx.notify();
                }))
        });
        let run_id = id.clone();
        let rm_id = id.clone();
        let open_id = id.clone();
        let session = w
            .last_session
            .as_ref()
            .and_then(|s| s.parse::<tern_core::SessionId>().ok());
        let mut card = div()
            .id(SharedString::from(format!("watch-card-{}", w.id)))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(if selected { p.muted } else { p.border })
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(div().pt(px(3.)).child(glyph("watcher", 14., p.text)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(p.text)
                                    .child(SharedString::from(w.question.clone())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.faint)
                                    .child(SharedString::from(meta)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl(px(22.))
                    .children(every)
                    .child(
                        Button::new(SharedString::from(format!("watch-run-{}", w.id)))
                            .label("Run now")
                            .ghost()
                            .xsmall()
                            .disabled(running)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model.watches_running += 1;
                                this.model.watch_selected = Some(run_id.clone());
                                this.bridge.run_watches(vec![run_id.clone()]);
                                cx.notify();
                            })),
                    )
                    .when_some(session, |d, s| {
                        d.child(
                            Button::new(SharedString::from(format!("watch-open-{}", w.id)))
                                .label("Open last run")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.model.screen = Screen::Chat;
                                    this.open_chat(s, cx);
                                })),
                        )
                    })
                    .child(
                        div().ml_auto().child(
                            Button::new(SharedString::from(format!("watch-rm-{}", w.id)))
                                .icon(AssetIcon::Trash)
                                .label("Remove")
                                .ghost()
                                .xsmall()
                                .tooltip("Stop watching this question (Undo for a few seconds)")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.remove_watch(rm_id.clone(), cx);
                                })),
                        ),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.model.watch_selected = Some(open_id.clone());
                cx.notify();
            }));
        card = card.child(
            div()
                .pl(px(22.))
                .text_xs()
                .text_color(p.muted)
                .child(SharedString::from(w.status_line())),
        );
        if selected && let Some(c) = &w.last_changes {
            card = card.child(self.what_changed(w, c, p, cx));
        }
        card.into_any_element()
    }

    /// New sources with their logos, changed verdicts with their glyphs,
    /// then new claims and what is gone.
    fn what_changed(
        &self,
        w: &Watch,
        c: &tern_agent::watch::WhatChanged,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let section = |title: &'static str| div().pt_2().text_xs().text_color(p.faint).child(title);
        let source_row = |k: usize, s: &ChangedSource, gone: bool, cx: &mut Context<Self>| {
            let pg = page_of_change(s);
            let url = s.url.clone();
            div()
                .id(SharedString::from(format!(
                    "watch-{}-src-{}-{k}",
                    w.id, gone
                )))
                .flex()
                .items_center()
                .gap_2()
                .px_1()
                .py(px(3.))
                .rounded_md()
                .cursor_pointer()
                .hover(|d| d.bg(p.hover))
                .child(self.site_icon(&pg, 14., p))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(if gone { p.faint } else { p.text })
                        .child(SharedString::from(pg.label().to_owned())),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.browse(&url, window, cx)))
                .into_any_element()
        };
        let claim_row =
            |v: Option<tern_agent::Verdict>, before: Option<tern_agent::Verdict>, text: &str| {
                let mut row = div().flex().items_start().gap_2().py(px(2.));
                if let Some(b) = before {
                    row = row
                        .child(verdict_badge(b, p))
                        .child(div().text_xs().text_color(p.faint).child("→"));
                }
                if let Some(v) = v {
                    row = row.child(verdict_badge(v, p));
                }
                row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(p.muted)
                        .child(SharedString::from(text.to_owned())),
                )
            };
        let mut col = div().flex().flex_col().pl(px(22.));
        if c.is_empty() {
            return col
                .child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child("Nothing changed since the run before."),
                )
                .into_any_element();
        }
        if !c.new_sources.is_empty() {
            col = col.child(section("New sources"));
            for (k, s) in c.new_sources.iter().enumerate() {
                col = col.child(source_row(k, s, false, cx));
            }
        }
        if !c.changed.is_empty() {
            col = col.child(section("Changed verdicts"));
            for ch in &c.changed {
                col = col.child(claim_row(ch.after, ch.before, &ch.claim));
            }
        }
        if !c.new_claims.is_empty() {
            col = col.child(section("New claims"));
            for ch in &c.new_claims {
                col = col.child(claim_row(ch.after, None, &ch.claim));
            }
        }
        if !c.gone_sources.is_empty() {
            col = col.child(section("No longer read"));
            for (k, s) in c.gone_sources.iter().enumerate() {
                col = col.child(source_row(k, s, true, cx));
            }
        }
        if !c.dropped_claims.is_empty() {
            col = col.child(section("No longer claimed"));
            for ch in &c.dropped_claims {
                col = col.child(claim_row(None, ch.before, &ch.claim));
            }
        }
        col.into_any_element()
    }
}
