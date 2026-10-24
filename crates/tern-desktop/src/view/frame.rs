//! Asking GPUI for another frame, safely from anywhere.
//!
//! `Window::request_animation_frame` reads `Window::current_view`, which
//! unwraps the top of GPUI's rendered-view stack. That stack holds a view
//! only while GPUI is rendering, prepainting or painting it. In a click,
//! wheel, pinch or key listener, a timer, an action or a spawned task the
//! stack is empty, so the call panics (`window.rs:5353`, `Option::unwrap()`
//! on a `None` value). On Windows that listener runs inside the window
//! procedure, an `extern "system"` function that cannot unwind, so the
//! panic became "panic in a function that cannot unwind" and Tern ended
//! with 0xC0000409: Explain in 3D's Next, Play, zoom and Reset.
//!
//! [`next_frame`] does what `request_animation_frame` does (notify this
//! view once the next frame is drawn) without asking GPUI which view is
//! rendering: the view comes from its own `Context`. It is correct inside
//! a render too, so Tern uses it everywhere: no Tern code calls
//! `request_animation_frame` or `current_view`, not even in a render or a
//! paint callback where they would work, so a listener can never pick the
//! habit up again (`tern-core/tests/dependency_rules.rs` keeps it that
//! way; this file's tests are the one exception, to show the crash).

use gpui_kit::{Context, EntityId, Window};

/// Draws this view again on the next frame. Safe in listeners, timers,
/// actions, spawned tasks and render alike.
pub(crate) fn next_frame<V: 'static>(window: &Window, cx: &mut Context<V>) {
    next_frame_for(window, cx.entity_id());
}

/// [`next_frame`] for a closure that has no `Context` (a canvas paint, a
/// popover's content): the view's id is taken in the render that built it.
pub(crate) fn next_frame_for(window: &Window, view: EntityId) {
    window.on_next_frame(move |_, cx| cx.notify(view));
}

#[cfg(test)]
mod tests {
    use super::next_frame;
    use gpui_kit::prelude::*;
    use gpui_kit::{
        Context, Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, TestDispatcher,
        TouchPhase, Window, div, point, px,
    };
    use std::panic::{AssertUnwindSafe, catch_unwind};

    /// Stands in for Explain in 3D: a button-sized surface whose click,
    /// mouse-down and wheel listeners step an animation and ask for the
    /// next frame, the way Next, Play, Reset and wheel-zoom do.
    struct Stage {
        renders: usize,
        steps: usize,
        /// Frames the animation still wants; render asks for the next one
        /// while it is above zero (Play).
        playing: usize,
    }

    impl Render for Stage {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.renders += 1;
            if self.playing > 0 {
                self.playing -= 1;
                next_frame(window, cx);
            }
            div()
                .id("stage")
                .size_full()
                .on_click(cx.listener(|this, _, window, cx| {
                    // Next.
                    this.steps += 1;
                    next_frame(window, cx);
                }))
                .on_mouse_down(
                    gpui_kit::MouseButton::Right,
                    cx.listener(|this, _, window, cx| {
                        // Play: a few frames of motion.
                        this.playing = 3;
                        next_frame(window, cx);
                    }),
                )
                .on_scroll_wheel(cx.listener(|this, _, window, cx| {
                    // Wheel-zoom.
                    this.steps += 10;
                    next_frame(window, cx);
                }))
                .child("stage")
        }
    }

    fn app() -> TestAppContext {
        TestAppContext::build(TestDispatcher::new(7), None)
    }

    /// Delivers the next frame the way the platform would: runs the
    /// next-frame callbacks (how many ran comes back), then draws.
    fn frame(cx: &mut gpui_kit::VisualTestContext) -> usize {
        let ran = cx.update(|window, cx| {
            let ran = window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
            ran
        });
        cx.run_until_parked();
        ran
    }

    #[test]
    fn next_click_wheel_and_play_ask_for_frames_without_a_panic() {
        let mut cx = app();
        let (stage, cx) = cx.add_window_view(|_, _| Stage {
            renders: 0,
            steps: 0,
            playing: 0,
        });
        let at = point(px(40.), px(40.));
        let renders = stage.read_with(cx, |s, _| s.renders);
        assert!(renders >= 1);

        // Next: the listener asked for a frame; delivering it re-renders.
        cx.simulate_click(at, Modifiers::default());
        assert_eq!(stage.read_with(cx, |s, _| s.steps), 1);
        assert_eq!(frame(cx), 1);
        assert!(stage.read_with(cx, |s, _| s.renders) > renders);

        // Wheel-zoom.
        cx.simulate_event(ScrollWheelEvent {
            position: at,
            delta: ScrollDelta::Lines(point(0., 1.)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
        assert_eq!(stage.read_with(cx, |s, _| s.steps), 11);
        assert_eq!(frame(cx), 1);

        // Play: render keeps asking while frames remain, then stops.
        cx.simulate_mouse_down(at, gpui_kit::MouseButton::Right, Modifiers::default());
        for _ in 0..8 {
            frame(cx);
        }
        assert_eq!(stage.read_with(cx, |s, _| s.playing), 0);
        assert_eq!(frame(cx), 0, "a finished animation asks for no more frames");
    }

    #[test]
    fn request_animation_frame_outside_a_render_is_the_crash() {
        // What every listener did before: no view is rendering, so GPUI's
        // `current_view` unwraps nothing. Here it unwinds; inside the
        // Windows window procedure it aborted the app.
        let mut cx = app();
        let (_stage, vcx) = cx.add_window_view(|_, _| Stage {
            renders: 0,
            steps: 0,
            playing: 0,
        });
        let crashed = catch_unwind(AssertUnwindSafe(|| {
            vcx.update(|window, _| window.request_animation_frame());
        }));
        assert!(crashed.is_err());
        // The app's state is mid-update after the unwind; leave it be.
        std::mem::forget(cx);
    }
}
