//! Stick-to-bottom for the chat (#90).
//!
//! While an answer streams, the chat follows its end only when the user is
//! already there. Scrolling up (by any amount) lets go, so earlier answers
//! can be read in peace while the run goes on; coming back within
//! [`THRESHOLD`] of the end, or pressing "Jump to latest", takes it again.
//! Text arriving never moves a chat the user scrolled away from.
//!
//! Offsets are in GPUI's sign: 0 at the top, `-max` at the bottom.
//! GPUI-free so it is unit tested.

/// How close to the end, px, still counts as at the end.
pub const THRESHOLD: f32 = 48.;

/// Whether the chat follows its end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Follow {
    on: bool,
    /// The offset seen last time, to tell a move up by the user from
    /// content growing under a still view.
    last: Option<f32>,
}

impl Default for Follow {
    fn default() -> Self {
        Self {
            on: true,
            last: None,
        }
    }
}

/// Whether `offset_y` is within [`THRESHOLD`] of the end of a page that
/// can scroll `max_y` (positive) at most.
pub fn at_bottom(offset_y: f32, max_y: f32) -> bool {
    max_y <= THRESHOLD || -offset_y >= max_y - THRESHOLD
}

impl Follow {
    /// Whether new text should bring the end into view.
    pub fn on(&self) -> bool {
        self.on
    }

    /// Follow again: the user sent a message, opened a chat, or pressed
    /// "Jump to latest".
    pub fn engage(&mut self) {
        self.on = true;
        self.last = None;
    }

    /// Reads where the chat is now (each frame). A move up lets go; being
    /// at the end takes hold again.
    pub fn observe(&mut self, offset_y: f32, max_y: f32) {
        if let Some(last) = self.last
            && offset_y > last + 0.5
        {
            self.on = false;
        }
        if at_bottom(offset_y, max_y) {
            self.on = true;
        }
        self.last = Some(offset_y);
    }

    /// Whether "Jump to latest" shows: not following, and not at the end.
    pub fn shows_jump(&self, offset_y: f32, max_y: f32) -> bool {
        !self.on && !at_bottom(offset_y, max_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_up_lets_go_and_the_end_takes_hold_again() {
        let mut f = Follow::default();
        // At the end of a 2000 px page.
        f.observe(-2000., 2000.);
        assert!(f.on());
        // Text arrives: the page grows, the view has not moved yet.
        f.observe(-2000., 2300.);
        assert!(f.on(), "growth alone never lets go");
        // The user scrolls up a notch while it streams.
        f.observe(-1900., 2300.);
        assert!(!f.on());
        assert!(f.shows_jump(-1900., 2300.));
        // More text: still let go.
        f.observe(-1900., 2600.);
        assert!(!f.on());
        // Back down to within the threshold of the end: following again.
        f.observe(-2570., 2600.);
        assert!(f.on());
        assert!(!f.shows_jump(-2570., 2600.));
        // "Jump to latest".
        f.observe(-100., 2600.);
        assert!(!f.on());
        f.engage();
        assert!(f.on());
        // A short page is always at its end.
        assert!(at_bottom(0., 20.));
    }
}
