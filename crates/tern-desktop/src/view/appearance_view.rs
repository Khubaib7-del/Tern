//! Appearance: only what you can change. Theme, accent, font, text size,
//! density, and motion; each applies at once and is saved. Scout has its
//! own page (Settings › Scout, `scout_page` below).

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
        let run_panel = vec![
            gpui_kit::component::switch::Switch::new("look-run-panel-auto")
                .checked(self.model.run_panel_auto)
                .small()
                .on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.set_run_panel_auto(*on);
                    cx.notify();
                }))
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
        let motion_rows = vec![
            Self::row(
                "Reduce motion",
                "No slides, wipes, or flapping; scrollbars show and hide at once. Also follows your system setting.",
                one(motion),
                p,
            ),
            Self::row(
                "Open run panel automatically",
                "The run panel (plan, sources, report, graph) opens by itself when deep research starts. Off: it stays closed until you open it. A plain question never opens it.",
                one(run_panel),
                p,
            ),
        ];
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

    /// Settings › Scout (like Codex's pets page): a live preview card,
    /// the look (normal or Study trim, or follow the harness), show or
    /// hide, and each state to preview.
    pub(super) fn scout_page(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        use crate::pet::State;
        let m = self.pet_manifest.clone();
        let now = std::time::Instant::now();
        let preview_state = self.pet_preview.unwrap_or(State::Idle);
        let frame = {
            let mut a = self.pet_preview_anim.borrow_mut();
            a.set(preview_state, now);
            let f = a.frame(&m, now, self.reduced_motion);
            if a.next_change(&m, self.reduced_motion)
                .is_some_and(|d| d <= std::time::Duration::from_millis(16))
            {
                self.anim_pending.set(true);
            }
            f
        };
        let variant = self.pet_snapshot().variant;
        let chip = |id: SharedString, label: &'static str, on: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_full()
                .cursor_pointer()
                .text_xs()
                .border_1()
                .border_color(if on { p.accent } else { p.border })
                .text_color(if on { p.text } else { p.muted })
                .when(on, |d| d.bg(p.accent.opacity(0.12)))
                .hover(move |d| d.bg(p.hover))
                .child(label)
        };
        let preview = div()
            .flex()
            .items_end()
            .justify_center()
            .gap_6()
            .py_4()
            .rounded_xl()
            .bg(p.sidebar)
            .child(super::pet_view::figure(
                &m,
                &variant,
                &frame,
                150.,
                self.model.dark,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pb_4()
                    .child(
                        div()
                            .text_lg()
                            .font_family(crate::assets::SERIF_FONT)
                            .italic()
                            .text_color(p.text)
                            .child("Scout"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.muted)
                            .child(SharedString::from(format!(
                                "{} · {}",
                                preview_state.label(),
                                if variant == "study" {
                                    "Study trim"
                                } else {
                                    "Lime trim"
                                }
                            ))),
                    )
                    .when(m.placeholder, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(p.faint)
                                .child("Placeholder art: the animated strips replace it."),
                        )
                    }),
            )
            .into_any_element();
        let states: Vec<AnyElement> = State::ALL
            .iter()
            .map(|&st| {
                chip(
                    SharedString::from(format!("scout-state-{}", st.key())),
                    st.label(),
                    st == preview_state,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pet_preview = Some(st);
                    cx.notify();
                }))
                .into_any_element()
            })
            .collect();
        let looks: Vec<AnyElement> = [
            (None, "Follow Research / Study"),
            (Some("normal"), "Lime trim"),
            (Some("study"), "Study trim"),
        ]
        .into_iter()
        .map(|(v, label)| {
            let on = self.scout_variant.as_deref() == v;
            chip(
                SharedString::from(format!("scout-look-{}", v.unwrap_or("auto"))),
                label,
                on,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_scout_variant(v.map(str::to_owned), cx);
            }))
            .into_any_element()
        })
        .collect();
        let hidden = self.scout_hidden;
        let wrap = |v: Vec<AnyElement>| -> Option<AnyElement> {
            Some(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap_1()
                    .max_w(px(420.))
                    .children(v)
                    .into_any_element(),
            )
        };
        let rows = vec![
            Self::row(
                "Show Scout",
                "Scout floats over the window: drag it anywhere, hover for New chat, Dictate and its menu. Ctrl+. (Cmd+. on macOS) hides or shows it.",
                Some(
                    gpui_kit::component::switch::Switch::new("scout-show")
                        .checked(!hidden)
                        .small()
                        .on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.set_scout_hidden(!*on, cx);
                        }))
                        .into_any_element(),
                ),
                p,
            ),
            Self::row(
                "Look",
                "Scout's trim. Study's rose trim follows the Study harness unless you pick one.",
                wrap(looks),
                p,
            ),
            Self::row(
                "Preview a state",
                "See each state here and on the floating Scout.",
                wrap(states),
                p,
            ),
        ];
        self.page(
            Section::Scout,
            p,
            None,
            vec![preview, Self::settings_card("Scout", p, rows)],
        )
    }
}
