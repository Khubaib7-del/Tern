//! An Explain in 3D tab's state, free of GPUI so it is tested: the scene,
//! the step on screen and the fade between steps, autoplay, the camera
//! (orbit, zoom, pan, focus, Reset; `tern_agent::explain::camera`), the part under
//! the pointer, and the node picked to read.
//!
//! The scene, its projection, and hit testing come from
//! `tern_agent::explain`; this decides what the next frame shows. Walking
//! the steps (Prev, Next, Play) moves the camera level by level onto each
//! step's nodes.
//!
//! Camera changes stay cheap. Labels are laid out (`labels::place`) once
//! for where the camera is going: right away when the step, the pointer's
//! node or the stage size changes, and [`RELAYOUT_AFTER`] after the camera's
//! target last moved for a drag, a wheel turn or a pinch. In between, every
//! frame carries the last layout along with its nodes (each label moved by
//! as much as its node moved), so a glide, a zoom or an orbit never lays
//! labels out per frame.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use tern_agent::explain::camera::{Camera, Drag, Key};
use tern_agent::explain::labels::{self, LabelIn, Metrics, Placed, Rank};
use tern_agent::explain::{
    Drawing, Fit, LEVELED_PITCH, LEVELED_YAW, LEVELS, Scene, Shape, View, citations, hit,
    parts_centre, project, sanitize,
};

use crate::motion;

/// How long a step's highlight takes to move.
pub const STEP_FADE: Duration = Duration::from_millis(400);
/// How long each step stays up on autoplay.
pub const AUTOPLAY: Duration = Duration::from_millis(3600);
/// Room around the drawing, in pixels.
pub const MARGIN: f32 = 36.0;
/// How long the camera's target must stay put (after a drag, a wheel turn
/// or a pinch) before labels are laid out again for it.
pub const RELAYOUT_AFTER: Duration = Duration::from_millis(150);
/// How strongly a link is drawn when it is not about the step on screen:
/// nearly invisible, so the step's own links read.
pub const LINK_FADED: f32 = 0.07;
/// How strongly a link joining two consecutive steps is drawn.
pub const LINK_FLOW: f32 = 0.3;

/// The last label layout: where the camera was going when it was made, and
/// where each part's label anchor was then (stage pixels), so later frames
/// move each label with its node instead of laying them out again.
#[derive(Debug)]
struct Solve {
    /// Step, pointer node, picked node and stage size.
    structure: u64,
    /// The camera target it was laid out for.
    pose: u64,
    anchors: Vec<Option<(f32, f32)>>,
    labels: Rc<Vec<Placed>>,
    font: f32,
    /// Counts layouts, so a frame made from a new one is a new frame.
    id: u64,
}

fn hash_of(f: impl FnOnce(&mut std::collections::hash_map::DefaultHasher)) -> u64 {
    let mut k = std::collections::hash_map::DefaultHasher::new();
    f(&mut k);
    k.finish()
}

