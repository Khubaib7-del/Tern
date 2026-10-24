//! The message timeline rail at the right edge of the chat: one tick per
//! question; hover for its start and time, click to glide to it; ticks of
//! questions on screen are lit. Ticks come from `crate::timeline`.

use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, Window, div, px};

use super::Workspace;
use crate::model::Entry;
use crate::theme::Palette;
use crate::timeline::{self, Question};

/// How long the glide to a question takes.
const GLIDE: Duration = Duration::from_millis(320);
/// The tallest the rail gets (px).
const RAIL_MAX_H: f32 = 420.;

impl Workspace {
    /// The rail, or nothing below two questions.
    pub(super) fn timeline_rail(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let now = tern_agent::checkpoint::now_rfc3339();
        let offset = tern_agent::checkpoint::local_offset();
        let questions: Vec<Question<'_>> = self
            .model
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match e {
                Entry::User { text, .. } => Some(Question {
                    entry: i,
                    text: text.as_str(),
                    when: self.model.stamp(i, &now, offset),
                }),
                _ => None,
            })
            .collect();
        if questions.len() < timeline::MIN_TICKS {
            return None;
        }
        let base = self.rail_base.get();
        let on_screen = (self.scroll.children_count() > base).then(|| {
            (
                self.scroll.top_item().saturating_sub(base),
                self.scroll.bottom_item().saturating_sub(base),
            )
        });
        let ticks = timeline::ticks(&questions, on_screen);
        let gap = timeline::gap(ticks.len(), RAIL_MAX_H);
        let hovered = self.rail_hover;
        let col =
            div()
                .flex()
                .flex_col()
                .items_end()
                .children(ticks.into_iter().map(|t| {
                    let ordinal = t.ordinal;
                    let entry = t.entry;
                    let on = hovered == Some(ordinal);
                    let (w, color) = match (on, t.lit) {
                        (true, _) => (18., p.text),
                        (false, true) => (12., p.text.opacity(0.85)),
                        (false, false) => (8., p.faint),
                    };
                    div()
                        .id(("rail-tick", ordinal))
                        .relative()
                        .h(px(gap))
                        .w(px(24.))
                        .flex()
                        .items_center()
                        .justify_end()
                        .cursor_pointer()
                        .child(div().h(px(2.)).w(px(w)).rounded_full().bg(color))
                        .when(on, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .right(px(28.))
                                    .top(px(gap / 2. - 22.))
                                    .w(px(260.))
                                    .p_2()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(p.border)
                                    .bg(p.raised)
                                    .shadow_md()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(p.text)
                                            .child(SharedString::from(t.preview.clone())),
                                    )
                                    .children(t.when.clone().map(|w| {
                                        div().text_xs().text_color(p.faint).child(
                                            SharedString::from(format!("Q{} · {w}", ordinal + 1)),
                                        )
                                    })),
                            )
                        })
                        .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                            if *over {
                                this.rail_hover = Some(ordinal);
                            } else if this.rail_hover == Some(ordinal) {
                                this.rail_hover = None;
                            }
                            cx.notify();
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.glide_to_entry(entry, window);
                            cx.notify();
                        }))
                }));
        Some(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(6.))
                .flex()
                .items_center()
                .child(col)
                .into_any_element(),
        )
    }

    /// Starts a smooth scroll that puts entry `entry` near the top.
    fn glide_to_entry(&mut self, entry: usize, window: &mut Window) {
        let child = self.rail_base.get() + entry;
        let Some(item) = self.scroll.bounds_for_item(child) else {
            self.scroll.scroll_to_top_of_item(child);
            return;
        };
        let view = self.scroll.bounds();
        let to = timeline::scroll_target(
            f32::from(view.top()),
            f32::from(item.top()),
            f32::from(self.scroll.max_offset().y),
            16.,
        );
        let from = f32::from(self.scroll.offset().y);
        if self.reduced_motion {
            self.set_scroll_y(to);
            return;
        }
        self.rail_scroll = Some((from, to, Instant::now()));
        window.request_animation_frame();
    }

    fn set_scroll_y(&self, y: f32) {
        let x = self.scroll.offset().x;
        self.scroll.set_offset(gpui_kit::point(x, px(y)));
    }

    /// Moves a glide on; draws the next frame only while it runs.
    pub(super) fn tick_rail_scroll(&mut self, window: &mut Window) {
        let Some((from, to, at)) = self.rail_scroll else {
            return;
        };
        let t = at.elapsed().as_secs_f32() / GLIDE.as_secs_f32();
        self.set_scroll_y(timeline::ease_scroll(from, to, t));
        if t >= 1.0 {
            self.rail_scroll = None;
        } else {
            window.request_animation_frame();
        }
    }
}
