//! The run panel's Graph view: "FIG 1 · RUN MAP" (an isometric hairline
//! figure of the run, no words inside it, its corner readout following the
//! pointer) and the spend-over-time chart under it, both drawn from
//! `crate::runmap`. Lines draw in on the motion curve when the view opens;
//! the live phase's dot travels while the run goes. Reduced motion shows
//! the finished figure and a still dot.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, PathBuilder,
    Pixels, SharedString, Window, canvas, div, point, px,
};
use tern_agent::Run;

use super::Workspace;
use crate::model::RunView;
use crate::runmap::{self, Figure, Ink, PhaseState, RunMap};
use crate::theme::Palette;

/// How long the figure takes to draw in.
const DRAW_IN: Duration = Duration::from_millis(900);
/// One trip of the travelling dot.
const TRAVEL: Duration = Duration::from_millis(1800);
/// The figure card's height.
const CARD_H: f32 = 270.;
/// The chart's height.
const CHART_H: f32 = 150.;

/// The Graph view's pointer state and the rectangles last painted.
#[derive(Debug, Default)]
pub(super) struct GraphUi {
    pub hover: Option<usize>,
    /// Where across the chart the pointer is, 0 to 1.
    pub chart_x: Option<f32>,
    /// When the view opened, for the draw-in.
    pub opened: Option<Instant>,
    pub card: Rc<Cell<[f32; 4]>>,
    pub chart: Rc<Cell<[f32; 4]>>,
}

fn stroke(pts: &[(f32, f32)], o: (f32, f32), w: f32, dash: Option<f32>) -> Option<gpui_kit::Path<Pixels>> {
    let mut pb = PathBuilder::stroke(px(w));
    if let Some(d) = dash {
        pb = pb.dash_array(&[px(d), px(d * 1.5)]);
    }
    let (first, rest) = pts.split_first()?;
    pb.move_to(point(px(o.0 + first.0), px(o.1 + first.1)));
    for p in rest {
        pb.line_to(point(px(o.0 + p.0), px(o.1 + p.1)));
    }
    pb.build().ok()
}

fn fill(pts: &[(f32, f32)], o: (f32, f32)) -> Option<gpui_kit::Path<Pixels>> {
    let mut pb = PathBuilder::fill();
    let (first, rest) = pts.split_first()?;
    pb.move_to(point(px(o.0 + first.0), px(o.1 + first.1)));
    for p in rest {
        pb.line_to(point(px(o.0 + p.0), px(o.1 + p.1)));
    }
    pb.close();
    pb.build().ok()
}

/// Colours for one frame of the figure.
#[derive(Clone, Copy)]
struct Inks {
    text: Hsla,
    muted: Hsla,
    faint: Hsla,
    accent: Hsla,
}

#[allow(clippy::too_many_arguments)]
fn paint_figure(
    fig: &Figure,
    map: &RunMap,
    hover: Option<usize>,
    o: (f32, f32),
    drawn: f32,
    travel: f32,
    c: Inks,
    window: &mut Window,
) {
    for l in &fig.lines {
        let (col, w) = match l.ink {
            Ink::Hub => (c.text, 0.9),
            Ink::HubInner => (c.muted, 0.9),
            Ink::Phase(i) | Ink::Link(i) | Ink::Cube(i) => {
                let st = map.phases.get(i).map_or(PhaseState::Pending, |p| p.state);
                let base = match st {
                    PhaseState::Live => c.accent,
                    PhaseState::Done => c.text,
                    PhaseState::Pending => c.faint,
                };
                let col = match l.ink {
                    Ink::Cube(_) => c.muted,
                    Ink::Phase(_) if hover == Some(i) => c.text,
                    _ => base,
                };
                let w = if matches!(l.ink, Ink::Link(_)) && st == PhaseState::Live {
                    1.2
                } else if matches!(l.ink, Ink::Cube(_)) {
                    0.7
                } else {
                    0.9
                };
                (col, w)
            }
        };
        let pts = runmap::trim(&l.pts, drawn);
        if let Some(path) = stroke(&pts, o, w, l.dotted.then_some(2.)) {
            window.paint_path(path, col);
        }
    }
    if drawn >= 1.
        && let Some([a, ctl, b]) = fig.live
    {
        let (x, y) = runmap::bezier(a, ctl, b, crate::motion::ease(travel));
        let r = 2.3;
        let ring: Vec<(f32, f32)> = (0..12)
            .map(|k| {
                let t = k as f32 / 12. * std::f32::consts::TAU;
                (x + r * t.cos(), y + r * t.sin())
            })
            .collect();
        if let Some(path) = fill(&ring, o) {
            window.paint_path(path, c.accent);
        }
    }
}

