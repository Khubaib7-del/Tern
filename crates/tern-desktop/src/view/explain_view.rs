//! An Explain in 3D tab: the concept as a small isometric machine.
//!
//! Solid, Anatomy-style faces (lit top, a lit side and a shaded side, in
//! the text colour over the canvas) with a Hairline overlay (every edge in
//! a thin line, the floor grid, dashed links). The parts a step is about
//! take the one accent and fade in on the motion curve; the part under the
//! pointer lifts. A real camera: drag to orbit (all the way round, the
//! tilt clamped), the wheel or a pinch to zoom, right- or Shift-drag to
//! pan, double-click a node to focus it, Reset view, arrows and + / - on
//! the keyboard, with damped inertia. Steps go level by level with Prev,
//! Next and Play, the camera gliding onto each step's nodes, and the side
//! caption reads the step (or the node clicked): its text, the passage
//! and its citations. Reduced motion jumps the camera, never spins it on,
//! and switches steps without a fade.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathBuilder, PinchEvent, Pixels, ScrollWheelEvent, SharedString, Window, canvas,
    div, point, px,
};
use tern_agent::explain::camera::Camera;
use tern_agent::explain::labels::Rank;
use tern_agent::explain::{Drawing, Edge, FaceKind, Fit, ink_amount};

use super::Workspace;
use super::study_view::serif;
use crate::explain::{ExplainState, KeyDid, MARGIN};
use crate::model::Screen;
use crate::motion::lerp_color;
use crate::theme::Palette;

/// A leader line from a node to its label, stage pixels.
type Leader = ((f32, f32), (f32, f32));
/// Drawing units to window pixels, `None` when not drawable.
type ToStage<'a> = &'a dyn Fn(&[(f32, f32)]) -> Option<Vec<gpui_kit::Point<Pixels>>>;

/// Colours and amounts the painter needs, captured for the frame.
struct Paint {
    drawing: Rc<Drawing>,
    /// The cached frame's fit and the stage size it was made for: used
    /// when the stage has not changed size, so shapes and labels agree.
    fit: Option<(Fit, (f32, f32))>,
    camera: Camera,
    lit: Vec<f32>,
    hover: Option<usize>,
    /// Leader lines from nodes to labels that moved away, stage pixels.
    leaders: Vec<Leader>,
    canvas: Hsla,
    ink: Hsla,
    faint: Hsla,
    muted: Hsla,
    accent: Hsla,
    dark: bool,
}

/// Farthest from the stage a point may be and still be drawn, pixels.
/// Anything past it (or not a number) is skipped, so a path is never
/// built from coordinates the tessellator cannot handle.
const FAR: f32 = 50_000.0;
/// Longest link drawn dashed, pixels; longer ones are solid, so a dash
/// pattern is never cut into an unbounded number of pieces.
const DASH_MAX: f32 = 4_000.0;

/// The points in window pixels, or `None` when any is not finite, is
/// absurdly far, or the shape lies wholly outside the `w` × `h` stage.
fn on_stage(
    pts: &[(f32, f32)],
    f: &Fit,
    o: (f32, f32),
    w: f32,
    h: f32,
) -> Option<Vec<gpui_kit::Point<Pixels>>> {
    if pts.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(pts.len());
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pts {
        let (x, y) = f.at(*p);
        if !(x.is_finite() && y.is_finite()) || x.abs() > FAR || y.abs() > FAR {
            return None;
        }
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
        out.push(point(px(o.0 + x), px(o.1 + y)));
    }
    let off = x1 < -2.0 || y1 < -2.0 || x0 > w + 2.0 || y0 > h + 2.0;
    (!off).then_some(out)
}

fn stroke(
    pts: &[gpui_kit::Point<Pixels>],
    w: f32,
    closed: bool,
    dash: Option<f32>,
) -> Option<gpui_kit::Path<Pixels>> {
    let mut pb = PathBuilder::stroke(px(w));
    if let Some(d) = dash {
        let len: f32 = pts
            .windows(2)
            .map(|s| {
                let (dx, dy) = (
                    f32::from(s[1].x) - f32::from(s[0].x),
                    f32::from(s[1].y) - f32::from(s[0].y),
                );
                dx.hypot(dy)
            })
            .sum();
        if len.is_finite() && len > 1.0 && len < DASH_MAX {
            pb = pb.dash_array(&[px(d), px(d * 0.8)]);
        }
    }
    let (first, rest) = pts.split_first()?;
    if rest.is_empty() {
        return None;
    }
    pb.move_to(*first);
    for p in rest {
        pb.line_to(*p);
    }
    if closed {
        pb.close();
    }
    pb.build().ok()
}

