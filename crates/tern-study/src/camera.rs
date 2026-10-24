//! The Explain in 3D camera, free of any UI so it is tested: drag to
//! orbit (a full turn about the vertical, the tilt clamped between a low
//! and a top-down view), the wheel or a pinch to zoom about the pointer,
//! right- or Shift-drag to pan, focusing a node (the camera turns about
//! it and moves in), keys, and a Reset back to the first view.
//!
//! The camera eases toward where it is going and keeps a little spin
//! after a drag (damped inertia). With reduced motion it jumps straight
//! there and never spins on.
//!
//! It drives [`View`] (the turn, the tilt and the pivot) and a [`Fit`]
//! (zoom and pan), so the projection and hit testing in
//! `crate::explain3d` stay as they are.

use crate::explain3d::{BASE_ELEVATION, ELEVATION, Fit, GRID, View};

/// Radians turned per pixel dragged.
pub const ORBIT_PER_PX: f32 = 0.008;
/// The closest and farthest zoom.
pub const ZOOM: (f32, f32) = (0.35, 5.0);
/// How close focusing a node comes, at least.
pub const FOCUS_ZOOM: f32 = 1.8;
/// A key press: 15° of turn, 5° of tilt, a fifth of zoom.
pub const KEY_YAW: f32 = std::f32::consts::PI / 12.0;
pub const KEY_PITCH: f32 = std::f32::consts::PI / 36.0;
pub const KEY_ZOOM: f32 = 1.2;
/// A press that moves less than this many pixels is a click, not a drag.
pub const CLICK_SLOP: f32 = 4.0;
/// How fast the camera eases toward its target, per second.
const EASE: f32 = 10.0;
/// How fast a spin after a drag dies away, per second.
const DAMP: f32 = 3.5;
/// Below this a spin (radians a second) stops.
const SPIN_STOP: f32 = 0.02;
/// Most spin a drag can leave, radians a second.
const SPIN_MAX: f32 = 6.0;

/// What a drag does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drag {
    Orbit,
    Pan,
}

/// The camera's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    In,
    Out,
}

impl Key {
    /// From a key name (`left`, `+`, `=`, `-` …).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "left" => Self::Left,
            "right" => Self::Right,
            "up" => Self::Up,
            "down" => Self::Down,
            "+" | "=" | "plus" | "equal" => Self::In,
            "-" | "_" | "minus" => Self::Out,
            _ => return None,
        })
    }
}

/// Where the camera is (or is going).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Turn about the vertical, radians, unbounded.
    pub yaw: f32,
    /// Tilt added to the base elevation, radians, clamped.
    pub pitch: f32,
    pub zoom: f32,
    /// Screen offset, pixels.
    pub pan: (f32, f32),
    /// The point turned about, floor units.
    pub pivot: [f32; 3],
    /// The projected point put at the centre of the view.
    pub centre: (f32, f32),
}

/// The tilt range [`Pose::pitch`] keeps to.
pub fn pitch_range() -> (f32, f32) {
    (ELEVATION.0 - BASE_ELEVATION, ELEVATION.1 - BASE_ELEVATION)
}

