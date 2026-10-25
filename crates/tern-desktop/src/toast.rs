//! Toasts: short notes from Tern (a model switch, a saved choice, an
//! error) shown over the window for a moment instead of as lines in the
//! chat. Info toasts leave on their own after [`INFO_FOR`]; errors stay
//! until dismissed and may offer Retry. Toasts stack, newest at the bottom,
//! at most [`MAX_SHOWN`] at a time; a repeat of a showing toast refreshes
//! it instead of stacking a copy.
//!
//! No GPUI here: the window draws [`Toasts::shown`] and calls
//! [`Toasts::tick`] with the time.

use std::time::{Duration, Instant};

/// How long an info toast stays.
pub const INFO_FOR: Duration = Duration::from_secs(4);
/// How long a toast takes to come in or go out (the house curve).
pub const MOTION: Duration = Duration::from_millis(200);
/// Toasts shown at once; older ones wait their turn.
pub const MAX_SHOWN: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

/// What a toast's Retry button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Retry {
    /// List this provider's models again.
    Models(&'static str),
    /// Open the default model again (a chat that could not connect).
    Connect,
    /// Send the last message again.
    Resend,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub id: u64,
    pub level: Level,
    pub text: String,
    pub retry: Option<Retry>,
    /// When it was shown (or last refreshed).
    pub at: Instant,
    /// When it started leaving, if it is.
    pub leaving: Option<Instant>,
}

impl Toast {
    /// How far in it is (0 hidden, 1 fully shown) at `now`, on the
    /// model's own clock (the view draws it on a spring instead).
    #[cfg(test)]
    pub fn presence(&self, now: Instant, reduced: bool) -> f32 {
        if reduced {
            return if self.leaving.is_some() { 0.0 } else { 1.0 };
        }
        let d = MOTION.as_secs_f32();
        match self.leaving {
            Some(l) => 1.0 - (now.saturating_duration_since(l).as_secs_f32() / d).min(1.0),
            None => (now.saturating_duration_since(self.at).as_secs_f32() / d).min(1.0),
        }
    }
}

#[derive(Debug, Default)]
pub struct Toasts {
    items: Vec<Toast>,
    next: u64,
}

impl Toasts {
    pub fn info(&mut self, text: impl Into<String>, now: Instant) -> u64 {
        self.push(Level::Info, text.into(), None, now)
    }

    pub fn error(&mut self, text: impl Into<String>, retry: Option<Retry>, now: Instant) -> u64 {
        self.push(Level::Error, text.into(), retry, now)
    }

    fn push(&mut self, level: Level, text: String, retry: Option<Retry>, now: Instant) -> u64 {
        let text = text.trim().to_owned();
        if let Some(t) = self
            .items
            .iter_mut()
            .find(|t| t.text == text && t.level == level && t.leaving.is_none())
        {
            t.at = now;
            t.retry = retry;
            return t.id;
        }
        self.next += 1;
        self.items.push(Toast {
            id: self.next,
            level,
            text,
            retry,
            at: now,
            leaving: None,
        });
        self.next
    }

    /// Starts a toast leaving (Dismiss, or Retry pressed).
    pub fn dismiss(&mut self, id: u64, now: Instant) {
        if let Some(t) = self.items.iter_mut().find(|t| t.id == id) {
            t.leaving.get_or_insert(now);
        }
    }

    /// Dismisses every error with this retry (it was retried and worked).
    pub fn resolve(&mut self, retry: &Retry, now: Instant) {
        for t in &mut self.items {
            if t.retry.as_ref() == Some(retry) {
                t.leaving.get_or_insert(now);
            }
        }
    }

