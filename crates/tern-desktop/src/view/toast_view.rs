//! Toasts over the window, bottom right: Tern's short notes (a model
//! switch, a saved voice model, an error) instead of lines in the chat.
//! State and timing live in `crate::toast`; this draws them and wakes the
//! window only when one comes, goes, or fades.

use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{IconName, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, SharedString, Window, div, px};

use super::Workspace;
use super::press::Capsule as _;
use crate::theme::Palette;
use crate::toast::{Level, Retry};

impl Workspace {
    /// Advances the toasts and, while one is fading or counting down,
    /// schedules the next look at them (a timer, not every frame).
    pub(super) fn tick_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.toasts.is_empty() {
            return;
        }
        let now = Instant::now();
        self.model.toasts.tick(now, self.reduced_motion);
        let Some(wait) = self.model.toasts.next_change(now) else {
            return;
        };
        if wait <= Duration::from_millis(16) {
            super::frame::next_frame(window, cx);
        } else if self.toast_wake.is_none() {
            self.toast_wake = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(wait + Duration::from_millis(10))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.toast_wake = None;
                    cx.notify();
                });
            }));
        }
    }

    /// The stack, newest at the bottom.
    pub(super) fn toasts(&self, p: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let shown = self.model.toasts.shown();
        if shown.is_empty() {
            return None;
        }
        let now = Instant::now();
        let reduced = self.reduced_motion;
        let red = gpui_kit::hsla(
            4. / 360.,
            0.72,
            if self.model.dark { 0.62 } else { 0.46 },
            1.,
        );
        // Each toast rides a spring in and out (`motion::Presence`).
        let mut springs = self.toast_springs.borrow_mut();
        springs.retain(|id, _| shown.iter().any(|t| t.id == *id));
        let mut moving = false;
        let fs: Vec<f32> = shown
            .iter()
            .map(|t| {
                let pr = springs.entry(t.id).or_insert_with(|| {
                    crate::motion::Presence::new(true, true, crate::motion::SpringSpec::UI)
                });
                let f = pr.frame(t.leaving.is_none(), now, reduced).unwrap_or(0.);
                moving |= pr.moving();
                f
            })
            .collect();
        drop(springs);
        if moving {
            self.anim_pending.set(true);
        }
        // Never over the message column: in the free gutter to its right,
        // else narrowed at the chat area's bottom right above the composer.
        let composer_top = self
            .measured
            .borrow()
            .get(&("composer", 0))
            .filter(|_| self.model.screen == crate::model::Screen::Chat)
            .map(|b| f32::from(b.origin.y));
        let at = crate::toast::place(
            self.window_w,
            self.window_h,
            self.centre_x,
            self.centre_w,
            super::COLUMN_MAX_W,
            composer_top,
        );
        let width = at.width;
        let items = shown.iter().zip(fs).map(|(t, f)| {
            let id = t.id;
            let f = f.clamp(0., 1.1);
            let error = t.level == Level::Error;
            let retry = t.retry.clone();
            div()
                .id(("toast", id as usize))
                .w(px(width))
                .flex()
                .items_start()
                .gap_2()
                .pl_3()
                .pr_1()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(if error { red.opacity(0.6) } else { p.border })
                .bg(p.raised)
                .shadow_lg()
                .opacity(f.min(1.))
                // Slides up 8 px as it comes in, down as it goes.
                .mt(px((1. - f) * 8.))
                .occlude()
                .child(
                    div()
                        .mt(px(6.))
                        .size(px(6.))
                        .flex_none()
                        .rounded_full()
                        .bg(if error { red } else { p.accent }),
                )
                // The text takes the whole width; its action (Retry, Open…)
                // sits under it, so a long label never squeezes the text
                // into one word per line.
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_1()
                        .child(
                            div()
                                .w_full()
                                .text_sm()
                                .text_color(p.text)
                                .child(SharedString::from(t.text.clone())),
                        )
                        .children(retry.map(|r| {
                            Button::new(("toast-retry", id as usize))
                                .capsule()
                                .label(r.label())
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.model.toasts.dismiss(id, Instant::now());
                                    this.retry(r.clone(), window, cx);
                                    cx.notify();
                                }))
                        })),
                )
                .child(
                    Button::new(("toast-dismiss", id as usize))
                        .capsule()
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .tooltip("Dismiss")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.toasts.dismiss(id, Instant::now());
                            cx.notify();
                        })),
                )
        });
        Some(
            div()
                .absolute()
                .bottom(px(at.bottom))
                .right(px(at.right))
                .flex()
                .flex_col()
                .items_end()
                .gap_2()
                .children(items)
                .into_any_element(),
        )
    }

    /// A toast's Retry.
    fn retry(&mut self, what: Retry, window: &mut Window, cx: &mut Context<Self>) {
        match what {
            Retry::Models(id) => {
                self.model.providers.forget_models(id);
                self.model.providers.models_loading(id);
                self.bridge.models(id, true);
            }
            Retry::Connect => match self.resume_retry.filter(|_| self.session.is_none()) {
                Some(id) => {
                    self.model.phase = crate::model::Phase::Connecting;
                    self.bridge.resume(id);
                }
                None if self.session.is_none() => {
                    self.model.phase = crate::model::Phase::Connecting;
                    self.bridge.connect();
                }
                None => {}
            },
            Retry::LocalModels => {
                self.open_settings(crate::settings::Section::LocalModels, window, cx);
            }
            Retry::Resend => {
                if self.model.is_busy() {
                    return;
                }
                let last = self.model.entries.iter().rev().find_map(|e| match e {
                    crate::model::Entry::User { text, .. } => Some(text.clone()),
                    _ => None,
                });
                if let Some(text) = last {
                    self.composer
                        .update(cx, |c, cx| crate::view::set_end(c, text, window, cx));
                    self.submit(window, cx);
                }
            }
        }
    }
}
