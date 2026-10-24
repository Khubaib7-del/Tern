//! Motion rules from `docs/02-ui-definition.md` §6, kept GPUI-free so they
//! can be unit tested.
//!
//! | Rule | Value |
//! | --- | --- |
//! | Curve | `cubic-bezier(.32,.72,0,1)` for fades and timed reveals |
//! | Springs | [`SpringSpec::UI`] (~0.26 s, slight overshoot) for pills, markers, underlines; [`SpringSpec::PANEL`] (~0.4 s, critical) for panels, cards, toasts; [`SpringSpec::SCOUT`] for Scout's moods. Closed form, velocity kept on retarget |
//! | UI (panels, chips, screens) | 200 ms |
//! | The tern mark | 700 ms |
//! | Theme switch | 800 ms: the new theme is drawn whole and revealed from the centre out (a vertical split, as v2 s17); no ripple, no colour blending |
//! | Shimmer | A light band across the status phrase every 1.6 s |
//!
//! Motion follows real events (a step starts, a page lands, text streams);
//! nothing blocks input, and reduced motion turns it all off.

use std::time::{Duration, Instant};

use gpui_kit::Hsla;

use crate::theme::Palette;

pub const UI: Duration = Duration::from_millis(200);
pub const MARK: Duration = Duration::from_millis(700);
pub const THEME: Duration = Duration::from_millis(800);
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

/// A spring's character: how stiff it is and how much it is damped,
/// for a unit mass. Solved in closed form, so a late frame never makes
/// it unstable and any frame can be sampled exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringSpec {
    pub stiffness: f32,
    /// 1 is critical (no overshoot); below 1 overshoots a little.
    pub damping_ratio: f32,
}

impl SpringSpec {
    /// The house spring for UI: settles in about 0.26 s with a hint of
    /// overshoot (v2's `spring(.26)` on highlights, markers and pills).
    pub const UI: SpringSpec = SpringSpec {
        stiffness: 380.0,
        damping_ratio: 0.86,
    };
    /// A softer spring for whole panels and cards (about 0.4 s, no
    /// overshoot), so a big surface never wobbles.
    pub const PANEL: SpringSpec = SpringSpec {
        stiffness: 170.0,
        damping_ratio: 1.0,
    };
    /// Scout's body between moods: slower, slightly bouncy, alive.
    pub const SCOUT: SpringSpec = SpringSpec {
        stiffness: 110.0,
        damping_ratio: 0.72,
    };

    fn omega(self) -> f32 {
        self.stiffness.max(1e-3).sqrt()
    }

    /// Displacement and velocity `t` seconds after leaving `x0` (as an
    /// offset from the target) with velocity `v0`.
    pub fn solve(self, x0: f32, v0: f32, t: f32) -> (f32, f32) {
        let w = self.omega();
        let z = self.damping_ratio.max(0.0);
        let t = t.max(0.0);
        if (z - 1.0).abs() < 1e-3 {
            // Critically damped.
            let e = (-w * t).exp();
            let b = v0 + w * x0;
            let x = (x0 + b * t) * e;
            let v = (b - w * (x0 + b * t)) * e;
            (x, v)
        } else if z < 1.0 {
            let wd = w * (1.0 - z * z).sqrt();
            let e = (-z * w * t).exp();
            let a = x0;
            let b = (v0 + z * w * x0) / wd;
            let (s, c) = (wd * t).sin_cos();
            let x = e * (a * c + b * s);
            let v = e * ((b * wd - z * w * a) * c - (a * wd + z * w * b) * s);
            (x, v)
        } else {
            let r = w * (z * z - 1.0).sqrt();
            let (r1, r2) = (-z * w + r, -z * w - r);
            let c2 = (v0 - r1 * x0) / (r2 - r1);
            let c1 = x0 - c2;
            let (e1, e2) = ((r1 * t).exp(), (r2 * t).exp());
            (c1 * e1 + c2 * e2, c1 * r1 * e1 + c2 * r2 * e2)
        }
    }
}

/// One number on a spring. A new target mid-flight starts from where the
/// value is *and how fast it is moving* (velocity-preserving retarget),
/// so a reversal curves instead of jerking. Reduced motion moves it at
/// once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    spec: SpringSpec,
    to: f32,
    /// Offset from `to` and velocity when the current flight began.
    x0: f32,
    v0: f32,
    since: Option<Instant>,
    shown: f32,
    velocity: f32,
}

/// How close is close enough to stop asking for frames.
const REST_X: f32 = 0.001;
const REST_V: f32 = 0.01;

