//! An Explain in 3D tab: the concept as a small isometric machine.
//!
//! Solid, Anatomy-style faces (lit top, a lit side and a shaded side, in
//! the text colour over the canvas) with a Hairline overlay (every edge in
//! a thin line, the floor grid, dashed links). The parts a step is about
//! take the one accent and fade in on the motion curve; the pointer tilts
//! the machine a few degrees and lifts the part under it. Steps go back
//! and forward with the buttons, the arrow keys, or autoplay (Space).
//! Reduced motion keeps it square and switches steps without a fade.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent,
    PathBuilder, Pixels, SharedString, Window, canvas, div, point, px,
};
use tern_agent::explain::{Drawing, Edge, FaceKind, ink_amount};

use super::Workspace;
use super::study_view::serif;
use crate::explain::{ExplainState, fit};
use crate::model::Screen;
use crate::motion::lerp_color;
use crate::theme::Palette;

/// Colours and amounts the painter needs, captured for the frame.
struct Paint {
    drawing: Drawing,
    lit: Vec<f32>,
    hover: Option<usize>,
    canvas: Hsla,
    ink: Hsla,
    faint: Hsla,
    muted: Hsla,
    accent: Hsla,
    dark: bool,
}

fn stroke(
    pts: &[(f32, f32)],
    f: &tern_agent::explain::Fit,
    o: (f32, f32),
    w: f32,
    closed: bool,
    dash: Option<f32>,
) -> Option<gpui_kit::Path<Pixels>> {
    let mut pb = PathBuilder::stroke(px(w));
    if let Some(d) = dash {
        pb = pb.dash_array(&[px(d), px(d * 0.8)]);
    }
    let (first, rest) = pts.split_first()?;
    let at = |p: (f32, f32)| {
        let (x, y) = f.at(p);
        point(px(o.0 + x), px(o.1 + y))
    };
    pb.move_to(at(*first));
    for p in rest {
        pb.line_to(at(*p));
    }
    if closed {
        pb.close();
    }
    pb.build().ok()
}

fn fill(
    pts: &[(f32, f32)],
    f: &tern_agent::explain::Fit,
    o: (f32, f32),
) -> Option<gpui_kit::Path<Pixels>> {
    let mut pb = PathBuilder::fill();
    let (first, rest) = pts.split_first()?;
    let at = |p: (f32, f32)| {
        let (x, y) = f.at(p);
        point(px(o.0 + x), px(o.1 + y))
    };
    pb.move_to(at(*first));
    for p in rest {
        pb.line_to(at(*p));
    }
    pb.close();
    pb.build().ok()
}

/// One hairline: in the accent when its part is lit or hovered.
fn paint_edge(
    e: &Edge,
    pt: &Paint,
    f: &tern_agent::explain::Fit,
    o: (f32, f32),
    window: &mut Window,
) {
    let lit = e.part.and_then(|i| pt.lit.get(i).copied()).unwrap_or(0.0);
    let hovered = e.part.is_some() && e.part == pt.hover;
    let (c, wid) = if lit > 0.5 || hovered {
        (pt.accent, 1.4)
    } else {
        (pt.ink.opacity(0.85), 0.9)
    };
    if let Some(p) = stroke(&e.pts, f, o, wid, e.closed, None) {
        window.paint_path(p, c);
    }
}

