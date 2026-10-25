//! The flashcard stack's swipe (v2 s14): drag the top card sideways,
//! flick it, or let it go and it springs back. Before the answer shows
//! the card only rubber-bands (it resists, then snaps home); once it is
//! turned, a swipe right answers Good and a swipe left Again. GPUI-free so
//! the feel is unit tested; the view is `view/cards_view.rs`.

use std::time::Instant;

use crate::motion::{Spring, SpringSpec};

/// How far a card must travel to count as a swipe, in px.
pub const THRESHOLD: f32 = 110.;
/// How fast a short flick must be to count, in px/s.
pub const FLICK: f32 = 900.;
/// The rubber band's reach: an unarmed card never goes further than this.
pub const BAND: f32 = 48.;

/// Which way a card left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
}

/// A rubber band: follows the pointer at first, then resists more and
/// more, never past [`BAND`].
pub fn rubber(dx: f32) -> f32 {
    let a = dx.abs();
    dx.signum() * BAND * (1. - 1. / (a / BAND + 1.))
}

#[derive(Clone, Debug)]
pub struct Swipe {
    /// The press point and the latest point, while a pointer is down.
    start: Option<f32>,
    last: Option<(f32, Instant)>,
    dx: f32,
    vel: f32,
    /// Set once the pointer moved past a click's slop; stays until the
    /// next press, so the click that ends a drag does not turn the card.
    dragged: bool,
    /// The card going home after a release.
    home: Spring,
}

impl Default for Swipe {
    fn default() -> Self {
        Self {
            start: None,
            last: None,
            dx: 0.,
            vel: 0.,
            dragged: false,
            home: Spring::new(0., SpringSpec::UI),
        }
    }
}

impl Swipe {
    pub fn press(&mut self, x: f32, now: Instant) {
        let at = self.home.sample(now);
        // Catch a card that is still going home where it is.
        self.start = Some(x - at);
        self.last = Some((x, now));
        self.dx = at;
        self.vel = 0.;
        self.dragged = false;
    }

    pub fn pressed(&self) -> bool {
        self.start.is_some()
    }

    pub fn move_to(&mut self, x: f32, now: Instant) {
        let Some(start) = self.start else { return };
        if let Some((lx, lt)) = self.last {
            let dt = now.saturating_duration_since(lt).as_secs_f32();
            if dt > 0.001 {
                // A light low-pass, so one jittery event is not a flick.
                self.vel = self.vel * 0.3 + (x - lx) / dt * 0.7;
            }
        }
        self.last = Some((x, now));
        self.dx = x - start;
        if self.dx.abs() > 4. {
            self.dragged = true;
        }
    }

    /// The pointer came up. With `armed` (the answer shows) a long drag or
    /// a flick swipes the card away; otherwise it springs home.
    pub fn release(&mut self, armed: bool, now: Instant) -> Option<Dir> {
        self.start?;
        self.start = None;
        self.last = None;
        let shown = if armed { self.dx } else { rubber(self.dx) };
        let gone = armed
            && (self.dx.abs() > THRESHOLD
                || (self.vel.abs() > FLICK && self.vel.signum() == self.dx.signum()));
        if gone {
            self.home = Spring::new(0., SpringSpec::UI);
            self.dx = 0.;
            return Some(if shown > 0. { Dir::Right } else { Dir::Left });
        }
        let mut s = Spring::new(shown, SpringSpec::UI);
        s.retarget(0., now);
        self.home = s;
        self.dx = 0.;
        None
    }

    /// The card's sideways offset now, in px.
    pub fn offset(&mut self, armed: bool, now: Instant) -> f32 {
        if self.start.is_some() {
            if armed { self.dx } else { rubber(self.dx) }
        } else {
            self.home.sample(now)
        }
    }

    /// Whether the last press became a drag (so its click is not a tap).
    pub fn dragged(&self) -> bool {
        self.dragged
    }

    /// Whether frames are still needed (dragging or going home).
    pub fn moving(&self) -> bool {
        self.start.is_some() || self.home.moving()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_band_resists_and_never_passes_its_reach() {
        assert_eq!(rubber(0.), 0.);
        assert!(rubber(20.) < 20. && rubber(20.) > 10.);
        assert!(rubber(10_000.) < BAND);
        assert_eq!(rubber(-30.), -rubber(30.));
    }

    #[test]
    fn a_long_drag_swipes_once_the_answer_shows() {
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        let mut s = Swipe::default();
        s.press(500., t);
        for i in 1..=10 {
            s.move_to(500. + i as f32 * 15., ms(i * 40));
        }
        assert!(s.dragged());
        assert_eq!(s.release(true, ms(420)), Some(Dir::Right));
        // Unarmed: the same drag only rubber-bands and goes home.
        s.press(500., ms(1000));
        for i in 1..=10 {
            s.move_to(500. - i as f32 * 15., ms(1000 + i * 40));
        }
        assert!(s.offset(false, ms(1400)).abs() < BAND);
        assert_eq!(s.release(false, ms(1420)), None);
        assert!(s.moving());
        assert!(s.offset(false, ms(1460)) < 0.);
        assert!(s.offset(false, ms(3000)).abs() < 0.5, "home");
    }

    #[test]
    fn a_quick_flick_counts_and_a_tap_does_not() {
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        let mut s = Swipe::default();
        s.press(300., t);
        s.move_to(330., ms(16));
        s.move_to(370., ms(32));
        assert_eq!(s.release(true, ms(40)), Some(Dir::Right));
        s.press(300., ms(500));
        s.move_to(302., ms(520));
        assert!(!s.dragged());
        assert_eq!(s.release(true, ms(560)), None);
        // A short slow drag left springs home.
        s.press(300., ms(1000));
        s.move_to(260., ms(1400));
        assert_eq!(s.release(true, ms(1420)), None);
    }
}
