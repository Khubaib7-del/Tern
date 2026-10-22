//! An Explain in 3D tab's state, free of GPUI so it is tested: the scene,
//! the step on screen and the fade between steps, autoplay, the pointer's
//! tilt (eased toward where the pointer is), and the part under it.
//!
//! The scene, its projection, and hit testing come from
//! `tern_agent::explain`; this decides what the next frame shows.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tern_agent::explain::{Drawing, Fit, Scene, View, hit, project};

use crate::motion;

/// How long a step's highlight takes to move.
pub const STEP_FADE: Duration = Duration::from_millis(400);
/// How long each step stays up on autoplay.
pub const AUTOPLAY: Duration = Duration::from_millis(3600);
/// Room around the drawing, in pixels.
pub const MARGIN: f32 = 36.0;

/// One explainer tab.
#[derive(Debug)]
pub struct ExplainState {
    /// What was asked, shown while the scene is being built.
    pub asked: String,
    pub scene: Option<Scene>,
    pub building: bool,
    /// A note from building (the model was not used, and why).
    pub note: Option<String>,
    pub error: Option<String>,
    pub step: usize,
    prev: usize,
    step_at: Option<Instant>,
    pub autoplay: bool,
    last_auto: Option<Instant>,
    /// Where the pointer is, each -1 to 1 from the centre.
    pointer: (f32, f32),
    /// The tilt drawn, easing toward the pointer.
    tilt: (f32, f32),
    pub hover: Option<usize>,
    /// Where the scene was last saved, said plainly.
    pub saved: Option<String>,
    /// The drawing area as last laid out: x, y, width, height in window
    /// pixels. Written while painting, read by the pointer handlers.
    pub bounds: Rc<Cell<[f32; 4]>>,
}

impl ExplainState {
    pub fn new(asked: &str) -> Self {
        Self {
            asked: asked.to_owned(),
            scene: None,
            building: true,
            note: None,
            error: None,
            step: 0,
            prev: 0,
            step_at: None,
            autoplay: false,
            last_auto: None,
            pointer: (0.0, 0.0),
            tilt: (0.0, 0.0),
            hover: None,
            saved: None,
            bounds: Rc::new(Cell::new([0.0; 4])),
        }
    }

