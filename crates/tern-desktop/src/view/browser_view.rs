//! The built-in browser pane, beside the chat in the run panel's place:
//! tabs, an address bar that takes an address or words, back, forward,
//! reload, and a toggle between the live page (a real engine, see
//! `crate::webview`), Tern's explanation of it (the page as Tern Crawl
//! reads it, search results for words, and Tern's cited answer), or both
//! side by side. Links inside it and citations in answers open here; the
//! globe button hands a page to the system browser.

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, ClickEvent, Context, SharedString, Window, div};

use super::Workspace;
use crate::browser::{PaneView, engine_address};
use crate::theme::Palette;

/// The browser pane's width beside the chat.
pub(super) const BROWSER_W: f32 = 460.;

impl Workspace {
    /// Opens an address (typed or clicked) in the pane.
    pub(super) fn browse(&mut self, input: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((request, url)) = self.model.browser.navigate(input) {
            self.model.run_panel_open = true;
            self.web.borrow_mut().go(&engine_address(&url));
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
            self.web.borrow_mut().go(&engine_address(&url));
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

    /// Once per frame: keep the native page hidden unless it is showing,
    /// and let the address bar, tab title and explanation follow where
    /// the live page went on its own.
    pub(super) fn sync_web(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = &self.model.browser;
        let show = b.visible
            && self.model.run_panel_open
            && b.view.shows_page()
            && self.model.screen != crate::model::Screen::Settings
            && self.switcher.is_none()
            && self.onboarding.is_none();
        let reports = {
            let mut host = self.web.borrow_mut();
            host.frame(show);
            host.take_reports()
        };
        if let Some(t) = reports.title.filter(|t| !t.trim().is_empty()) {
            self.model.browser.title = t;
        }
        if let Some(u) = reports.url
            && let Some((request, url)) = self.model.browser.engine_moved(&u)
        {
            self.bridge.browse(request, url.clone());
            let shown = tern_agent::browse::display_address(&url);
            self.address_input
                .update(cx, |i, cx| i.set_value(shown, window, cx));
        }
    }

    /// Tern's answer about the page (or the words) showing, when the chat
    /// has one: the reply to the last message that named it.
    fn page_explanation(&self) -> Option<String> {
        let b = &self.model.browser;
        let key = tern_agent::browse::display_address(b.history.current()?);
        let entries = &self.model.entries;
        let at = entries.iter().rposition(
            |e| matches!(e, crate::model::Entry::User { text, .. } if text.contains(&key)),
        )?;
        entries[at + 1..].iter().find_map(|e| match e {
            crate::model::Entry::Assistant { text, .. } if !text.trim().is_empty() => {
                Some(text.clone())
            }
            _ => None,
        })
    }

    /// Asks Tern to explain the page with citations (or, for words, a
    /// short answer with sources). The answer comes in the chat and shows
    /// on the explanation side.
    fn explain_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = &self.model.browser;
        let Some(addr) = b.history.current().map(str::to_owned) else {
            return;
        };
        let shown = tern_agent::browse::display_address(&addr);
        let msg = if b.is_search() {
            format!("In a few lines, what is {shown}? Cite the sources you use.")
        } else {
            format!("Explain this page with citations: {shown}")
        };
        if !b.is_search()
            && let Some(pg) = &b.page
        {
            let a = pg.attachment();
            self.model.attached(Ok(a));
        }
        if self.model.browser.view == PaneView::Page {
            self.model.browser.view = PaneView::Split;
        }
        self.composer
            .update(cx, |c, cx| c.set_value(msg, window, cx));
        self.submit(window, cx);
    }

    /// The live page: a real engine where the build has one, placed on
    /// this element's bounds every frame; else Tern's reader view.
    fn web_page(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let engine_error = self.web.borrow().error.clone();
        if crate::webview::AVAILABLE && engine_error.is_none() {
            let host = self.web.clone();
            return div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .relative()
                .bg(p.canvas)
                .child(
                    gpui_kit::canvas(
                        move |bounds, window, _| {
                            let b = [
                                f32::from(bounds.origin.x),
                                f32::from(bounds.origin.y),
                                f32::from(bounds.size.width),
                                f32::from(bounds.size.height),
                            ];
                            host.borrow_mut().place(b, &*window);
                        },
                        |_, _, _, _| {},
                    )
                    .size_full(),
                )
                .into_any_element();
        }
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .when_some(engine_error, |d, e| {
                d.child(div().px_4().pt_2().text_xs().text_color(p.faint).child(
                    SharedString::from(format!(
                        "The page engine did not start ({e}); showing the page as text."
                    )),
                ))
            })
            .child(self.reader_body("web-page", p, cx))
            .into_any_element()
    }

    /// The explanation side: Tern's answer about the page when there is
    /// one, the button that asks for it, and the page as Tern reads it
    /// (or the search results, each a link that opens on the page side).
    fn web_explanation(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.model.browser;
        let search = b.is_search();
        let answer = self.page_explanation();
        let can = b.history.current().is_some() && !self.model.is_busy();
        div()
            .id("web-explanation")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(p.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .child(
                        div()
                            .font_family(crate::assets::SERIF_FONT)
                            .text_lg()
                            .child(if search { "Results" } else { "Explanation" }),
                    )
                    .child(
                        Button::new("web-explain")
                            .icon(AssetIcon::Sparkles)
                            .label(if search {
                                "Explain briefly"
                            } else {
                                "Explain with citations"
                            })
                            .small()
                            .disabled(!can)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.explain_page(window, cx)),
                            ),
                    ),
            )
            .when_some(answer, |d, text| {
                d.child(
                    div()
                        .mx_4()
                        .mb_2()
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(p.border)
                        .bg(p.raised)
                        .max_h(gpui_kit::relative(0.5))
                        .overflow_hidden()
                        .child(
                            TextView::markdown("web-answer", text)
                                .selectable(true)
                                .on_link_click(self.link_opener(cx)),
                        ),
                )
            })
            .child(self.reader_body("web-reader", p, cx))
            .into_any_element()
    }

    /// The page as Tern's reader shows it: loading, an error, the text,
    /// or what to do first.
    fn reader_body(&self, id: &'static str, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.model.browser;
        if let Some(url) = &b.loading {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(p.muted)
                .child(Spinner::new().small())
                .child(SharedString::from(format!(
                    "Reading {}",
                    tern_agent::browse::display_address(url)
                )))
                .into_any_element();
        }
        if let Some(e) = &b.error {
            return div()
                .flex_1()
                .p_4()
                .text_sm()
                .text_color(p.muted)
                .child(SharedString::from(e.clone()))
                .into_any_element();
        }
        let Some(pg) = &b.page else {
            return div()
                .flex_1()
                .p_4()
                .text_sm()
                .text_color(p.muted)
                .child("Type an address to open a page, or words to search. Sources in answers open here too.")
                .into_any_element();
        };
        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
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
                    SharedString::from(format!("{id}-{}", pg.page.url)),
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
    }

    /// The tab strip over the bar.
    fn web_tabs(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let b = &self.model.browser;
        let titles = b.tab_titles();
        let many = titles.len() > 1;
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .pt_2()
            .children(titles.into_iter().enumerate().map(|(i, t)| {
                let on = i == b.tab || !many;
                div()
                    .id(("web-tab", i))
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .max_w(gpui_kit::px(180.))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(if on { p.text } else { p.muted })
                    .when(on, |d| d.bg(p.raised))
                    .hover(|d| d.bg(p.hover))
                    .child(div().min_w_0().truncate().child(SharedString::from(t)))
                    .when(many, |d| {
                        d.child(
                            Button::new(("web-tab-x", i))
                                .icon(AssetIcon::X)
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    let to = this.model.browser.close_tab(i);
                                    this.after_tab_change(to, window, cx);
                                })),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let to = this.model.browser.switch_tab(i);
                        this.after_tab_change(to, window, cx);
                    }))
            }))
            .child(
                Button::new("web-tab-new")
                    .icon(AssetIcon::Plus)
                    .ghost()
                    .xsmall()
                    .tooltip("New tab")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.model.browser.new_tab();
                        this.web.borrow_mut().go("about:blank");
                        this.address_input.update(cx, |i, cx| {
                            i.set_value("", window, cx);
                            i.focus(window, cx);
                        });
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn after_tab_change(
        &mut self,
        to: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(url) = to {
            self.web.borrow_mut().go(&url);
        }
        let shown = self.model.browser.address();
        self.address_input
            .update(cx, |i, cx| i.set_value(shown, window, cx));
        cx.notify();
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
                    this.web.borrow().reload();
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

        let view = b.view;
        let segs: Vec<super::segmented::Segment> = PaneView::ALL
            .iter()
            .map(|v| super::segmented::Segment::text(v.label()))
            .collect();
        let toggle = self.segmented(
            "web-view",
            &segs,
            view.index(),
            super::segmented::Look {
                framed: true,
                fill: false,
                small: true,
            },
            p,
            std::rc::Rc::new(|this: &mut Self, i, _, cx| {
                this.model.browser.view = PaneView::ALL[i];
                cx.notify();
            }),
            cx,
        );
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .when(view.shows_page(), |d| d.child(self.web_page(p, cx)))
            .when(view.shows_explanation(), |d| {
                d.child(self.web_explanation(p, cx))
            });

        div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .bg(p.sidebar)
            .border_l_1()
            .border_color(p.border)
            .child(self.web_tabs(p, cx))
            .child(div().p_2().child(bar))
            .child(div().px_2().pb_2().flex().justify_center().child(toggle))
            .child(body)
            .into_any_element()
    }
}
