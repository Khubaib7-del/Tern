//! The first-run tour as a modal over the window (v2 s02): a step rail
//! on the left (done steps get a check, the current one a bar), the page
//! on the right sliding in from the side it came from, and Skip, Back,
//! Continue at the foot. The steps and their rules are
//! `crate::onboarding`; this draws them and does what each page asks
//! (test a key, make a provider or search service the default, pick a
//! model, switch the theme).

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Icon, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, FontWeight, SharedString, Window, div, px,
    svg, uniform_list,
};

use super::Workspace;
use super::press::Capsule as _;
use crate::appearance::{Choice, ThemeChoice};
use crate::assets;
use crate::onboarding::{KeyTest, Onboarding, Outcome, Step};
use crate::providers::{Kind, Models};
use crate::theme::Palette;

const MODAL_W: f32 = 760.;
const MODAL_H: f32 = 500.;
const RAIL_W: f32 = 200.;

impl Workspace {
    /// At launch, once the provider list is read: the tour opens when it
    /// was never finished or skipped and no model key is saved yet.
    /// Someone set up before the tour existed is marked done silently.
    pub(super) fn consider_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.onboarding_checked {
            return;
        }
        self.onboarding_checked = true;
        let onboarded = self.model.providers.prefs.onboarded;
        let any_key = self
            .model
            .providers
            .rows
            .iter()
            .any(|r| r.kind == Kind::Model && r.key_saved);
        if crate::onboarding::shows_at_launch(onboarded, any_key) {
            self.start_onboarding(window, cx);
        } else if !onboarded {
            self.save_ui_prefs(|p| p.onboarded = true);
        }
    }

    /// Opens the tour (at launch, or from Settings › About).
    pub(super) fn start_onboarding(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.model.providers.loaded {
            self.bridge.load_providers();
        }
        self.onboarding = Some(Onboarding::new());
        cx.notify();
    }

    fn finish_onboarding(&mut self, how: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        self.onboarding = None;
        self.model.providers.cancel_edit();
        self.save_ui_prefs(|p| p.onboarded = true);
        if how == Outcome::Finished {
            self.model
                .toast_info("You're set. Ask a research question to begin.");
        }
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    /// A key test came back: the tour's key page shows it.
    pub(super) fn onboarding_checked_key(
        &mut self,
        kind: Kind,
        id: &'static str,
        result: &Result<String, String>,
    ) {
        let Some(o) = self.onboarding.as_mut() else {
            return;
        };
        if kind != Kind::Model || o.provider != Some(id) {
            return;
        }
        match result {
            Ok(text) => {
                o.set_test(KeyTest::Passed(text.clone()));
                // A local server that answered becomes the default.
                if o.step == Step::Model {
                    self.apply_choice(Kind::Model, id);
                }
            }
            Err(e) if o.step == Step::Key => o.set_test(KeyTest::Failed(e.clone())),
            Err(_) => {}
        }
    }

    /// What a page does when it is entered.
    fn enter_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(o) = self.onboarding.clone() else {
            return;
        };
        match (o.step, o.provider) {
            (Step::Key, Some(id)) => {
                if !self.model.providers.is_editing(Kind::Model, id) {
                    self.edit_key(Kind::Model, id, window, cx);
                }
            }
            (Step::Model, Some(id)) => {
                self.model.providers.cancel_edit();
                if self.model.providers.default_id(Kind::Model) != Some(id) {
                    let local = self
                        .model
                        .providers
                        .rows
                        .iter()
                        .any(|r| r.kind == Kind::Model && r.id == id && r.local);
                    if local && !self.model.providers.tested_ok(Kind::Model, id) {
                        self.model.providers.begin(Kind::Model, id, "Checking…");
                        self.bridge.test(Kind::Model, id);
                    } else {
                        self.apply_choice(Kind::Model, id);
                    }
                } else {
                    self.refresh_models();
                }
            }
            _ => {}
        }
    }

    fn onboarding_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(o) = self.onboarding.as_mut() else {
            return;
        };
        if let Some(how) = o.next() {
            self.finish_onboarding(how, window, cx);
            return;
        }
        self.enter_step(window, cx);
        cx.notify();
    }

    fn onboarding_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(o) = self.onboarding.as_mut() {
            o.back();
        }
        self.enter_step(window, cx);
        cx.notify();
    }

    /// The modal, while the tour is open.
    pub(super) fn onboarding_modal(
        &self,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let o = self.onboarding.as_ref()?;
        let vw = f32::from(window.viewport_size().width);
        let vh = f32::from(window.viewport_size().height);
        let w = MODAL_W.min(vw - 32.).max(320.);
        let h = MODAL_H.min(vh - 32.).max(320.);
        let narrow = w < 640.;
        let reduced = self.reduced_motion;

        let rail = div()
            .w(px(RAIL_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap_1()
            .p_4()
            .bg(p.sidebar)
            .border_r_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pb_3()
                    .child(svg().path(assets::MARK).size(px(16.)).text_color(p.text))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Welcome to Tern"),
                    ),
            )
            .children(o.steps().into_iter().map(|s| {
                let on = s == o.step;
                let done = o.done(s);
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(30.))
                    .px_2()
                    .rounded_md()
                    .text_sm()
                    .text_color(if on {
                        p.text
                    } else if done {
                        p.muted
                    } else {
                        p.faint
                    })
                    .when(on, |d| d.bg(p.hover))
                    .child(
                        div()
                            .size(px(16.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(done, |d| {
                                d.child(Icon::new(AssetIcon::Check).small().text_color(p.accent))
                            })
                            .when(!done, |d| {
                                d.child(div().size(px(6.)).rounded_full().bg(if on {
                                    p.accent
                                } else {
                                    p.border
                                }))
                            }),
                    )
                    .child(s.label())
                    .when(on, |d| {
                        d.child(
                            div()
                                .absolute()
                                .right_0()
                                .top(px(6.))
                                .bottom(px(6.))
                                .w(px(2.))
                                .rounded_full()
                                .bg(p.accent),
                        )
                    })
            }))
            .child(div().flex_1())
            .child(
                div().text_xs().text_color(p.faint).child(
                    "Keys never leave this computer. You can change all of this in Settings.",
                ),
            );

        let dir = f32::from(o.direction);
        let page_key = SharedString::from(format!("onboarding-{}", o.step.label()));
        let page = div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::MEDIUM)
                    .child(o.step.title()),
            )
            .child(self.onboarding_body(o, p, cx))
            .with_animation(
                page_key,
                Animation::new(if reduced {
                    std::time::Duration::ZERO
                } else {
                    crate::motion::UI
                })
                .with_easing(crate::motion::ease),
                move |d, t| d.opacity(t).left(px((1. - t) * 28. * dir)),
            );

        let can = o.can_continue();
        let first = o.index() == 0;
        let footer = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(p.border)
            .child(
                Button::new("onboarding-skip")
                    .capsule()
                    .label("Skip for now")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| {
                        let how = this
                            .onboarding
                            .as_ref()
                            .map_or(Outcome::Skipped, Onboarding::skip);
                        this.finish_onboarding(how, window, cx);
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(!first, |d| {
                        d.child(
                            Button::new("onboarding-back")
                                .capsule()
                                .label("Back")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.onboarding_back(window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("onboarding-next")
                            .capsule()
                            .label(o.continue_label())
                            .primary()
                            .small()
                            .disabled(!can)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.onboarding_next(window, cx)),
                            ),
                    ),
            );

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::hsla(0., 0., 0., 0.55))
                .occlude()
                .child(
                    div()
                        .w(px(w))
                        .h(px(h))
                        .flex()
                        .flex_row()
                        .rounded_xl()
                        .overflow_hidden()
                        .border_1()
                        .border_color(p.border)
                        .bg(p.canvas)
                        .shadow_lg()
                        .when(!narrow, |d| d.child(rail))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .flex()
                                .flex_col()
                                .p_6()
                                .child(page)
                                .child(footer),
                        ),
                )
                .with_animation(
                    "onboarding-in",
                    Animation::new(if reduced {
                        std::time::Duration::ZERO
                    } else {
                        crate::motion::UI
                    })
                    .with_easing(crate::motion::ease),
                    |d, t| d.opacity(t),
                )
                .into_any_element(),
        )
    }

    fn onboarding_body(&self, o: &Onboarding, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        match o.step {
            Step::Welcome => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_4()
                .child(
                    svg()
                        .path(assets::MARK)
                        .size(px(72.))
                        .text_color(p.text)
                        .with_animation(
                            "onboarding-mark",
                            Animation::new(if self.reduced_motion {
                                std::time::Duration::ZERO
                            } else {
                                std::time::Duration::from_millis(700)
                            })
                            .with_easing(crate::motion::ease),
                            |d, t| d.opacity(t).size(px(48. + 24. * t)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_2xl()
                                .font_family(assets::MONO_FONT)
                                .font_weight(FontWeight::MEDIUM)
                                .child("TERN"),
                        )
                        .child(div().w(px(56.)).h(px(2.)).rounded_full().bg(p.accent)),
                )
                .child(
                    div()
                        .max_w(px(420.))
                        .text_sm()
                        .text_color(p.muted)
                        .text_center()
                        .child("Research and study in one calm window. Tern plans first, then searches, reads, and checks every claim before it writes. A minute to set up."),
                )
                .into_any_element(),
            Step::Provider => {
                let rows: Vec<_> = self
                    .model
                    .providers
                    .section(Kind::Model)
                    .cloned()
                    .collect();
                if rows.is_empty() {
                    return div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(p.muted)
                        .child(Spinner::new().small().color(p.muted))
                        .child("Reading the keychain…")
                        .into_any_element();
                }
                div()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .children(rows.into_iter().map(|r| {
                                let on = o.provider == Some(r.id);
                                let (id, needs, saved) = (r.id, r.needs_key(), r.key_saved);
                                div()
                                    .id(SharedString::from(format!("ob-provider-{id}")))
                                    .w(px(162.))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .p_2()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(if on { p.accent } else { p.border })
                                    .when(on, |d| d.bg(p.hover))
                                    .cursor_pointer()
                                    .hover(|d| d.bg(p.hover))
                                    .child(super::brand_logo(id, 18., p))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .min_w_0()
                                            .child(div().text_sm().truncate().child(r.name))
                                            .child(div().text_xs().text_color(p.faint).child(
                                                if r.local {
                                                    "On this computer"
                                                } else if saved {
                                                    "Key saved"
                                                } else {
                                                    "Needs a key"
                                                },
                                            )),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(o) = this.onboarding.as_mut() {
                                            o.pick_provider(id, needs, saved);
                                        }
                                        cx.notify();
                                    }))
                            })),
                    )
                    .overflow_y_scrollbar()
                    .id("ob-providers")
                    .into_any_element()
            }
            Step::Key => {
                let row = o.provider.and_then(|id| {
                    self.model
                        .providers
                        .rows
                        .iter()
                        .find(|r| r.kind == Kind::Model && r.id == id)
                        .cloned()
                });
                let name = row.as_ref().map_or("the provider", |r| r.name);
                let key_url = row.as_ref().map_or("", |r| r.key_url);
                let status: AnyElement = match &o.test {
                    KeyTest::NotYet => div()
                        .text_xs()
                        .text_color(p.faint)
                        .child("Tern tests the key with the provider before it saves it.")
                        .into_any_element(),
                    KeyTest::Testing => div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(p.muted)
                        .child(Spinner::new().small().color(p.muted))
                        .child("Testing the key…")
                        .into_any_element(),
                    KeyTest::Passed(t) => div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .child(Icon::new(AssetIcon::Check).small().text_color(p.accent))
                        .child(SharedString::from(format!("Connected · {t}")))
                        .into_any_element(),
                    KeyTest::Failed(e) => div()
                        .text_sm()
                        .text_color(p.text)
                        .child(SharedString::from(format!("That key did not work: {e}")))
                        .into_any_element(),
                };
                let testing = matches!(o.test, KeyTest::Testing);
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .children(o.provider.map(|id| super::brand_logo(id, 18., p)))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(p.muted)
                                    .child(SharedString::from(format!(
                                        "Your {name} key. It goes to your system keychain, never to Tern."
                                    ))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.key_input).mask_toggle()))
                            .child(
                                Button::new("ob-test").capsule()
                                    .label("Test connection")
                                    .small()
                                    .disabled(testing)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let Some(o) = this.onboarding.as_mut() {
                                            o.set_test(KeyTest::Testing);
                                        }
                                        this.save_key(window, cx);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(status)
                    .when(!key_url.is_empty(), |d| {
                        d.child(
                            Button::new("ob-get-key").capsule()
                                .label("Get a key")
                                .ghost()
                                .xsmall()
                                .on_click(move |_, _, cx| cx.open_url(key_url)),
                        )
                    })
                    .into_any_element()
            }
            Step::Model => {
                let st = &self.model.providers;
                let body: AnyElement = match &st.models {
                    Some(Models::Loaded(_, list)) if !list.is_empty() => {
                        let n = list.len();
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .rounded_lg()
                            .border_1()
                            .border_color(p.border)
                            .child(super::wheel_first(
                                "onboarding-models",
                                &self.settings_inputs.onboarding_models_scroll,
                                None,
                                cx.entity_id(),
                            ))
                            .child(
                                uniform_list(
                                    "ob-models",
                                    n,
                                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                        this.onboarding_model_rows(range, cx)
                                    }),
                                )
                                .size_full()
                                .track_scroll(&self.settings_inputs.onboarding_models_scroll),
                            )
                            .vertical_scrollbar(&self.settings_inputs.onboarding_models_scroll)
                            .into_any_element()
                    }
                    Some(Models::Failed(_, e)) => div()
                        .text_sm()
                        .text_color(p.muted)
                        .child(SharedString::from(format!(
                            "Could not list models: {e}. Continue, and Tern uses the provider's first chat model."
                        )))
                        .into_any_element(),
                    _ => div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(p.muted)
                        .child(Spinner::new().small().color(p.muted))
                        .child("Listing models…")
                        .into_any_element(),
                };
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_sm().text_color(p.muted).child(
                        "The model that answers. You can switch any time from the composer, and set one per job in Settings › Models.",
                    ))
                    .child(body)
                    .into_any_element()
            }
            Step::Search => {
                let rows: Vec<_> = self
                    .model
                    .providers
                    .section(Kind::Search)
                    .cloned()
                    .collect();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_sm().text_color(p.muted).child(
                        "Tern searches the web, then Tern Crawl reads the pages. Wikipedia needs no key; the others take a key you can add later.",
                    ))
                    .when(self.model.providers.only_wikipedia(), |d| {
                        d.child(
                            div()
                                .id("ob-search-nudge")
                                .flex()
                                .items_center()
                                .gap_2()
                                .p_2()
                                .rounded_lg()
                                .border_1()
                                .border_color(p.accent)
                                .child(super::brand_logo("tavily", 18., p))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_xs()
                                        .text_color(p.text)
                                        .child(crate::chatflow::TAVILY_NUDGE),
                                ),
                        )
                    })
                    .children(rows.into_iter().map(|r| {
                        let id = r.id;
                        let usable = r.usable() && self.model.providers.can_choose(&r);
                        let on = o.search == Some(id)
                            || (o.search.is_none() && self.model.providers.is_default(&r));
                        div()
                            .id(SharedString::from(format!("ob-search-{id}")))
                            .flex()
                            .items_center()
                            .gap_3()
                            .p_2()
                            .rounded_lg()
                            .border_1()
                            .border_color(if on { p.accent } else { p.border })
                            .when(usable, |d| d.cursor_pointer().hover(|d| d.bg(p.hover)))
                            .when(!usable, |d| d.opacity(0.6))
                            .child(super::brand_logo(id, 18., p))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .child(div().text_sm().child(r.name))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(p.faint)
                                            .child(if usable {
                                                r.status()
                                            } else {
                                                "Add its key or address later in Settings"
                                            }),
                                    ),
                            )
                            .when(usable, |d| {
                                d.on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(o) = this.onboarding.as_mut() {
                                        o.search = Some(id);
                                    }
                                    this.apply_choice(Kind::Search, id);
                                    cx.notify();
                                }))
                            })
                    }))
                    .into_any_element()
            }
            Step::Theme => {
                let look = self.look;
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.muted)
                            .child("Dark, light, or follow the system. More in Settings › Appearance."),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .children(ThemeChoice::ALL.map(|t| {
                                Button::new(SharedString::from(format!("ob-theme-{}", t.label()))).capsule()
                                    .label(t.label())
                                    .small()
                                    .selected(look.theme == t)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.set_appearance(Choice::Theme(t), window, cx);
                                    }))
                            })),
                    )
                    .into_any_element()
            }
        }
    }

    fn onboarding_model_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = Palette::get(self.model.dark);
        let Some(Models::Loaded(_, list)) = &self.model.providers.models else {
            return Vec::new();
        };
        let chosen = self
            .onboarding
            .as_ref()
            .and_then(|o| o.model.clone())
            .or_else(|| self.model.providers.chosen_model().map(str::to_owned));
        range
            .filter_map(|i| list.get(i).map(|m| (i, m)))
            .map(|(i, m)| {
                let on = chosen.as_deref() == Some(m.id.as_str());
                let id = m.id.clone();
                div()
                    .id(("ob-model", i))
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .text_sm()
                    .cursor_pointer()
                    .text_color(if on { p.text } else { p.muted })
                    .when(on, |d| d.bg(p.hover))
                    .hover(|d| d.bg(p.hover).text_color(p.text))
                    .child(div().truncate().child(SharedString::from(m.id.clone())))
                    .when(on, |d| d.child(Icon::new(AssetIcon::Check).small()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(o) = this.onboarding.as_mut() {
                            o.model = Some(id.clone());
                        }
                        this.pick_model(id.clone(), cx);
                    }))
                    .into_any_element()
            })
            .collect()
    }
}