    pub fn ready(&mut self, r: Result<(Scene, Option<String>), String>, now: Instant) {
        self.building = false;
        match r {
            Ok((scene, note)) => {
                self.scene = Some(scene);
                self.note = note;
                self.error = None;
                self.step = 0;
                self.prev = 0;
                self.step_at = Some(now);
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// The tab's title.
    pub fn title(&self) -> String {
        let c = self
            .scene
            .as_ref()
            .map(|s| s.concept.clone())
            .unwrap_or_else(|| self.asked.clone());
        format!("3D · {}", tern_agent::study::clip(&c, 28))
    }

    pub fn steps(&self) -> usize {
        self.scene.as_ref().map_or(0, |s| s.steps.len())
    }

    /// Moves to step `i`. Returns whether it changed.
    pub fn go(&mut self, i: usize, now: Instant) -> bool {
        let n = self.steps();
        if n == 0 || i >= n || i == self.step {
            return false;
        }
        self.prev = self.step;
        self.step = i;
        self.step_at = Some(now);
        self.last_auto = Some(now);
        true
    }

    pub fn next(&mut self, now: Instant) -> bool {
        self.go(self.step + 1, now)
    }

    pub fn back(&mut self, now: Instant) -> bool {
        self.step > 0 && self.go(self.step - 1, now)
    }

    /// Starts or stops autoplay; starting on the last step goes back to
    /// the first.
    pub fn toggle_autoplay(&mut self, now: Instant) {
        self.autoplay = !self.autoplay;
        if self.autoplay {
            if self.step + 1 >= self.steps() {
                self.go(0, now);
            }
            self.last_auto = Some(now);
        }
    }

    /// Advances autoplay when its step has been up long enough. Stops on
    /// the last step. Returns whether the step changed.
    pub fn tick_autoplay(&mut self, now: Instant) -> bool {
        if !self.autoplay {
            return false;
        }
        let due = self
            .last_auto
            .is_none_or(|t| now.saturating_duration_since(t) >= AUTOPLAY);
        if !due {
            return false;
        }
        let moved = self.next(now);
        if self.step + 1 >= self.steps() {
            self.autoplay = false;
        }
        moved
    }

    /// The step a part is first lit in, for clicking a part.
    pub fn step_of(&self, part: usize) -> Option<usize> {
        let s = self.scene.as_ref()?;
        (0..s.steps.len()).find(|&i| s.lit(i).contains(&part))
    }

    /// How lit each part is (0 to 1): the step's parts fade in as the last
    /// step's fade out, on the motion curve.
    pub fn lit(&self, part: usize, now: Instant, reduced: bool) -> f32 {
        let Some(s) = &self.scene else {
            return 0.0;
        };
        let cur: BTreeSet<usize> = s.lit(self.step);
        let old: BTreeSet<usize> = s.lit(self.prev);
        let t = match self.step_at {
            Some(at) if !reduced => motion::progress(at, STEP_FADE, now),
            _ => 1.0,
        };
        match (cur.contains(&part), old.contains(&part)) {
            (true, true) => 1.0,
            (true, false) => t,
            (false, true) => 1.0 - t,
            (false, false) => 0.0,
        }
    }

    /// The pointer moved to `(x, y)` inside a `w` × `h` area (local
    /// pixels). Sets the tilt target and the part under it.
    pub fn point(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if w <= 1.0 || h <= 1.0 {
            return;
        }
        self.pointer = (
            ((x / w) * 2.0 - 1.0).clamp(-1.0, 1.0),
            ((y / h) * 2.0 - 1.0).clamp(-1.0, 1.0),
        );
        self.hover = self.scene.as_ref().and_then(|s| {
            let d = project(s, self.view());
            hit(&d, &fit(&d, w, h), x, y)
        });
    }

    /// The pointer left: the machine settles back square.
    pub fn leave(&mut self) {
        self.pointer = (0.0, 0.0);
        self.hover = None;
    }

    /// Eases the tilt toward the pointer by `dt` seconds. With reduced
    /// motion the drawing never tilts. Returns whether it is still moving.
    pub fn settle(&mut self, dt: f32, reduced: bool) -> bool {
        if reduced {
            self.tilt = (0.0, 0.0);
            return false;
        }
        let k = (dt * 9.0).min(1.0);
        self.tilt.0 += (self.pointer.0 - self.tilt.0) * k;
        self.tilt.1 += (self.pointer.1 - self.tilt.1) * k;
        (self.pointer.0 - self.tilt.0).abs() > 0.002 || (self.pointer.1 - self.tilt.1).abs() > 0.002
    }

    pub fn view(&self) -> View {
        View {
            lift: self.hover,
            ..View::tilted(self.tilt.0, self.tilt.1)
        }
    }

    /// The drawing for this frame.
    pub fn drawing(&self) -> Option<Drawing> {
        self.scene.as_ref().map(|s| project(s, self.view()))
    }

    /// Whether the next frame differs: a step fading, or the tilt easing.
    pub fn moving(&self, now: Instant, reduced: bool) -> bool {
        if reduced {
            return false;
        }
        let fading = self
            .step_at
            .is_some_and(|t| now.saturating_duration_since(t) < STEP_FADE);
        let tilting = (self.pointer.0 - self.tilt.0).abs() > 0.002
            || (self.pointer.1 - self.tilt.1).abs() > 0.002;
        fading || tilting
    }
}

/// The drawing fitted into `w` × `h`.
pub fn fit(d: &Drawing, w: f32, h: f32) -> Fit {
    Fit::new(d.bounds, w, h, MARGIN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_agent::explain::{Part, Shape, Step};

    fn scene() -> Scene {
        let part = |id: &str, x: f32| Part {
            id: id.into(),
            label: id.to_uppercase(),
            shape: Shape::Block,
            at: [x, 4.0, 0.0],
            size: [2.0, 2.0, 1.5],
            note: String::new(),
        };
        Scene {
            id: "x1".into(),
            concept: "Pipeline".into(),
            summary: String::new(),
            parts: vec![part("a", 1.0), part("b", 5.0), part("c", 9.0)],
            links: vec![],
            steps: vec![
                Step {
                    caption: "A".into(),
                    parts: vec!["a".into()],
                    anchor: None,
                },
                Step {
                    caption: "B".into(),
                    parts: vec!["b".into()],
                    anchor: None,
                },
                Step {
                    caption: "C".into(),
                    parts: vec!["c".into()],
                    anchor: None,
                },
            ],
            topic: None,
            written_by: "fallback".into(),
            created_at: 0,
        }
    }

    fn ready() -> (ExplainState, Instant) {
        let t0 = Instant::now();
        let mut s = ExplainState::new("pipeline");
        assert_eq!(s.title(), "3D · pipeline");
        s.ready(Ok((scene(), None)), t0);
        (s, t0)
    }

    #[test]
    fn steps_move_and_fade_on_the_curve() {
        let (mut s, t0) = ready();
        assert_eq!(s.title(), "3D · Pipeline");
        assert!(!s.back(t0));
        assert!(s.next(t0));
        assert_eq!(s.step, 1);
        let mid = t0 + STEP_FADE / 4;
        let in_b = s.lit(1, mid, false);
        let out_a = s.lit(0, mid, false);
        assert!(in_b > 0.5 && in_b < 1.0, "the curve front-loads: {in_b}");
        assert!((in_b + out_a - 1.0).abs() < 1e-4);
        assert_eq!(s.lit(1, mid, true), 1.0, "reduced motion: no fade");
        assert_eq!(s.lit(2, mid, false), 0.0);
        assert_eq!(s.lit(1, t0 + STEP_FADE, false), 1.0);
        assert!(s.moving(mid, false));
        assert!(!s.moving(mid, true));
        assert!(!s.go(9, t0));
        assert_eq!(s.step_of(2), Some(2));
    }

    #[test]
    fn autoplay_walks_and_stops_at_the_end() {
        let (mut s, t0) = ready();
        s.toggle_autoplay(t0);
        assert!(s.autoplay);
        assert!(!s.tick_autoplay(t0 + AUTOPLAY / 2));
        assert!(s.tick_autoplay(t0 + AUTOPLAY));
        assert_eq!(s.step, 1);
        assert!(s.tick_autoplay(t0 + AUTOPLAY * 2));
        assert_eq!(s.step, 2);
        assert!(!s.autoplay, "stops on the last step");
        // Starting again from the end begins at the first step.
        s.toggle_autoplay(t0 + AUTOPLAY * 3);
        assert_eq!(s.step, 0);
    }

    #[test]
    fn pointer_tilts_hovers_and_settles() {
        let (mut s, _) = ready();
        let (w, h) = (800.0, 500.0);
        // Find a point on part b's top in this layout.
        let d = s.drawing().unwrap();
        let f = fit(&d, w, h);
        let top = d
            .faces
            .iter()
            .find(|x| x.part == 1 && x.kind == tern_agent::explain::FaceKind::Top)
            .unwrap();
        let n = top.pts.len() as f32;
        let c = (
            top.pts.iter().map(|p| p.0).sum::<f32>() / n,
            top.pts.iter().map(|p| p.1).sum::<f32>() / n,
        );
        let (x, y) = f.at(c);
        s.point(x, y, w, h);
        assert_eq!(s.hover, Some(1));
        assert_eq!(s.view().lift, Some(1));
        assert!(s.settle(1.0 / 60.0, false), "easing toward the pointer");
        for _ in 0..240 {
            s.settle(1.0 / 60.0, false);
        }
        assert!(!s.settle(1.0 / 60.0, false));
        assert!(s.view().yaw.abs() > 0.0);
        s.leave();
        assert_eq!(s.hover, None);
        assert!(!s.settle(0.1, true));
        assert_eq!(s.view().yaw, 0.0, "reduced motion keeps it square");
        let mut e = ExplainState::new("x");
        e.ready(Err("no".into()), Instant::now());
        assert_eq!(e.error.as_deref(), Some("no"));
        assert!(!e.building);
    }
}
