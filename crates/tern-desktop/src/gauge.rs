//! The context gauge: a segmented donut, one arc per kind of content in
//! the window, the free space left as empty track. GPUI-free geometry and
//! colours so it is unit tested; `view` draws it.
//!
//! | Part | Colour (soft, the same in the row's dot) |
//! | --- | --- |
//! | System + instructions | slate |
//! | Tools | teal |
//! | Attachments and sources | sand |
//! | Conversation | lichen (the accent) |
//! | Summary (after a compaction) | violet-grey |
//! | Free | the empty track |
//!
//! The centre's percent is coloured by fullness: green while low, amber
//! from 70 %, red from 90 %. On open the arcs sweep in clockwise one after
//! another over 450 ms on the house curve and the number counts up;
//! reduced motion draws it at once.

use std::f32::consts::TAU;
use std::time::Duration;

use gpui_kit::{Hsla, hsla};

use crate::model::ContextLevel;

/// The whole sweep on open.
pub const SWEEP: Duration = Duration::from_millis(450);
/// The popover gauge's size and ring width.
pub const SIZE: f32 = 96.;
pub const RING: f32 = 10.;
/// The gap between arcs, in degrees, at the popover's size.
pub const GAP_DEG: f32 = 3.;

/// What an arc stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    System,
    Tools,
    Attachments,
    Conversation,
    Summary,
}

impl Part {
    /// The breakdown row's label.
    pub fn of_label(label: &str) -> Option<Self> {
        Some(match label {
            "System + instructions" => Self::System,
            "Tools" => Self::Tools,
            "Attachments and sources" => Self::Attachments,
            "Conversation" => Self::Conversation,
            "Summary" => Self::Summary,
            _ => return None,
        })
    }

    /// Soft and distinct on both themes: a touch darker on light.
    pub fn color(self, dark: bool, accent: Hsla) -> Hsla {
        let l = |d: f32, lt: f32| if dark { d } else { lt };
        match self {
            Self::System => hsla(215. / 360., 0.16, l(0.62, 0.50), 1.),
            Self::Tools => hsla(176. / 360., 0.30, l(0.52, 0.40), 1.),
            Self::Attachments => hsla(36. / 360., 0.42, l(0.64, 0.52), 1.),
            Self::Conversation => accent,
            Self::Summary => hsla(262. / 360., 0.12, l(0.64, 0.52), 1.),
        }
    }
}

/// The fullness colour: calm green, amber from 70 %, red from 90 %.
pub fn fullness(level: ContextLevel, dark: bool) -> Hsla {
    let l = |d: f32, lt: f32| if dark { d } else { lt };
    match level {
        ContextLevel::Fine => hsla(140. / 360., 0.32, l(0.56, 0.38), 1.),
        ContextLevel::Filling => hsla(38. / 360., 0.80, l(0.56, 0.44), 1.),
        ContextLevel::Full => hsla(2. / 360., 0.70, l(0.60, 0.48), 1.),
    }
}

/// The arcs for parts with shares `shares` (0 to 1 of the window each):
/// start and sweep in degrees clockwise from twelve o'clock, a gap of
/// `gap` degrees after each so arcs never touch. Parts too small to show
/// after their gap get a sliver so they are still seen.
pub fn arcs(shares: &[f32], gap: f32) -> Vec<(f32, f32)> {
    let mut at = 0.;
    let total: f32 = shares.iter().sum::<f32>().clamp(0., 1.);
    let scale = if shares.iter().sum::<f32>() > 1. {
        total / shares.iter().sum::<f32>()
    } else {
        1.
    };
    shares
        .iter()
        .map(|&s| {
            let deg = (s.max(0.) * scale) * 360.;
            let sweep = if deg <= 0. { 0. } else { (deg - gap).max(1.2) };
            let arc = (at, sweep);
            at += deg;
            arc
        })
        .collect()
}