fn fill(pts: &[gpui_kit::Point<Pixels>]) -> Option<gpui_kit::Path<Pixels>> {
    if pts.len() < 3 {
        return None;
    }
    let mut pb = PathBuilder::fill();
    let (first, rest) = pts.split_first()?;
    pb.move_to(*first);
    for p in rest {
        pb.line_to(*p);
    }
    pb.close();
    pb.build().ok()
}

/// One hairline: in the accent when its part is lit or hovered.
fn paint_edge(e: &Edge, pt: &Paint, at: ToStage<'_>, window: &mut Window) {
    let lit = e.part.and_then(|i| pt.lit.get(i).copied()).unwrap_or(0.0);
    let hovered = e.part.is_some() && e.part == pt.hover;
    let (c, wid) = if lit > 0.5 || hovered {
        (pt.accent, 1.4)
    } else {
        (pt.ink.opacity(0.85), 0.9)
    };
    if let Some(p) = at(&e.pts).and_then(|q| stroke(&q, wid, e.closed, None)) {
        window.paint_path(p, c);
    }
}

fn paint(b: Bounds<Pixels>, pt: &Paint, window: &mut Window) {
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    let o = (f32::from(b.origin.x), f32::from(b.origin.y));
    if !(w > 1.0 && h > 1.0) {
        return;
    }
    let f = match pt.fit {
        Some((f, size)) if size == (w, h) => f,
        _ => pt.camera.fit(w, h, MARGIN),
    };
    let at = |pts: &[(f32, f32)]| on_stage(pts, &f, o, w, h);
    // The floor grid: the faintest hairlines.
    for e in &pt.drawing.grid {
        if let Some(p) = at(&e.pts).and_then(|q| stroke(&q, 0.6, false, None)) {
            window.paint_path(p, pt.faint.opacity(0.35));
        }
    }
    // The solid machine, back to front, with the Hairline overlay: each
    // part's edges go down right after its last face, so nearer parts hide
    // the lines behind them.
    let after = pt.drawing.edges_after_faces();
    for (i, face) in pt.drawing.faces.iter().enumerate() {
        let mut c = lerp_color(pt.canvas, pt.ink, ink_amount(face.tone, pt.dark));
        let lit = pt.lit.get(face.part).copied().unwrap_or(0.0);
        if lit > 0.0 {
            let k = if face.kind == FaceKind::Top {
                0.55
            } else {
                0.4
            };
            c = lerp_color(c, pt.accent, lit * k);
        }
        if let Some(q) = at(&face.pts) {
            if let Some(p) = fill(&q) {
                window.paint_path(p, c);
            }
            // The same colour as a thin stroke closes the seams between facets.
            if let Some(p) = stroke(&q, 0.6, true, None) {
                window.paint_path(p, c);
            }
        }
        for &ei in after.get(i).into_iter().flatten() {
            if let Some(e) = pt.drawing.edges.get(ei) {
                paint_edge(e, pt, &at, window);
            }
        }
    }
    for &ei in after.last().into_iter().flatten() {
        if let Some(e) = pt.drawing.edges.get(ei) {
            paint_edge(e, pt, &at, window);
        }
    }
    // Links: dashed, with a head.
    for l in &pt.drawing.links {
        if let Some(p) = at(&[l.a, l.b]).and_then(|q| stroke(&q, 1.0, false, Some(4.0))) {
            window.paint_path(p, pt.muted);
        }
        if let Some(p) = at(&l.head).and_then(|q| fill(&q)) {
            window.paint_path(p, pt.muted);
        }
    }
    // Leaders from nodes to labels that had to move.
    for (a, z) in &pt.leaders {
        let q = [
            point(px(o.0 + a.0), px(o.1 + a.1)),
            point(px(o.0 + z.0), px(o.1 + z.1)),
        ];
        let finite = [a.0, a.1, z.0, z.1]
            .iter()
            .all(|v| v.is_finite() && v.abs() < FAR);
        if finite && let Some(p) = stroke(&q, 0.8, false, None) {
            window.paint_path(p, pt.faint);
        }
    }
}

