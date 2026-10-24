//! The decision model in the window: the Decision model settings page
//! (Off, Local, or Remote; the remote providers with their logos and keys;
//! a Test button) and the run panel's Decisions view (what was decided,
//! by which model, with each choice's probability and why).

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, Window, div, px, svg};
use tern_agent::Run;
use tern_agent::decide::{DecisionKind, Job, Mode};
use tern_core::SecretString;

use super::Workspace;
use super::segmented::{Look, OnPick, Segment};
use crate::assets;
use crate::decision::{RowNote, RowView};
use crate::theme::Palette;

/// The provider's logo on a neutral tile, like every service row.
fn logo(icon: &str, p: Palette) -> AnyElement {
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
        .child(super::brand_logo(icon, 16., p))
        .into_any_element()
}

/// The local scorer's tile: Tern's own mark.
fn local_logo(p: Palette) -> AnyElement {
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
        .child(svg().path(assets::MARK).size(px(15.)).text_color(p.text))
        .into_any_element()
}

impl Workspace {
    pub(super) fn open_decision(&mut self, cx: &mut Context<Self>) {
        self.show_settings(crate::settings::Section::Decision);
        if !self.model.decision.loaded {
            self.bridge.decision_load();
        }
        cx.notify();
    }

    fn set_decision_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.model.decision.set_mode(mode);
        self.save_decision_prefs();
        cx.notify();
    }

    fn save_decision_prefs(&mut self) {
        let mut prefs = tern_agent::Preferences::load();
        prefs.decision = self.model.decision.prefs.clone();
        self.bridge.save_prefs(prefs);
    }

    fn edit_decision_key(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.model.decision.editing = Some(id);
        self.key_input.update(cx, |k, cx| {
            k.set_value("", window, cx);
            k.focus(window, cx);
        });
        cx.notify();
    }

    fn save_decision_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.model.decision.editing else {
            return;
        };
        if self.model.decision.is_working(id) {
            return;
        }
        let key = self.key_input.read(cx).value().trim().to_owned();
        if key.is_empty() {
            return;
        }
        self.model.decision.set_note(
            id,
            RowNote::Working("Asking one small decision with the key…".into()),
        );
        self.bridge.decision_add_key(id, SecretString::new(key));
        self.key_input
            .update(cx, |k, cx| k.set_value("", window, cx));
        cx.notify();
    }

    pub(super) fn decision_screen(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.decision;
        let mode = st.mode();
        let selected = match mode {
            Mode::Off => 0,
            Mode::Local => 1,
            Mode::Remote => 2,
        };
        let options = [
            Segment::text("Off"),
            Segment::text("Local"),
            Segment::text("Remote"),
        ];
        let pick: OnPick = Rc::new(
            |this: &mut Workspace, i: usize, _: &mut Window, cx: &mut Context<Workspace>| {
                let m = match i {
                    1 => Mode::Local,
                    2 => Mode::Remote,
                    _ => Mode::Off,
                };
                this.set_decision_mode(m, cx);
            },
        );
        let explain = match mode {
            Mode::Off => {
                "Off: Tern keeps the search order, the plan's order, and the word-overlap claim check."
            }
            Mode::Local => {
                "Local: Tern's own scorer (word statistics and a few rules). Offline, free, the same answer every time."
            }
            Mode::Remote => {
                "Remote: the provider you pick below scores each choice. If it is slow or fails, the local scorer answers and the log says why."
            }
        };
        let modes = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().text_color(p.text).child(
                "Tern makes small, fixed choices many times a turn (which page to read first, which tool to start with, whether a passage backs a claim). A decision model makes them fast and cheap; hard checks always win.",
            ))
            .child(
                div()
                    .flex()
                    .child(self.segmented("decision-mode", &options, selected, Look::default(), p, pick, cx)),
            )
            .child(div().text_xs().text_color(p.muted).child(explain))
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!("In use: {}", st.in_use()))),
            );
        let jobs: Vec<AnyElement> = Job::ALL
            .into_iter()
            .map(|job| {
                let on = st.prefs.job_on(job);
                Self::row(
                    job.label(),
                    job.blurb(),
                    Some(
                        Switch::new(SharedString::from(format!("decision-job-{}", job.id())))
                            .checked(on)
                            .small()
                            .disabled(mode == Mode::Off)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.model.decision.prefs.set_job(job, *checked);
                                tern_agent::decide::set_jobs_off(
                                    &this.model.decision.prefs.jobs_off,
                                );
                                this.save_decision_prefs();
                                cx.notify();
                            }))
                            .into_any_element(),
                    ),
                    p,
                )
            })
            .collect();
        let recent: Vec<AnyElement> = self
            .model
            .last_run
            .decisions
            .iter()
            .rev()
            .take(5)
            .map(|d| {
                let chosen = d
                    .choices
                    .get(d.chosen)
                    .map(|c| short(c))
                    .unwrap_or_default();
                Self::row(
                    SharedString::from(format!("{} · {}", kind_label(d.kind), short(&d.question))),
                    SharedString::from(format!(
                        "Chose {chosen} · {:.0}% sure · {}{}",
                        d.confidence * 100.,
                        d.model,
                        if d.fallback.is_some() {
                            " (fell back to local)"
                        } else {
                            ""
                        }
                    )),
                    None,
                    p,
                )
            })
            .collect();
        let recent = if recent.is_empty() {
            div()
                .text_xs()
                .text_color(p.faint)
                .child("Recent decisions: none yet. After a chat turn, the last few show here (all of them are in the run panel's Decisions view).")
                .into_any_element()
        } else {
            Self::settings_card("Recent decisions (last run)", p, recent)
        };
        let test_line: AnyElement = match &st.test {
            Some(RowNote::Working(t)) => div()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(p.muted)
                .child(Spinner::new().small().color(p.muted))
                .child(SharedString::from(t.clone()))
                .into_any_element(),
            Some(RowNote::Ok(t)) => div()
                .text_xs()
                .text_color(p.muted)
                .child(SharedString::from(t.clone()))
                .into_any_element(),
            Some(RowNote::Failed(t)) => div()
                .text_xs()
                .text_color(p.text)
                .child(SharedString::from(format!("Failed: {t}")))
                .into_any_element(),
            None => div().into_any_element(),
        };
        let test = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                Button::new("decision-test")
                    .label("Test")
                    .small()
                    .disabled(mode == Mode::Off || matches!(st.test, Some(RowNote::Working(_))))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.decision.test =
                            Some(RowNote::Working("Asking a sample decision…".into()));
                        this.bridge.decision_test(this.model.decision.prefs.clone());
                        cx.notify();
                    })),
            )
            .child(test_line);

        let local_row = div()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2p5()
            .border_b_1()
            .border_color(p.border)
            .child(local_logo(p))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Tern local scorer"),
                            )
                            .when(mode == Mode::Local, |d| d.child(chosen_chip(p))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child("Built in · offline · free · every decision point"),
                    ),
            );
        let rows: Vec<AnyElement> = st
            .rows
            .clone()
            .into_iter()
            .map(|r| self.decision_row(r, p, cx))
            .collect();
        let mut column: Vec<AnyElement> = vec![
            modes.into_any_element(),
            test.into_any_element(),
            Self::settings_card("What it decides", p, jobs),
        ];
        if let Some(e) = &st.keychain_error {
            column.push(
                div()
                    .text_sm()
                    .text_color(p.muted)
                    .child(SharedString::from(format!(
                        "The keychain cannot be read on this computer: {e}"
                    )))
                    .into_any_element(),
            );
        }
        column.push(if !st.loaded {
            div()
                .flex()
                .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(p.muted)
                    .child(Spinner::new().small().color(p.muted))
                    .child("Reading the keychain…")
                    .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_xs().text_color(p.faint).child("Providers"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .rounded_xl()
                        .border_1()
                        .border_color(p.border)
                        .child(local_row)
                        .children(rows),
                )
                .child(div().text_xs().text_color(p.faint).child(
                    "Keys are tested with one small decision before they are saved, and kept only in the system keychain. Prices are the providers' published input prices.",
                ))
                .into_any_element()
        });
        column.push(recent);
        self.page(crate::settings::Section::Decision, p, None, column)
    }

    fn decision_row(&self, r: RowView, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.decision;
        let id = r.id;
        let chosen = st.is_chosen(id);
        let working = st.is_working(id);
        let editing = st.editing == Some(id);
        let key_url = r.key_url;
        let bid = |what: &str| SharedString::from(format!("decision-{what}-{id}"));
        let mut info = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from(format!("{} {}", r.name, r.product))),
                    )
                    .when(chosen, |d| d.child(chosen_chip(p))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!(
                        "{} · {} · {}",
                        r.default_model, r.price, r.scope
                    ))),
            );
        if let Some(n) = st.note(id) {
            let text = match n {
                RowNote::Working(t) | RowNote::Ok(t) => t.clone(),
                RowNote::Failed(t) => format!("Failed: {t}"),
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
        } else if !r.key_saved {
            if !editing {
                actions = actions.child(Button::new(bid("add")).label("Add").small().on_click(
                    cx.listener(move |this, _, window, cx| this.edit_decision_key(id, window, cx)),
                ));
            }
        } else {
            if !chosen {
                actions = actions.child(Button::new(bid("use")).label("Use").small().on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.model.decision.use_provider(id);
                        this.save_decision_prefs();
                        cx.notify();
                    }),
                ));
            }
            actions = actions
                .child(
                    Button::new(bid("test"))
                        .label("Test")
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model
                                .decision
                                .set_note(id, RowNote::Working("Checking…".into()));
                            this.bridge.decision_test_key(id);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(bid("remove"))
                        .label("Remove")
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model
                                .decision
                                .set_note(id, RowNote::Working("Removing…".into()));
                            this.bridge.decision_remove(id);
                            cx.notify();
                        })),
                );
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
                    .child(logo(r.icon, p))
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_decision_key(window, cx)
                            })),
                    )
                    .child(
                        Button::new(bid("get"))
                            .label("Get a key")
                            .ghost()
                            .small()
                            .on_click(move |_, _, cx| cx.open_url(key_url)),
                    )
                    .child(
                        Button::new(bid("cancel"))
                            .label("Cancel")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.decision.editing = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        row.into_any_element()
    }

    /// The run panel's Decisions view.
    pub(super) fn run_decisions(&self, run: &Run, p: Palette) -> AnyElement {
        if run.decisions.is_empty() {
            return div()
                .text_sm()
                .text_color(p.faint)
                .child("What the decision model decides during a run shows here.")
                .into_any_element();
        }
        let spent: f64 = run.decisions.iter().map(|d| d.cost.usd).sum();
        let fell_back = run
            .decisions
            .iter()
            .filter(|d| d.fallback.is_some())
            .count();
        let mut summary = format!(
            "{} decision{} · {}",
            run.decisions.len(),
            if run.decisions.len() == 1 { "" } else { "s" },
            run.decisions
                .last()
                .map(|d| d.model.clone())
                .unwrap_or_default()
        );
        if spent > 0.0 {
            summary.push_str(&format!(" · ${spent:.6}"));
        }
        if fell_back > 0 {
            summary.push_str(&format!(" · {fell_back} fell back to local"));
        }
        let cards = run.decisions.iter().enumerate().map(|(k, d)| {
            let bars = d.ranking().into_iter().take(4).map(|i| {
                let prob = d.probabilities.get(i).copied().unwrap_or(0.0);
                let label = short(d.choices.get(i).map(String::as_str).unwrap_or(""));
                let ruled = d
                    .overrides
                    .iter()
                    .any(|o| o.choice == i && o.action == "ruled_out");
                let label = if ruled {
                    format!("ruled out · {label}")
                } else {
                    label
                };
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(34.))
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(if i == d.chosen { p.text } else { p.faint })
                            .child(SharedString::from(format!("{:.0}%", prob * 100.0))),
                    )
                    .child(
                        div()
                            .w(px(56.))
                            .h(px(4.))
                            .flex_shrink_0()
                            .rounded_sm()
                            .bg(p.hover)
                            .child(
                                div()
                                    .h_full()
                                    .rounded_sm()
                                    .w(px(56. * prob.clamp(0.0, 1.0)))
                                    .bg(if i == d.chosen { p.accent } else { p.muted }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(if i == d.chosen { p.text } else { p.muted })
                            .child(SharedString::from(label)),
                    )
            });
            let mut meta = vec![d.model.clone(), format!("{} ms", d.latency_ms)];
            if d.cost.input_tokens > 0 {
                meta.push(format!(
                    "{} tokens · ${:.6}",
                    d.cost.input_tokens, d.cost.usd
                ));
            }
            if d.cached {
                meta.push("cached".into());
            }
            div()
                .id(SharedString::from(format!("decision-{k}")))
                .flex()
                .flex_col()
                .gap_1()
                .px_2()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .child(kind_label(d.kind)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .child(SharedString::from(format!(
                                    "confidence {:.2}",
                                    d.confidence
                                ))),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .truncate()
                        .child(SharedString::from(d.question.clone())),
                )
                .children(bars)
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .child(SharedString::from(format!("Why: {}", d.rationale))),
                )
                .children(d.overrides.iter().map(|o| {
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .child(SharedString::from(format!(
                            "Hard check: {} {} ({})",
                            short(d.choices.get(o.choice).map(String::as_str).unwrap_or("")),
                            if o.action == "forced" {
                                "forced"
                            } else {
                                "ruled out"
                            },
                            o.reason
                        )))
                }))
                .when_some(d.fallback.clone(), |c, f| {
                    c.child(
                        div()
                            .text_xs()
                            .text_color(p.muted)
                            .child(SharedString::from(format!("Fell back to local: {f}"))),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(meta.join(" · "))),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(summary)),
            )
            .children(cards)
            .into_any_element()
    }
}

fn chosen_chip(p: Palette) -> impl IntoElement {
    div()
        .px_1p5()
        .rounded_md()
        .border_1()
        .border_color(p.border)
        .text_xs()
        .text_color(p.muted)
        .child("In use")
}

fn kind_label(k: DecisionKind) -> &'static str {
    k.label()
}

/// A choice id for a narrow row: URLs without their scheme, cut short.
fn short(s: &str) -> String {
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s)
        .trim_start_matches("www.");
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(59).collect::<String>())
    } else {
        s.to_owned()
    }
}
