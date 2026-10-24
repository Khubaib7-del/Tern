//! Settings: one screen with a section list on the left (icons and
//! labels; icons only when the window is narrow) and the section's page on
//! the right. Opened from the rail's gear and Ctrl+, (Cmd+, on macOS);
//! Back or Esc returns to the screen it was opened from.
//!
//! The pages that used to be their own screens (Providers, Customize,
//! Appearance, Decision, Evaluation, the Study settings card) are drawn
//! here unchanged in what they do. New pages: Models (the catalogue by
//! capability and the default model per job), Tools, Search & Crawler,
//! Privacy & data, and About. The model logic is `crate::settings`.

use std::rc::Rc;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Icon, Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnimationExt as _, AnyElement, Context, Entity, FontWeight, SharedString, Subscription,
    UniformListScrollHandle, Window, div, px, svg, uniform_list,
};

use super::Workspace;
use super::segmented::{Look, OnPick, Segment};
use crate::assets;
use crate::model::Screen;
use crate::providers::{Kind, Models};
use crate::settings::{self, CatalogRow, Group, Role, Section};
use crate::theme::Palette;

/// One row of the section list.
const NAV_ROW_H: f32 = 32.;
/// One row of a virtualised model list.
const LIST_ROW_H: f32 = 30.;

/// The section's icon in the list.
fn section_icon(s: Section) -> AssetIcon {
    match s {
        Section::Providers => AssetIcon::KeyRound,
        Section::Models => AssetIcon::Cpu,
        Section::Tools => AssetIcon::Wrench,
        Section::Decision => AssetIcon::Signpost,
        Section::Evaluation => AssetIcon::FlaskConical,
        Section::Extensions => AssetIcon::Blocks,
        Section::Instructions => AssetIcon::NotebookPen,
        Section::Search => AssetIcon::Globe,
        Section::Appearance => AssetIcon::Palette,
        Section::Study => AssetIcon::GraduationCap,
        Section::Privacy => AssetIcon::ShieldCheck,
        Section::About => AssetIcon::Info,
    }
}

/// A small icon for each of Tern's own tools.
fn tool_icon(name: &str) -> AssetIcon {
    match name {
        "web_search" => AssetIcon::Search,
        "read_url" => AssetIcon::Globe,
        "vault_search" => AssetIcon::BookMarked,
        "vault_read" => AssetIcon::NotebookPen,
        "vault_write" => AssetIcon::Pencil,
        "files_list" => AssetIcon::FolderOpen,
        "file_read" => AssetIcon::FileText,
        "file_search" => AssetIcon::Search,
        "load_skill" => AssetIcon::Sparkles,
        n if n.starts_with("vault") => AssetIcon::NotebookPen,
        n if n.starts_with("file") => AssetIcon::FileText,
        _ => AssetIcon::Wrench,
    }
}

fn icon(name: AssetIcon, color: gpui_kit::Hsla) -> AnyElement {
    Icon::new(name).small().text_color(color).into_any_element()
}

/// Settings' own text fields and list scroll positions.
pub(super) struct SettingsInputs {
    /// Search every setting.
    pub search: Entity<InputState>,
    /// A provider's server address.
    pub base_url: Entity<InputState>,
    /// Models › catalogue filter.
    pub filter: Entity<InputState>,
    pub catalog_scroll: UniformListScrollHandle,
    pub role_scroll: UniformListScrollHandle,
    pub provider_models_scroll: UniformListScrollHandle,
    pub onboarding_models_scroll: UniformListScrollHandle,
}

impl SettingsInputs {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> (Self, Vec<Subscription>) {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        let base_url = cx
            .new(|cx| InputState::new(window, cx).placeholder("Leave empty for the usual address"));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter models"));
        let subs = vec![
            cx.subscribe_in(
                &search,
                window,
                |this: &mut Workspace, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.model.settings.query = input.read(cx).value().to_string();
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        let q = input.read(cx).value().to_string();
                        if let Some(hit) = settings::search(&q).first().copied() {
                            this.open_settings(hit.section, window, cx);
                        }
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &base_url,
                window,
                |this: &mut Workspace, _, event: &InputEvent, _, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.save_base_url(cx);
                    }
                },
            ),
            cx.subscribe_in(
                &filter,
                window,
                |this: &mut Workspace, input, event: &InputEvent, _, cx| {
                    if let InputEvent::Change = event {
                        this.model.settings.filter = input.read(cx).value().to_string();
                        cx.notify();
                    }
                },
            ),
        ];
        (
            Self {
                search,
                base_url,
                filter,
                catalog_scroll: UniformListScrollHandle::new(),
                role_scroll: UniformListScrollHandle::new(),
                provider_models_scroll: UniformListScrollHandle::new(),
                onboarding_models_scroll: UniformListScrollHandle::new(),
            },
            subs,
        )
    }
}

impl Workspace {
    // ----- opening and closing -----

    /// Puts Settings on screen at `s`, remembering where to go back to.
    pub(super) fn show_settings(&mut self, s: Section) {
        if self.model.screen != Screen::Settings {
            self.model.settings_back = self.model.screen;
        }
        self.model.screen = Screen::Settings;
        self.model.settings.open(s);
    }