impl Spring {
    pub fn new(at: f32, spec: SpringSpec) -> Self {
        Self {
            spec,
            to: at,
            x0: 0.0,
            v0: 0.0,
            since: None,
            shown: at,
            velocity: 0.0,
        }
    }

    pub fn value(&self) -> f32 {
        self.shown
    }

    /// Heads for `target` from wherever the value is now.
    pub fn retarget(&mut self, target: f32, now: Instant) {
        if (target - self.to).abs() <= f32::EPSILON {
            return;
        }
        self.sample(now);
        self.x0 = self.shown - target;
        self.v0 = self.velocity;
        self.to = target;
        self.since = Some(now);
    }

    /// Shows `at` at once with nothing in flight (a dragged handle).
    pub fn jump(&mut self, at: f32) {
        self.to = at;
        self.shown = at;
        self.velocity = 0.0;
        self.since = None;
    }

    /// Where the value is at `now`.
    pub fn sample(&mut self, now: Instant) -> f32 {
        if let Some(since) = self.since {
            let t = now.saturating_duration_since(since).as_secs_f32();
            let (x, v) = self.spec.solve(self.x0, self.v0, t);
            if x.abs() < REST_X * self.scale() && v.abs() < REST_V * self.scale() {
                self.since = None;
                self.shown = self.to;
                self.velocity = 0.0;
            } else {
                self.shown = self.to + x;
                self.velocity = v;
            }
        }
        self.shown
    }

    /// Heads for `target` and returns this frame's value; reduced motion
    /// jumps.
    pub fn frame(&mut self, target: f32, now: Instant, reduced: bool) -> f32 {
        if reduced {
            self.jump(target);
            return target;
        }
        self.retarget(target, now);
        self.sample(now)
    }

    pub fn moving(&self) -> bool {
        self.since.is_some()
    }

    /// Advances `dt` seconds toward `target` on a caller's own clock
    /// (Scout's rig). Exact for any `dt`: the closed form is applied from
    /// the current value and velocity, so a late frame never overshoots.
    pub fn advance(&mut self, target: f32, dt: f32) -> f32 {
        self.since = None;
        self.to = target;
        let (x, v) = self
            .spec
            .solve(self.shown - target, self.velocity, dt.max(0.0));
        self.shown = target + x;
        self.velocity = v;
        self.shown
    }

    /// Adds velocity on a caller's own clock (see [`Spring::advance`]).
    pub fn kick(&mut self, dv: f32) {
        self.velocity += dv;
    }

    /// Whether the value has come to rest at `target`.
    pub fn settled(&self, target: f32) -> bool {
        (self.shown - target).abs() < 0.002 && self.velocity.abs() < 0.01
    }

    /// Rest thresholds scale with the distance travelled, so pixel-sized
    /// and unit-sized values both settle cleanly.
    fn scale(&self) -> f32 {
        (self.x0.abs()).clamp(1.0, 1000.0)
    }
}

/// Something that animates in when it appears and out before it goes
/// (v2's AnimatePresence): a toast, a row, a panel. Holds 0 to 1.
#[derive(Clone, Copy, Debug)]
pub struct Presence {
    spring: Spring,
}

impl Presence {
    /// `present` at first; `appear` starts it at 0 so it grows in.
    pub fn new(present: bool, appear: bool, spec: SpringSpec) -> Self {
        let start = if present && !appear { 1.0 } else { 0.0 };
        Self {
            spring: Spring::new(start, spec),
        }
    }

    /// How far in to draw it (0 to 1), or `None` once it has gone.
    pub fn frame(&mut self, present: bool, now: Instant, reduced: bool) -> Option<f32> {
        let v = self
            .spring
            .frame(if present { 1.0 } else { 0.0 }, now, reduced);
        (present || self.spring.moving() || v > 0.0).then_some(v.clamp(0.0, 1.2))
    }

    pub fn moving(&self) -> bool {
        self.spring.moving()
    }
}

/// The start delay of item `i` in a staggered list (v2: 25–60 ms per
/// item), capped so a long list never makes the last row wait.
pub fn stagger(i: usize, step: Duration) -> Duration {
    step * (i.min(12) as u32)
}

/// Progress (0 to 1, on the house curve) of item `i` of a staggered
/// entrance that began at `start` and lasts `dur` per item.
pub fn staggered(
    start: Instant,
    i: usize,
    step: Duration,
    dur: Duration,
    now: Instant,
    reduced: bool,
) -> f32 {
    if reduced {
        return 1.0;
    }
    let begin = start + stagger(i, step);
    if now < begin {
        return 0.0;
    }
    progress(begin, dur, now)
}