fn clamp_pitch(p: f32) -> f32 {
    let (lo, hi) = pitch_range();
    p.clamp(lo, hi)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub now: Pose,
    pub target: Pose,
    /// The first view's drawing bounds (projected), which set the base
    /// scale and centre so turning never rescales the drawing.
    home_bounds: (f32, f32, f32, f32),
    /// The first view's turn and tilt.
    home_view: (f32, f32),
    /// Spin left by a drag, radians a second (turn, tilt).
    spin: (f32, f32),
    drag: Option<DragState>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DragState {
    kind: Drag,
    last: (f32, f32),
    /// Past the click slop: the press is a drag.
    live: bool,
    /// Recent speed, radians a second, smoothed.
    speed: (f32, f32),
}

impl Camera {
    /// A camera at the first view of a drawing with `home_bounds` (the
    /// scene projected with `View::default()`).
    pub fn new(home_bounds: (f32, f32, f32, f32)) -> Self {
        Self::framed(home_bounds, 0.0, 0.0)
    }

    /// A camera whose first view is turned by `yaw` and tilted by `pitch`
    /// and framed on `bounds` (the figure projected with that view, so it
    /// fills the stage: zoom to fit).
    pub fn framed(bounds: (f32, f32, f32, f32), yaw: f32, pitch: f32) -> Self {
        let home_bounds = safe_bounds(bounds);
        let ok = |v: f32| if v.is_finite() { v } else { 0.0 };
        let home_view = (ok(yaw), clamp_pitch(ok(pitch)));
        let home = Self::home_pose(home_bounds, home_view);
        Self {
            now: home,
            target: home,
            home_bounds,
            home_view,
            spin: (0.0, 0.0),
            drag: None,
        }
    }

    fn home_pose(b: (f32, f32, f32, f32), v: (f32, f32)) -> Pose {
        Pose {
            yaw: v.0,
            pitch: v.1,
            zoom: 1.0,
            pan: (0.0, 0.0),
            pivot: [GRID / 2.0, GRID / 2.0, 0.0],
            centre: ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0),
        }
    }

    /// Back to the first view.
    pub fn reset(&mut self, reduced: bool) {
        self.target = Self::home_pose(self.home_bounds, self.home_view);
        self.spin = (0.0, 0.0);
        if reduced {
            self.now = self.target;
        }
    }

    /// Whether the camera is at (and going to) the first view.
    pub fn at_home(&self) -> bool {
        let h = Self::home_pose(self.home_bounds, self.home_view);
        close(&self.now, &h) && close(&self.target, &h)
    }

    /// The view for the projection, with `lift` raised.
    pub fn view(&self, lift: Option<usize>) -> View {
        View {
            yaw: self.now.yaw,
            pitch: self.now.pitch,
            lift,
            pivot: Some(self.now.pivot),
        }
    }

    /// Where drawing units land in a `w` × `h` area with `margin`.
    pub fn fit(&self, w: f32, h: f32, margin: f32) -> Fit {
        let base = Fit::new(self.home_bounds, w, h, margin).scale;
        let scale = base * self.now.zoom;
        let f = Fit {
            scale,
            dx: w / 2.0 + self.now.pan.0 - self.now.centre.0 * scale,
            dy: h / 2.0 + self.now.pan.1 - self.now.centre.1 * scale,
        };
        if [f.scale, f.dx, f.dy].iter().all(|v| v.is_finite()) && f.scale > 0.0 {
            f
        } else {
            Fit::new(self.home_bounds, w, h, margin)
        }
    }

    /// The view the camera is going to, with `lift` raised: where labels
    /// are laid out while it is on its way.
    pub fn target_view(&self, lift: Option<usize>) -> View {
        View {
            yaw: self.target.yaw,
            pitch: self.target.pitch,
            lift,
            pivot: Some(self.target.pivot),
        }
    }

    /// [`Camera::fit`] for where the camera is going.
    pub fn target_fit(&self, w: f32, h: f32, margin: f32) -> Fit {
        let base = Fit::new(self.home_bounds, w, h, margin).scale;
        let scale = base * self.target.zoom;
        let f = Fit {
            scale,
            dx: w / 2.0 + self.target.pan.0 - self.target.centre.0 * scale,
            dy: h / 2.0 + self.target.pan.1 - self.target.centre.1 * scale,
        };
        if [f.scale, f.dx, f.dy].iter().all(|v| v.is_finite()) && f.scale > 0.0 {
            f
        } else {
            Fit::new(self.home_bounds, w, h, margin)
        }
    }

    /// The zoom now, for sizing labels.
    pub fn zoom(&self) -> f32 {
        self.now.zoom
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// A press at `(x, y)` (local pixels) starts a drag.
    pub fn begin_drag(&mut self, kind: Drag, x: f32, y: f32) {
        self.spin = (0.0, 0.0);
        self.drag = Some(DragState {
            kind,
            last: (x, y),
            live: false,
            speed: (0.0, 0.0),
        });
    }

    /// The pointer moved to `(x, y)`, `dt` seconds after the last move.
    /// Returns whether the camera moved.
    pub fn drag_to(&mut self, x: f32, y: f32, dt: f32) -> bool {
        if !(x.is_finite() && y.is_finite()) {
            return false;
        }
        let Some(d) = self.drag.as_mut() else {
            return false;
        };
        let (dx, dy) = (x - d.last.0, y - d.last.1);
        // Until it leaves the slop it may still be a click; then the
        // whole way from the press counts.
        if !d.live {
            if dx.hypot(dy) < CLICK_SLOP {
                return false;
            }
            d.live = true;
        }
        d.last = (x, y);
        match d.kind {
            Drag::Orbit => {
                let (ty, tp) = (-dx * ORBIT_PER_PX, dy * ORBIT_PER_PX);
                // A drag is direct: the camera is where the hand is.
                self.target.yaw += ty;
                self.target.pitch = clamp_pitch(self.target.pitch + tp);
                self.now.yaw = self.target.yaw;
                self.now.pitch = self.target.pitch;
                if dt > 1e-4 {
                    let k = 0.5;
                    d.speed = (
                        d.speed.0 * (1.0 - k) + (ty / dt) * k,
                        d.speed.1 * (1.0 - k) + (tp / dt) * k,
                    );
                }
            }
            Drag::Pan => {
                self.target.pan = clamp_pan((self.target.pan.0 + dx, self.target.pan.1 + dy));
                self.now.pan = self.target.pan;
            }
        }
        true
    }

    /// The press ended. Returns whether it was a click (it barely moved).
    /// An orbit leaves its spin, unless motion is reduced.
    pub fn end_drag(&mut self, reduced: bool) -> bool {
        let Some(d) = self.drag.take() else {
            return false;
        };
        if d.kind == Drag::Orbit && !reduced && d.live {
            self.spin = (
                d.speed.0.clamp(-SPIN_MAX, SPIN_MAX),
                d.speed.1.clamp(-SPIN_MAX, SPIN_MAX),
            );
        }
        !d.live
    }

    /// Zooms by `factor` keeping the point at `(x, y)` (local pixels, in a
    /// `w` × `h` area) where it is.
    pub fn zoom_at(&mut self, factor: f32, x: f32, y: f32, w: f32, h: f32, reduced: bool) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let z0 = self.target.zoom;
        let z1 = (z0 * factor).clamp(ZOOM.0, ZOOM.1);
        let r = z1 / z0;
        let (sx, sy) = (x - w / 2.0, y - h / 2.0);
        if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite()) {
            return;
        }
        self.target.zoom = z1;
        self.target.pan = clamp_pan((
            sx * (1.0 - r) + self.target.pan.0 * r,
            sy * (1.0 - r) + self.target.pan.1 * r,
        ));
        if reduced {
            self.now = self.target;
        }
    }

    /// A wheel turn of `dy` pixels (positive is toward the user: out).
    pub fn wheel(&mut self, dy: f32, x: f32, y: f32, w: f32, h: f32, reduced: bool) {
        self.zoom_at((-dy * 0.0025).exp(), x, y, w, h, reduced);
    }

    /// A pinch of `delta` (0.1 is 10% closer).
    pub fn pinch(&mut self, delta: f32, x: f32, y: f32, w: f32, h: f32, reduced: bool) {
        self.zoom_at(1.0 + delta, x, y, w, h, reduced);
    }

    pub fn key(&mut self, k: Key, reduced: bool) {
        match k {
            Key::Left => self.target.yaw += KEY_YAW,
            Key::Right => self.target.yaw -= KEY_YAW,
            Key::Up => self.target.pitch = clamp_pitch(self.target.pitch + KEY_PITCH),
            Key::Down => self.target.pitch = clamp_pitch(self.target.pitch - KEY_PITCH),
            Key::In | Key::Out => {
                let f = if k == Key::In {
                    KEY_ZOOM
                } else {
                    1.0 / KEY_ZOOM
                };
                let z1 = (self.target.zoom * f).clamp(ZOOM.0, ZOOM.1);
                let r = z1 / self.target.zoom;
                self.target.zoom = z1;
                self.target.pan = (self.target.pan.0 * r, self.target.pan.1 * r);
            }
        }
        self.spin = (0.0, 0.0);
        if reduced {
            self.now = self.target;
        }
    }

    /// Turns about `point` (floor units) and moves in on it, centred.
    pub fn focus(&mut self, point: [f32; 3], reduced: bool) {
        if !point.iter().all(|v| v.is_finite()) {
            return;
        }
        self.target.pivot = point;
        self.target.centre = (0.0, 0.0);
        self.target.pan = (0.0, 0.0);
        self.target.zoom = self.target.zoom.max(FOCUS_ZOOM).min(ZOOM.1);
        self.spin = (0.0, 0.0);
        if reduced {
            self.now = self.target;
        }
    }

    /// Like [`Camera::focus`], keeping the zoom: for walking the steps.
    pub fn glide_to(&mut self, point: [f32; 3], reduced: bool) {
        if !point.iter().all(|v| v.is_finite()) {
            return;
        }
        self.target.pivot = point;
        self.target.centre = (0.0, 0.0);
        self.target.pan = (0.0, 0.0);
        if reduced {
            self.now = self.target;
        }
    }

    /// Moves `dt` seconds on: the spin, then the ease. Returns whether
    /// the next frame differs.
    pub fn tick(&mut self, dt: f32, reduced: bool) -> bool {
        if reduced {
            self.spin = (0.0, 0.0);
            self.now = self.target;
            return false;
        }
        let dt = dt.clamp(0.0, 0.1);
        if self.drag.is_none() && (self.spin.0 != 0.0 || self.spin.1 != 0.0) {
            self.target.yaw += self.spin.0 * dt;
            self.target.pitch = clamp_pitch(self.target.pitch + self.spin.1 * dt);
            let decay = (-DAMP * dt).exp();
            self.spin = (self.spin.0 * decay, self.spin.1 * decay);
            if self.spin.0.hypot(self.spin.1) < SPIN_STOP {
                self.spin = (0.0, 0.0);
            }
        }
        let k = 1.0 - (-EASE * dt).exp();
        let (n, t) = (&mut self.now, &self.target);
        n.yaw += (t.yaw - n.yaw) * k;
        n.pitch += (t.pitch - n.pitch) * k;
        // Zoom eases in log space so in and out feel the same.
        n.zoom = (n.zoom.ln() + (t.zoom.ln() - n.zoom.ln()) * k).exp();
        n.pan.0 += (t.pan.0 - n.pan.0) * k;
        n.pan.1 += (t.pan.1 - n.pan.1) * k;
        for i in 0..3 {
            n.pivot[i] += (t.pivot[i] - n.pivot[i]) * k;
        }
        n.centre.0 += (t.centre.0 - n.centre.0) * k;
        n.centre.1 += (t.centre.1 - n.centre.1) * k;
        if close(&self.now, &self.target) {
            self.now = self.target;
        }
        // Never let a bad number through to the drawing: back home.
        if !finite(&self.now)
            || !finite(&self.target)
            || !self.spin.0.is_finite()
            || !self.spin.1.is_finite()
        {
            let home = Self::home_pose(self.home_bounds, self.home_view);
            self.now = home;
            self.target = home;
            self.spin = (0.0, 0.0);
            return false;
        }
        self.moving()
    }

    /// Whether the camera is still on its way, or spinning.
    pub fn moving(&self) -> bool {
        self.spin != (0.0, 0.0) || !close(&self.now, &self.target)
    }
}

