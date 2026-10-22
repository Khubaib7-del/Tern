//! Motion rules from `docs/02-ui-definition.md` §6, kept GPUI-free so they
//! can be unit tested.
//!
//! | Rule | Value |
//! | --- | --- |
//! | Curve | `cubic-bezier(.32,.72,0,1)` for everything |
//! | UI (panels, chips, screens) | 200 ms |
//! | The tern mark | 700 ms |
//! | Theme switch | 600 ms, every colour token at once, soft circle from the toggle |
//! | Shimmer | A light band across the status phrase every 1.6 s |
//!
//! Motion follows real events (a step starts, a page lands, text streams);
//! nothing blocks input, and reduced motion turns it all off.

use std::time::{Duration, Instant};

use gpui_kit::Hsla;

use crate::theme::Palette;

pub const UI: Duration = Duration::from_millis(200);
pub const MARK: Duration = Duration::from_millis(700);
pub const THEME: Duration = Duration::from_millis(600);
pub const SHIMMER: Duration = Duration::from_millis(1600);

/// `cubic-bezier(.32,.72,0,1)`: fast start, long calm settle.
pub fn ease(t: f32) -> f32 {
    bezier(0.32, 0.72, 0.0, 1.0, t)
}

/// Solves a CSS cubic-bezier for x = `t` and returns y.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let curve = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let slope = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    // Newton steps, then bisection if the slope is too flat.
    let mut s = t;
    for _ in 0..8 {
        let err = curve(x1, x2, s) - t;
        if err.abs() < 1e-5 {
            return curve(y1, y2, s);
        }
        let d = slope(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - err / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    s = t;
    for _ in 0..30 {
        let x = curve(x1, x2, s);
        if (x - t).abs() < 1e-5 {
            break;
        }
        if x < t {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.0;
    }
    curve(y1, y2, s)
}

/// Eased progress of something that started at `start` and lasts `dur`.
pub fn progress(start: Instant, dur: Duration, now: Instant) -> f32 {
    let t = now.saturating_duration_since(start).as_secs_f32() / dur.as_secs_f32();
    ease(t.min(1.0))
}

/// A panel that slides open and shut instead of appearing.
#[derive(Clone, Copy, Debug, Default)]
pub struct Slide {
    open: bool,
    /// The slide in flight: when it started and how open it was then.
    moving: Option<(Instant, f32)>,
    shown: f32,
}

impl Slide {
    pub fn new(open: bool) -> Self {
        Self {
            open,
            moving: None,
            shown: if open { 1.0 } else { 0.0 },
        }
    }

    /// How far open to draw the panel (0 to 1), or `None` to leave it out.
    /// Call every frame with the panel's real state; a change starts a
    /// slide from wherever the panel is now, so reversing never jumps.
    pub fn frame(&mut self, open: bool, now: Instant, reduced: bool) -> Option<f32> {
        if open != self.open {
            self.open = open;
            self.moving = Some((now, self.shown));
        }
        let target = if self.open { 1.0 } else { 0.0 };
        self.shown = match self.moving {
            Some((since, from)) if !reduced => {
                let r = raw(since, UI, now);
                if r >= 1.0 {
                    self.moving = None;
                    target
                } else {
                    from + (target - from) * ease(r)
                }
            }
            _ => {
                self.moving = None;
                target
            }
        };
        (self.shown > 0.0 || self.moving.is_some()).then_some(self.shown)
    }

    pub fn moving(&self) -> bool {
        self.moving.is_some()
    }
}

fn raw(start: Instant, dur: Duration, now: Instant) -> f32 {
    (now.saturating_duration_since(start).as_secs_f32() / dur.as_secs_f32()).min(1.0)
}

/// The light/dark switch in progress.
#[derive(Clone, Copy, Debug)]
pub struct ThemeWipe {
    pub since: Instant,
    /// Where the toggle was clicked, in window pixels.
    pub origin: (f32, f32),
    pub to_dark: bool,
}

impl ThemeWipe {
    /// The palette at `now`: every token moves together from the old theme
    /// to the new one.
    pub fn palette(&self, now: Instant) -> Palette {
        let t = progress(self.since, THEME, now);
        Palette::get(!self.to_dark).lerp(&Palette::get(self.to_dark), t)
    }

    /// The soft circle's radius (as a share of the window's diagonal) and
    /// strength, both 0 to 1.
    pub fn circle(&self, now: Instant) -> (f32, f32) {
        let t = progress(self.since, THEME, now);
        (t, (1.0 - t) * 0.35)
    }

    pub fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.since) >= THEME
    }
}

/// How bright each character of a shimmering phrase is (0 = muted, 1 = full
/// text colour) at `phase` (0 to 1) of the sweep.
pub fn shimmer(len: usize, phase: f32) -> Vec<f32> {
    const BAND: f32 = 4.0;
    let center = phase * (len as f32 + 2.0 * BAND) - BAND;
    (0..len)
        .map(|i| (1.0 - (i as f32 - center).abs() / BAND).max(0.0))
        .collect()
}

pub fn lerp_color(a: Hsla, b: Hsla, t: f32) -> Hsla {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    // Neutral greys: hue and saturation barely matter, so plain lerp works.
    Hsla {
        h: a.h + (b.h - a.h) * t,
        s: a.s + (b.s - a.s) * t,
        l: a.l + (b.l - a.l) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_starts_fast_and_settles() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        let mut last = 0.0;
        for i in 1..=20 {
            let v = ease(i as f32 / 20.0);
            assert!(v >= last, "monotonic");
            last = v;
        }
        assert!(ease(0.25) > 0.6, "most of the way early: {}", ease(0.25));
        assert!(ease(0.5) > 0.85);
    }

    #[test]
    fn slides_open_close_and_reverse_without_a_jump() {
        let t0 = Instant::now();
        let mut s = Slide::new(false);
        assert_eq!(s.frame(false, t0, false), None);
        assert!(s.frame(true, t0, false).unwrap() < 0.01);
        let mid = s.frame(true, t0 + UI / 4, false).unwrap();
        assert!(mid > 0.3 && mid < 1.0);
        // Close half-way: it shrinks from where it was, not from fully open.
        let back = s.frame(false, t0 + UI / 4, false).unwrap();
        assert!((back - mid).abs() < 0.05, "{back} vs {mid}");
        assert_eq!(s.frame(false, t0 + UI * 3, false), None);
        assert!(!s.moving());
        assert_eq!(
            s.frame(true, t0 + UI * 4, true),
            Some(1.0),
            "reduced motion"
        );
    }

    #[test]
    fn the_shimmer_band_sweeps_across() {
        let start = shimmer(10, 0.0);
        let end = shimmer(10, 1.0);
        assert!(start.iter().all(|&w| w == 0.0));
        assert!(end.iter().all(|&w| w == 0.0));
        let mid = shimmer(10, 0.5);
        let peak = mid
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert!((4..=5).contains(&peak));
    }

    #[test]
    fn theme_wipe_moves_every_token() {
        let t0 = Instant::now();
        let w = ThemeWipe {
            since: t0,
            origin: (0.0, 0.0),
            to_dark: true,
        };
        assert_eq!(w.palette(t0).canvas, Palette::light().canvas);
        assert_eq!(w.palette(t0 + THEME).canvas, Palette::dark().canvas);
        let mid = w.palette(t0 + THEME / 2).text;
        assert!(mid.l < Palette::dark().text.l && mid.l > Palette::light().text.l);
        assert!(w.done(t0 + THEME));
        assert_eq!(w.circle(t0 + THEME).1, 0.0);
    }
}
