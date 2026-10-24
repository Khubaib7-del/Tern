//! An Explain in 3D tab's state, free of GPUI so it is tested: the scene,
//! the step on screen and the fade between steps, autoplay, the camera
//! (orbit, zoom, pan, focus, Reset; `tern_agent::explain::camera`), the part under
//! the pointer, and the node picked to read.
//!
//! The scene, its projection, and hit testing come from
//! `tern_agent::explain`; this decides what the next frame shows. Walking
//! the steps (Prev, Next, Play) moves the camera level by level onto each
//! step's nodes.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use tern_agent::explain::camera::{Camera, Drag, Key};
use tern_agent::explain::{
    Drawing, Fit, LEVELS, Scene, View, citations, hit, parts_centre, project,
};

use crate::motion;

/// How long a step's highlight takes to move.
pub const STEP_FADE: Duration = Duration::from_millis(400);
/// How long each step stays up on autoplay.
pub const AUTOPLAY: Duration = Duration::from_millis(3600);
/// Room around the drawing, in pixels.
pub const MARGIN: f32 = 36.0;

/// What the side caption reads: the step on screen, or the node picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    /// The node's label, or `Step 3 of 9`.
    pub title: String,
    /// `Core idea` for an answer's map; `None` for a topic's machine.
    pub level: Option<&'static str>,
    pub text: String,
    /// The passage of the answer it comes from, citations kept.
    pub passage: String,
    /// `This answer · How it works`, `graphs.pdf p. 4`.
    pub cite: Option<String>,
    /// Citations in the passage (`[2]`), in order.
    pub citations: Vec<String>,
}