/// Farthest the view pans, in pixels, either way.
pub const PAN_MAX: f32 = 20_000.0;

fn clamp_pan(p: (f32, f32)) -> (f32, f32) {
    let c = |v: f32| {
        if v.is_finite() {
            v.clamp(-PAN_MAX, PAN_MAX)
        } else {
            0.0
        }
    };
    (c(p.0), c(p.1))
}

/// Bounds a camera can frame: finite, ordered, at least a unit each way.
fn safe_bounds(b: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    if ![b.0, b.1, b.2, b.3].iter().all(|v| v.is_finite()) || b.0 > b.2 || b.1 > b.3 {
        return (-1.0, -1.0, 1.0, 1.0);
    }
    let grow = |lo: f32, hi: f32| {
        if hi - lo < 1.0 {
            let c = (lo + hi) / 2.0;
            (c - 0.5, c + 0.5)
        } else {
            (lo, hi)
        }
    };
    let (x0, x1) = grow(b.0, b.2);
    let (y0, y1) = grow(b.1, b.3);
    (x0, y0, x1, y1)
}

fn finite(p: &Pose) -> bool {
    p.yaw.is_finite()
        && p.pitch.is_finite()
        && p.zoom.is_finite()
        && p.zoom > 0.0
        && p.pan.0.is_finite()
        && p.pan.1.is_finite()
        && p.pivot.iter().all(|v| v.is_finite())
        && p.centre.0.is_finite()
        && p.centre.1.is_finite()
}

