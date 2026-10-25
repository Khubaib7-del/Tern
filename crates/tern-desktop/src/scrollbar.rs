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

/// How quickly a wheel notch glides to where it lands: the time constant
/// of an exponential approach. About five of them (~0.4 s) and it is
/// there; most of the way in the first 100 ms, so it feels immediate.
pub const GLIDE_TAU: f32 = 0.075;

/// Smooth wheel scrolling for one scroll area. A mouse wheel moves in
/// notches (GPUI reports lines: three lines, about 60 px, at once), which
/// drew as a jump per notch; here each notch moves the target and the
/// drawn offset glides to it, notches in a row adding up. Touchpads and
/// precise wheels already report pixels in small steps: those land at
/// once, so the content stays under the fingers. Offsets use GPUI's sign
/// (0 at the top, `-max` at the bottom).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Glide {
    pub current: f32,
    pub target: f32,
    last: Option<std::time::Instant>,
}

impl Glide {
    pub fn moving(&self) -> bool {
        (self.target - self.current).abs() > 0.01
    }

    /// A wheel turn of `dy` px with the area at `offset` and `max` px of
    /// scroll. Returns whether it moves the target (false at an end, so
    /// the turn can go on to the area around it). `instant` lands at once
    /// (precise deltas, reduced motion).
    pub fn push(
        &mut self,
        offset: f32,
        dy: f32,
        max: f32,
        instant: bool,
        now: std::time::Instant,
    ) -> bool {
        if !(offset.is_finite() && dy.is_finite() && max.is_finite()) {
            return false;
        }
        let max = max.max(0.);
        // Idle, or moved by something else (the thumb, a jump to a row):
        // start from where the area really is.
        if !self.moving() || (offset - self.current).abs() > 1. {
            self.current = offset.clamp(-max, 0.);
            self.target = self.current;
        }
        let next = (self.target + dy).clamp(-max, 0.);
        if (next - self.target).abs() <= 0.01 {
            return false;
        }
        self.target = next;
        if instant {
            self.current = next;
        }
        if self.last.is_none() || !self.moving() {
            self.last = Some(now);
        }
        true
    }

    /// The offset to draw at `now`; call once per frame while
    /// [`Glide::moving`].
    pub fn tick(&mut self, now: std::time::Instant) -> f32 {
        let dt = self
            .last
            .map_or(0.016, |l| now.saturating_duration_since(l).as_secs_f32())
            .min(0.1);
        self.last = Some(now);
        let k = 1. - (-dt / GLIDE_TAU).exp();
        self.current += (self.target - self.current) * k;
        if (self.target - self.current).abs() < 0.5 {
            self.current = self.target;
        }
        self.current
    }
}

/// How long a wheel gesture stays with the area that took it.
pub const LATCH: std::time::Duration = std::time::Duration::from_millis(260);

/// Which scroll area a wheel gesture belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The page around the lists.
    Page,
    /// A list inside the page.
    List,
}

/// Keeps a wheel gesture with the area it started in. Without it, a page
/// scrolling under a still pointer brought a model list under it and the
/// list took the rest of the turn, so the page stopped dead mid-scroll.
/// A gesture that started on the page stays on the page until the wheel
/// rests for [`LATCH`]; a list hands on to the page at its ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WheelLatch {
    owner: Option<(Owner, std::time::Instant)>,
}

impl WheelLatch {
    /// Whether the page owns the gesture going on at `now`.
    pub fn page_owns(&self, now: std::time::Instant) -> bool {
        matches!(self.owner, Some((Owner::Page, at)) if now.saturating_duration_since(at) < LATCH)
    }

    /// `who` handled a turn at `now`.
    pub fn took(&mut self, who: Owner, now: std::time::Instant) {
        self.owner = Some((who, now));
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

    #[test]
    fn a_wheel_notch_glides_instead_of_jumping() {
        let t0 = std::time::Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut g = Glide::default();
        assert!(g.push(0., -60., 1000., false, t0));
        assert!(g.moving());
        assert_eq!(g.target, -60.);
        // Frame by frame it closes in, never past, never backwards.
        let mut prev = 0.;
        let mut frames = 0;
        for f in 1..100 {
            let y = g.tick(ms(f * 16));
            assert!(y <= prev && y >= -60., "{y}");
            prev = y;
            frames += 1;
            if !g.moving() {
                break;
            }
        }
        assert_eq!(g.current, -60.);
        assert!((10..40).contains(&frames), "{frames} frames");
        // The first frame already moves a good part of the way.
        let mut g = Glide::default();
        g.push(0., -60., 1000., false, t0);
        assert!(g.tick(ms(16)) < -10.);
    }

    #[test]
    fn notches_add_up_stop_at_the_ends_and_precise_deltas_land_at_once() {
        let t0 = std::time::Instant::now();
        let mut g = Glide::default();
        g.push(0., -60., 100., false, t0);
        g.push(0., -60., 100., false, t0);
        assert_eq!(g.target, -100., "clamped to the end");
        assert!(!g.push(g.current, -60., 100., false, t0), "nothing left");
        // Touchpad: lands at once.
        let mut g = Glide::default();
        assert!(g.push(-10., -3., 100., true, t0));
        assert_eq!((g.current, g.target), (-13., -13.));
        assert!(!g.moving());
        // Up at the top: nothing to do.
        assert!(!Glide::default().push(0., 40., 100., false, t0));
        // Moved by the thumb meanwhile: starts from the real offset.
        let mut g = Glide::default();
        g.push(0., -60., 1000., false, t0);
        g.push(-500., -60., 1000., false, t0);
        assert_eq!(g.target, -560.);
        assert!(!g.push(f32::NAN, -1., 10., false, t0));
    }

    #[test]
    fn a_gesture_stays_with_the_page_until_the_wheel_rests() {
        let t0 = std::time::Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut l = WheelLatch::default();
        assert!(!l.page_owns(t0));
        l.took(Owner::Page, t0);
        assert!(l.page_owns(ms(100)));
        l.took(Owner::Page, ms(200));
        assert!(l.page_owns(ms(400)));
        assert!(!l.page_owns(ms(200 + 261)));
        l.took(Owner::List, ms(1000));
        assert!(!l.page_owns(ms(1001)));
    }
}
