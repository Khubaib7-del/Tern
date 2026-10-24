//! Appearance: only what you can change. Theme, accent, font, text size,
//! density, and motion; each applies at once and is saved. (The design
//! system's specimens — type, glyphs, logos, Scout's moods, the palette —
//! are not settings, so they are not here.)

use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, div, px};

use super::Workspace;
use crate::appearance::{Accent, Choice, FontChoice, TextSize, ThemeChoice};
use crate::settings::Section;
use crate::theme::Palette;

impl Workspace {
    pub(super) fn open_appearance(&mut self, cx: &mut Context<Self>) {
        self.show_settings(Section::Appearance);
        cx.notify();
    }

    pub(super) fn appearance_screen(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        // The choices: each click applies at once and is saved.
        let look = self.look;
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
        let densities = row_of(
            "look-density",
            crate::appearance::Density::ALL
                .iter()
                .map(|&d| (d.label(), Choice::Density(d)))
                .collect(),
            cx,
        );
        let reduced = self.reduced_motion;
        let motion = vec![
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    gpui_kit::component::switch::Switch::new("look-reduce-motion")
                        .checked(reduced)
                        .small()
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            let on = *on;
                            this.reduced_motion = on || tern_agent::platform::os_reduces_motion();
                            this.save_ui_prefs(|p| p.reduced_motion = on);
                            crate::theme::set_reduced_motion(this.reduced_motion, cx);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        ];
        let one = |v: Vec<AnyElement>| -> Option<AnyElement> {
            Some(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap_1()
                    .children(v)
                    .into_any_element(),
            )
        };
        let look_rows = vec![
            Self::row(
                "Theme",
                "Dark, light, or follow the system.",
                one(themes),
                p,
            ),
            Self::row(
                "Accent",
                "The one colour for selection, focus, and what is live.",
                Some(
                    div()
                        .flex()
                        .flex_wrap()
                        .justify_end()
                        .gap_1()
                        .max_w(px(360.))
                        .children(accents)
                        .into_any_element(),
                ),
                p,
            ),
        ];
        let type_rows = vec![
            Self::row("Font", "The interface typeface.", one(fonts), p),
            Self::row("Text size", "Scales text across the app.", one(sizes), p),
            Self::row(
                "Density",
                "How much space rows and lists get.",
                one(densities),
                p,
            ),
        ];
        let motion_rows = vec![Self::row(
            "Reduce motion",
            "No slides, wipes, or flapping; scrollbars show and hide at once. Also follows your system setting.",
            one(motion),
            p,
        )];
        self.page(
            Section::Appearance,
            p,
            None,
            vec![
                Self::settings_card("Look", p, look_rows),
                Self::settings_card("Text", p, type_rows),
                Self::settings_card("Motion", p, motion_rows),
            ],
        )
    }
}