    /// Opens Settings at a section and loads what that page needs.
    pub(super) fn open_settings(
        &mut self,
        s: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match s {
            Section::Providers => self.open_providers(cx),
            Section::Models => {
                self.show_settings(s);
                if !self.model.providers.loaded {
                    self.bridge.load_providers();
                }
                self.refresh_catalog();
            }
            Section::Decision => self.open_decision(cx),
            Section::Evaluation => self.open_eval(cx),
            Section::Extensions => self.open_customize(cx),
            Section::Appearance => self.open_appearance(cx),
            Section::Search => {
                self.show_settings(s);
                if !self.model.providers.loaded {
                    self.bridge.load_providers();
                }
                self.bridge.crawlers_load();
                self.measure_cache();
                let typed = self.searx_input.read(cx).value().to_string();
                if typed.is_empty()
                    && let Some(u) = self.model.providers.prefs.search_url.clone()
                {
                    self.searx_input
                        .update(cx, |i, cx| i.set_value(u, window, cx));
                }
            }
            Section::Instructions => self.open_instructions(window, cx),
            Section::Tools | Section::Study | Section::Privacy | Section::About => {
                self.show_settings(s);
            }
        }
        cx.notify();
    }

    /// Back (or Esc): the screen Settings was opened from, the chat
    /// otherwise.
    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let back = match self.model.settings_back {
            Screen::Settings => Screen::Chat,
            s => s,
        };
        self.model.screen = back;
        self.model.providers.cancel_edit();
        self.model.customize.preview = None;
        self.model.decision.editing = None;
        self.model.settings.confirm = None;
        self.model.settings.editing_url = None;
        self.scout_preview = None;
        if back == Screen::Chat {
            self.composer.update(cx, |c, cx| c.focus(window, cx));
        }
        cx.notify();
    }

    /// Ctrl+, (Cmd+, on macOS) opens Settings from anywhere; Esc leaves it.
    pub(super) fn settings_keys(
        &mut self,
        e: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &e.keystroke;
        if k.key == "," && (k.modifiers.control || k.modifiers.platform) {
            if self.model.screen == Screen::Settings {
                self.close_settings(window, cx);
            } else {
                let s = self.model.settings.section;
                self.open_settings(s, window, cx);
            }
            cx.stop_propagation();
            return true;
        }
        if k.key == "escape"
            && self.model.screen == Screen::Settings
            && self.onboarding.is_none()
            && self.model.customize.preview.is_none()
        {
            if !self.model.settings.query.is_empty() {
                self.model.settings.query.clear();
                self.settings_inputs
                    .search
                    .update(cx, |i, cx| i.set_value("", window, cx));
            } else {
                self.close_settings(window, cx);
            }
            cx.stop_propagation();
            cx.notify();
            return true;
        }
        false
    }

    // ----- the screen -----

    pub(super) fn settings_screen(
        &self,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = f32::from(window.viewport_size().width);
        let labels = settings::nav_labels(width);
        let nav = self.settings_nav(p, width, cx);
        let page = self.settings_page(p, labels, window, cx);
        div()
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .child(nav)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(page),
            )
            .into_any_element()
    }

    fn settings_nav(&self, p: Palette, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let labels = settings::nav_labels(width);
        let st = &self.model.settings;
        let current = Section::ALL
            .iter()
            .position(|s| *s == st.section)
            .unwrap_or(0);
        // The selection slides between rows (v2's layoutId highlight).
        let at = self.glide("settings-nav", current as f32);
        let heights = vec![NAV_ROW_H; Section::ALL.len()];
        let (y, h) = super::segmented::pill_at(&heights, at);
        let matching = settings::sections_matching(&st.query);
        let searching = !st.query.trim().is_empty();
        let rows = Section::ALL.into_iter().map(|s| {
            let on = s == st.section;
            if !labels {
                return div()
                    .h(px(NAV_ROW_H))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Button::new(SharedString::from(format!("settings-nav-{}", s.id())))
                            .icon(section_icon(s))
                            .ghost()
                            .small()
                            .tooltip(s.title())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_settings(s, window, cx);
                            })),
                    )
                    .into_any_element();
            }
            let dim = searching && !matching.contains(&s);
            div()
                .id(SharedString::from(format!("settings-nav-{}", s.id())))
                .h(px(NAV_ROW_H))
                .flex()
                .items_center()
                .gap_2p5()
                .px(px(10.))
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .text_color(if on { p.text } else { p.muted })
                .when(dim, |d| d.opacity(0.4))
                .hover(|d| d.text_color(p.text))
                .child(icon(section_icon(s), if on { p.accent } else { p.faint }))
                .child(div().truncate().child(s.title()))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_settings(s, window, cx);
                }))
                .into_any_element()
        });
        let list = div()
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(y))
                    .h(px(h))
                    .rounded_md()
                    .bg(p.hover),
            )
            .children(rows);
        let results = (labels && searching).then(|| {
            let hits = settings::search(&st.query);
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .pb_2()
                .mb_2()
                .border_b_1()
                .border_color(p.border)
                .when(hits.is_empty(), |d| {
                    d.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(p.faint)
                            .child("No setting matches."),
                    )
                })
                .children(hits.into_iter().take(8).enumerate().map(|(i, hit)| {
                    div()
                        .id(("settings-hit", i))
                        .flex()
                        .flex_col()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|d| d.bg(p.hover))
                        .child(div().text_sm().text_color(p.text).child(hit.label))
                        .child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .child(hit.section.title()),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_settings(hit.section, window, cx);
                        }))
                }))
        });
        div()
            .w(px(settings::nav_width(width)))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .px(px(if labels { 10. } else { 6. }))
            .py_3()
            .bg(p.sidebar)
            .border_r_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Button::new("settings-back")
                            .icon(AssetIcon::ArrowLeft)
                            .ghost()
                            .small()
                            .tooltip("Back (Esc)")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_settings(window, cx)),
                            ),
                    )
                    .when(labels, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .font_family(assets::MONO_FONT)
                                .text_color(p.faint)
                                .child("SETTINGS"),
                        )
                    }),
            )
            .when(labels, |d| {
                d.child(Input::new(&self.settings_inputs.search).small())
            })
            .children(results)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    .overflow_y_scrollbar()
                    .id("settings-nav-scroll"),
            )
            .into_any_element()
    }

    /// The page for the open section.
    fn settings_page(
        &self,
        p: Palette,
        labels: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = self.model.settings.section;
        let page = match s {
            Section::Providers => self.providers_page(p, cx),
            Section::Models => self.models_page(p, cx),
            Section::Tools => self.tools_page(p, cx),
            Section::Decision => self.decision_screen(p, cx),
            Section::Evaluation => self.eval_screen(p, cx),
            Section::Extensions => self.customize_screen(p, window, cx),
            Section::Instructions => self.instructions_page(p, cx),
            Section::Search => self.search_page(p, cx),
            Section::Appearance => self.appearance_screen(p, cx),
            Section::Study => self.page(s, p, None, vec![self.study_settings(p, cx)]),
            Section::Privacy => self.privacy_page(p, cx),
            Section::About => self.about_page(p, cx),
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            // Narrow: the list is icons only, so search sits over the page.
            .when(!labels, |d| {
                d.child(
                    div()
                        .px_6()
                        .pt_3()
                        .child(Input::new(&self.settings_inputs.search).small()),
                )
                .children(self.narrow_results(p, cx))
            })
            .child(page)
            .with_animation(
                SharedString::from(format!("settings-page-{}", s.id())),
                gpui_kit::Animation::new(if self.reduced_motion {
                    std::time::Duration::ZERO
                } else {
                    crate::motion::UI
                })
                .with_easing(crate::motion::ease),
                |d, t| d.opacity(t),
            )
            .into_any_element()
    }

    fn narrow_results(&self, _p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let q = &self.model.settings.query;
        if q.trim().is_empty() {
            return None;
        }
        let hits = settings::search(q);
        Some(
            div()
                .px_6()
                .pt_2()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(hits.into_iter().take(8).enumerate().map(|(i, hit)| {
                    Button::new(("settings-hit-narrow", i))
                        .label(SharedString::from(format!(
                            "{} · {}",
                            hit.label,
                            hit.section.title()
                        )))
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_settings(hit.section, window, cx);
                        }))
                }))
                .into_any_element(),
        )
    }

    /// A settings page: the title and its line, then `body`, in a column
    /// that scrolls with the thin scrollbar.
    pub(super) fn page(
        &self,
        s: Section,
        p: Palette,
        extra: Option<AnyElement>,
        body: Vec<AnyElement>,
    ) -> AnyElement {
        let header = div()
            .flex()
            .items_start()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::MEDIUM)
                            .child(s.title()),
                    )
                    .child(div().text_sm().text_color(p.muted).child(s.blurb())),
            )
            .children(extra);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(settings::PAGE_MAX_W))
                    .flex()
                    .flex_col()
                    .gap_5()
                    .px_6()
                    .py_6()
                    .child(header)
                    .children(body),
            )
            .overflow_y_scrollbar()
            .id(SharedString::from(format!("settings-scroll-{}", s.id())))
            .into_any_element()
    }

    /// A titled group of rows on a card.
    pub(super) fn settings_card(
        title: &'static str,
        p: Palette,
        rows: Vec<AnyElement>,
    ) -> AnyElement {
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
            .into_any_element()
    }

    /// A row: a title, a line under it, and a control on the right.
    pub(super) fn row(
        title: impl Into<SharedString>,
        line: impl Into<SharedString>,
        control: Option<AnyElement>,
        p: Palette,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .px_3()
            .py_2p5()
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .child(div().text_sm().child(title.into()))
                    .child(div().text_xs().text_color(p.faint).child(line.into())),
            )
            .children(control.map(|c| div().flex_none().child(c)))
            .into_any_element()
    }

    // ----- Providers -----

    fn providers_page(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.providers;
        let mut body: Vec<AnyElement> = Vec::new();
        if let Some(e) = &st.keychain_error {
            body.push(
                div()
                    .text_sm()
                    .text_color(p.muted)
                    .child(format!("The keychain cannot be read on this computer: {e}"))
                    .into_any_element(),
            );
        }
        if !st.loaded {
            body.push(loading("Reading the keychain…", p));
        } else {
            body.push(
                self.section(Kind::Model, "Model providers", p, cx)
                    .into_any_element(),
            );
            body.push(self.custom_provider_block(p, cx));
        }
        self.page(Section::Providers, p, None, body)
    }

    /// The server-address field under a provider row.
    pub(super) fn base_url_row(
        &self,
        id: &'static str,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .pl(px(40.))
            .child(div().text_xs().text_color(p.faint).child("Server address"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.settings_inputs.base_url)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("base-url-save-{id}")))
                            .label("Save")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.save_base_url(cx))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("base-url-cancel-{id}")))
                            .label("Cancel")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.settings.editing_url = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(div().text_xs().text_color(p.faint).child(
                "For a local server on another port or machine, or a proxy. Test checks it.",
            ))
            .into_any_element()
    }

    /// Opens the server-address field for provider `id`.
    pub(super) fn edit_base_url(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = self
            .model
            .providers
            .prefs
            .base_url(id)
            .unwrap_or_default()
            .to_owned();
        self.model.settings.editing_url = Some(id);
        self.settings_inputs.base_url.update(cx, |i, cx| {
            i.set_value(now, window, cx);
            i.focus(window, cx);
        });
        cx.notify();
    }

    fn save_base_url(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.model.settings.editing_url.take() else {
            return;
        };
        let url = self.settings_inputs.base_url.read(cx).value().to_string();
        self.save_ui_prefs(|p| p.set_base_url(id, &url));
        self.model.providers.forget_models(id);
        self.model.providers.begin(Kind::Model, id, "Checking…");
        self.bridge.test(Kind::Model, id);
        if self.model.providers.default_id(Kind::Model) == Some(id) {
            self.refresh_models();
            self.reconnect();
        }
        cx.notify();
    }

    /// The default provider's models, virtualised: only the rows in view
    /// are built, so a provider listing hundreds of models costs the same
    /// per frame as one listing ten.
    pub(super) fn provider_model_list(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.providers;
        let body: AnyElement = match &st.models {
            None | Some(Models::Loading(_)) => loading("Listing models…", p),
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
                let n = list.len();
                let h = (n as f32 * LIST_ROW_H).min(settings::MODEL_LIST_MAX_H);
                // Its own bounded box with its own thin scrollbar: the
                // wheel over it scrolls the list, never the page behind it.
                div()
                    .id("provider-models-box")
                    .relative()
                    .h(px(h + 2.))
                    .rounded_lg()
                    .border_1()
                    .border_color(p.border)
                    .overflow_hidden()
                    // The wheel over the box scrolls the models first; at
                    // the list's ends it goes on to the page.
                    .child(super::wheel_first(
                        &self.settings_inputs.provider_models_scroll,
                    ))
                    .child(
                        uniform_list(
                            "provider-models",
                            n,
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                this.provider_model_rows(range, cx)
                            }),
                        )
                        .size_full()
                        .track_scroll(&self.settings_inputs.provider_models_scroll),
                    )
                    .child(super::inner_list_bar(
                        "provider-models-bar",
                        &self.settings_inputs.provider_models_scroll,
                    ))
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .pl(px(40.))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(p.faint)
                    .child("Model")
                    .children(match &st.models {
                        Some(Models::Loaded(_, l))
                            if l.len() as f32 * LIST_ROW_H > settings::MODEL_LIST_MAX_H =>
                        {
                            Some(SharedString::from(format!(
                                "{} models · scroll the list",
                                l.len()
                            )))
                        }
                        _ => None,
                    }),
            )
            .child(body)
            .into_any_element()
    }

    fn provider_model_rows(
        &mut self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = Palette::get(self.model.dark);
        let st = &self.model.providers;
        let Some(Models::Loaded(_, list)) = &st.models else {
            return Vec::new();
        };
        let chosen = st
            .chosen_model()
            .map(str::to_owned)
            .or_else(|| self.model.connection.as_ref().map(|c| c.model.clone()));
        let busy = self.model.is_busy();
        range
            .filter_map(|i| list.get(i).map(|m| (i, m)))
            .map(|(i, m)| {
                let selected = chosen.as_deref() == Some(m.id.as_str());
                let id = m.id.clone();
                model_line(
                    SharedString::from(format!("pm-{i}")),
                    &m.id,
                    m.context_window,
                    selected,
                    p,
                )
                .when(!busy && !selected, |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.pick_model(id.clone(), cx)))
                })
                .into_any_element()
            })
            .collect()
    }

    // ----- Models -----

    fn catalog_list(&self) -> Vec<(&'static str, &tern_agent::providers::ModelInfo)> {
        self.model.catalog.models().collect()
    }

    fn models_page(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.settings;
        let prefs = &self.model.providers.prefs;
        let models = self.catalog_list();
        // The default model per job.
        let roles: Vec<AnyElement> = Role::ALL
            .into_iter()
            .map(|role| {
                let current = match role {
                    Role::Decision => Some(self.model.decision.mode().label().to_owned()),
                    Role::Chat => Role::Chat.saved(prefs).or_else(|| {
                        self.model
                            .connection
                            .as_ref()
                            .map(|c| format!("{}/{}", c.provider, c.model))
                    }),
                    r => r.saved(prefs),
                };
                let open = st.role_open == Some(role);
                let control: AnyElement = if role == Role::Decision {
                    Button::new("role-decision")
                        .label("Open")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_settings(Section::Decision, window, cx)
                        }))
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .max_w(px(240.))
                                .truncate()
                                .text_xs()
                                .font_family(assets::MONO_FONT)
                                .text_color(if current.is_some() { p.text } else { p.faint })
                                .child(SharedString::from(
                                    current.clone().unwrap_or_else(|| "Automatic".into()),
                                )),
                        )
                        .child(
                            Button::new(SharedString::from(format!("role-{}", role.label())))
                                .label(if open { "Done" } else { "Change" })
                                .ghost()
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let s = &mut this.model.settings;
                                    s.role_open = if s.role_open == Some(role) {
                                        None
                                    } else {
                                        Some(role)
                                    };
                                    cx.notify();
                                })),
                        )
                        .into_any_element()
                };
                let mut row = div().flex().flex_col().child(Self::row(
                    role.label(),
                    role.blurb(),
                    Some(control),
                    p,
                ));
                if open {
                    row = row.child(self.role_picker(role, &models, p, cx));
                }
                row.into_any_element()
            })
            .collect();

        // The catalogue, by capability.
        let rows = settings::catalog_rows(&models, st.group, &st.filter);
        let present: Vec<Group> = Group::ALL
            .into_iter()
            .filter(|g| models.iter().any(|(_, m)| !m.is_retired() && g.holds(m)))
            .collect();
        let chips = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .child(
                Button::new("group-all")
                    .label("All")
                    .ghost()
                    .xsmall()
                    .selected(st.group.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.settings.group = None;
                        cx.notify();
                    })),
            )
            .children(present.into_iter().map(|g| {
                Button::new(SharedString::from(format!("group-{}", g.label())))
                    .label(g.label())
                    .ghost()
                    .xsmall()
                    .selected(st.group == Some(g))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.settings.group = Some(g);
                        cx.notify();
                    }))
            }));
        let n = rows.len();
        let catalogue: AnyElement = if models.is_empty() {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .text_sm()
                .text_color(p.muted)
                .child("No models listed yet. Add a provider's key, or list again.")
                .child(
                    Button::new("catalog-refresh")
                        .label("List models")
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.refresh_catalog();
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else if n == 0 {
            div()
                .text_sm()
                .text_color(p.muted)
                .child("No model matches.")
                .into_any_element()
        } else {
            div()
                .relative()
                .id("model-catalogue-box")
                .h(px((n as f32 * LIST_ROW_H).min(440.)))
                .rounded_xl()
                .border_1()
                .border_color(p.border)
                .overflow_hidden()
                .child(super::wheel_first(&self.settings_inputs.catalog_scroll))
                .child(
                    uniform_list(
                        "model-catalogue",
                        n,
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            this.catalog_rows_view(range, cx)
                        }),
                    )
                    .size_full()
                    .track_scroll(&self.settings_inputs.catalog_scroll),
                )
                .child(super::inner_list_bar(
                    "model-catalogue-bar",
                    &self.settings_inputs.catalog_scroll,
                ))
                .vertical_scrollbar(&self.settings_inputs.catalog_scroll)
                .into_any_element()
        };
        let catalogue_block = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!(
                        "Catalogue · {} models from your providers",
                        models.iter().filter(|(_, m)| !m.is_retired()).count()
                    ))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .w(px(220.))
                            .child(Input::new(&self.settings_inputs.filter).small()),
                    )
                    .child(chips),
            )
            .child(catalogue)
            .into_any_element();
        self.page(
            Section::Models,
            p,
            None,
            vec![
                Self::settings_card("Default model per job", p, roles),
                catalogue_block,
            ],
        )
    }

    fn role_picker(
        &self,
        role: Role,
        models: &[(&'static str, &tern_agent::providers::ModelInfo)],
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let choices = settings::role_choices(models.iter().copied(), role);
        if choices.is_empty() {
            return div()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(p.muted)
                .child("None of your providers lists a model for this yet.")
                .into_any_element();
        }
        // Automatic first, except for chat, which always has a model.
        let n = choices.len() + usize::from(role != Role::Chat);
        div()
            .id("role-choices-box")
            .relative()
            .mx_3()
            .mb_2()
            .h(px((n as f32 * LIST_ROW_H).min(220.)))
            .child(super::wheel_first(&self.settings_inputs.role_scroll))
            .child(
                uniform_list(
                    "role-choices",
                    n,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        this.role_rows(role, range, cx)
                    }),
                )
                .size_full()
                .track_scroll(&self.settings_inputs.role_scroll),
            )
            .child(super::inner_list_bar(
                "role-choices-bar",
                &self.settings_inputs.role_scroll,
            ))
            .into_any_element()
    }

    fn role_rows(
        &mut self,
        role: Role,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = Palette::get(self.model.dark);
        let models = self.catalog_list();
        let choices = settings::role_choices(models.iter().copied(), role);
        let auto = role != Role::Chat;
        let saved = role.saved(&self.model.providers.prefs);
        range
            .filter_map(|i| {
                let key = if auto {
                    if i == 0 {
                        None
                    } else {
                        Some(choices.get(i - 1)?.clone())
                    }
                } else {
                    Some(choices.get(i)?.clone())
                };
                let selected = key == saved;
                let label = key.clone().unwrap_or_else(|| "Automatic".into());
                Some(
                    model_line(
                        SharedString::from(format!("role-{i}")),
                        &label,
                        None,
                        selected,
                        p,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_role(role, key.clone(), cx);
                    }))
                    .into_any_element(),
                )
            })
            .collect()
    }

    /// Saves a role's default model.
    fn set_role(&mut self, role: Role, key: Option<String>, cx: &mut Context<Self>) {
        if role == Role::Chat {
            let Some((provider, model)) = key.as_deref().and_then(|k| k.split_once('/')) else {
                return;
            };
            let Some(id) = self
                .model
                .providers
                .rows
                .iter()
                .find(|r| r.kind == Kind::Model && r.id == provider)
                .map(|r| r.id)
            else {
                return;
            };
            if self.model.is_busy() {
                return;
            }
            self.model.providers.choose(Kind::Model, id);
            self.model.providers.choose_model(model);
            self.refresh_models();
            self.reconnect();
        } else {
            self.save_ui_prefs(|p| role.save(p, key.as_deref()));
        }
        cx.notify();
    }

    fn catalog_rows_view(
        &mut self,
        range: std::ops::Range<usize>,
        _cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let p = Palette::get(self.model.dark);
        let st = &self.model.settings;
        let models = self.catalog_list();
        let rows = settings::catalog_rows(&models, st.group, &st.filter);
        range
            .filter_map(|i| rows.get(i).cloned().map(|r| (i, r)))
            .map(|(i, r)| match r {
                CatalogRow::Header { group, count } => div()
                    .id(("cat", i))
                    .h(px(LIST_ROW_H))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .bg(p.raised)
                    .text_xs()
                    .text_color(p.faint)
                    .child(group.label())
                    .child(SharedString::from(count.to_string()))
                    .into_any_element(),
                CatalogRow::Model {
                    provider,
                    id,
                    context,
                    ..
                } => {
                    div()
                        .id(("cat", i))
                        .h(px(LIST_ROW_H))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .text_sm()
                        .hover(|d| d.bg(p.hover))
                        .child(super::brand_logo(provider, 14., p))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(SharedString::from(id)),
                        )
                        .children(context.map(|w| {
                            div().flex_none().text_xs().text_color(p.faint).child(
                                SharedString::from(format!("{} context", crate::model::compact(w))),
                            )
                        }))
                        .into_any_element()
                }
            })
            .collect()
    }

    // ----- Tools -----

    fn tools_page(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let prefs = &self.model.providers.prefs;
        let rows: Vec<AnyElement> = tern_agent::tools::BUILTIN_TOOLS
            .iter()
            .map(|t| {
                let name = t.name;
                let on = prefs.tool_on(name);
                let mode = prefs.tool_mode(name);
                let selected = match mode {
                    None => 0,
                    Some(tern_agent::Mode::Ask) => 1,
                    Some(tern_agent::Mode::Allow) => 2,
                    Some(tern_agent::Mode::Deny) => 3,
                };
                let class_default = if t.effect.asks_by_default() {
                    "Auto (asks)"
                } else {
                    "Auto (runs)"
                };
                let options = [
                    Segment::text(class_default),
                    Segment::text("Ask"),
                    Segment::text("Allow"),
                    Segment::text("Deny"),
                ];
                let pick: OnPick = Rc::new(
                    move |this: &mut Workspace,
                          i: usize,
                          _: &mut Window,
                          _: &mut Context<Workspace>| {
                        let m = match i {
                            1 => Some(tern_agent::Mode::Ask),
                            2 => Some(tern_agent::Mode::Allow),
                            3 => Some(tern_agent::Mode::Deny),
                            _ => None,
                        };
                        this.save_ui_prefs(|p| p.set_tool_mode(name, m));
                    },
                );
                let switch = Switch::new(SharedString::from(format!("tool-on-{name}")))
                    .checked(on)
                    .small()
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        let checked = *checked;
                        this.save_ui_prefs(|p| p.set_tool_on(name, checked));
                        cx.notify();
                    }));
                // Two lines, so the approval choices never run past the
                // card's edge: the tool and its switch, then (when on)
                // the choices under it, left-aligned.
                div()
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
                            .child(
                                div()
                                    .size(px(28.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(p.border)
                                    .bg(p.raised)
                                    .child(icon(tool_icon(name), p.text)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .flex_1()
                                    .child(div().text_sm().child(SharedString::from(format!(
                                        "{} · {}",
                                        t.label, t.name
                                    ))))
                                    .child(div().text_xs().text_color(p.faint).child(
                                        SharedString::from(format!(
                                            "{}{}",
                                            effect_line(t.effect),
                                            if on {
                                                ""
                                            } else {
                                                " · off: the model never sees it"
                                            }
                                        )),
                                    )),
                            )
                            .child(div().flex_none().child(switch)),
                    )
                    .when(on, |d| {
                        d.child(div().pl(px(40.)).flex().flex_wrap().child(self.segmented(
                            name,
                            &options,
                            selected,
                            Look::default(),
                            p,
                            pick,
                            cx,
                        )))
                    })
                    .into_any_element()
            })
            .collect();
        self.page(
            Section::Tools,
            p,
            None,
            vec![
                Self::settings_card("Built-in tools", p, rows),
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(
                        "Changes apply to chats opened from now on. Auto follows the tool's kind: reading runs, writing and reaching new sites ask. MCP tools have their own switches under MCP & Skills.",
                    )
                    .into_any_element(),
            ],
        )
    }

    // ----- Search & Crawler -----

    fn search_page(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.model.providers;
        let mut body: Vec<AnyElement> = Vec::new();
        if !st.loaded {
            body.push(loading("Reading the keychain…", p));
        } else {
            body.push(
                self.section(Kind::Search, "Search services", p, cx)
                    .into_any_element(),
            );
        }
        body.push(self.crawlers_card(p, cx));
        let cache = tern_agent::platform::AppPaths::discover()
            .ok()
            .map(|a| a.crawl_cache());
        // Measured on opening the page and after a clear, never per frame.
        let size = self.model.settings.cache_size.unwrap_or(0);
        let confirm = self.model.settings.confirm == Some("crawl-cache");
        let crawl_rows = vec![
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_3()
                .border_b_1()
                .border_color(p.border)
                .child(
                    div()
                        .size(px(28.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_lg()
                        .border_1()
                        .border_color(p.border)
                        .bg(p.raised)
                        .child(gpui_kit::img(assets::CRAWL_LOGO).size(px(18.))),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Tern Crawl"))
                        .child(div().text_xs().text_color(p.faint).child(
                            "The built-in crawler, and the fallback when a hosted one fails: HTML and PDFs, in reader form, with the sources kept.",
                        )),
                )
                .into_any_element(),
            Self::row(
                "Polite by design",
                "Obeys robots.txt, sends one request per site at a time with pauses between, and names itself as Tern.",
                None,
                p,
            ),
            Self::row(
                "Guarded",
                "Pages are untrusted: instructions inside them are fenced off, and a new site found after reading one asks first.",
                None,
                p,
            ),
            Self::row(
                "Page cache",
                SharedString::from(format!(
                    "{} · {}",
                    human_bytes(size),
                    cache
                        .as_ref()
                        .map(|c| c.display().to_string())
                        .unwrap_or_else(|| "no cache folder".into())
                )),
                Some(
                    Button::new("crawl-clear")
                        .label(if confirm { "Click again to clear" } else { "Clear" })
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.model.settings.confirm("crawl-cache") {
                                if let Ok(a) = tern_agent::platform::AppPaths::discover() {
                                    let _ = std::fs::remove_dir_all(a.crawl_cache());
                                }
                                this.measure_cache();
                                this.model.toast_info("Page cache cleared");
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                p,
            ),
        ];
        body.push(Self::settings_card("Tern Crawl", p, crawl_rows));
        self.page(Section::Search, p, None, body)
    }

    // ----- Privacy & data -----

    fn privacy_page(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let leaves = vec![
            Self::row(
                "Your question and what it needs",
                "Go to the model provider you chose, with your key.",
                None,
                p,
            ),
            Self::row(
                "Search queries",
                "Go to the search service you chose (Wikipedia needs no key).",
                None,
                p,
            ),
            Self::row(
                "Page requests",
                "Go to the sites Tern Crawl reads, nothing else.",
                None,
                p,
            ),
            Self::row(
                "Stays here",
                "Your vault, keys (system keychain), memory, chats, and run logs. No telemetry.",
                None,
                p,
            ),
        ];
        let paths = tern_agent::platform::AppPaths::discover().ok();
        let vault = tern_agent::vault::default_root(&self.model.providers.prefs).ok();
        let mut places: Vec<(&'static str, Option<std::path::PathBuf>)> = vec![
            (
                "Chats, runs, and the database",
                paths.as_ref().map(|a| a.data_dir.clone()),
            ),
            ("Settings", paths.as_ref().map(|a| a.config_dir.clone())),
            ("Cache", paths.as_ref().map(|a| a.cache_dir.clone())),
        ];
        places.push(("Vault", vault));
        let storage: Vec<AnyElement> = places
            .into_iter()
            .enumerate()
            .map(|(i, (what, path))| {
                let shown = path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "Not set".into());
                let control = path.map(|path| {
                    Button::new(("storage-open", i))
                        .label("Open")
                        .ghost()
                        .small()
                        .on_click(move |_, _, cx| {
                            cx.open_url(&tern_agent::platform::file_url(&path.to_string_lossy()));
                        })
                        .into_any_element()
                });
                Self::row(what, shown, control, p)
            })
            .collect();
        let confirm = self.model.settings.confirm;
        let danger = |id: &'static str, label: &'static str, again: &'static str| {
            Button::new(id)
                .label(if confirm == Some(id) { again } else { label })
                .ghost()
                .small()
        };
        let delete = vec![
            Self::row(
                "Clear the page cache",
                "Pages Tern Crawl saved to read again faster.",
                Some(
                    danger("del-cache", "Clear", "Click again to clear")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.model.settings.confirm("del-cache") {
                                if let Ok(a) = tern_agent::platform::AppPaths::discover() {
                                    let _ = std::fs::remove_dir_all(a.crawl_cache());
                                }
                                this.measure_cache();
                                this.model.toast_info("Page cache cleared");
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                p,
            ),
            Self::row(
                "Empty the trash",
                "Deleted chats are kept until the trash is emptied.",
                Some(
                    danger("del-trash", "Empty", "Click again to empty")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.model.settings.confirm("del-trash") {
                                this.bridge.empty_trash();
                                this.model.toast_info("Trash emptied");
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                p,
            ),
            Self::row(
                "Reset settings",
                "Every choice back to its default. Keys, chats, and the vault stay.",
                Some(
                    danger("del-prefs", "Reset", "Click again to reset")
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.model.settings.confirm("del-prefs") {
                                this.reset_settings(window, cx);
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                p,
            ),
        ];
        self.page(
            Section::Privacy,
            p,
            None,
            vec![
                Self::settings_card("What leaves this computer", p, leaves),
                Self::settings_card("Where Tern keeps your data", p, storage),
                Self::settings_card("Delete", p, delete),
            ],
        )
    }

    fn reset_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The tour is not shown again just because settings were reset.
        let fresh = tern_agent::Preferences {
            onboarded: true,
            ..tern_agent::Preferences::default()
        };
        self.model.providers.prefs = fresh.clone();
        self.bridge.save_prefs(fresh);
        self.look = crate::appearance::Appearance::default();
        let dark = self.look.is_dark(super::system_dark(window));
        self.model.dark = dark;
        crate::theme::apply_kit(dark, &self.look, Some(window), cx);
        self.model.study = crate::study::StudyState::from_prefs(&self.model.providers.prefs);
        self.model.toast_info("Settings reset");
    }

    // ----- About -----

    fn about_page(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let lockup = div()
            .flex()
            .items_center()
            .gap_3()
            .py_2()
            .child(svg().path(assets::MARK).size(px(40.)).text_color(p.text))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_xl()
                            .font_family(assets::MONO_FONT)
                            .font_weight(FontWeight::MEDIUM)
                            .child("TERN"),
                    )
                    .child(div().w(px(64.)).h(px(2.)).rounded_full().bg(p.accent)),
            )
            .into_any_element();
        let info = vec![
            Self::row(
                "Version",
                SharedString::from(format!(
                    "{} · {}",
                    env!("CARGO_PKG_VERSION"),
                    tern_agent::platform::os_name()
                )),
                None,
                p,
            ),
            Self::row(
                "Source and changes",
                "github.com/Khubaib7-del/Tern",
                Some(
                    Button::new("about-github")
                        .label("Open")
                        .ghost()
                        .small()
                        .on_click(|_, _, cx| cx.open_url("https://github.com/Khubaib7-del/Tern"))
                        .into_any_element(),
                ),
                p,
            ),
            Self::row(
                "Welcome tour",
                "Provider, key, model, search, and theme, one step each.",
                Some(
                    Button::new("about-onboarding")
                        .label("Run it again")
                        .small()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_onboarding(window, cx)),
                        )
                        .into_any_element(),
                ),
                p,
            ),
        ];
        let keys = [
            ("Settings", "Ctrl+,  (Cmd+, on macOS)"),
            ("Back from Settings", "Esc"),
            ("Send · new line", "Enter · Shift+Enter"),
            ("Queue a message while a run goes", "Tab"),
            ("Stop", "Esc in the composer"),
            ("Maximise the right pane", "Ctrl+\\"),
        ]
        .into_iter()
        .map(|(what, key)| {
            Self::row(
                what,
                "",
                Some(
                    div()
                        .text_xs()
                        .font_family(assets::MONO_FONT)
                        .text_color(p.muted)
                        .child(key)
                        .into_any_element(),
                ),
                p,
            )
        })
        .collect();
        self.page(
            Section::About,
            p,
            None,
            vec![
                lockup,
                Self::settings_card("Tern", p, info),
                Self::settings_card("Shortcuts", p, keys),
            ],
        )
    }
}

/// A model on one line of a list, selected or not.
fn model_line(
    id: SharedString,
    label: &str,
    context: Option<u64>,
    selected: bool,
    p: Palette,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .h(px(LIST_ROW_H))
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .px_2()
        .rounded_md()
        .text_sm()
        .cursor_pointer()
        .text_color(if selected { p.text } else { p.muted })
        .when(selected, |d| d.bg(p.hover))
        .hover(|d| d.bg(p.hover).text_color(p.text))
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(SharedString::from(label.to_owned())),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(p.faint)
                .when_some(context, |d, w| {
                    d.child(SharedString::from(format!(
                        "{} context",
                        crate::model::compact(w)
                    )))
                })
                .when(selected, |d| d.child(Icon::new(AssetIcon::Check).small())),
        )
}

fn loading(text: &'static str, p: Palette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .text_color(p.muted)
        .child(Spinner::new().small().color(p.muted))
        .child(text)
        .into_any_element()
}

fn effect_line(e: tern_core::EffectClass) -> &'static str {
    use tern_core::EffectClass as E;
    match e {
        E::Read => "Reads local data",
        E::Analysis => "Works on what is already in the chat",
        E::Write => "Writes files",
        E::Network => "Reaches the web",
        E::Credentials => "Touches stored keys",
        E::ExternalSideEffect => "Acts outside Tern",
        E::Destructive => "Deletes data",
    }
}

/// The size of everything under `dir`, in bytes (0 when it is missing).
pub(super) fn dir_size(dir: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

fn human_bytes(n: u64) -> String {
    const K: f64 = 1024.;
    let f = n as f64;
    if f < K {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.0} KB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MB", f / K / K)
    } else {
        format!("{:.1} GB", f / K / K / K)
    }
}

#[cfg(test)]
mod tests {
    use super::human_bytes;

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(2048), "2 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024 + 1024 * 300), "5.3 MB");
    }
}
