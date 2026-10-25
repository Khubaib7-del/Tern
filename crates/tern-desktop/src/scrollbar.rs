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

/// Where a wheel turn of `dy` px takes an area at `offset` with `max` px
/// of scroll (GPUI's sign: 0 at the top, `-max` at the bottom), or `None`
/// when it is already at the end the turn pushes toward. The area moves at
/// once by the whole turn, as GPUI's own scroll areas (the chat) do.
pub fn step(offset: f32, dy: f32, max: f32) -> Option<f32> {
    if !(offset.is_finite() && dy.is_finite() && max.is_finite()) {
        return None;
    }
    let max = max.max(0.);
    let from = offset.clamp(-max, 0.);
    let to = (from + dy).clamp(-max, 0.);
    ((to - from).abs() > 0.01).then_some(to)
}

/// How long a wheel gesture stays with the area that took it: the gap
/// between turns that ends a gesture. A mouse wheel spun on sends turns
/// well inside it; a pause of a quarter second starts a new gesture.
pub const LATCH: std::time::Duration = std::time::Duration::from_millis(260);

/// Which scroll area a wheel gesture belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The page around the lists.
    Page,
    /// The list inside the page with this id.
    List(&'static str),
}

/// What a wheel turn over a list does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// The list scrolls.
    List,
    /// The list has the gesture but is at its end: nothing moves.
    Hold,
    /// The page scrolls.
    Page,
}

/// Keeps a wheel gesture with the area it started in, as browsers do
/// ("scroll latching"). A page scrolling under a still pointer does not
/// hand the rest of the spin to a list it slides under the pointer, and a
/// list spun to its end does not jerk the page; the next gesture (after
/// the wheel rests for [`LATCH`]) at the list's end scrolls the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WheelLatch {
    owner: Option<(Owner, std::time::Instant)>,
}

impl WheelLatch {
    /// Who owns the gesture going on at `now`, if one is.
    pub fn owner(&self, now: std::time::Instant) -> Option<Owner> {
        self.owner
            .filter(|(_, at)| now.saturating_duration_since(*at) < LATCH)
            .map(|(o, _)| o)
    }

    /// Whether the page owns the gesture going on at `now`.
    pub fn page_owns(&self, now: std::time::Instant) -> bool {
        self.owner(now) == Some(Owner::Page)
    }

    /// The page took a turn at `now`.
    pub fn page_took(&mut self, now: std::time::Instant) {
        self.owner = Some((Owner::Page, now));
    }

    /// A turn at `now` with the pointer over the list `id`, which `can`
    /// move the way the turn goes. Records who has the gesture.
    pub fn over_list(&mut self, id: &'static str, can: bool, now: std::time::Instant) -> Route {
        match self.owner(now) {
            // The page's gesture: the page goes on (the caller records it).
            Some(Owner::Page) => Route::Page,
            // This list's gesture: it keeps it, even at its end.
            Some(Owner::List(o)) if o == id => {
                self.owner = Some((Owner::List(id), now));
                if can { Route::List } else { Route::Hold }
            }
            // A new gesture: the list takes it if it can move, else the page.
            _ if can => {
                self.owner = Some((Owner::List(id), now));
                Route::List
            }
            _ => Route::Page,
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

    #[test]
    fn a_turn_moves_the_whole_way_at_once_and_stops_at_the_ends() {
        // A Windows notch (three lines, about 60 px) lands in one step,
        // as in the chat: no glide, no frames to wait for.
        assert_eq!(step(0., -60., 1000.), Some(-60.));
        assert_eq!(step(-990., -60., 1000.), Some(-1000.), "clamped");
        assert_eq!(step(-1000., -60., 1000.), None, "nothing left");
        assert_eq!(step(0., 40., 1000.), None, "already at the top");
        assert_eq!(step(-500., 3., 1000.), Some(-497.), "touchpad pixels");
        assert_eq!(step(0., -60., -5.), None, "nothing to scroll");
        assert_eq!(step(f32::NAN, -1., 10.), None);
        // An offset past the end (content shrank) counts from the end.
        assert_eq!(step(-1200., 30., 1000.), Some(-970.));
    }

    #[test]
    fn a_gesture_stays_with_the_page_until_the_wheel_rests() {
        let t0 = std::time::Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut l = WheelLatch::default();
        assert!(!l.page_owns(t0));
        l.page_took(t0);
        assert!(l.page_owns(ms(100)));
        // The page slid a list under the pointer mid-spin: the page goes on.
        assert_eq!(l.over_list("models", true, ms(150)), Route::Page);
        l.page_took(ms(200));
        assert!(l.page_owns(ms(400)));
        assert!(!l.page_owns(ms(200 + 261)));
        // The wheel rested: the list under the pointer takes the next turn.
        assert_eq!(l.over_list("models", true, ms(500)), Route::List);
        assert!(!l.page_owns(ms(501)));
    }

    #[test]
    fn a_list_keeps_its_gesture_to_its_end_and_the_next_one_goes_to_the_page() {
        let t0 = std::time::Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut l = WheelLatch::default();
        assert_eq!(l.over_list("models", true, t0), Route::List);
        assert_eq!(l.over_list("models", true, ms(40)), Route::List);
        // Spun to its end mid-gesture: the page does not jerk.
        assert_eq!(l.over_list("models", false, ms(80)), Route::Hold);
        assert_eq!(l.over_list("models", false, ms(300)), Route::Hold);
        // A new gesture at the end: the page scrolls.
        assert_eq!(l.over_list("models", false, ms(300 + 261)), Route::Page);
        // Never trapped: a list that cannot move never holds a new gesture.
        let mut l = WheelLatch::default();
        assert_eq!(l.over_list("roles", false, t0), Route::Page);
        // Another list under the pointer starts its own gesture.
        let mut l = WheelLatch::default();
        l.over_list("models", true, t0);
        assert_eq!(l.over_list("roles", true, ms(10)), Route::List);
        assert_eq!(l.owner(ms(10)), Some(Owner::List("roles")));
    }
}
