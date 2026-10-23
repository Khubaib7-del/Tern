//! Appearance: the locked design in one place for review (ADR 0002). The
//! palette, the three faces, Tern's glyphs, the built-in service logos,
//! and Scout, with a button for each of its moods.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Selectable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, Hsla, SharedString, Window, div, px, svg};

use super::Workspace;
use crate::appearance::{Accent, Choice, FontChoice, TextSize, ThemeChoice};
use crate::assets::{self, MONO_FONT, SERIF_FONT, UI_FONT};
use crate::model::Screen;
use crate::scout::Mood;
use crate::theme::Palette;

impl Workspace {
    pub(super) fn open_appearance(&mut self, cx: &mut Context<Self>) {
        self.model.screen = Screen::Appearance;
        cx.notify();
    }

    pub(super) fn close_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.screen = Screen::Chat;
        self.scout_preview = None;
        self.composer.update(cx, |c, cx| c.focus(window, cx));
        cx.notify();
    }

    pub(super) fn appearance_screen(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let section = |title: &'static str| {
            div()
                .pt_6()
                .pb_2()
                .text_xs()
                .text_color(p.faint)
                .child(title)
        };
        let swatch = |name: &'static str, c: Hsla| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .w(px(76.))
                .child(
                    div()
                        .h(px(36.))
                        .rounded_md()
                        .border_1()
                        .border_color(p.border)
                        .bg(c),
                )
                .child(div().text_xs().text_color(p.muted).child(name))
        };
        let tile = |label: String, path: SharedString| {
            div()
                .w(px(96.))
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .py_2()
                .rounded_md()
                .child(svg().path(path).size(px(18.)).text_color(p.text))
                .child(
                    div()
                        .text_xs()
                        .text_color(p.muted)
                        .child(SharedString::from(label)),
                )
        };

        // The choices: each click applies at once and is saved.
        let look = self.look;
        let choice_row = |label: &'static str, buttons: Vec<AnyElement>| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .py_1()
                .child(div().w(px(96.)).text_sm().text_color(p.muted).child(label))
                .child(div().flex().flex_wrap().gap_1().children(buttons))
        };
        // Theme, font and size are segmented controls whose highlight
        // slides to the picked option.
        let row_of =
            |key: &'static str, choices: Vec<(&'static str, Choice)>, cx: &mut Context<Self>| {
                let selected = choices.iter().position(|(_, c)| look.is(*c)).unwrap_or(0);
                let segs: Vec<super::segmented::Segment> = choices
                    .iter()
                    .map(|(l, _)| super::segmented::Segment::text(*l))
                    .collect();
                let picks: Vec<Choice> = choices.iter().map(|(_, c)| *c).collect();
                vec![self.segmented(
                    key,
                    &segs,
                    selected,
                    super::segmented::Look {
                        small: true,
                        ..Default::default()
                    },
                    p,
                    std::rc::Rc::new(move |this: &mut Self, i, window, cx| {
                        if let Some(&c) = picks.get(i) {
                            this.set_appearance(c, window, cx);
                        }
                    }),
                    cx,
                )]
            };
        let themes = row_of(
            "look-theme",
            ThemeChoice::ALL
                .iter()
                .map(|&t| (t.label(), Choice::Theme(t)))
                .collect(),
            cx,
        );
        let accents: Vec<AnyElement> = Accent::ALL
            .iter()
            .map(|&a| {
                let c = Choice::Accent(a);
                let on = look.is(c);
                let dark = self.model.dark;
                div()
                    .id(SharedString::from(format!("look-accent-{}", a.label())))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if on { p.muted } else { p.border })
                    .when(on, |d| d.bg(p.hover))
                    .hover(move |d| d.bg(p.hover))
                    .child(
                        div()
                            .size(px(12.))
                            .rounded_full()
                            .bg(gpui_kit::rgb(a.hex(dark))),
                    )
                    .child(div().text_sm().text_color(p.text).child(a.label()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_appearance(c, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        let fonts = row_of(
            "look-font",
            FontChoice::ALL
                .iter()
                .map(|&f| (f.label(), Choice::Font(f)))
                .collect(),
            cx,
        );
        let sizes = row_of(
            "look-size",
            TextSize::ALL
                .iter()
                .map(|&z| (z.label(), Choice::Size(z)))
                .collect(),
            cx,
        );
        let choices = div()
            .flex()
            .flex_col()
            .child(choice_row("Theme", themes))
            .child(choice_row("Accent", accents))
            .child(choice_row("Font", fonts))
            .child(choice_row("Text size", sizes));

        let current = self.scout_mood();
        let moods = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(Mood::ALL.iter().map(|&m| {
                Button::new(SharedString::from(format!("scout-{}", m.label())))
                    .label(m.label())
                    .ghost()
                    .xsmall()
                    .selected(current == m)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.scout_preview = Some(m);
                        if matches!(m, Mood::Searching | Mood::Reading | Mood::Verifying) {
                            this.scout.kick();
                        }
                        cx.notify();
                    }))
            }));

        div()
            .id("appearance-screen")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .mx_auto()
                    .w_full()
                    .max_w(px(760.))
                    .px_6()
                    .py_6()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Appearance"))
                            .child(self.scout_figure(28., p)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.muted)
                            .child("Palette C: black and white, one quiet accent. Changes apply at once and are saved."),
                    )
                    .child(section("Look"))
                    .child(choices)
                    .child(section("Palette"))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_3()
                            .child(swatch("Background", p.rail))
                            .child(swatch("Surface", p.canvas))
                            .child(swatch("Raised", p.raised))
                            .child(swatch("Border", p.border))
                            .child(swatch("Text", p.text))
                            .child(swatch("Muted", p.muted))
                            .child(swatch("Faint", p.faint))
                            .child(swatch("Accent", p.accent)),
                    )
                    .child(section("Type"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .font_family(UI_FONT)
                                    .text_base()
                                    .child("IBM Plex Sans for the interface: sources, steps, answers."),
                            )
                            .child(
                                div()
                                    .font_family(MONO_FONT)
                                    .text_sm()
                                    .text_color(p.muted)
                                    .child("IBM Plex Mono for paths, costs, and metadata · 12,480 tokens · $0.042"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap_2()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_xl()
                                            .child("TERN"),
                                    )
                                    .child(
                                        div()
                                            .font_family(SERIF_FONT)
                                            .italic()
                                            .text_xl()
                                            .text_color(p.muted)
                                            .child("Research"),
                                    )
                                    .child(
                                        div()
                                            .font_family(SERIF_FONT)
                                            .italic()
                                            .text_xl()
                                            .text_color(p.muted)
                                            .child("Study"),
                                    ),
                            ),
                    )
                    .child(section("Tern Crawl"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(gpui_kit::img(assets::CRAWL_LOGO).size(px(40.)).flex_none())
                            .children(assets::glyph("tern-crawl").map(|path| {
                                svg().path(path).size(px(24.)).text_color(p.muted)
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex()
                                            .items_baseline()
                                            .gap_2()
                                            .child(div().font_weight(FontWeight::SEMIBOLD).text_lg().child("TERN"))
                                            .child(
                                                div()
                                                    .font_family(SERIF_FONT)
                                                    .italic()
                                                    .text_lg()
                                                    .text_color(p.muted)
                                                    .child("Crawl"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(p.muted)
                                            .child("Tern's own crawler and reader. Shown wherever Tern read the pages itself."),
                                    ),
                            ),
                    )
                    .child(section("Tern glyphs"))
                    .child(
                        div().flex().flex_wrap().children(
                            assets::glyph_names()
                                .filter_map(|n| assets::glyph(n).map(|path| tile(n.replace('-', " "), path))),
                        ),
                    )
                    .child(section("Built-in service logos"))
                    .child(
                        div().flex().flex_wrap().children(
                            assets::service_names().map(|n| tile(n.to_owned(), assets::service_path(n))),
                        ),
                    )
                    .child(section("Scout"))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap_4()
                            .child(
                                div()
                                    .p_2()
                                    .rounded_lg()
                                    .bg(p.sidebar)
                                    .child(self.scout_figure(120., p)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .max_w(px(380.))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(p.muted)
                                            .child("Each mood comes from a real event in a run. Try one:"),
                                    )
                                    .child(moods),
                            ),
                    ),
            )
            .into_any_element()
    }
}