/// A panel that slides open and shut instead of appearing, on
/// [`SpringSpec::PANEL`]. Reversing mid-slide keeps its momentum.
#[derive(Clone, Copy, Debug)]
pub struct Slide {
    spring: Spring,
}

impl Slide {
    pub fn new(open: bool) -> Self {
        Self {
            spring: Spring::new(if open { 1.0 } else { 0.0 }, SpringSpec::PANEL),
        }
    }

    /// How far open to draw the panel (0 to 1), or `None` to leave it out.
    /// Call every frame with the panel's real state.
    pub fn frame(&mut self, open: bool, now: Instant, reduced: bool) -> Option<f32> {
        let v = self
            .spring
            .frame(if open { 1.0 } else { 0.0 }, now, reduced)
            .clamp(0.0, 1.0);
        (v > 0.0 || open || self.spring.moving()).then_some(v)
    }

    pub fn moving(&self) -> bool {
        self.spring.moving()
    }
}

/// A value that springs to each new target (a segmented control's pill,
/// the ribbon marker, a tab underline, the effort knob) on
/// [`SpringSpec::UI`]. A new target mid-flight keeps the velocity, so
/// nothing jumps; reduced motion moves it at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glide {
    spring: Spring,
}

impl Glide {
    pub fn new(at: f32) -> Self {
        Self {
            spring: Spring::new(at, SpringSpec::UI),
        }
    }

    /// Where to draw the value this frame, heading for `target`.
    pub fn frame(&mut self, target: f32, now: Instant, reduced: bool) -> f32 {
        self.spring.frame(target, now, reduced)
    }

    pub fn moving(&self) -> bool {
        self.spring.moving()
    }

    /// Shows `at` at once with nothing in flight (a dragged handle follows
    /// the pointer); the next `frame` with another target springs from it.
    pub fn jump(&mut self, at: f32) {
        self.spring.jump(at);
    }
}

/// The light/dark switch in progress.
#[derive(Clone, Copy, Debug)]
pub struct ThemeWipe {
    pub since: Instant,
    pub to_dark: bool,
}

impl ThemeWipe {
    /// The palette the window is drawn in: the new one, whole, from the
    /// first frame. Nothing blends.
    pub fn palette(&self, _now: Instant) -> Palette {
        Palette::get(self.to_dark)
    }

    /// The old theme's canvas, still covering each side: the share of the
    /// window's width covered on the left and on the right (0.5 at the
    /// start, 0 when done). The new theme shows in the middle band.
    pub fn inset(&self, now: Instant) -> f32 {
        (1.0 - progress(self.since, THEME, now)) * 0.5
    }

    /// The old theme's canvas colour, for the covers.
    pub fn old_canvas(&self) -> Hsla {
        Palette::get(!self.to_dark).canvas
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
        let ms = |n: u64| Duration::from_millis(n);
        let mut s = Slide::new(false);
        assert_eq!(s.frame(false, t0, false), None);
        assert!(s.frame(true, t0, false).unwrap() < 0.01);
        let mid = s.frame(true, t0 + ms(80), false).unwrap();
        assert!(mid > 0.2 && mid < 1.0, "{mid}");
        // Close part-way: it turns from where it was, not from fully open.
        let back = s.frame(false, t0 + ms(80), false).unwrap();
        assert!((back - mid).abs() < 1e-4, "{back} vs {mid}");
        // It still carries the opening momentum for a moment.
        let after = s.frame(false, t0 + ms(90), false).unwrap();
        assert!(after > back - 0.05, "{after} vs {back}");
        assert_eq!(s.frame(false, t0 + ms(2000), false), None);
        assert!(!s.moving());
        assert_eq!(
            s.frame(true, t0 + ms(2100), true),
            Some(1.0),
            "reduced motion"
        );
    }

    #[test]
    fn a_glide_springs_to_each_target_and_keeps_its_velocity() {
        let t0 = Instant::now();
        let ms = |n: u64| Duration::from_millis(n);
        let mut g = Glide::new(0.0);
        assert_eq!(g.frame(0.0, t0, false), 0.0);
        assert!(!g.moving());
        assert!(g.frame(3.0, t0, false) < 0.01, "starts where it was");
        let early = g.frame(3.0, t0 + ms(60), false);
        assert!(early > 0.6 && early < 3.0, "fast start: {early}");
        // Retarget mid-flight: same place this frame, and still moving
        // forward for a moment (velocity kept), then it turns.
        let turn = g.frame(0.5, t0 + ms(60), false);
        assert!((turn - early).abs() < 1e-4);
        assert!(g.frame(0.5, t0 + ms(66), false) > turn - 0.01);
        let settled = g.frame(0.5, t0 + ms(1500), false);
        assert_eq!(settled, 0.5);
        assert!(!g.moving());
        assert_eq!(g.frame(4.0, t0 + ms(1600), true), 4.0, "reduced motion");
        g.jump(2.4);
        assert!(!g.moving());
        assert!((g.frame(2.0, t0 + ms(1700), false) - 2.4).abs() < 1e-4);
        assert!(g.moving());
        assert_eq!(g.frame(2.0, t0 + ms(3000), false), 2.0);
    }