impl Workspace {
    /// The Graph view of the run panel.
    pub(super) fn run_graph(&self, run: &Run, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let map = RunMap::from_run(run);
        let ui = &self.graph;
        let now = Instant::now();
        let opened = ui.opened.unwrap_or(now);
        let reduced = self.reduced_motion;
        let drawn = if reduced {
            1.
        } else {
            crate::motion::ease(((now - opened).as_secs_f32() / DRAW_IN.as_secs_f32()).clamp(0., 1.))
        };
        let travel = if reduced {
            0.5
        } else {
            (now.duration_since(opened).as_secs_f32() / TRAVEL.as_secs_f32()).fract()
        };
        let animating = !reduced && (drawn < 1. || map.current.is_some_and(|c| map.phases[c].state == PhaseState::Live));
        let (line1, line2) = map.readout(ui.hover);
        let inks = Inks {
            text: p.text,
            muted: p.muted,
            faint: p.faint.opacity(0.7),
            accent: p.accent,
        };
        let card_cell = ui.card.clone();
        let hover = ui.hover;
        let fig_map = map.clone();
        let mono = |t: &'static str| {
            div()
                .absolute()
                .top(px(8.))
                .text_size(px(10.5))
                .font_family(crate::assets::MONO_FONT)
                .text_color(p.faint)
                .child(t)
        };
        let card = div()
            .id("runmap-card")
            .relative()
            .w_full()
            .h(px(CARD_H))
            .rounded(px(8.))
            .border_1()
            .border_color(p.border)
            .bg(p.canvas)
            .overflow_hidden()
            .cursor_pointer()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                let [x, y, w, h] = this.graph.card.get();
                let map = RunMap::from_run(this.model.panel_run());
                let fig = runmap::figure(&map, w, h);
                let at = runmap::hit(&fig, f32::from(e.position.x) - x, f32::from(e.position.y) - y);
                if at != this.graph.hover {
                    this.graph.hover = at;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, over: &bool, _, cx| {
                if !*over && this.graph.hover.is_some() {
                    this.graph.hover = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    // A phase opens the view that shows its steps.
                    if let Some(i) = this.graph.hover {
                        let deep = this.model.panel_run().deep.is_some();
                        let view = match runmap::PHASES[i] {
                            "read" if deep => RunView::Workers,
                            "write" => RunView::Summary,
                            "verify" => RunView::Report,
                            _ => RunView::Detailed,
                        };
                        this.model.set_run_view(view);
                        this.graph.hover = None;
                        cx.notify();
                    }
                }),
            )
            .child(
                canvas(
                    |_, _, _: &mut App| (),
                    move |b, _, window, _: &mut App| {
                        let r = [
                            f32::from(b.origin.x),
                            f32::from(b.origin.y),
                            f32::from(b.size.width),
                            f32::from(b.size.height),
                        ];
                        card_cell.set(r);
                        let fig = runmap::figure(&fig_map, r[2], r[3]);
                        paint_figure(&fig, &fig_map, hover, (r[0], r[1]), drawn, travel, inks, window);
                        if animating {
                            window.request_animation_frame();
                        }
                    },
                )
                .size_full(),
            )
            .child(mono("FIG 1 · RUN MAP").left(px(10.)))
            .child(mono("POINT AT A PHASE").right(px(10.)))
            .child(
                div()
                    .absolute()
                    .right(px(10.))
                    .bottom(px(8.))
                    .flex()
                    .flex_col()
                    .items_end()
                    .text_size(px(11.))
                    .font_family(crate::assets::MONO_FONT)
                    .text_color(p.muted)
                    .child(
                        div()
                            .when(hover.is_some(), |d| d.text_color(p.text))
                            .child(SharedString::from(line1)),
                    )
                    .child(SharedString::from(line2)),
            );
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(card)
            .child(self.spend_chart(&map, drawn, p, cx))
            .into_any_element()
    }

    /// Spend over time against the budget, with a crosshair and tooltip.
    fn spend_chart(&self, map: &RunMap, drawn: f32, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let top = map
            .spend
            .last()
            .map_or(0, |s| s.tokens)
            .max(map.budget.unwrap_or(0));
        let (max, ticks) = runmap::chart_ticks(top);
        let end = map.spend.last().map_or(Duration::ZERO, |s| s.at).as_secs_f32().max(1.);
        let pts: Vec<(f32, f32)> = map
            .spend
            .iter()
            .map(|s| (s.at.as_secs_f32() / end, s.tokens as f32 / max as f32))
            .collect();
        let budget = map.budget.map(|b| b as f32 / max as f32);
        let chart_cell = self.graph.chart.clone();
        let x = self.graph.chart_x;
        let (text, accent_fill, faint) = (p.text, p.text.opacity(0.07), p.faint);
        let legend_item = |dashed: bool, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(p.muted)
                .child(if dashed {
                    div()
                        .flex()
                        .gap(px(2.))
                        .child(div().w(px(5.)).h(px(2.)).bg(p.muted))
                        .child(div().w(px(5.)).h(px(2.)).bg(p.muted))
                } else {
                    div().child(div().w(px(12.)).h(px(2.)).rounded_full().bg(p.text))
                })
                .child(label)
        };
        let tip = x.map(|fx| {
            let (t, s, b) = runmap::tooltip(map, fx);
            div()
                .absolute()
                .top(px(18.))
                .left(gpui_kit::relative((fx + 0.04).min(0.55)))
                .px_3()
                .py_2()
                .rounded_lg()
                .border_1()
                .border_color(p.border)
                .bg(p.raised)
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_1()
                .text_xs()
                .child(div().text_color(p.muted).child(SharedString::from(t)))
                .child(div().text_color(p.text).child(SharedString::from(s)))
                .children(b.map(|b| div().text_color(p.muted).child(SharedString::from(b))))
        });
        let axis = div()
            .absolute()
            .right_0()
            .top_0()
            .h(px(CHART_H))
            .w(px(44.))
            .flex()
            .flex_col()
            .justify_between()
            .items_end()
            .text_xs()
            .font_family(crate::assets::MONO_FONT)
            .text_color(p.faint)
            .children(
                ticks
                    .iter()
                    .rev()
                    .map(|t| div().child(SharedString::from(runmap::tokens_label(*t)))),
            );
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(legend_item(false, "Spend"))
                    .when(budget.is_some(), |d| d.child(legend_item(true, "Budget")))
                    .child(div().flex_1())
                    .child(div().text_xs().text_color(p.faint).child("tokens")),
            )
            .child(
                div()
                    .id("spend-chart")
                    .relative()
                    .w_full()
                    .h(px(CHART_H))
                    .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                        let [x, _, w, _] = this.graph.chart.get();
                        if w > 0. {
                            let fx = ((f32::from(e.position.x) - x) / w).clamp(0., 1.);
                            this.graph.chart_x = Some(fx);
                            cx.notify();
                        }
                    }))
                    .on_hover(cx.listener(|this, over: &bool, _, cx| {
                        if !*over {
                            this.graph.chart_x = None;
                            cx.notify();
                        }
                    }))
                    .child(
                        div().absolute().left_0().top_0().bottom_0().right(px(48.)).child(
                            canvas(
                                |_, _, _: &mut App| (),
                                move |b, _, window, _: &mut App| {
                                    let (ox, oy) = (f32::from(b.origin.x), f32::from(b.origin.y));
                                    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                                    chart_cell.set([ox, oy, w, h]);
                                    let at = |p: (f32, f32)| (p.0 * w, h - p.1 * h);
                                    // Grid lines at the ticks.
                                    for k in 0..4 {
                                        let y = h - h * k as f32 / 3.;
                                        if let Some(path) = stroke(&[(0., y), (w, y)], (ox, oy), 0.6, None) {
                                            window.paint_path(path, faint.opacity(0.25));
                                        }
                                    }
                                    let line: Vec<(f32, f32)> = runmap::trim(
                                        &pts.iter().map(|p| at(*p)).collect::<Vec<_>>(),
                                        drawn,
                                    );
                                    if line.len() >= 2 {
                                        let mut area = line.clone();
                                        area.push((line.last().map_or(0., |p| p.0), h));
                                        area.push((0., h));
                                        if let Some(path) = fill(&area, (ox, oy)) {
                                            window.paint_path(path, accent_fill);
                                        }
                                        if let Some(path) = stroke(&line, (ox, oy), 1.6, None) {
                                            window.paint_path(path, text);
                                        }
                                    }
                                    if let Some(bv) = budget {
                                        let y = h - bv * h;
                                        if let Some(path) = stroke(&[(0., y), (w, y)], (ox, oy), 1.0, Some(4.)) {
                                            window.paint_path(path, faint);
                                        }
                                    }
                                    if let Some(fx) = x
                                        && let Some(path) =
                                            stroke(&[(fx * w, 0.), (fx * w, h)], (ox, oy), 0.8, None)
                                    {
                                        window.paint_path(path, faint.opacity(0.6));
                                    }
                                },
                            )
                            .size_full(),
                        ),
                    )
                    .child(axis)
                    .children(tip),
            )
            .into_any_element()
    }
}
