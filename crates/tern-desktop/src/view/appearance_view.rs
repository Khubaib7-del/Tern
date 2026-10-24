//! Appearance: only what you can change. Theme, accent, font, text size,
//! density, and motion; each applies at once and is saved. Scout is drawn
//! here too, live, with its moods to preview (v2 s16 shows the companion
//! beside the look settings); picking a mood is a preview, not a setting.

use gpui_kit::component::Sizable as _;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, div, px};

use super::Workspace;
use crate::appearance::{Accent, Choice, FontChoice, TextSize, ThemeChoice};
use crate::settings::Section;
use crate::theme::Palette;

/// The moods Appearance can preview, in v2's order.
const SCOUT_MOODS: [crate::scout::Mood; 8] = [
    crate::scout::Mood::Idle,
    crate::scout::Mood::Thinking,
    crate::scout::Mood::Searching,
    crate::scout::Mood::Reading,
    crate::scout::Mood::Verifying,
    crate::scout::Mood::NeedsYou,
    crate::scout::Mood::Blocked,
    crate::scout::Mood::Studying,
];

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
        // Scout, live: tap a mood to watch it move into that pose.
        let current = self.scout.mood();
        let moods: Vec<AnyElement> = SCOUT_MOODS
            .iter()
            .map(|&m| {
                let on = current == m;
                div()
                    .id(SharedString::from(format!("look-scout-{}", m.label())))
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_xs()
                    .border_1()
                    .border_color(if on { p.muted } else { p.border })
                    .text_color(if on { p.text } else { p.muted })
                    .when(on, |d| d.bg(p.hover))
                    .hover(move |d| d.bg(p.hover))
                    .child(m.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.scout_preview = Some(m);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let scout_rows = vec![
            div()
                .flex()
                .items_center()
                .gap_4()
                .py_2()
                .child(
                    div()
                        .flex_none()
                        .rounded_lg()
                        .bg(p.sidebar)
                        .p_2()
                        .child(self.scout_figure(crate::scout::CORNER, p)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .text_color(p.muted)
                                .child("Scout shows what Tern is doing from the sidebar's corner. Pick a mood to preview it."),
                        )
                        .child(div().flex().flex_wrap().gap_1().children(moods)),
                )
                .into_any_element(),
        ];
        self.page(
            Section::Appearance,
            p,
            None,
            vec![
                Self::settings_card("Look", p, look_rows),
                Self::settings_card("Scout", p, scout_rows),
                Self::settings_card("Text", p, type_rows),
                Self::settings_card("Motion", p, motion_rows),
            ],
        )
    }
}