    #[test]
    fn the_closed_form_spring_matches_a_numeric_integration() {
        for spec in [
            SpringSpec::UI,
            SpringSpec::PANEL,
            SpringSpec::SCOUT,
            SpringSpec {
                stiffness: 200.0,
                damping_ratio: 1.6,
            },
        ] {
            let (mut x, mut v) = (1.0f32, 0.5f32);
            let h = 1.0 / 4000.0;
            let c = 2.0 * spec.damping_ratio * spec.stiffness.sqrt();
            for i in 1..=2000 {
                let a = -spec.stiffness * x - c * v;
                v += a * h;
                x += v * h;
                if i % 500 == 0 {
                    let (cx, cv) = spec.solve(1.0, 0.5, i as f32 * h);
                    assert!((cx - x).abs() < 0.01, "{spec:?} x {cx} vs {x}");
                    assert!((cv - v).abs() < 0.08, "{spec:?} v {cv} vs {v}");
                }
            }
        }
        // The UI spring settles in about a quarter second.
        let (x, _) = SpringSpec::UI.solve(1.0, 0.0, 0.3);
        assert!(x.abs() < 0.03, "{x}");
        // A late frame is still exact, never unstable.
        let (x, v) = SpringSpec::UI.solve(1.0, 0.0, 30.0);
        assert!(x.abs() < 1e-6 && v.abs() < 1e-6);
    }

    #[test]
    fn presence_grows_in_and_animates_out_before_it_goes() {
        let t0 = Instant::now();
        let ms = |n: u64| Duration::from_millis(n);
        let mut p = Presence::new(true, true, SpringSpec::UI);
        assert!(p.frame(true, t0, false).unwrap() < 0.01);
        assert!(p.frame(true, t0 + ms(100), false).unwrap() > 0.5);
        assert!(p.frame(true, t0 + ms(1000), false).unwrap() >= 0.99);
        // Removed: still drawn while it leaves.
        assert!(p.frame(false, t0 + ms(1000), false).is_some());
        assert!(p.frame(false, t0 + ms(1050), false).is_some());
        assert_eq!(p.frame(false, t0 + ms(3000), false), None);
        // Reduced motion: there or gone at once.
        let mut r = Presence::new(true, true, SpringSpec::UI);
        assert_eq!(r.frame(true, t0, true), Some(1.0));
        assert_eq!(r.frame(false, t0, true), None);
    }

    #[test]
    fn a_stagger_delays_each_item_and_caps_the_wait() {
        let t0 = Instant::now();
        let step = Duration::from_millis(40);
        assert_eq!(stagger(0, step), Duration::ZERO);
        assert_eq!(stagger(3, step), Duration::from_millis(120));
        assert_eq!(stagger(500, step), stagger(12, step));
        let at = t0 + Duration::from_millis(60);
        let first = staggered(t0, 0, step, UI, at, false);
        let third = staggered(t0, 2, step, UI, at, false);
        assert!(first > 0.0 && third == 0.0);
        assert_eq!(staggered(t0, 9, step, UI, t0, true), 1.0);
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
    fn theme_reveal_splits_from_the_centre() {
        let t0 = Instant::now();
        let w = ThemeWipe {
            since: t0,
            to_dark: true,
        };
        // The new palette is drawn whole from the start: no blending.
        assert_eq!(w.palette(t0).canvas, Palette::dark().canvas);
        assert_eq!(w.palette(t0 + THEME / 2).text, Palette::dark().text);
        assert_eq!(w.old_canvas(), Palette::light().canvas);
        assert_eq!(w.inset(t0), 0.5);
        let mid = w.inset(t0 + THEME / 2);
        assert!(mid > 0.0 && mid < 0.5);
        assert_eq!(w.inset(t0 + THEME), 0.0);
        assert!(w.done(t0 + THEME));
        assert_eq!(THEME, Duration::from_millis(800));
    }
}