    /// Advances to `now`: info toasts past their time start leaving, and
    /// toasts that finished leaving are dropped. Returns whether anything
    /// is still moving or waiting (the window should check again).
    pub fn tick(&mut self, now: Instant, reduced: bool) -> bool {
        let shown: Vec<u64> = self.shown().iter().map(|t| t.id).collect();
        for t in &mut self.items {
            // Only a toast on screen counts down.
            if t.level == Level::Info
                && t.leaving.is_none()
                && shown.contains(&t.id)
                && now.saturating_duration_since(t.at) >= INFO_FOR
            {
                t.leaving = Some(now);
            }
        }
        let gone = if reduced { Duration::ZERO } else { MOTION };
        self.items.retain(|t| {
            t.leaving
                .is_none_or(|l| now.saturating_duration_since(l) < gone)
        });
        !self.items.is_empty()
    }

    /// The toasts on screen, oldest first (drawn top to bottom).
    pub fn shown(&self) -> &[Toast] {
        let n = self.items.len().min(MAX_SHOWN);
        &self.items[..n]
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The soonest time something changes on its own, so the window can
    /// sleep until then instead of drawing every frame.
    pub fn next_change(&self, now: Instant) -> Option<Duration> {
        self.shown()
            .iter()
            .filter_map(|t| match (t.leaving, t.level) {
                (Some(_), _) => Some(Duration::from_millis(16)),
                (None, _) if now.saturating_duration_since(t.at) < MOTION => {
                    Some(Duration::from_millis(16))
                }
                (None, Level::Info) => {
                    Some(INFO_FOR.saturating_sub(now.saturating_duration_since(t.at)))
                }
                (None, Level::Error) => None,
            })
            .min()
    }
}

/// The stack's widest.
pub const TOAST_W: f32 = 340.;
/// The narrowest stack that still reads in the right gutter.
pub const GUTTER_MIN_W: f32 = 220.;
/// The stack's widest when it sits over the chat area, above the composer.
pub const NARROW_W: f32 = 280.;
/// Space between the stack and the column, the pane, or the window edge.
const GAP: f32 = 12.;

/// Where the stack sits, in window coordinates: its right edge's distance
/// from the window's right edge, its bottom's distance from the window's
/// bottom, and its width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub right: f32,
    pub bottom: f32,
    pub width: f32,
    /// In the free gutter right of the message column (never over it).
    pub in_gutter: bool,
}

/// Places the stack so it never covers the message column: in the free
/// gutter between the column's right edge and the chat area's right edge
/// when that is at least [`GUTTER_MIN_W`] wide, else narrowed at the chat
/// area's bottom right, above the composer (whose top is `composer_top`,
/// from the window's top). The chat area runs from `centre_x` for
/// `centre_w`; the column is centred in it, at most `column_max` wide with
/// 16 px padding each side.
pub fn place(
    window_w: f32,
    window_h: f32,
    centre_x: f32,
    centre_w: f32,
    column_max: f32,
    composer_top: Option<f32>,
) -> Place {
    let column_w = column_max.min(centre_w - 32.).max(0.);
    let centre_right = centre_x + centre_w;
    let column_right = centre_x + (centre_w + column_w) / 2.;
    let gutter = centre_right - column_right - 2. * GAP;
    let right = (window_w - centre_right + GAP).max(GAP);
    if gutter >= GUTTER_MIN_W {
        return Place {
            right,
            bottom: 20.,
            width: gutter.min(TOAST_W),
            in_gutter: true,
        };
    }
    let above = composer_top
        .map(|top| window_h - top + 10.)
        .unwrap_or(20.)
        .max(20.);
    Place {
        right,
        bottom: above,
        width: (centre_w * 0.4)
            .clamp(200., NARROW_W)
            .min((centre_w - 2. * GAP).max(0.)),
        in_gutter: false,
    }
}

