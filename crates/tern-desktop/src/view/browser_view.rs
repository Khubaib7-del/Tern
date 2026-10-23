//! The built-in browser pane: pages read as clean Markdown beside the
//! chat, in the run panel's place. Links inside it (and citations in
//! answers) open here; the globe button hands a page to the system browser.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, ClickEvent, Context, SharedString, Window, div};

use super::Workspace;
use crate::theme::Palette;

/// The browser pane's width beside the chat.
pub(super) const BROWSER_W: f32 = 460.;

impl Workspace {
    /// Opens an address (typed or clicked) in the pane.
    pub(super) fn browse(&mut self, input: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((request, url)) = self.model.browser.navigate(input) {
            self.model.run_panel_open = true;
            self.bridge.browse(request, url.clone());
            let shown = tern_agent::browse::display_address(&url);
            self.address_input
                .update(cx, |i, cx| i.set_value(shown, window, cx));
        }
        cx.notify();
    }

    fn browse_step(
        &mut self,
        step: Option<(u64, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((request, url)) = step {
            self.bridge.browse(request, url.clone());
            let shown = tern_agent::browse::display_address(&url);
            self.address_input
                .update(cx, |i, cx| i.set_value(shown, window, cx));
        }
        cx.notify();
    }

    /// Shows or hides the pane from the rail.
    pub(super) fn toggle_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = &mut self.model.browser;
        if b.visible && self.model.run_panel_open {
            self.model.run_panel_open = false;
        } else {
            b.visible = true;
            self.model.run_panel_open = true;
            self.address_input.update(cx, |i, cx| i.focus(window, cx));
        }
        cx.notify();
    }

    /// A handler for links in rendered Markdown that opens them here.
    pub(super) fn link_opener(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&SharedString, &ClickEvent, &mut Window, &mut App) + Send + Sync + 'static {
        let this = cx.entity().downgrade();
        move |url: &SharedString, _: &ClickEvent, window: &mut Window, app: &mut App| {
            let url = url.to_string();
            // A verdict badge opens its claim in the run report.
            if let Some((entry, claim)) = tern_agent::verify::claim_of_link(&url) {
                let _ = this.update(app, |w, cx| {
                    w.model.show_claim(entry, claim);
                    cx.notify();
                });
                return;
            }
            let _ = this.update(app, |w, cx| w.browse(&url, window, cx));
        }
    }

    pub(super) fn browser_pane(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.model.browser;
        let nav = |id: &'static str, icon: AssetIcon, tip: &'static str, on: bool| {
            Button::new(id)
                .icon(icon)
                .ghost()
                .xsmall()
                .tooltip(tip)
                .disabled(!on)
        };
        let current = b.page.as_ref().map(|pg| pg.page.url.clone());
        let bar = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                nav(
                    "web-back",
                    AssetIcon::ArrowLeft,
                    "Back",
                    b.history.can_back(),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    let step = this.model.browser.back();
                    this.browse_step(step, window, cx);
                })),
            )
            .child(
                nav(
                    "web-forward",
                    AssetIcon::ArrowRight,
                    "Forward",
                    b.history.can_forward(),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    let step = this.model.browser.forward();
                    this.browse_step(step, window, cx);
                })),
            )
            .child(
                nav(
                    "web-reload",
                    AssetIcon::RotateCw,
                    "Reload",
                    b.history.current().is_some(),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    let step = this.model.browser.reload();
                    this.browse_step(step, window, cx);
                })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.address_input).small()),
            )
            .child(
                nav(
                    "web-ask",
                    AssetIcon::MessageSquarePlus,
                    "Ask about this page",
                    b.page.is_some(),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    if let Some(pg) = &this.model.browser.page {
                        let a = pg.attachment();
                        this.model.attached(Ok(a));
                        this.composer.update(cx, |c, cx| c.focus(window, cx));
                    }
                    cx.notify();
                })),
            )
            .child(
                nav(
                    "web-external",
                    AssetIcon::Globe,
                    "Open in your browser",
                    current.is_some(),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(pg) = &this.model.browser.page {
                        cx.open_url(&pg.page.url);
                    }
                })),
            )
            .child(
                Button::new("web-close")
                    .icon(AssetIcon::X)
                    .ghost()
                    .xsmall()
                    .tooltip("Close")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.browser.visible = false;
                        this.model.run_panel_open = false;
                        cx.notify();
                    })),
            );

        let body: AnyElement = if let Some(url) = &b.loading {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(p.muted)
                .child(Spinner::new().small())
                .child(SharedString::from(format!("Reading {url}")))
                .into_any_element()
        } else if let Some(e) = &b.error {
            div()
                .flex_1()
                .p_4()
                .text_sm()
                .text_color(p.muted)
                .child(SharedString::from(e.clone()))
                .into_any_element()
        } else if let Some(pg) = &b.page {
            div()
                .id("web-page")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_4()
                .pb_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .py_2()
                        .child(self.site_icon(&pg.page, 16., p))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .child(SharedString::from(pg.page.label().to_owned())),
                        ),
                )
                .child(
                    TextView::markdown(
                        SharedString::from(format!("web-{}", pg.page.url)),
                        pg.markdown.clone(),
                    )
                    .selectable(true)
                    .on_link_click(self.link_opener(cx)),
                )
                .when(pg.truncated, |d| {
                    d.child(
                        div()
                            .pt_3()
                            .text_xs()
                            .text_color(p.faint)
                            .child("The page was long; Tern shows the first part."),
                    )
                })
                .into_any_element()
        } else {
            div()
                .flex_1()
                .p_4()
                .text_sm()
                .text_color(p.muted)
                .child("Type an address, or click a source. Pages open as readable text, without scripts.")
                .into_any_element()
        };

        div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .bg(p.sidebar)
            .border_l_1()
            .border_color(p.border)
            .child(div().p_2().child(bar))
            .child(body)
            .into_any_element()
    }
}