impl Workspace {
    /// Opens an explainer in a new tab and starts building its scene.
    pub(super) fn open_explain(
        &mut self,
        ask: tern_agent::explain::Ask,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.next_explain += 1;
        let key = self.next_explain;
        let state = ExplainState::new(label);
        self.tabs.open_explain(key, &state.title());
        self.explains.insert(key, state);
        self.model.screen = Screen::Chat;
        self.explain_asks.insert(key, ask.clone());
        self.bridge.explain_build(key, ask);
        self.keys.focus(window, cx);
        cx.notify();
    }

    /// Builds tab `key`'s scene again from what it was asked.
    pub(super) fn retry_explain(&mut self, key: u32, cx: &mut Context<Self>) {
        let Some(ask) = self.explain_asks.get(&key).cloned() else {
            return;
        };
        if let Some(s) = self.explains.get_mut(&key) {
            s.rebuild();
            self.bridge.explain_build(key, ask);
        }
        cx.notify();
    }

    /// Stops building tab `key`'s scene; the tab says so and offers Try
    /// again.
    pub(super) fn cancel_explain(&mut self, key: u32, cx: &mut Context<Self>) {
        self.bridge.explain_cancel(key);
        if let Some(s) = self.explains.get_mut(&key)
            && s.building
        {
            s.ready(
                Err("Stopped. Try again when you are ready.".into()),
                Instant::now(),
            );
        }
        cx.notify();
    }

    /// Forgets tab `key`'s explainer, stopping its build.
    pub(super) fn drop_explain(&mut self, key: u32) {
        self.bridge.explain_cancel(key);
        self.explains.remove(&key);
        self.explain_asks.remove(&key);
    }

    /// The explainer in the active tab, if it is one.
    pub(super) fn active_explain(&self) -> Option<u32> {
        self.tabs.active_tab().explain
    }

    pub(super) fn on_explain(
        &mut self,
        key: u32,
        r: Result<(tern_agent::explain::Scene, Option<String>), String>,
    ) {
        if let Some(s) = self.explains.get_mut(&key) {
            s.ready(r, Instant::now());
            let title = s.title();
            self.tabs.retitle_explain(key, &title);
        }
    }

