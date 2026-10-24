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
use crate::theme::Palette;
use crate::toast::{Level, Retry};

/// The stack's width.
const TOAST_W: f32 = 340.;

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
        let items = shown.iter().map(|t| {
            let id = t.id;
            let f = crate::motion::ease(t.presence(now, reduced));
            let error = t.level == Level::Error;
            let retry = t.retry.clone();
            div()
                .id(("toast", id as usize))
                .w(px(TOAST_W))
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
                .opacity(f)
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
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(p.text)
                        .child(SharedString::from(t.text.clone())),
                )
                .children(retry.map(|r| {
                    Button::new(("toast-retry", id as usize))
                        .label("Retry")
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.model.toasts.dismiss(id, Instant::now());
                            this.retry(r.clone(), window, cx);
                            cx.notify();
                        }))
                }))
                .child(
                    Button::new(("toast-dismiss", id as usize))
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
                .bottom(px(96.))
                .right(px(20.))
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
                        .update(cx, |c, cx| c.set_value(text, window, cx));
                    self.submit(window, cx);
                }
            }
        }
    }
}