fn paint(b: Bounds<Pixels>, pt: &Paint, window: &mut Window) {
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    let o = (f32::from(b.origin.x), f32::from(b.origin.y));
    let f = fit(&pt.drawing, w, h);
    // The floor grid: the faintest hairlines.
    for e in &pt.drawing.grid {
        if let Some(p) = stroke(&e.pts, &f, o, 0.6, false, None) {
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
        if let Some(p) = fill(&face.pts, &f, o) {
            window.paint_path(p, c);
        }
        // The same colour as a thin stroke closes the seams between facets.
        if let Some(p) = stroke(&face.pts, &f, o, 0.6, true, None) {
            window.paint_path(p, c);
        }
        for &ei in &after[i] {
            paint_edge(&pt.drawing.edges[ei], pt, &f, o, window);
        }
    }
    for &ei in &after[pt.drawing.faces.len()] {
        paint_edge(&pt.drawing.edges[ei], pt, &f, o, window);
    }
    // Links: dashed, with a head.
    for l in &pt.drawing.links {
        if let Some(p) = stroke(&[l.a, l.b], &f, o, 1.0, false, Some(4.0)) {
            window.paint_path(p, pt.muted);
        }
        if let Some(p) = fill(&l.head, &f, o) {
            window.paint_path(p, pt.muted);
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
        self.bridge.explain_build(key, ask);
        self.keys.focus(window, cx);
        cx.notify();
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
        match e.keystroke.key.as_str() {
            "right" | "down" | "pagedown" => self.explain_step(key, 1, cx),
            "left" | "up" | "pageup" => self.explain_step(key, -1, cx),
            "space" => self.explain_play(key, cx),
            _ => return,
        }
        cx.stop_propagation();
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

    /// The explainer in tab `key`.
    pub(super) fn explain_view(&self, key: u32, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = self.explains.get(&key) else {
            return div().into_any_element();
        };
        let now = Instant::now();
        let reduced = self.reduced_motion;
        let header = div()
            .flex()
            .items_start()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                gpui_kit::component::Icon::new(AssetIcon::Cuboid)
                                    .small()
                                    .text_color(p.accent),
                            )
                            .child(
                                serif(
                                    s.scene
                                        .as_ref()
                                        .map(|x| x.concept.clone())
                                        .unwrap_or_else(|| s.asked.clone()),
                                    p,
                                )
                                .text_2xl(),
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
                    .gap_1()
                    .flex_none()
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
            .size_full()
            .px_4()
            .py_4()
            .child(header);
        for line in [&s.note, &s.saved].into_iter().flatten() {
            col = col.child(
                div()
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
        let Some(scene) = &s.scene else {
            let body = if let Some(e) = &s.error {
                div()
                    .text_sm()
                    .text_color(p.muted)
                    .child(e.clone())
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(p.muted)
                    .child(Spinner::new().small())
                    .child("Picking the parts and steps from your sources…")
                    .into_any_element()
            };
            return col.child(body).into_any_element();
        };
        if !scene.summary.is_empty() {
            col = col.child(
                div()
                    .text_sm()
                    .text_color(p.muted)
                    .child(scene.summary.clone()),
            );
        }
        // The drawing.
        let drawing = s.drawing().unwrap_or_default();
        let lit: Vec<f32> = (0..scene.parts.len())
            .map(|i| s.lit(i, now, reduced))
            .collect();
        let bounds = s.bounds.clone();
        let [_, _, bw, bh] = bounds.get();
        let labels: Vec<AnyElement> = if bw > 1.0 {
            let f = fit(&drawing, bw, bh);
            drawing
                .labels
                .iter()
                .map(|l| {
                    let (x, y) = f.at(l.at);
                    let on =
                        lit.get(l.part).copied().unwrap_or(0.0) > 0.5 || s.hover == Some(l.part);
                    div()
                        .absolute()
                        .left(px(x - 80.))
                        .top(px(y - 18.))
                        .w(px(160.))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .px_1()
                                .text_xs()
                                .bg(p.canvas.opacity(0.75))
                                .rounded_sm()
                                .text_color(if on { p.accent } else { p.text })
                                .child(l.text.clone()),
                        )
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        let pt = Paint {
            drawing,
            lit,
            hover: s.hover,
            canvas: p.canvas,
            ink: p.text,
            faint: p.faint,
            muted: p.muted,
            accent: p.accent,
            dark: self.model.dark,
        };
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
        let stage = div()
            .id(("x3d-stage", key as usize))
            .relative()
            .flex_1()
            .min_h(px(320.))
            .rounded(px(16.))
            .border_1()
            .border_color(p.border)
            .bg(p.canvas)
            .overflow_hidden()
            .cursor_pointer()
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                if let Some(s) = this.explains.get_mut(&key) {
                    let [x, y, w, h] = s.bounds.get();
                    let (mx, my) = (f32::from(e.position.x) - x, f32::from(e.position.y) - y);
                    let before = s.hover;
                    if mx < 0.0 || my < 0.0 || mx > w || my > h {
                        s.leave();
                    } else {
                        s.point(mx, my, w, h);
                    }
                    if s.hover != before || !this.reduced_motion {
                        cx.notify();
                    }
                }
            }))
            .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                if !*over && let Some(s) = this.explains.get_mut(&key) {
                    s.leave();
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.keys.focus(window, cx);
                    if let Some(s) = this.explains.get_mut(&key)
                        && let Some(part) = s.hover
                        && let Some(step) = s.step_of(part)
                    {
                        s.autoplay = false;
                        s.go(step, Instant::now());
                    }
                    cx.notify();
                }),
            )
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
                        paint(b, &pt, window);
                    },
                )
                .size_full(),
            )
            .children(labels)
            .children(readout);
        // The step bar and caption.
        let n = scene.steps.len();
        let i = s.step.min(n.saturating_sub(1));
        let step = scene.steps.get(i);
        let dots = (0..n).map(|j| {
            div()
                .id(("x3d-dot", key as usize * 100 + j))
                .size(px(8.))
                .rounded_full()
                .cursor_pointer()
                .when(j == i, |d| d.bg(p.accent))
                .when(j != i, |d| d.border_1().border_color(p.faint))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(s) = this.explains.get_mut(&key) {
                        s.autoplay = false;
                        s.go(j, Instant::now());
                    }
                    cx.notify();
                }))
        });
        let bar = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Button::new(("x3d-prev", key as usize))
                    .icon(AssetIcon::ChevronLeft)
                    .ghost()
                    .small()
                    .disabled(i == 0)
                    .tooltip("Previous step (←)")
                    .on_click(cx.listener(move |this, _, _, cx| this.explain_step(key, -1, cx))),
            )
            .child(
                Button::new(("x3d-play", key as usize))
                    .icon(if s.autoplay {
                        AssetIcon::Pause
                    } else {
                        AssetIcon::Play
                    })
                    .ghost()
                    .small()
                    .tooltip(if s.autoplay {
                        "Pause (Space)"
                    } else {
                        "Play the steps (Space)"
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.explain_play(key, cx))),
            )
            .child(
                Button::new(("x3d-next", key as usize))
                    .icon(AssetIcon::ChevronRight)
                    .ghost()
                    .small()
                    .disabled(i + 1 >= n)
                    .tooltip("Next step (→)")
                    .on_click(cx.listener(move |this, _, _, cx| this.explain_step(key, 1, cx))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(format!("Step {} of {n}", i + 1))),
            )
            .child(div().flex().gap_1p5().items_center().children(dots));
        let caption = step.map(|st| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .rounded(px(16.))
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .child(div().text_sm().text_color(p.text).child(st.caption.clone()))
                .children(st.cite().map(|c| {
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(format!("Source: {c}")))
                }))
        });
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
            .child(col.child(stage).child(bar).children(caption).child(
                div().text_xs().text_color(p.faint).child(
                    "Hover to see a part; click it to jump to its step. ← → step, Space plays.",
                ),
            ))
            .into_any_element()
    }
}