    /// Eases the tilt and keeps frames coming while something moves.
    pub(super) fn tick_explain(&mut self, window: &mut Window) {
        let now = Instant::now();
        let dt = self
            .explain_at
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.0);
        self.explain_at = Some(now);
        let study = &self.model.study;
        let key = match self.active_explain() {
            Some(k) if self.model.screen == Screen::Chat => k,
            // The Study window's Explain tab.
            _ if self.model.harness == tern_agent::Harness::Study
                && study.tab == crate::study::StudyTab::Explain =>
            {
                match study.explain {
                    Some(k) => k,
                    None => return,
                }
            }
            _ => return,
        };
        let reduced = self.reduced_motion;
        if let Some(s) = self.explains.get_mut(&key) {
            s.settle(dt, reduced);
            if s.moving(now, reduced) {
                window.request_animation_frame();
            }
        }
    }

    /// Steps forward on autoplay while an explainer plays.
    fn start_explain_ticker(&mut self, cx: &mut Context<Self>) {
        if self.explain_ticker.is_some() {
            return;
        }
        self.explain_ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let go = this.update(cx, |this, cx| {
                    let now = Instant::now();
                    let mut any = false;
                    for s in this.explains.values_mut() {
                        if s.tick_autoplay(now) {
                            cx.notify();
                        }
                        any |= s.autoplay;
                    }
                    if !any {
                        this.explain_ticker = None;
                    }
                    any
                });
                if !matches!(go, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn explain_step(&mut self, key: u32, by: i32, cx: &mut Context<Self>) {
        let now = Instant::now();
        if let Some(s) = self.explains.get_mut(&key) {
            s.autoplay = false;
            if by > 0 {
                s.next(now);
            } else {
                s.back(now);
            }
        }
        cx.notify();
    }

    fn explain_play(&mut self, key: u32, cx: &mut Context<Self>) {
        if let Some(s) = self.explains.get_mut(&key) {
            s.toggle_autoplay(Instant::now());
            if s.autoplay {
                self.start_explain_ticker(cx);
            }
        }
        cx.notify();
    }

    fn explain_keys(&mut self, key: u32, e: &gpui_kit::KeyDownEvent, cx: &mut Context<Self>) {
        let reduced = self.reduced_motion;
        let did = match self.explains.get_mut(&key) {
            Some(s) => {
                s.reduced = reduced;
                s.key(e.keystroke.key.as_str(), Instant::now())
            }
            None => return,
        };
        match did {
            KeyDid::Nothing => return,
            KeyDid::Play if self.explains.get(&key).is_some_and(|s| s.autoplay) => {
                self.start_explain_ticker(cx);
            }
            _ => {}
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// A pointer event's position inside the stage, with its size.
    fn stage_local(&self, key: u32, x: f32, y: f32) -> Option<(f32, f32, f32, f32)> {
        let [bx, by, w, h] = self.explains.get(&key)?.bounds.get();
        Some((x - bx, y - by, w, h))
    }

    fn explain_export(&mut self, key: u32, cx: &mut Context<Self>) {
        let Some(s) = self.explains.get(&key) else {
            return;
        };
        let Some(scene) = s.scene.clone() else {
            return;
        };
        let step = s.step;
        let name = format!("{}.svg", tern_agent::vault::clean_name(&scene.concept));
        let dir = super::cards_view::home_dir();
        let picked = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = picked.await {
                let _ = this.update(cx, |this, cx| {
                    this.bridge.explain_svg(key, scene, step, path);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// A press on the stage ended: a click reads the node under it.
    fn explain_release(&mut self, key: u32, window: &mut Window, cx: &mut Context<Self>) {
        let reduced = self.reduced_motion;
        if let Some(s) = self.explains.get_mut(&key)
            && s.camera.dragging()
        {
            s.reduced = reduced;
            s.release(Instant::now());
            // The spin after a drag needs frames.
            self.explain_at = None;
            window.request_animation_frame();
            cx.notify();
        }
    }

    /// The explainer in tab `key`.
    pub(super) fn explain_view(&self, key: u32, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = self.explains.get(&key) else {
            return div().into_any_element();
        };
        let now = Instant::now();
        let reduced = self.reduced_motion;
        let title: SharedString = s
            .scene
            .as_ref()
            .map(|x| x.concept.clone())
            .unwrap_or_else(|| s.asked.clone())
            .into();
        // The header: the title takes the room left and wraps to two
        // lines, then ends in "…"; the buttons keep their place on the
        // right and move to a row of their own when the tab is narrow.
        let header = div()
            .flex()
            .flex_wrap()
            .items_start()
            .gap_3()
            .flex_none()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .flex_1()
                    .min_w(px(220.))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div().pt(px(6.)).flex_none().child(
                                    gpui_kit::component::Icon::new(AssetIcon::Cuboid)
                                        .small()
                                        .text_color(p.accent),
                                ),
                            )
                            .child(
                                serif(title.clone(), p)
                                    .id(("x3d-title", key as usize))
                                    .text_2xl()
                                    .flex_1()
                                    .min_w_0()
                                    .line_clamp(2)
                                    .text_ellipsis()
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            title.to_string(),
                                        )
                                        .build(window, cx)
                                    }),
                            ),
                    )
                    .children(s.scene.as_ref().map(|x| {
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(format!(
                                "Explain in 3D · {} parts · {} steps · {}",
                                x.parts.len(),
                                x.steps.len(),
                                if x.written_by == "model" {
                                    "built by the model from your sources"
                                } else {
                                    "built by Tern from your materials' key terms"
                                }
                            )))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .ml_auto()
                    .gap_1()
                    .child(
                        Button::new(("x3d-save", key as usize))
                            .icon(AssetIcon::Save)
                            .label("Save")
                            .ghost()
                            .small()
                            .disabled(s.scene.is_none())
                            .tooltip("Save to your study library (JSON and an SVG snapshot)")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(scene) =
                                    this.explains.get(&key).and_then(|s| s.scene.clone())
                                {
                                    this.bridge.explain_save(key, scene, false);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("x3d-vault", key as usize))
                            .icon(AssetIcon::NotebookPen)
                            .label("Save to vault")
                            .ghost()
                            .small()
                            .disabled(s.scene.is_none())
                            .tooltip(
                                "Also write Explain/<concept>.md with the snapshot into your vault",
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(scene) =
                                    this.explains.get(&key).and_then(|s| s.scene.clone())
                                {
                                    this.bridge.explain_save(key, scene, true);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("x3d-svg", key as usize))
                            .icon(AssetIcon::Download)
                            .label("SVG")
                            .ghost()
                            .small()
                            .disabled(s.scene.is_none())
                            .tooltip("Export this step as an SVG snapshot")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.explain_export(key, cx);
                            })),
                    ),
            );
        let mut col = div()
            .flex()
            .flex_col()
            .gap_3()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .px_4()
            .py_3()
            .child(header);
        for line in [&s.note, &s.saved].into_iter().flatten() {
            col = col.child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1p5()
                    .rounded(px(16.))
                    .border_1()
                    .border_color(p.border)
                    .text_xs()
                    .text_color(p.muted)
                    .child(line.clone()),
            );
        }
        let failed = s.paint_failed.get();
        let error = s.error.clone().or_else(|| {
            failed.then(|| {
                "This scene could not be drawn. Nothing was lost; try building it again.".to_owned()
            })
        });
        let can_retry = self.explain_asks.contains_key(&key);
        let scene = match &s.scene {
            Some(scene) if !failed => scene,
            _ => {
                let body = if let Some(e) = error {
                    div()
                        .id(("x3d-error", key as usize))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .max_w(px(560.))
                        .p_4()
                        .rounded(px(16.))
                        .border_1()
                        .border_color(p.border)
                        .bg(p.raised)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_sm()
                                .text_color(p.text)
                                .child(
                                    gpui_kit::component::Icon::new(AssetIcon::TriangleAlert)
                                        .small()
                                        .text_color(p.muted),
                                )
                                .child("Explain in 3D could not show this scene"),
                        )
                        .child(div().text_sm().text_color(p.muted).child(e))
                        .when(can_retry, |d| {
                            d.child(
                                div().flex().gap_2().child(
                                    Button::new(("x3d-retry", key as usize))
                                        .icon(AssetIcon::RefreshCw)
                                        .label("Try again")
                                        .small()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.retry_explain(key, cx);
                                        })),
                                ),
                            )
                        })
                        .into_any_element()
                } else {
                    // Building: a still line, not an animation, so the
                    // window is not redrawn every frame while the model
                    // thinks.
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(p.muted)
                        .child(gpui_kit::component::Icon::new(AssetIcon::Hourglass).small())
                        .child("Building the scene from your sources… a local model can take a minute.")
                        .child(
                            Button::new(("x3d-cancel", key as usize))
                                .icon(AssetIcon::X)
                                .label("Cancel")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cancel_explain(key, cx);
                                })),
                        )
                        .into_any_element()
                };
                return div()
                    .id(("x3d", key as usize))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(col.child(body))
                    .into_any_element();
            }
        };
        if !scene.summary.is_empty() {
            col = col.child(
                div()
                    .flex_none()
                    .text_sm()
                    .text_color(p.muted)
                    .line_clamp(2)
                    .child(scene.summary.clone()),
            );
        }
        // The drawing and its labels, from the frame cache: projected and
        // laid out again only when the camera, the size, or the step moved.
        let bounds = s.bounds.clone();
        let [_, _, bw, bh] = bounds.get();
        let frame = s.frame(bw.max(0.0), bh.max(0.0));
        let lit: Vec<f32> = (0..scene.parts.len())
            .map(|i| s.lit(i, now, reduced))
            .collect();
        let (label_els, leaders): (Vec<AnyElement>, Vec<Leader>) = match frame
            .as_ref()
            .filter(|_| bw > 1.0)
        {
            Some(f) => {
                let font = f.font;
                let els = f
                    .labels
                    .iter()
                    .map(|l| {
                        let on =
                            l.rank == Rank::Focus || lit.get(l.part).copied().unwrap_or(0.0) > 0.5;
                        let base = div()
                            .absolute()
                            .left(px(l.x))
                            .top(px(l.y))
                            .w(px(l.w))
                            .h(px(l.h))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(font))
                            .rounded_sm();
                        if l.full {
                            base.bg(p.canvas.opacity(0.85))
                                .text_color(if on { p.accent } else { p.text })
                                .child(div().truncate().child(SharedString::from(l.text.clone())))
                                .into_any_element()
                        } else {
                            base.rounded_full()
                                .border_1()
                                .border_color(p.border)
                                .bg(p.raised)
                                .text_color(p.muted)
                                .child(SharedString::from(l.text.clone()))
                                .into_any_element()
                        }
                    })
                    .collect();
                let leaders = f.labels.iter().filter_map(|l| l.leader).collect();
                (els, leaders)
            }
            None => (Vec::new(), Vec::new()),
        };
        let drawing = frame
            .as_ref()
            .map(|f| f.drawing.clone())
            .unwrap_or_else(|| Rc::new(s.drawing().unwrap_or_default()));
        let pt = Paint {
            drawing,
            fit: frame.as_ref().map(|f| (f.fit, f.size)),
            camera: s.camera.clone(),
            lit,
            hover: s.hover,
            leaders,
            canvas: p.canvas,
            ink: p.text,
            faint: p.faint,
            muted: p.muted,
            accent: p.accent,
            dark: self.model.dark,
        };
        let failed_cell = s.paint_failed.clone();
        let readout = s.hover.and_then(|i| scene.parts.get(i)).map(|part| {
            div()
                .absolute()
                .left(px(12.))
                .bottom(px(10.))
                .max_w(px(360.))
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .text_xs()
                .child(div().text_color(p.text).child(part.label.clone()))
                .when(!part.note.is_empty(), |d| {
                    d.child(div().text_color(p.muted).child(part.note.clone()))
                })
        });
        let reset = div().absolute().top(px(8.)).right(px(8.)).child(
            Button::new(("x3d-reset", key as usize))
                .icon(AssetIcon::RotateCcw)
                .label("Reset view")
                .ghost()
                .xsmall()
                .disabled(s.camera.at_home())
                .tooltip("Back to the first view (0)")
                .on_click(cx.listener(move |this, _, window, cx| {
                    let reduced = this.reduced_motion;
                    if let Some(s) = this.explains.get_mut(&key) {
                        s.reduced = reduced;
                        s.reset_view();
                    }
                    this.explain_at = None;
                    window.request_animation_frame();
                    cx.notify();
                })),
        );
        let stage = div()
            .id(("x3d-stage", key as usize))
            .relative()
            .flex_1()
            .min_w_0()
            .min_h(px(160.))
            .rounded(px(16.))
            .border_1()
            .border_color(p.border)
            .bg(p.canvas)
            .overflow_hidden()
            .when(s.camera.dragging(), |d| d.cursor_grabbing())
            .when(!s.camera.dragging(), |d| d.cursor_grab())
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, window, cx| {
                let Some((mx, my, w, h)) =
                    this.stage_local(key, f32::from(e.position.x), f32::from(e.position.y))
                else {
                    return;
                };
                let reduced = this.reduced_motion;
                if let Some(s) = this.explains.get_mut(&key) {
                    s.reduced = reduced;
                    let dragging = s.camera.dragging();
                    // A press released outside the window ends the drag.
                    if dragging && e.pressed_button.is_none() {
                        s.release(Instant::now());
                        cx.notify();
                        return;
                    }
                    if mx < 0.0 || my < 0.0 || mx > w || my > h {
                        if !dragging && s.hover.is_some() {
                            s.leave();
                            cx.notify();
                        }
                        return;
                    }
                    if s.point(mx, my, w, h, Instant::now()) || dragging {
                        window.request_animation_frame();
                        cx.notify();
                    }
                }
            }))
            .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                if !*over
                    && let Some(s) = this.explains.get_mut(&key)
                    && !s.camera.dragging()
                {
                    s.leave();
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    this.keys.focus(window, cx);
                    let Some((mx, my, w, h)) =
                        this.stage_local(key, f32::from(e.position.x), f32::from(e.position.y))
                    else {
                        return;
                    };
                    let reduced = this.reduced_motion;
                    if let Some(s) = this.explains.get_mut(&key) {
                        s.reduced = reduced;
                        if e.click_count >= 2 {
                            s.double_click(mx, my, w, h);
                        } else {
                            s.press(mx, my, e.modifiers.shift, Instant::now());
                        }
                    }
                    this.explain_at = None;
                    window.request_animation_frame();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    this.keys.focus(window, cx);
                    let Some((mx, my, _, _)) =
                        this.stage_local(key, f32::from(e.position.x), f32::from(e.position.y))
                    else {
                        return;
                    };
                    if let Some(s) = this.explains.get_mut(&key) {
                        s.press(mx, my, true, Instant::now());
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                    this.explain_release(key, window, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                    this.explain_release(key, window, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                    this.explain_release(key, window, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                    this.explain_release(key, window, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(move |this, e: &ScrollWheelEvent, window, cx| {
                cx.stop_propagation();
                let Some((mx, my, w, h)) =
                    this.stage_local(key, f32::from(e.position.x), f32::from(e.position.y))
                else {
                    return;
                };
                let dy = f32::from(e.delta.pixel_delta(px(20.)).y);
                let reduced = this.reduced_motion;
                if let Some(s) = this.explains.get_mut(&key) {
                    s.reduced = reduced;
                    // Away from you zooms in.
                    s.wheel(-dy, mx, my, w, h);
                }
                this.explain_at = None;
                window.request_animation_frame();
                cx.notify();
            }))
            .on_pinch(cx.listener(move |this, e: &PinchEvent, window, cx| {
                let Some((mx, my, w, h)) =
                    this.stage_local(key, f32::from(e.position.x), f32::from(e.position.y))
                else {
                    return;
                };
                let reduced = this.reduced_motion;
                if let Some(s) = this.explains.get_mut(&key) {
                    s.reduced = reduced;
                    s.pinch(e.delta, mx, my, w, h);
                }
                this.explain_at = None;
                window.request_animation_frame();
                cx.notify();
            }))
            .child(
                canvas(
                    |_, _, _: &mut App| (),
                    move |b, _, window, _: &mut App| {
                        let now = [
                            f32::from(b.origin.x),
                            f32::from(b.origin.y),
                            f32::from(b.size.width),
                            f32::from(b.size.height),
                        ];
                        if now != bounds.get() {
                            bounds.set(now);
                            // Labels follow the new size next frame.
                            window.request_animation_frame();
                        }
                        // A panic here would unwind into the window
                        // procedure and abort the app: catch it, and show
                        // an error card instead of drawing again.
                        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            paint(b, &pt, window)
                        }))
                        .is_ok();
                        if !ok {
                            failed_cell.set(true);
                            window.request_animation_frame();
                        }
                    },
                )
                .size_full(),
            )
            .children(label_els)
            .children(readout)
            .child(reset);
        // The side caption: the step, or the node clicked.
        let side = s.reading().map(|r| {
            div()
                .id(("x3d-side", key as usize))
                .flex()
                .flex_col()
                .gap_2()
                .w(px(300.))
                .flex_none()
                .p_3()
                .rounded(px(16.))
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .overflow_y_scrollbar()
                .children(r.level.map(|l| {
                    div()
                        .text_xs()
                        .text_color(p.accent)
                        .child(SharedString::from(l.to_uppercase()))
                }))
                .child(div().text_xs().text_color(p.faint).child(r.title.clone()))
                .child(div().text_sm().text_color(p.text).child(r.text.clone()))
                .when(!r.passage.is_empty(), |d| {
                    d.child(
                        div()
                            .pl_2()
                            .border_l_2()
                            .border_color(p.border)
                            .text_xs()
                            .text_color(p.muted)
                            .child(r.passage.clone()),
                    )
                })
                .when(!r.citations.is_empty(), |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child(SharedString::from(format!(
                                "Cites {}",
                                r.citations.join(" ")
                            ))),
                    )
                })
                .children(r.cite.map(|c| {
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(format!("Source: {c}")))
                }))
        });
        // The step bar: Prev, Play, Next, where it is, and the levels.
        let n = scene.steps.len();
        let i = s.step.min(n.saturating_sub(1));
        let dots = (0..n).map(|j| {
            div()
                .id(("x3d-dot", key as usize * 100 + j))
                .size(px(8.))
                .rounded_full()
                .cursor_pointer()
                .when(j == i, |d| d.bg(p.accent))
                .when(j != i, |d| d.border_1().border_color(p.faint))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(s) = this.explains.get_mut(&key) {
                        s.autoplay = false;
                        s.go(j, Instant::now());
                    }
                    this.explain_at = None;
                    window.request_animation_frame();
                    cx.notify();
                }))
        });
        let cur_level = s
            .level_marks()
            .iter()
            .rev()
            .find(|(_, at)| *at <= i)
            .map(|(name, _)| *name);
        let marks = s
            .level_marks()
            .into_iter()
            .enumerate()
            .map(|(k, (name, at))| {
                let here = Some(name) == cur_level;
                div()
                    .id(("x3d-level", key as usize * 10 + k))
                    .px_1p5()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(if here { p.accent } else { p.faint })
                    .hover(move |d| d.text_color(p.text))
                    .child(name)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(s) = this.explains.get_mut(&key) {
                            s.autoplay = false;
                            s.go(at, Instant::now());
                        }
                        this.explain_at = None;
                        window.request_animation_frame();
                        cx.notify();
                    }))
            });
        let bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                Button::new(("x3d-prev", key as usize))
                    .icon(AssetIcon::ChevronLeft)
                    .label("Prev")
                    .ghost()
                    .small()
                    .disabled(i == 0)
                    .tooltip("Previous step ([ or Page Up)")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.explain_step(key, -1, cx);
                        this.explain_at = None;
                        window.request_animation_frame();
                    })),
            )
            .child(
                Button::new(("x3d-play", key as usize))
                    .icon(if s.autoplay {
                        AssetIcon::Pause
                    } else {
                        AssetIcon::Play
                    })
                    .label(if s.autoplay { "Pause" } else { "Play" })
                    .ghost()
                    .small()
                    .tooltip(if s.autoplay {
                        "Pause (Space)"
                    } else {
                        "Walk the levels from the basics up (Space)"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.explain_play(key, cx);
                        this.explain_at = None;
                        window.request_animation_frame();
                    })),
            )
            .child(
                Button::new(("x3d-next", key as usize))
                    .icon(AssetIcon::ChevronRight)
                    .label("Next")
                    .ghost()
                    .small()
                    .disabled(i + 1 >= n)
                    .tooltip("Next step (] or Page Down)")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.explain_step(key, 1, cx);
                        this.explain_at = None;
                        window.request_animation_frame();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!("Step {} of {n}", i + 1))),
            )
            .child(div().flex().gap_1p5().items_center().children(dots))
            .child(div().flex().gap_1().items_center().children(marks));
        let body = div()
            .flex()
            .gap_3()
            .flex_1()
            .min_h(px(160.))
            .child(stage)
            .children(side);
        // How to drive the camera, behind a "?" instead of a long line.
        let help = Button::new(("x3d-help", key as usize))
            .icon(AssetIcon::CircleQuestionMark)
            .ghost()
            .xsmall()
            .tooltip(
                "Drag to turn · right- or Shift-drag to pan · scroll or pinch to zoom · double-click a node to focus · click a node to read it · arrows turn, + − zoom, 0 resets · [ ] step, Space plays",
            );
        div()
            .id(("x3d", key as usize))
            .key_context("Explain3d")
            .track_focus(&self.keys)
            .on_key_down(cx.listener(move |this, e: &gpui_kit::KeyDownEvent, _, cx| {
                this.explain_keys(key, e, cx);
            }))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(
                col.child(body).child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(bar))
                        .child(help),
                ),
            )
            .into_any_element()
    }
}