/// One frame's drawing and placed labels, kept until the camera, the
/// stage size, the step, or the pointer's node changes: a still scene
/// costs nothing to redraw, and labels are laid out once, not every frame.
#[derive(Debug)]
pub struct Frame {
    key: u64,
    pub drawing: Rc<Drawing>,
    pub fit: Fit,
    /// The stage size it was laid out for.
    pub size: (f32, f32),
    pub labels: Rc<Vec<Placed>>,
    /// Label text size, pixels.
    pub font: f32,
    /// How strongly each of `drawing.links` is drawn (1 for the step's
    /// links, [`LINK_FADED`] for the rest).
    pub links: Rc<Vec<f32>>,
    /// The camera is on its way: the painter leaves out what only matters
    /// when still (dashes, seam strokes).
    pub moving: bool,
}

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
    /// The last frame's drawing and labels ([`Frame`]).
    frame: RefCell<Option<Rc<Frame>>>,
    /// The last label layout ([`Solve`]).
    solve: RefCell<Option<Solve>>,
    /// The camera target last seen, and since when.
    target_seen: Cell<(u64, Option<Instant>)>,
    /// Label layouts made so far, for tests and the slow-frame log.
    pub solves: Cell<u64>,
    /// Set when drawing the scene failed; the tab shows an error card
    /// instead of drawing again.
    pub paint_failed: Rc<Cell<bool>>,
    /// When the build in progress started, for the loading line.
    building_since: Option<Instant>,
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
            frame: RefCell::new(None),
            solve: RefCell::new(None),
            target_seen: Cell::new((0, None)),
            solves: Cell::new(0),
            paint_failed: Rc::new(Cell::new(false)),
            building_since: Some(Instant::now()),
        }
    }

    /// How long the scene has been building (zero once built).
    pub fn building_for(&self, now: Instant) -> Duration {
        match self.building_since {
            Some(t) if self.building => now.saturating_duration_since(t),
            _ => Duration::ZERO,
        }
    }

    /// Building again (Try again): back to the loading state.
    pub fn rebuild(&mut self) {
        self.building = true;
        self.building_since = Some(Instant::now());
        self.error = None;
        self.note = None;
        self.scene = None;
        self.paint_failed.set(false);
        self.forget_frames();
    }

    fn forget_frames(&self) {
        self.frame.replace(None);
        self.solve.replace(None);
        self.target_seen.set((0, None));
    }

    /// The first view: an answer's map is turned so its levels are rows
    /// across the screen; a topic's machine keeps the isometric corner.
    fn home_view(scene: &Scene) -> (f32, f32) {
        if scene.steps.iter().any(|st| !st.passage.is_empty()) {
            (LEVELED_YAW, LEVELED_PITCH)
        } else {
            (0.0, 0.0)
        }
    }

    /// A camera framed on the figure itself (zoom to fit), not the floor.
    fn framed(scene: &Scene) -> Camera {
        let (yaw, pitch) = Self::home_view(scene);
        let d = project(
            scene,
            View {
                yaw,
                pitch,
                ..View::default()
            },
        );
        Camera::framed(d.figure_bounds().unwrap_or(d.bounds), yaw, pitch)
    }

    pub fn ready(&mut self, r: Result<(Scene, Option<String>), String>, now: Instant) {
        self.building = false;
        self.forget_frames();
        self.paint_failed.set(false);
        match r {
            Ok((mut scene, note)) => {
                sanitize(&mut scene);
                self.camera = Self::framed(&scene);
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
        fading || self.camera.moving() || self.relayout_pending()
    }

    /// Labels are still laid out for an older camera target: one more
    /// layout is due once the target has stayed put.
    pub fn relayout_pending(&self) -> bool {
        let Some(s) = self.solve.borrow().as_ref().map(|s| s.pose) else {
            return false;
        };
        s != self.target_key()
    }

    fn target_key(&self) -> u64 {
        let t = &self.camera.target;
        hash_of(|k| {
            for v in [
                t.yaw, t.pitch, t.zoom, t.pan.0, t.pan.1, t.centre.0, t.centre.1,
            ]
            .into_iter()
            .chain(t.pivot)
            {
                v.to_bits().hash(k);
            }
        })
    }

    /// How lit every part is now ([`ExplainState::lit`] for all of them,
    /// the step's sets worked out once).
    pub fn lit_all(&self, now: Instant, reduced: bool) -> Vec<f32> {
        let Some(s) = &self.scene else {
            return Vec::new();
        };
        let cur = s.lit(self.step);
        let old = s.lit(self.prev);
        let t = match self.step_at {
            Some(at) if !reduced => motion::progress(at, STEP_FADE, now),
            _ => 1.0,
        };
        (0..s.parts.len())
            .map(|p| match (cur.contains(&p), old.contains(&p)) {
                (true, true) => 1.0,
                (true, false) => t,
                (false, true) => 1.0 - t,
                (false, false) => 0.0,
            })
            .collect()
    }

    /// How strongly the link from part `from` to part `to` is drawn: in
    /// full when it touches the step's parts; lightly ([`LINK_FLOW`]) when
    /// it joins two consecutive steps (the walk from one step to the
    /// next); nearly invisible ([`LINK_FADED`]) otherwise, so a dense map
    /// does not read as a tangle.
    pub fn link_weight(&self, from: usize, to: usize) -> f32 {
        let Some(s) = &self.scene else {
            return LINK_FADED;
        };
        let cur = s.lit(self.step);
        if cur.contains(&from) || cur.contains(&to) {
            return 1.0;
        }
        let first = |p: usize| (0..s.steps.len()).find(|&i| s.lit(i).contains(&p));
        match (first(from), first(to)) {
            (Some(a), Some(b)) if a.abs_diff(b) == 1 => LINK_FLOW,
            _ => LINK_FADED,
        }
    }

    /// The step number (from 1) each part is first lit in: the short
    /// label it shows when its full label is not wanted.
    fn step_numbers(&self) -> Vec<Option<usize>> {
        let Some(s) = &self.scene else {
            return Vec::new();
        };
        let mut out = vec![None; s.parts.len()];
        for i in 0..s.steps.len() {
            for p in s.lit(i) {
                if let Some(slot) = out.get_mut(p)
                    && slot.is_none()
                {
                    *slot = Some(i + 1);
                }
            }
        }
        out
    }

    /// How much each part's label matters now: the step's parts, the one
    /// under the pointer and the one picked are the focus; parts linked to
    /// them or in the step's level are near; the rest show numbers.
    pub fn ranks(&self) -> Vec<Rank> {
        let Some(s) = &self.scene else {
            return Vec::new();
        };
        let cur = s.lit(self.step);
        let focus = |i: usize| cur.contains(&i) || self.hover == Some(i) || self.picked == Some(i);
        let level = s.steps.get(self.step).map(|st| st.level);
        // A small machine shows every label in full when it fits.
        let small = s.parts.len() <= 8;
        (0..s.parts.len())
            .map(|i| {
                if focus(i) {
                    return Rank::Focus;
                }
                let id = &s.parts[i].id;
                let linked = s.links.iter().any(|l| {
                    (l.from == *id && s.part_index(&l.to).is_some_and(focus))
                        || (l.to == *id && s.part_index(&l.from).is_some_and(focus))
                });
                let same_level = self.leveled()
                    && level == Some(s.parts[i].level)
                    && s.parts[i].shape != Shape::Label;
                if small || linked || same_level {
                    Rank::Near
                } else {
                    Rank::Rest
                }
            })
            .collect()
    }

    /// This frame's drawing and labels for a `w` × `h` stage, from the
    /// cache when nothing that moves them changed. `None` before a scene.
    pub fn frame(&self, w: f32, h: f32) -> Option<Rc<Frame>> {
        self.frame_at(w, h, Instant::now())
    }

    /// [`ExplainState::frame`] at `now`. Labels are laid out at most once
    /// per step, pointer node or size change, and once per camera target
    /// that has stayed put for [`RELAYOUT_AFTER`]; otherwise the last
    /// layout moves with its nodes.
    pub fn frame_at(&self, w: f32, h: f32, now: Instant) -> Option<Rc<Frame>> {
        let s = self.scene.as_ref()?;
        let structure = hash_of(|k| {
            (self.step, self.hover, self.picked).hash(k);
            w.to_bits().hash(k);
            h.to_bits().hash(k);
        });
        // When did the camera's target last move?
        let target = self.target_key();
        let (seen, since) = self.target_seen.get();
        let since = if seen == target {
            since.unwrap_or(now)
        } else {
            self.target_seen.set((target, Some(now)));
            now
        };
        let due = match self.solve.borrow().as_ref() {
            None => true,
            Some(old) if old.structure != structure => true,
            Some(old) => {
                old.pose != target && now.saturating_duration_since(since) >= RELAYOUT_AFTER
            }
        };
        if due {
            self.lay_out_labels(s, w, h, structure, target);
        }
        let solve = self.solve.borrow();
        let solve = solve.as_ref()?;
        let view = self.view();
        let fit = self.fit(w, h);
        let moving = self.camera.moving();
        let key = hash_of(|k| {
            for v in [view.yaw, view.pitch, fit.scale, fit.dx, fit.dy, w, h]
                .into_iter()
                .chain(view.pivot.unwrap_or_default())
            {
                v.to_bits().hash(k);
            }
            (
                view.lift,
                self.step,
                self.hover,
                self.picked,
                solve.id,
                moving,
            )
                .hash(k);
        });
        if let Some(f) = self.frame.borrow().as_ref().filter(|f| f.key == key) {
            return Some(f.clone());
        }
        let drawing = project(s, view);
        // Each label moves by as much as its node moved since the layout.
        let mut now_at: Vec<Option<(f32, f32)>> = vec![None; s.parts.len()];
        for l in &drawing.labels {
            if let Some(slot) = now_at.get_mut(l.part) {
                *slot = Some(fit.at(l.at));
            }
        }
        let labels: Vec<Placed> = solve
            .labels
            .iter()
            .filter_map(|l| {
                let a0 = solve.anchors.get(l.part).copied().flatten()?;
                let a1 = now_at.get(l.part).copied().flatten()?;
                let (dx, dy) = (a1.0 - a0.0, a1.1 - a0.1);
                let p = Placed {
                    x: l.x + dx,
                    y: l.y + dy,
                    leader: l.leader.map(|(_, z)| (a1, (z.0 + dx, z.1 + dy))),
                    ..l.clone()
                };
                let ok = [p.x, p.y].iter().all(|v| v.is_finite() && v.abs() < 1e5);
                ok.then_some(p)
            })
            .collect();
        let links = drawing
            .links
            .iter()
            .map(|l| self.link_weight(l.from, l.to))
            .collect();
        let f = Rc::new(Frame {
            key,
            drawing: Rc::new(drawing),
            fit,
            size: (w, h),
            labels: Rc::new(labels),
            font: solve.font,
            links: Rc::new(links),
            moving,
        });
        self.frame.replace(Some(f.clone()));
        Some(f)
    }

    /// Lays the labels out once, for where the camera is going.
    fn lay_out_labels(&self, s: &Scene, w: f32, h: f32, structure: u64, pose: u64) {
        let view = self.camera.target_view(self.hover);
        let fit = self.camera.target_fit(w, h, MARGIN);
        let drawing = project(s, view);
        // Half-pixel steps, so the text is not re-rasterised at every zoom.
        let font = (labels::label_size(self.camera.target.zoom) * 2.0).round() / 2.0;
        let ranks = self.ranks();
        let nums = self.step_numbers();
        let mut anchors: Vec<Option<(f32, f32)>> = vec![None; s.parts.len()];
        let ins: Vec<LabelIn> = drawing
            .labels
            .iter()
            .filter_map(|l| {
                let part = s.parts.get(l.part)?;
                let anchor = fit.at(l.at);
                if let Some(slot) = anchors.get_mut(l.part) {
                    *slot = Some(anchor);
                }
                let short = match (part.shape, nums.get(l.part).copied().flatten()) {
                    (Shape::Label, _) | (_, None) => String::new(),
                    (_, Some(n)) => n.to_string(),
                };
                Some(LabelIn {
                    part: l.part,
                    anchor,
                    full: l.text.clone(),
                    short,
                    rank: ranks.get(l.part).copied().unwrap_or(Rank::Rest),
                })
            })
            .collect();
        let placed = labels::place(&ins, w, h, &Metrics::for_size(font));
        let id = self.solves.get() + 1;
        self.solves.set(id);
        self.solve.replace(Some(Solve {
            structure,
            pose,
            anchors,
            labels: Rc::new(placed),
            font,
            id,
        }));
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

    #[test]
    fn frames_are_cached_and_labels_never_overlap() {
        let (mut s, t0) = ready();
        let (w, h) = (800.0, 500.0);
        let a = s.frame(w, h).unwrap();
        let b = s.frame(w, h).unwrap();
        assert!(Rc::ptr_eq(&a, &b), "a still scene is not projected again");
        // The step's node is the focus and shows in full.
        assert!(a.labels.iter().any(|l| l.part == 0 && l.full));
        for (i, x) in a.labels.iter().enumerate() {
            for y in &a.labels[i + 1..] {
                let hit = x.x < y.x + y.w && y.x < x.x + x.w && x.y < y.y + y.h && y.y < x.y + x.h;
                assert!(!hit, "{x:?} {y:?}");
            }
        }
        // Moving on, or a new size, makes a new frame.
        s.next(t0);
        let c = s.frame(w, h).unwrap();
        assert!(!Rc::ptr_eq(&a, &c));
        assert!(!Rc::ptr_eq(&c, &s.frame(w + 10.0, h).unwrap()));
        assert_eq!(s.ranks()[1], Rank::Focus);
    }

    #[test]
    fn the_figure_fills_the_stage_on_load_and_reset() {
        let (mut s, _) = ready();
        let (w, h) = (800.0, 500.0);
        let f = s.frame(w, h).unwrap();
        let b = f.drawing.figure_bounds().unwrap();
        let (x0, y0) = f.fit.at((b.0, b.1));
        let (x1, y1) = f.fit.at((b.2, b.3));
        // Framed with the margin: the figure spans the stage one way.
        let span_w = (x1 - x0) / (w - 2.0 * MARGIN);
        let span_h = (y1 - y0) / (h - 2.0 * MARGIN);
        assert!(span_w.max(span_h) > 0.95, "{span_w} {span_h}");
        assert!(x0 >= MARGIN - 1.0 && y0 >= MARGIN - 1.0 && x1 <= w - MARGIN + 1.0);
        s.wheel(-400.0, 10.0, 10.0, w, h);
        settle(&mut s);
        s.reset_view();
        settle(&mut s);
        assert!(s.camera.at_home());
        let g = s.frame(w, h).unwrap();
        assert!((g.fit.scale - f.fit.scale).abs() < 1e-3);
    }

    #[test]
    fn degenerate_scenes_load_without_panics() {
        let mut bad = scene();
        bad.parts[0].at = [f32::NAN, f32::INFINITY, 0.0];
        bad.parts[1].size = [f32::NAN; 3];
        bad.steps.push(Step {
            parts: vec!["missing".into()],
            ..Step::default()
        });
        let mut s = ExplainState::new("x");
        s.ready(Ok((bad, None)), Instant::now());
        let f = s.frame(640.0, 400.0).unwrap();
        assert!(f.fit.scale.is_finite());
        assert!(f.labels.iter().all(|l| l.x.is_finite() && l.y.is_finite()));
        // An empty scene: nothing to frame, nothing breaks.
        let mut empty = scene();
        empty.parts.clear();
        empty.steps.clear();
        let mut s = ExplainState::new("x");
        s.ready(Ok((empty, None)), Instant::now());
        assert!(s.frame(640.0, 400.0).unwrap().labels.is_empty());
        assert!(s.reading().is_none());
        assert!(!s.next(Instant::now()));
        assert_eq!(s.key("space", Instant::now()), KeyDid::Play);
        assert!(!s.tick_autoplay(Instant::now() + AUTOPLAY));
        // Every part on one spot, and a stage of no size.
        let mut pile = scene();
        for p in &mut pile.parts {
            p.at = [3.0, 3.0, 0.0];
        }
        let mut s = ExplainState::new("x");
        s.ready(Ok((pile, None)), Instant::now());
        assert!(s.frame(0.0, 0.0).unwrap().labels.is_empty());
        assert!(!s.point(1.0, 1.0, 0.0, 0.0, Instant::now()));
        s.rebuild();
        assert!(s.building && s.scene.is_none());
    }

    /// A scene like the one that froze on Windows: 20 parts (blocks and
    /// cylinders) in five levels, 16 steps, densely linked.
    fn dense() -> Scene {
        use tern_agent::explain::Link;
        let parts: Vec<Part> = (0..20)
            .map(|i| Part {
                id: format!("n{i}"),
                label: format!("IPv6 concept number {i} with a long name"),
                shape: if i % 3 == 0 {
                    Shape::Cylinder
                } else {
                    Shape::Block
                },
                at: [1.0 + (i % 5) as f32 * 2.2, 1.0 + (i / 5) as f32 * 2.6, 0.0],
                size: [1.6, 1.6, 1.0 + (i % 4) as f32 * 0.3],
                note: format!("node {i}"),
                level: (i / 4) as u8,
            })
            .collect();
        let mut links = Vec::new();
        for i in 0..20 {
            for d in 1..=3 {
                let j = (i + d * 3) % 20;
                links.push(Link {
                    from: format!("n{i}"),
                    to: format!("n{j}"),
                    label: String::new(),
                });
            }
        }
        let steps: Vec<Step> = (0..16)
            .map(|k| Step {
                caption: format!("Step {k}"),
                parts: if k % 4 == 3 {
                    vec![format!("n{k}"), format!("n{}", k + 4)]
                } else {
                    vec![format!("n{k}")]
                },
                anchor: None,
                level: (k / 4) as u8,
                passage: format!("Passage {k}."),
            })
            .collect();
        Scene {
            id: "dense".into(),
            concept: "IPv6 Mapping & Operation".into(),
            summary: String::new(),
            parts,
            links,
            steps,
            topic: None,
            written_by: "model".into(),
            created_at: 0,
        }
    }

    const FRAME: Duration = Duration::from_millis(16);
    /// Most any one frame may take here. Release builds take well under a
    /// millisecond; this leaves room for a debug build on a slow runner.
    const FRAME_BUDGET: Duration = Duration::from_millis(40);

    /// Runs frames at 60 a second until nothing moves (at most `cap`),
    /// ticking autoplay like the app does. Returns the frames run.
    fn run_frames(s: &mut ExplainState, t: &mut Instant, w: f32, h: f32, cap: usize) -> usize {
        for n in 0..cap {
            *t += FRAME;
            s.tick_autoplay(*t);
            s.settle(FRAME.as_secs_f32(), false);
            let started = Instant::now();
            let f = s.frame_at(w, h, *t).expect("a frame");
            let took = started.elapsed();
            assert!(took < FRAME_BUDGET, "frame {n} took {took:?}");
            assert!(f.labels.iter().all(|l| l.x.is_finite() && l.y.is_finite()));
            assert_eq!(f.links.len(), f.drawing.links.len());
            if !s.moving(*t, false) && !s.autoplay {
                return n + 1;
            }
        }
        cap
    }

    fn dense_ready() -> (ExplainState, Instant) {
        let t0 = Instant::now();
        let mut s = ExplainState::new("ipv6");
        s.ready(Ok((dense(), None)), t0);
        (s, t0)
    }

    #[test]
    fn every_step_of_a_dense_scene_lays_labels_out_once() {
        let (mut s, mut t) = dense_ready();
        let (w, h) = (330.0, 455.0);
        run_frames(&mut s, &mut t, w, h, 120);
        assert_eq!(s.solves.get(), 1);
        for k in 1..16 {
            assert!(s.next(t));
            let before = s.solves.get();
            let frames = run_frames(&mut s, &mut t, w, h, 600);
            assert!(frames < 600, "step {k} never settled");
            assert_eq!(
                s.solves.get(),
                before + 1,
                "step {k}: one layout for the glide, not one per frame"
            );
            let f = s.frame_at(w, h, t).unwrap();
            // The step's nodes are labelled in full.
            for p in s.scene.as_ref().unwrap().lit(k) {
                assert!(f.labels.iter().any(|l| l.part == p && l.full), "step {k}");
            }
        }
        assert!(!s.next(t), "stops at the last step");
        // Back again, step by step.
        while s.back(t) {
            run_frames(&mut s, &mut t, w, h, 600);
        }
        assert_eq!(s.step, 0);
    }

    #[test]
    fn play_runs_the_whole_dense_scene_on_one_clock() {
        let (mut s, mut t) = dense_ready();
        let (w, h) = (330.0, 455.0);
        s.toggle_autoplay(t);
        let frames = run_frames(&mut s, &mut t, w, h, 16 * 400);
        assert!(!s.autoplay, "stops at the end");
        assert_eq!(s.step, 15);
        // Fifteen moves of AUTOPLAY each, no faster.
        let ran = FRAME * frames as u32;
        assert!(ran >= AUTOPLAY * 15, "{ran:?}");
        assert!(s.solves.get() <= 17, "{}", s.solves.get());
        // Play again from the end starts over; Pause stops at once.
        s.toggle_autoplay(t);
        assert_eq!(s.step, 0);
        s.toggle_autoplay(t);
        assert!(!s.autoplay);
        assert!(!s.tick_autoplay(t + AUTOPLAY * 2));
        assert_eq!(s.step, 0);
    }

    #[test]
    fn zooming_carries_labels_and_lays_out_once_it_settles() {
        let (mut s, mut t) = dense_ready();
        let (w, h) = (640.0, 420.0);
        run_frames(&mut s, &mut t, w, h, 60);
        let base = s.solves.get();
        let f0 = s.frame_at(w, h, t).unwrap();
        let first = f0.labels[0].clone();
        // A burst of wheel turns, one a frame, in and then out.
        for i in 0..40 {
            // Small turns, so the zoom never reaches its limit (where the
            // target would stop moving and a layout would be due).
            let dy = if i < 25 { -20.0 } else { 15.0 };
            s.wheel(dy, 300.0, 200.0, w, h);
            t += FRAME;
            s.settle(FRAME.as_secs_f32(), false);
            let started = Instant::now();
            let f = s.frame_at(w, h, t).unwrap();
            assert!(started.elapsed() < FRAME_BUDGET);
            // Labels ride with their nodes: same layout, moved.
            assert_eq!(s.solves.get(), base, "no layout while the wheel turns");
            if let Some(l) = f.labels.iter().find(|l| l.part == first.part) {
                assert_eq!(l.text, first.text);
            }
        }
        // Once the target stays put, one layout for it.
        run_frames(&mut s, &mut t, w, h, 600);
        assert_eq!(s.solves.get(), base + 1);
        assert!(!s.relayout_pending());
        // Pinches and the keyboard behave the same.
        for i in 0..20 {
            s.pinch(0.01, 100.0, 100.0, w, h);
            s.key(if i % 2 == 0 { "+" } else { "-" }, t);
            t += FRAME;
            s.settle(FRAME.as_secs_f32(), false);
            s.frame_at(w, h, t).unwrap();
        }
        assert_eq!(s.solves.get(), base + 1);
        run_frames(&mut s, &mut t, w, h, 600);
        assert_eq!(s.solves.get(), base + 2);
    }

    #[test]
    fn orbiting_carries_labels_and_lays_out_after_release() {
        let (mut s, mut t) = dense_ready();
        let (w, h) = (640.0, 420.0);
        run_frames(&mut s, &mut t, w, h, 60);
        let base = s.solves.get();
        s.press(300.0, 200.0, false, t);
        for i in 1..=60 {
            t += FRAME;
            s.point(300.0 - i as f32 * 12.0, 200.0 + i as f32, w, h, t);
            s.settle(FRAME.as_secs_f32(), false);
            s.frame_at(w, h, t).unwrap();
        }
        assert_eq!(s.solves.get(), base, "no layout per drag move");
        s.release(t);
        run_frames(&mut s, &mut t, w, h, 2000);
        assert_eq!(s.solves.get(), base + 1, "one layout once the spin stops");
    }

    #[test]
    fn only_the_steps_links_are_drawn_strongly() {
        let (mut s, t) = dense_ready();
        let f = s.frame_at(640.0, 420.0, t).unwrap();
        let strong = f.links.iter().filter(|w| **w > 0.5).count();
        assert!(strong > 0 && strong < f.links.len() / 4, "{strong}");
        let cur = s.scene.as_ref().unwrap().lit(0);
        for (l, w) in f.drawing.links.iter().zip(f.links.iter()) {
            let touches = cur.contains(&l.from) || cur.contains(&l.to);
            assert_eq!(*w == 1.0, touches);
            assert!([1.0, LINK_FLOW, LINK_FADED].contains(w));
        }
        assert!(f.links.iter().filter(|w| **w == LINK_FADED).count() > f.links.len() / 2);
        s.next(t);
        let g = s.frame_at(640.0, 420.0, t).unwrap();
        assert_ne!(f.links, g.links, "the strong links follow the step");
    }

    #[test]
    fn degenerate_scenes_survive_steps_play_and_zoom() {
        let mut scenes = Vec::new();
        // Everything on one spot.
        let mut pile = dense();
        for p in &mut pile.parts {
            p.at = [6.0, 6.0, 0.0];
        }
        scenes.push(pile);
        // NaN and huge numbers, links to nowhere, a step with no parts.
        let mut bad = dense();
        bad.parts[0].at = [f32::NAN, 1e30, f32::INFINITY];
        bad.parts[1].size = [f32::NAN, 0.0, -5.0];
        bad.links.push(tern_agent::explain::Link {
            from: "n0".into(),
            to: "nowhere".into(),
            label: String::new(),
        });
        bad.steps[3].parts.clear();
        scenes.push(bad);
        // Far more than the caps, all labels identical.
        let mut big = dense();
        for i in 20..200 {
            let mut p = big.parts[i % 20].clone();
            p.id = format!("m{i}");
            big.parts.push(p);
            big.steps.push(Step {
                parts: vec![format!("m{i}")],
                ..Step::default()
            });
        }
        scenes.push(big);
        // One part, one step.
        let mut one = dense();
        one.parts.truncate(1);
        one.steps.truncate(1);
        scenes.push(one);
        for (i, sc) in scenes.into_iter().enumerate() {
            let mut t = Instant::now();
            let mut s = ExplainState::new("x");
            s.ready(Ok((sc, None)), t);
            for (w, h) in [(640.0, 420.0), (0.0, 0.0), (3.0, 900.0)] {
                s.toggle_autoplay(t);
                run_frames(&mut s, &mut t, w, h, 40 * 260);
                for _ in 0..10 {
                    s.wheel(-300.0, 10.0, 10.0, w, h);
                    s.double_click(w / 2.0, h / 2.0, w, h);
                    s.next(t);
                    run_frames(&mut s, &mut t, w, h, 30);
                }
                s.reset_view();
                run_frames(&mut s, &mut t, w, h, 600);
            }
            assert!(s.camera.now.zoom.is_finite(), "scene {i}");
        }
    }
}