/// What a key did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyDid {
    Camera,
    Step,
    Play,
    Reset,
    Nothing,
}

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
    pub camera: Camera,
    /// When the pointer last moved, for the drag's speed.
    moved_at: Option<Instant>,
    pub hover: Option<usize>,
    /// The node clicked to read; `None` reads the step.
    pub picked: Option<usize>,
    /// Motion reduced: the camera jumps and nothing fades.
    pub reduced: bool,
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
            camera: Camera::new((0.0, 0.0, 1.0, 1.0)),
            moved_at: None,
            hover: None,
            picked: None,
            reduced: false,
            saved: None,
            bounds: Rc::new(Cell::new([0.0; 4])),
        }
    }

    pub fn ready(&mut self, r: Result<(Scene, Option<String>), String>, now: Instant) {
        self.building = false;
        match r {
            Ok((scene, note)) => {
                self.camera = Camera::new(project(&scene, View::default()).bounds);
                self.scene = Some(scene);
                self.note = note;
                self.error = None;
                self.step = 0;
                self.prev = 0;
                self.picked = None;
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

    /// Whether the scene maps an answer by level (its steps carry
    /// passages), rather than a topic's machine.
    pub fn leveled(&self) -> bool {
        self.scene
            .as_ref()
            .is_some_and(|s| s.steps.iter().any(|st| !st.passage.is_empty()))
    }

    /// Moves to step `i` and glides the camera onto its nodes. Returns
    /// whether it changed.
    pub fn go(&mut self, i: usize, now: Instant) -> bool {
        let n = self.steps();
        if n == 0 || i >= n || i == self.step {
            return false;
        }
        self.prev = self.step;
        self.step = i;
        self.step_at = Some(now);
        self.last_auto = Some(now);
        self.picked = None;
        self.look_at_step();
        true
    }

    /// The camera onto the step's nodes.
    fn look_at_step(&mut self) {
        let Some(s) = &self.scene else {
            return;
        };
        let lit: Vec<usize> = s.lit(self.step).into_iter().collect();
        if lit.is_empty() {
            return;
        }
        let c = parts_centre(s, &lit);
        self.camera.glide_to(c, self.reduced);
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
            self.look_at_step();
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

    /// Where drawing units land in a `w` × `h` area.
    pub fn fit(&self, w: f32, h: f32) -> Fit {
        self.camera.fit(w, h, MARGIN)
    }

    fn part_at(&self, x: f32, y: f32, w: f32, h: f32) -> Option<usize> {
        let s = self.scene.as_ref()?;
        let d = project(s, self.view());
        hit(&d, &self.fit(w, h), x, y)
    }

    /// The pointer moved to `(x, y)` inside a `w` × `h` area (local
    /// pixels): drags the camera while pressed, else finds the part under
    /// it. Returns whether the frame changed.
    pub fn point(&mut self, x: f32, y: f32, w: f32, h: f32, now: Instant) -> bool {
        if w <= 1.0 || h <= 1.0 {
            return false;
        }
        let dt = self
            .moved_at
            .map(|t| now.saturating_duration_since(t).as_secs_f32())
            .unwrap_or(1.0 / 60.0);
        self.moved_at = Some(now);
        if self.camera.dragging() {
            return self.camera.drag_to(x, y, dt);
        }
        let before = self.hover;
        self.hover = self.part_at(x, y, w, h);
        self.hover != before
    }

    /// A press: left orbits, right (or Shift with left) pans.
    pub fn press(&mut self, x: f32, y: f32, pan: bool, now: Instant) {
        self.autoplay = false;
        self.moved_at = Some(now);
        self.camera
            .begin_drag(if pan { Drag::Pan } else { Drag::Orbit }, x, y);
    }

    /// The press ended. A click (it barely moved) on a node reads it and
    /// goes to its step. Returns whether it was a click.
    pub fn release(&mut self, now: Instant) -> bool {
        let click = self.camera.end_drag(self.reduced);
        if click && let Some(part) = self.hover {
            if let Some(step) = self.step_of(part)
                && step != self.step
            {
                self.go(step, now);
            }
            self.picked = Some(part);
        }
        click
    }

    /// A double-click focuses the node under `(x, y)`: the camera turns
    /// about it and moves in.
    pub fn double_click(&mut self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let Some(part) = self.part_at(x, y, w, h) else {
            return false;
        };
        let Some(s) = &self.scene else {
            return false;
        };
        let c = parts_centre(s, &[part]);
        self.camera.focus(c, self.reduced);
        self.picked = Some(part);
        true
    }

    pub fn wheel(&mut self, dy: f32, x: f32, y: f32, w: f32, h: f32) {
        self.camera.wheel(dy, x, y, w, h, self.reduced);
    }

    pub fn pinch(&mut self, delta: f32, x: f32, y: f32, w: f32, h: f32) {
        self.camera.pinch(delta, x, y, w, h, self.reduced);
    }

    pub fn reset_view(&mut self) {
        self.camera.reset(self.reduced);
    }

    /// A key: arrows turn and tilt, + and - zoom, 0 or R resets the view,
    /// Page Up and Down or [ and ] step, Space plays.
    pub fn key(&mut self, name: &str, now: Instant) -> KeyDid {
        if let Some(k) = Key::parse(name) {
            self.camera.key(k, self.reduced);
            return KeyDid::Camera;
        }
        match name {
            "pagedown" | "]" | "n" => {
                self.autoplay = false;
                self.next(now);
                KeyDid::Step
            }
            "pageup" | "[" | "p" => {
                self.autoplay = false;
                self.back(now);
                KeyDid::Step
            }
            "space" => {
                self.toggle_autoplay(now);
                KeyDid::Play
            }
            "0" | "r" | "home" => {
                self.reset_view();
                KeyDid::Reset
            }
            _ => KeyDid::Nothing,
        }
    }

    /// The pointer left.
    pub fn leave(&mut self) {
        self.hover = None;
    }

    /// Moves the camera `dt` seconds on. Returns whether it still moves.
    pub fn settle(&mut self, dt: f32, reduced: bool) -> bool {
        self.reduced = reduced;
        self.camera.tick(dt, reduced)
    }

    pub fn view(&self) -> View {
        self.camera.view(self.hover)
    }

    /// The drawing for this frame.
    pub fn drawing(&self) -> Option<Drawing> {
        self.scene.as_ref().map(|s| project(s, self.view()))
    }

    /// Whether the next frame differs: a step fading, or the camera on
    /// its way.
    pub fn moving(&self, now: Instant, reduced: bool) -> bool {
        if reduced {
            return false;
        }
        let fading = self
            .step_at
            .is_some_and(|t| now.saturating_duration_since(t) < STEP_FADE);
        fading || self.camera.moving()
    }

    /// What the side caption reads now.
    pub fn reading(&self) -> Option<Reading> {
        let s = self.scene.as_ref()?;
        let leveled = self.leveled();
        let level_name = |l: u8| leveled.then(|| LEVELS[(l as usize).min(LEVELS.len() - 1)]);
        let step = s.steps.get(self.step.min(s.steps.len().saturating_sub(1)));
        if let Some(part) = self.picked.and_then(|i| s.parts.get(i)) {
            // A node: its explanation, and the passage of the step it is in.
            let st = self
                .picked
                .and_then(|i| self.step_of(i))
                .and_then(|i| s.steps.get(i));
            let passage = st.map(|x| x.passage.clone()).unwrap_or_default();
            return Some(Reading {
                title: part.label.clone(),
                level: level_name(part.level),
                text: if part.note.is_empty() {
                    st.map(|x| x.caption.clone()).unwrap_or_default()
                } else {
                    part.note.clone()
                },
                citations: citations(&passage),
                passage,
                cite: st.and_then(|x| x.cite()),
            });
        }
        let st = step?;
        Some(Reading {
            title: format!("Step {} of {}", self.step + 1, s.steps.len()),
            level: level_name(st.level),
            text: st.caption.clone(),
            citations: citations(&st.passage),
            passage: st.passage.clone(),
            cite: st.cite(),
        })
    }

    /// The levels the steps walk through, in order, with the first step
    /// of each: for the level strip over the step bar.
    pub fn level_marks(&self) -> Vec<(&'static str, usize)> {
        let Some(s) = self.scene.as_ref().filter(|_| self.leveled()) else {
            return Vec::new();
        };
        let mut out: Vec<(&'static str, usize)> = Vec::new();
        for (i, st) in s.steps.iter().enumerate() {
            let name = LEVELS[(st.level as usize).min(LEVELS.len() - 1)];
            if out.last().is_none_or(|(n, _)| *n != name) {
                out.push((name, i));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_agent::explain::{FaceKind, Part, Shape, Step};

    fn scene() -> Scene {
        let part = |id: &str, x: f32, level: u8| Part {
            id: id.into(),
            label: id.to_uppercase(),
            shape: Shape::Block,
            at: [x, 4.0, 0.0],
            size: [2.0, 2.0, 1.5],
            note: format!("{id} explained"),
            level,
        };
        let step = |c: &str, id: &str, level: u8| Step {
            caption: c.into(),
            parts: vec![id.into()],
            anchor: None,
            level,
            passage: format!("The passage for {id} [{}].", level + 1),
        };
        Scene {
            id: "x1".into(),
            concept: "Pipeline".into(),
            summary: String::new(),
            parts: vec![part("a", 1.0, 0), part("b", 5.0, 1), part("c", 9.0, 1)],
            links: vec![],
            steps: vec![step("A", "a", 0), step("B", "b", 1), step("C", "c", 1)],
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

    /// The screen centre of part `i`'s top face.
    fn top_of(s: &ExplainState, i: usize, w: f32, h: f32) -> (f32, f32) {
        let d = s.drawing().unwrap();
        let f = s.fit(w, h);
        let top = d
            .faces
            .iter()
            .find(|x| x.part == i && x.kind == FaceKind::Top)
            .unwrap();
        let n = top.pts.len() as f32;
        f.at((
            top.pts.iter().map(|p| p.0).sum::<f32>() / n,
            top.pts.iter().map(|p| p.1).sum::<f32>() / n,
        ))
    }

    fn settle(s: &mut ExplainState) {
        for _ in 0..600 {
            if !s.settle(1.0 / 60.0, false) {
                break;
            }
        }
    }

    #[test]
    fn steps_move_fade_and_walk_the_camera_level_by_level() {
        let (mut s, t0) = ready();
        assert_eq!(s.title(), "3D · Pipeline");
        assert!(s.leveled());
        assert_eq!(s.level_marks(), vec![("Foundations", 0), ("Core idea", 1)]);
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
        assert!(s.moving(mid, false));
        assert!(!s.moving(mid, true));
        assert!(!s.go(9, t0));
        assert_eq!(s.step_of(2), Some(2));
        // The camera glides onto step 2's node: it ends up centred.
        settle(&mut s);
        let (w, h) = (800.0, 500.0);
        let (x, y) = top_of(&s, 1, w, h);
        assert!(
            (x - w / 2.0).abs() < 2.0 && (y - h / 2.0).abs() < 2.0,
            "{x},{y}"
        );
        let r = s.reading().unwrap();
        assert_eq!(r.title, "Step 2 of 3");
        assert_eq!(r.level, Some("Core idea"));
        assert_eq!(r.text, "B");
        assert_eq!(r.citations, vec!["[2]".to_owned()]);
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
        s.toggle_autoplay(t0 + AUTOPLAY * 3);
        assert_eq!(s.step, 0);
        assert_eq!(s.key("space", t0), KeyDid::Play);
        assert!(!s.autoplay);
    }

    #[test]
    fn hover_click_to_read_drag_to_orbit_and_reset() {
        let (mut s, t0) = ready();
        let (w, h) = (800.0, 500.0);
        let (x, y) = top_of(&s, 2, w, h);
        s.point(x, y, w, h, t0);
        assert_eq!(s.hover, Some(2));
        assert_eq!(s.view().lift, Some(2));
        // A click reads the node and goes to its step.
        s.press(x, y, false, t0);
        assert!(s.release(t0));
        assert_eq!(s.step, 2);
        assert_eq!(s.picked, Some(2));
        let r = s.reading().unwrap();
        assert_eq!(r.title, "C");
        assert_eq!(r.text, "c explained");
        assert!(r.passage.contains("passage for c"));
        // A drag orbits: a full turn is possible, and it is not a click.
        s.press(100.0, 100.0, false, t0);
        for i in 1..=40 {
            s.point(
                100.0 - i as f32 * 30.0,
                100.0,
                w,
                h,
                t0 + Duration::from_millis(16 * i),
            );
        }
        assert!(!s.release(t0));
        assert!(s.camera.now.yaw > 1.0);
        assert!(s.moving(t0 + Duration::from_secs(9), false), "it spins on");
        s.reset_view();
        settle(&mut s);
        assert!(s.camera.at_home());
        // Reduced motion: no spin after a drag, keys jump.
        s.settle(0.0, true);
        s.press(100.0, 100.0, false, t0);
        for i in 1..=10 {
            s.point(
                100.0 - i as f32 * 30.0,
                100.0,
                w,
                h,
                t0 + Duration::from_millis(16 * i),
            );
        }
        s.release(t0);
        assert!(!s.camera.moving());
        assert_eq!(s.key("left", t0), KeyDid::Camera);
        assert!(!s.camera.moving());
        assert_eq!(s.key("0", t0), KeyDid::Reset);
        assert!(s.camera.at_home());
        assert_eq!(s.key("pagedown", t0), KeyDid::Step);
        assert_eq!(s.key("q", t0), KeyDid::Nothing);
        s.leave();
        assert_eq!(s.hover, None);
    }

    #[test]
    fn double_click_focuses_wheel_zooms_and_pan_drags() {
        let (mut s, t0) = ready();
        let (w, h) = (800.0, 500.0);
        let (x, y) = top_of(&s, 0, w, h);
        assert!(s.double_click(x, y, w, h));
        settle(&mut s);
        let (cx, cy) = top_of(&s, 0, w, h);
        assert!((cx - w / 2.0).abs() < 2.0 && (cy - h / 2.0).abs() < 2.0);
        assert!(s.camera.now.zoom > 1.5);
        assert!(!s.double_click(-100.0, -100.0, w, h));
        let z = s.camera.target.zoom;
        s.wheel(-120.0, 400.0, 250.0, w, h);
        assert!(s.camera.target.zoom > z);
        s.pinch(-0.2, 400.0, 250.0, w, h);
        s.press(10.0, 10.0, true, t0);
        s.point(60.0, 10.0, w, h, t0 + Duration::from_millis(16));
        s.release(t0);
        assert!((s.camera.target.pan.0 - 50.0).abs() < 1.0);
        let mut e = ExplainState::new("x");
        e.ready(Err("no".into()), Instant::now());
        assert_eq!(e.error.as_deref(), Some("no"));
        assert!(!e.building);
        assert!(e.reading().is_none());
    }
}