/// How far arc `i` of `n` has swept in, 0 to 1, `t` into the opening:
/// one after another, each taking an equal slot of [`SWEEP`].
pub fn sweep_in(i: usize, n: usize, t: Duration, reduced: bool) -> f32 {
    if reduced || n == 0 {
        return 1.;
    }
    let slot = SWEEP.as_secs_f32() / n as f32;
    let local = (t.as_secs_f32() - slot * i as f32) / slot;
    crate::motion::ease(local.clamp(0., 1.))
}

/// The centre number counting up with the sweep.
pub fn count_up(value: f32, t: Duration, reduced: bool) -> f32 {
    if reduced {
        return value;
    }
    value * crate::motion::ease((t.as_secs_f32() / SWEEP.as_secs_f32()).clamp(0., 1.))
}

/// Points along an arc of a circle centred at (`cx`, `cy`) with radius
/// `r`, from `start` degrees for `sweep` degrees, clockwise from twelve
/// o'clock, one point every 3 degrees or so.
pub fn arc_points(cx: f32, cy: f32, r: f32, start: f32, sweep: f32) -> Vec<(f32, f32)> {
    let steps = ((sweep / 3.).ceil() as usize).max(2);
    (0..=steps)
        .map(|k| {
            let a = (start + sweep * k as f32 / steps as f32) / 360. * TAU;
            (cx + r * a.sin(), cy - r * a.cos())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcs_follow_the_shares_with_gaps() {
        let a = arcs(&[0.25, 0.25], GAP_DEG);
        assert_eq!(a[0], (0., 90. - GAP_DEG));
        assert_eq!(a[1], (90., 90. - GAP_DEG));
        // A zero part draws nothing; a tiny one still shows a sliver.
        let a = arcs(&[0., 0.001, 0.5], GAP_DEG);
        assert_eq!(a[0].1, 0.);
        assert!(a[1].1 > 0.);
        // Never more than the circle, even when the shares overrun it.
        let a = arcs(&[0.8, 0.8], GAP_DEG);
        let end = a[1].0 + a[1].1;
        assert!(end <= 360., "{end}");
    }

    #[test]
    fn arcs_sweep_in_one_after_another() {
        let n = 4;
        assert_eq!(sweep_in(0, n, Duration::ZERO, false), 0.);
        let early = Duration::from_millis(60);
        assert!(sweep_in(0, n, early, false) > 0.);
        assert_eq!(sweep_in(3, n, early, false), 0., "the last waits its turn");
        for i in 0..n {
            assert_eq!(sweep_in(i, n, SWEEP, false), 1.);
            assert_eq!(sweep_in(i, n, Duration::ZERO, true), 1., "reduced motion");
        }
        assert_eq!(count_up(42., SWEEP, false), 42.);
        assert!(count_up(42., Duration::from_millis(50), false) < 42.);
        assert_eq!(count_up(42., Duration::ZERO, true), 42.);
    }

    #[test]
    fn fullness_reads_green_amber_red_and_parts_are_distinct() {
        let g = fullness(ContextLevel::Fine, true);
        let a = fullness(ContextLevel::Filling, true);
        let r = fullness(ContextLevel::Full, true);
        assert!(g.h > a.h && a.h > r.h);
        let accent = crate::theme::Palette::dark().accent;
        let parts = [
            Part::System,
            Part::Tools,
            Part::Attachments,
            Part::Conversation,
            Part::Summary,
        ];
        for dark in [true, false] {
            let cs: Vec<Hsla> = parts.iter().map(|p| p.color(dark, accent)).collect();
            for i in 0..cs.len() {
                for j in i + 1..cs.len() {
                    assert_ne!(cs[i], cs[j]);
                }
            }
        }
        assert_eq!(Part::of_label("Summary"), Some(Part::Summary));
        assert_eq!(Part::of_label("Free"), None);
    }

    #[test]
    fn arc_points_run_clockwise_from_the_top() {
        let pts = arc_points(50., 50., 40., 0., 90.);
        let first = pts[0];
        let last = *pts.last().unwrap();
        assert!((first.0 - 50.).abs() < 1e-3 && (first.1 - 10.).abs() < 1e-3);
        assert!((last.0 - 90.).abs() < 1e-3 && (last.1 - 50.).abs() < 1e-3);
    }
}