fn close(a: &Pose, b: &Pose) -> bool {
    let e = 1e-3;
    (a.yaw - b.yaw).abs() < e
        && (a.pitch - b.pitch).abs() < e
        && (a.zoom - b.zoom).abs() < e
        && (a.pan.0 - b.pan.0).abs() < 0.05
        && (a.pan.1 - b.pan.1).abs() < 0.05
        && (0..3).all(|i| (a.pivot[i] - b.pivot[i]).abs() < e)
        && (a.centre.0 - b.centre.0).abs() < e
        && (a.centre.1 - b.centre.1).abs() < e
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explain3d::{Part, Scene, Shape, Step, part_centre, project};

    fn scene() -> Scene {
        let part = |id: &str, x: f32, y: f32| Part {
            id: id.into(),
            label: id.to_uppercase(),
            shape: Shape::Block,
            at: [x, y, 0.0],
            size: [2.0, 2.0, 1.5],
            ..Part::default()
        };
        Scene {
            id: "x".into(),
            concept: "c".into(),
            summary: String::new(),
            parts: vec![
                part("a", 1.0, 1.0),
                part("b", 8.0, 2.0),
                part("c", 4.0, 8.0),
            ],
            links: vec![],
            steps: vec![Step {
                caption: "a".into(),
                parts: vec!["a".into()],
                ..Step::default()
            }],
            topic: None,
            written_by: "fallback".into(),
            created_at: 0,
        }
    }

    fn cam(s: &Scene) -> Camera {
        Camera::new(project(s, View::default()).bounds)
    }

    fn settle(c: &mut Camera) {
        for _ in 0..600 {
            if !c.tick(1.0 / 60.0, false) {
                break;
            }
        }
        assert!(!c.moving(), "settles");
    }

    /// Where a floor point lands on screen now.
    fn screen(c: &Camera, s: &Scene, p: [f32; 3], w: f32, h: f32) -> (f32, f32) {
        // Project a tiny label part at p and read its anchor.
        let mut s = s.clone();
        s.parts.push(Part {
            id: "probe".into(),
            label: "p".into(),
            shape: Shape::Label,
            at: p,
            ..Part::default()
        });
        let d = project(&s, c.view(None));
        let l = d.labels.last().unwrap();
        c.fit(w, h, 36.0).at(l.at)
    }

    #[test]
    fn the_first_view_is_the_old_fitted_drawing() {
        let s = scene();
        let c = cam(&s);
        let d = project(&s, c.view(None));
        let old = project(&s, View::default());
        assert_eq!(d.faces.len(), old.faces.len());
        let (a, b) = (
            c.fit(800.0, 500.0, 36.0),
            Fit::new(old.bounds, 800.0, 500.0, 36.0),
        );
        let p = old.faces[0].pts[0];
        let (x0, y0) = a.at(d.faces[0].pts[0]);
        let (x1, y1) = b.at(p);
        assert!((x0 - x1).abs() < 0.01 && (y0 - y1).abs() < 0.01);
        assert!(c.at_home());
    }

    #[test]
    fn orbit_turns_all_the_way_round_and_clamps_the_tilt() {
        let s = scene();
        let mut c = cam(&s);
        c.begin_drag(Drag::Orbit, 0.0, 0.0);
        // 2π of turn, dragged in small moves.
        let px_per_turn = std::f32::consts::TAU / ORBIT_PER_PX;
        let steps = 200;
        for i in 1..=steps {
            c.drag_to(-(px_per_turn * i as f32 / steps as f32), 0.0, 1.0 / 60.0);
        }
        assert!(
            (c.now.yaw - std::f32::consts::TAU).abs() < 0.02,
            "{}",
            c.now.yaw
        );
        // Down a long way: the tilt stops at the top-down limit.
        c.drag_to(-px_per_turn, 10_000.0, 1.0 / 60.0);
        assert_eq!(c.now.pitch, pitch_range().1);
        c.drag_to(-px_per_turn, -20_000.0, 1.0 / 60.0);
        assert_eq!(c.now.pitch, pitch_range().0);
        assert!(!c.end_drag(false), "a drag is not a click");
    }

    #[test]
    fn a_press_that_barely_moves_is_a_click() {
        let s = scene();
        let mut c = cam(&s);
        c.begin_drag(Drag::Orbit, 10.0, 10.0);
        assert!(!c.drag_to(12.0, 11.0, 0.016));
        assert!(c.end_drag(false));
        assert!(c.at_home());
    }

    #[test]
    fn inertia_spins_on_then_dies_away_and_not_with_reduced_motion() {
        let s = scene();
        let mut c = cam(&s);
        c.begin_drag(Drag::Orbit, 0.0, 0.0);
        for i in 1..10 {
            c.drag_to(-(i as f32) * 20.0, 0.0, 1.0 / 60.0);
        }
        let at_release = c.now.yaw;
        c.end_drag(false);
        assert!(c.moving());
        c.tick(1.0 / 60.0, false);
        assert!(c.now.yaw > at_release, "keeps turning the same way");
        settle(&mut c);
        let coasted = c.now.yaw - at_release;
        assert!(coasted > 0.05 && coasted < 3.0, "{coasted}");

        let mut r = cam(&s);
        r.begin_drag(Drag::Orbit, 0.0, 0.0);
        for i in 1..10 {
            r.drag_to(-(i as f32) * 20.0, 0.0, 1.0 / 60.0);
        }
        let at = r.now.yaw;
        r.end_drag(true);
        assert!(!r.tick(1.0 / 60.0, true));
        assert_eq!(r.now.yaw, at, "no spin with reduced motion");
    }

    #[test]
    fn wheel_zoom_keeps_the_point_under_the_pointer_and_clamps() {
        let s = scene();
        let mut c = cam(&s);
        let (w, h) = (800.0, 500.0);
        let p = part_centre(&s.parts[1]);
        let (x, y) = screen(&c, &s, p, w, h);
        c.wheel(-200.0, x, y, w, h, true);
        assert!(c.now.zoom > 1.0);
        let (x2, y2) = screen(&c, &s, p, w, h);
        assert!(
            (x - x2).abs() < 0.5 && (y - y2).abs() < 0.5,
            "{x},{y} -> {x2},{y2}"
        );
        for _ in 0..50 {
            c.wheel(-500.0, x, y, w, h, true);
        }
        assert_eq!(c.now.zoom, ZOOM.1);
        for _ in 0..80 {
            c.pinch(-0.5, x, y, w, h, true);
        }
        assert_eq!(c.now.zoom, ZOOM.0);
    }

    #[test]
    fn pan_moves_the_drawing_with_the_hand() {
        let s = scene();
        let mut c = cam(&s);
        let (w, h) = (800.0, 500.0);
        let p = part_centre(&s.parts[0]);
        let before = screen(&c, &s, p, w, h);
        c.begin_drag(Drag::Pan, 100.0, 100.0);
        c.drag_to(160.0, 130.0, 0.016);
        c.end_drag(false);
        let after = screen(&c, &s, p, w, h);
        assert!((after.0 - before.0 - 60.0).abs() < 0.01);
        assert!((after.1 - before.1 - 30.0).abs() < 0.01);
        assert!(!c.moving(), "panning leaves no spin");
    }

    #[test]
    fn focus_centres_a_node_moves_in_and_reset_goes_home() {
        let s = scene();
        let mut c = cam(&s);
        let (w, h) = (800.0, 500.0);
        let p = part_centre(&s.parts[2]);
        c.focus(p, false);
        assert!(c.moving());
        settle(&mut c);
        let (x, y) = screen(&c, &s, p, w, h);
        assert!((x - w / 2.0).abs() < 0.5 && (y - h / 2.0).abs() < 0.5);
        assert!(c.now.zoom >= FOCUS_ZOOM);
        // Orbiting now turns about the node: it stays centred.
        c.key(Key::Left, true);
        let (x, y) = screen(&c, &s, p, w, h);
        assert!((x - w / 2.0).abs() < 0.5 && (y - h / 2.0).abs() < 0.5);
        c.reset(false);
        settle(&mut c);
        assert!(c.at_home());
    }

    #[test]
    fn keys_turn_tilt_and_zoom() {
        let s = scene();
        let mut c = cam(&s);
        c.key(Key::parse("left").unwrap(), true);
        assert!((c.now.yaw - KEY_YAW).abs() < 1e-6);
        c.key(Key::Right, true);
        c.key(Key::Right, true);
        assert!((c.now.yaw + KEY_YAW).abs() < 1e-6);
        c.key(Key::Up, true);
        assert!(c.now.pitch > 0.0);
        c.key(Key::parse("+").unwrap(), true);
        assert!((c.now.zoom - KEY_ZOOM).abs() < 1e-6);
        c.key(Key::parse("-").unwrap(), true);
        assert!((c.now.zoom - 1.0).abs() < 1e-6);
        assert_eq!(Key::parse("x"), None);
        for _ in 0..100 {
            c.key(Key::Down, true);
        }
        assert_eq!(c.now.pitch, pitch_range().0);
    }

    #[test]
    fn eased_moves_arrive_and_reduced_motion_jumps() {
        let s = scene();
        let mut c = cam(&s);
        c.key(Key::Left, false);
        assert_eq!(c.now.yaw, 0.0, "eases, does not jump");
        assert!(c.tick(1.0 / 60.0, false));
        assert!(c.now.yaw > 0.0 && c.now.yaw < KEY_YAW);
        settle(&mut c);
        assert!((c.now.yaw - KEY_YAW).abs() < 1e-6);
        c.focus(part_centre(&s.parts[0]), true);
        assert!(!c.moving(), "reduced motion: already there");
    }

    #[test]
    fn bad_numbers_never_reach_the_drawing() {
        // Degenerate bounds: NaN, reversed, a single point.
        for b in [
            (f32::NAN, 0.0, 1.0, 1.0),
            (5.0, 5.0, 1.0, 1.0),
            (2.0, 2.0, 2.0, 2.0),
            (0.0, 0.0, f32::INFINITY, 1.0),
        ] {
            let c = Camera::new(b);
            let f = c.fit(640.0, 480.0, 36.0);
            assert!(
                f.scale.is_finite() && f.scale > 0.0 && f.dx.is_finite(),
                "{b:?}"
            );
        }
        let mut c = Camera::new((0.0, 0.0, 10.0, 10.0));
        // NaN input is ignored.
        c.focus([f32::NAN, 0.0, 0.0], false);
        c.glide_to([0.0, f32::INFINITY, 0.0], false);
        c.zoom_at(2.0, f32::NAN, 0.0, 100.0, 100.0, false);
        c.begin_drag(Drag::Pan, 0.0, 0.0);
        assert!(!c.drag_to(f32::NAN, 1.0, 0.016));
        // A huge pan is clamped.
        assert!(c.drag_to(1e9, 1e9, 0.016));
        assert!(c.target.pan.0 <= PAN_MAX);
        c.end_drag(false);
        // A pose gone bad (forced here) goes home on the next tick.
        c.target.zoom = f32::NAN;
        assert!(!c.tick(0.016, false));
        assert!(c.at_home());
        assert!(c.fit(640.0, 480.0, 36.0).scale.is_finite());
    }
}
