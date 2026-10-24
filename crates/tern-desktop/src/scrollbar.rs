//! Tern's scrollbar: thin, overlaid on the content, hidden until the area
//! scrolls (or the pointer is on its track while it shows), a rounded
//! thumb that widens under the pointer, in the theme's own greys, and
//! draggable. Every scroll area in the app uses it (the chat, the sidebar,
//! pickers, Settings, panels); `theme::apply_kit` installs it.
//!
//! The thumb's geometry and dragging come from the kit's Base scrollbar
//! (gpui-base); this module is the look and timing, GPUI-free so it is
//! unit tested.

use std::time::Duration;

/// The thumb at rest, while the area scrolls.
pub const IDLE_W: f32 = 4.;
/// The thumb under the pointer or being dragged.
pub const HOVER_W: f32 = 8.;
/// Space between the thumb and the area's edge.
pub const INSET: f32 = 2.;
/// The shortest the thumb gets on a very long list, so it stays grabbable.
pub const MIN_THUMB: f32 = 28.;

/// The thumb's state, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thumb {
    Idle,
    Hover,
    Drag,
}

/// How strong the thumb is over the canvas: a share of the text colour,
/// so it reads on both themes without a hard white or grey block. Light
/// needs a little more to show on near-white.
pub fn thumb_alpha(dark: bool, t: Thumb) -> f32 {
    match (dark, t) {
        (true, Thumb::Idle) => 0.20,
        (true, Thumb::Hover) => 0.34,
        (true, Thumb::Drag) => 0.46,
        (false, Thumb::Idle) => 0.24,
        (false, Thumb::Hover) => 0.38,
        (false, Thumb::Drag) => 0.50,
    }
}

/// Fully round at either width.
pub fn radius(width: f32) -> f32 {
    width / 2.
}

/// The scrollbar's timing: how long it stays after the last scroll, how
/// long it takes to fade in and out, and to widen. All zero under reduced
/// motion, so it shows and hides at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub idle: Duration,
    pub enter: Duration,
    pub exit: Duration,
    pub expand: Duration,
}

pub fn timing(reduced_motion: bool) -> Timing {
    if reduced_motion {
        Timing {
            // It still has to go away after a while.
            idle: Duration::from_millis(1200),
            enter: Duration::ZERO,
            exit: Duration::ZERO,
            expand: Duration::ZERO,
        }
    } else {
        Timing {
            idle: Duration::from_millis(1200),
            enter: Duration::from_millis(140),
            exit: Duration::from_millis(380),
            expand: Duration::from_millis(160),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thin_at_rest_wider_under_the_pointer_and_round() {
        assert_eq!(IDLE_W, 4.);
        assert_eq!(HOVER_W, 8.);
        assert_eq!(radius(IDLE_W), 2.);
        assert_eq!(radius(HOVER_W), 4.);
        const { assert!(MIN_THUMB > HOVER_W * 2.) };
    }

    #[test]
    fn the_thumb_gets_stronger_as_you_reach_for_it() {
        for dark in [true, false] {
            let i = thumb_alpha(dark, Thumb::Idle);
            let h = thumb_alpha(dark, Thumb::Hover);
            let d = thumb_alpha(dark, Thumb::Drag);
            assert!(i < h && h < d, "{dark}");
            // Never a solid block.
            assert!(d <= 0.5 && i >= 0.15);
        }
    }

    #[test]
    fn reduced_motion_shows_and_hides_at_once() {
        let r = timing(true);
        assert_eq!(r.enter, Duration::ZERO);
        assert_eq!(r.exit, Duration::ZERO);
        assert_eq!(r.expand, Duration::ZERO);
        let t = timing(false);
        assert!(t.enter < t.exit && t.idle > t.exit);
        assert_eq!(r.idle, t.idle);
    }
}