/// Whether a plain notice reads as a failure (it gets the error look and
/// stays until dismissed).
pub fn is_error_text(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "could not",
        "failed",
        "cannot",
        "can't",
        "error",
        "was not changed",
        "not available",
        "no voice model",
        "refused",
        "timed out",
    ]
    .iter()
    .any(|w| t.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_leaves_after_four_seconds_and_errors_stay() {
        let t0 = Instant::now();
        let mut q = Toasts::default();
        q.info("Now answering with Groq", t0);
        q.error("Could not reach Groq", Some(Retry::Connect), t0);
        assert!(q.tick(t0 + Duration::from_secs(3), false));
        assert_eq!(q.shown().len(), 2);
        q.tick(t0 + INFO_FOR, false);
        q.tick(t0 + INFO_FOR + MOTION, false);
        assert_eq!(q.shown().len(), 1);
        assert_eq!(q.shown()[0].level, Level::Error);
        q.tick(t0 + Duration::from_secs(60), false);
        assert_eq!(q.shown().len(), 1, "errors persist");
        let id = q.shown()[0].id;
        q.dismiss(id, t0 + Duration::from_secs(61));
        q.tick(t0 + Duration::from_secs(62), false);
        assert!(q.is_empty());
    }

    #[test]
    fn repeats_refresh_instead_of_stacking() {
        let t0 = Instant::now();
        let mut q = Toasts::default();
        let a = q.info("Saved", t0);
        let b = q.info("Saved", t0 + Duration::from_secs(3));
        assert_eq!(a, b);
        assert_eq!(q.shown().len(), 1);
        q.tick(t0 + Duration::from_secs(5), false);
        assert_eq!(q.shown().len(), 1, "the refresh restarted its time");
    }

    #[test]
    fn stack_caps_and_queued_ones_wait() {
        let t0 = Instant::now();
        let mut q = Toasts::default();
        for i in 0..6 {
            q.info(format!("n{i}"), t0);
        }
        assert_eq!(q.shown().len(), MAX_SHOWN);
        assert_eq!(q.shown()[0].text, "n0");
        // The first four leave; the waiting two come up with full time.
        q.tick(t0 + INFO_FOR, false);
        q.tick(t0 + INFO_FOR + MOTION, false);
        assert_eq!(q.shown().len(), 2);
        assert_eq!(q.shown()[0].text, "n4");
    }

    #[test]
    fn reduced_motion_has_no_fade() {
        let t0 = Instant::now();
        let mut q = Toasts::default();
        let id = q.info("x", t0);
        assert_eq!(q.shown()[0].presence(t0, true), 1.0);
        assert!(q.shown()[0].presence(t0, false) < 0.01);
        q.dismiss(id, t0);
        q.tick(t0, true);
        assert!(q.is_empty());
    }

    #[test]
    fn resolve_clears_matching_errors() {
        let t0 = Instant::now();
        let mut q = Toasts::default();
        q.error("Could not list models", Some(Retry::Models("groq")), t0);
        q.resolve(&Retry::Models("groq"), t0);
        q.tick(t0 + MOTION, false);
        assert!(q.is_empty());
        assert!(is_error_text("Tern could not reopen that chat"));
        assert!(!is_error_text("Now answering with Mistral"));
    }

    #[test]
    fn the_stack_takes_the_right_gutter_and_never_the_column() {
        // The reported window: 1600 px, rail 52 + sidebar 260, no pane.
        let p = place(1600., 900., 312., 1288., 760., Some(780.));
        assert!(p.in_gutter);
        let column_right = 312. + (1288. + 760.) / 2.;
        let stack_left = 1600. - p.right - p.width;
        assert!(stack_left >= column_right, "{stack_left} < {column_right}");
        assert!(p.width >= GUTTER_MIN_W && p.width <= TOAST_W);
        assert_eq!(p.bottom, 20.);
        // A right pane beside the chat: the stack stays left of it.
        let p = place(1900., 900., 312., 1288., 760., None);
        assert!(1900. - p.right <= 312. + 1288.);
    }

    #[test]
    fn a_narrow_window_stacks_small_above_the_composer() {
        let p = place(1000., 800., 52., 948., 760., Some(700.));
        assert!(!p.in_gutter);
        assert_eq!(p.bottom, 110.);
        assert!(p.width < TOAST_W && p.width >= 200.);
        // Very narrow: still inside the chat area.
        let p = place(400., 800., 52., 348., 760., Some(700.));
        assert!(p.width <= 348.);
    }
}
