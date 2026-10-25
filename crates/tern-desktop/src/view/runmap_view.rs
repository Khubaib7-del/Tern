//! The run panel's Graph view: "FIG 1 · CHAT MAP" (an isometric hairline
//! figure of the active chat: the question at the centre, the sources,
//! claims, workers and key terms its run touched around it, and the run's
//! phases as a strip; the corner readout follows the pointer) and the
//! spend-over-time chart under it, both from `crate::runmap` and
//! `crate::chatgraph`. The graph is cached under
//! `crate::chatgraph::graph_key`, so it is rebuilt when the chat, the run
//! or the answer changes (a new source, a new check, switching chats)
//! and never per frame or per pointer move. Lines draw in on the motion
//! curve when the view opens or the graph changes; the newest node's dot
//! travels while the run goes. Reduced motion shows the finished figure
//! and a still dot.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, PathBuilder,
    Pixels, SharedString, Window, canvas, div, point, px,
};
use tern_agent::{Run, Verdict};

use super::Workspace;
use crate::chatgraph::{ChatGraph, NodeKind};
use crate::model::RunView;
use crate::runmap::{self, Figure, Ink, PhaseState, RunMap, Target};
use crate::theme::Palette;

/// How long the figure takes to draw in.
const DRAW_IN: Duration = Duration::from_millis(900);
/// One trip of the travelling dot.
const TRAVEL: Duration = Duration::from_millis(1800);
/// The figure card's height.
const CARD_H: f32 = 270.;
/// The chart's height.
const CHART_H: f32 = 150.;

/// The graph built for one key, and its figure for one card size.
#[derive(Debug)]
pub(super) struct GraphCache {
    pub key: u64,
    pub map: Rc<RunMap>,
    pub graph: Rc<ChatGraph>,
    pub fig: Option<((u32, u32), Rc<Figure>)>,
}

/// The Graph view's pointer state, cache, and the rectangles last painted.
#[derive(Debug, Default)]
pub(super) struct GraphUi {
    pub hover: Option<Target>,
    /// Where across the chart the pointer is, 0 to 1.
    pub chart_x: Option<f32>,
    /// When the view opened (or the graph last changed), for the draw-in.
    pub opened: Option<Instant>,
    pub card: Rc<Cell<[f32; 4]>>,
    pub chart: Rc<Cell<[f32; 4]>>,
    pub cache: Rc<RefCell<Option<GraphCache>>>,
}

impl GraphUi {
    /// The figure for a `w` × `h` card, laid out once per graph and size.
    fn figure(cache: &RefCell<Option<GraphCache>>, w: f32, h: f32) -> Option<Rc<Figure>> {
        let mut c = cache.borrow_mut();
        let c = c.as_mut()?;
        let size = (w.round() as u32, h.round() as u32);
        if let Some((s, f)) = &c.fig
            && *s == size
        {
            return Some(f.clone());
        }
        let f = Rc::new(runmap::figure(&c.map, &c.graph, w, h));
        c.fig = Some((size, f.clone()));
        Some(f)
    }
}

fn stroke(
    pts: &[(f32, f32)],
    o: (f32, f32),
    w: f32,
    dash: Option<f32>,
) -> Option<gpui_kit::Path<Pixels>> {
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

/// A graph node's colour: read sources and supported claims in the text
/// colour, results only seen faint, doubtful claims and running workers
/// in the accent, terms muted; the newest node in the accent.
fn node_ink(graph: &ChatGraph, i: usize, c: Inks) -> Hsla {
    let Some(n) = graph.nodes.get(i) else {
        return c.faint;
    };
    if n.fresh {
        return c.accent;
    }
    match &n.kind {
        NodeKind::Source { read: true, .. } => c.text,
        NodeKind::Source { read: false, .. } => c.faint,
        NodeKind::Worker { running, .. } => {
            if *running {
                c.accent
            } else {
                c.text
            }
        }
        NodeKind::Claim { verdict, .. } => match verdict {
            Verdict::Supported => c.text,
            Verdict::Partly => c.muted,
            Verdict::NotSupported | Verdict::NoSource => c.accent,
        },
        NodeKind::Entity { .. } => c.muted,
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_figure(
    fig: &Figure,
    map: &RunMap,
    graph: &ChatGraph,
    hover: Option<Target>,
    o: (f32, f32),
    drawn: f32,
    travel: f32,
    c: Inks,
    window: &mut Window,
) {
    for l in &fig.lines {
        let (col, w) = match l.ink {
            Ink::Hub => (
                if hover == Some(Target::Hub) {
                    c.accent
                } else {
                    c.text
                },
                0.9,
            ),
            Ink::HubInner => (c.muted, 0.9),
            Ink::Phase(i) | Ink::Link(i) => {
                let st = map.phases.get(i).map_or(PhaseState::Pending, |p| p.state);
                let base = match st {
                    PhaseState::Live => c.accent,
                    PhaseState::Done => c.muted,
                    PhaseState::Pending => c.faint,
                };
                let col = match l.ink {
                    Ink::Phase(_) if hover == Some(Target::Phase(i)) => c.text,
                    _ => base,
                };
                (col, 0.8)
            }
            Ink::Node(i) => {
                if hover == Some(Target::Node(i)) {
                    (c.text, 1.3)
                } else {
                    (node_ink(graph, i, c), 0.9)
                }
            }
            Ink::Edge(i) => {
                let fresh = graph.nodes.get(i).is_some_and(|n| n.fresh) && !map.ended;
                if hover == Some(Target::Node(i)) {
                    (c.muted, 1.0)
                } else if fresh {
                    (c.accent, 1.1)
                } else {
                    (c.faint, 0.7)
                }
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
    /// The Graph view's data for the panel's run: rebuilt only when
    /// [`crate::chatgraph::graph_key`] changes (another chat, a new step,
    /// source or check, the answer growing), else the cached copy.
    pub(super) fn graph_data(&self) -> (Rc<RunMap>, Rc<ChatGraph>, bool) {
        let run = self.model.panel_run();
        let (chat, entries, question, answer) = self.model.graph_inputs();
        let key = crate::chatgraph::graph_key(chat, entries, run, question, answer);
        let mut cache = self.graph.cache.borrow_mut();
        if let Some(c) = cache.as_ref()
            && c.key == key
        {
            return (c.map.clone(), c.graph.clone(), false);
        }
        let map = Rc::new(RunMap::from_run(run));
        let graph = Rc::new(ChatGraph::build(run, question, answer));
        // A different set of nodes draws in again; the same nodes with a
        // new count or shade do not.
        let changed = cache
            .as_ref()
            .is_none_or(|c| c.graph.nodes.len() != graph.nodes.len());
        *cache = Some(GraphCache {
            key,
            map: map.clone(),
            graph: graph.clone(),
            fig: None,
        });
        (map, graph, changed)
    }

    /// What is under the pointer at window point (`x`, `y`).
    fn graph_hit(&self, x: f32, y: f32) -> Option<Target> {
        let [cx0, cy0, w, h] = self.graph.card.get();
        let _ = self.graph_data();
        let fig = GraphUi::figure(&self.graph.cache, w, h)?;
        runmap::hit(&fig, x - cx0, y - cy0)
    }

    /// The Graph view of the run panel.
    pub(super) fn run_graph(&self, _run: &Run, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        // `Workspace::render` refreshed the cache (and restarted the
        // draw-in when the graph changed) before this.
        let (map, graph, _) = self.graph_data();
        let now = Instant::now();
        let ui = &self.graph;
        let opened = ui.opened.unwrap_or(now);
        let reduced = self.reduced_motion;
        let drawn = if reduced {
            1.
        } else {
            crate::motion::ease(
                ((now - opened).as_secs_f32() / DRAW_IN.as_secs_f32()).clamp(0., 1.),
            )
        };
        let travel = if reduced {
            0.5
        } else {
            (now.duration_since(opened).as_secs_f32() / TRAVEL.as_secs_f32()).fract()
        };
        let me = cx.entity_id();
        let running = !map.ended
            && (map
                .current
                .is_some_and(|c| map.phases[c].state == PhaseState::Live)
                || graph.nodes.iter().any(|n| n.fresh));
        let animating = !reduced && (drawn < 1. || running);
        let (line1, line2) = runmap::readout(&map, &graph, ui.hover);
        let inks = Inks {
            text: p.text,
            muted: p.muted,
            faint: p.faint.opacity(0.7),
            accent: p.accent,
        };
        let card_cell = ui.card.clone();
        let cache = ui.cache.clone();
        let hover = ui.hover;
        let empty = graph.is_empty();
        let mono = |t: &'static str| {
            div()
                .absolute()
                .top(px(8.))
                .text_size(px(10.5))
                .font_family(crate::assets::MONO_FONT)
                .text_color(p.faint)
                .child(t)
        };
        let fig_map = map.clone();
        let fig_graph = graph.clone();
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
            .when(hover.is_some(), |d| d.cursor_pointer())
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                let at = this.graph_hit(f32::from(e.position.x), f32::from(e.position.y));
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
                    let deep = this.model.panel_run().deep.is_some();
                    let (_, graph, _) = this.graph_data();
                    // A phase or node opens the view that shows it.
                    let view = match this.graph.hover {
                        Some(Target::Phase(i)) => Some(match runmap::PHASES[i] {
                            "read" if deep => RunView::Workers,
                            "write" => RunView::Summary,
                            "verify" => RunView::Report,
                            _ => RunView::Detailed,
                        }),
                        Some(Target::Node(i)) => match graph.nodes.get(i).map(|n| &n.kind) {
                            Some(NodeKind::Claim { .. }) => Some(RunView::Report),
                            Some(NodeKind::Worker { .. }) => Some(RunView::Workers),
                            Some(NodeKind::Source { .. }) => Some(RunView::Ledger),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some(view) = view {
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
                        let Some(fig) = GraphUi::figure(&cache, r[2], r[3]) else {
                            return;
                        };
                        paint_figure(
                            &fig,
                            &fig_map,
                            &fig_graph,
                            hover,
                            (r[0], r[1]),
                            drawn,
                            travel,
                            inks,
                            window,
                        );
                        if animating {
                            super::frame::next_frame_for(window, me);
                        }
                    },
                )
                .size_full(),
            )
            .child(mono("FIG 1 · CHAT MAP").left(px(10.)))
            .child(mono(if empty { "" } else { "POINT AT A NODE" }).right(px(10.)))
            .when(empty, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(CARD_H / 2. + 26.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_0p5()
                        .text_size(px(12.))
                        .text_color(p.muted)
                        .child("Nothing to map in this chat yet")
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(p.faint)
                                .child("Sources, checked claims and key terms show here."),
                        ),
                )
            })
            .child(
                div()
                    .absolute()
                    .right(px(10.))
                    .bottom(px(8.))
                    .max_w(px(260.))
                    .flex()
                    .flex_col()
                    .items_end()
                    .text_size(px(11.))
                    .font_family(crate::assets::MONO_FONT)
                    .text_color(p.muted)
                    .child(
                        div()
                            .truncate()
                            .when(hover.is_some(), |d| d.text_color(p.text))
                            .child(SharedString::from(line1)),
                    )
                    .child(div().truncate().child(SharedString::from(line2))),
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
    fn spend_chart(
        &self,
        map: &RunMap,
        drawn: f32,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let top = map
            .spend
            .last()
            .map_or(0, |s| s.tokens)
            .max(map.budget.unwrap_or(0));
        let (max, ticks) = runmap::chart_ticks(top);
        let end = map
            .spend
            .last()
            .map_or(Duration::ZERO, |s| s.at)
            .as_secs_f32()
            .max(1.);
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
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .right(px(48.))
                            .child(
                                canvas(
                                    |_, _, _: &mut App| (),
                                    move |b, _, window, _: &mut App| {
                                        let (ox, oy) =
                                            (f32::from(b.origin.x), f32::from(b.origin.y));
                                        let (w, h) =
                                            (f32::from(b.size.width), f32::from(b.size.height));
                                        chart_cell.set([ox, oy, w, h]);
                                        let at = |p: (f32, f32)| (p.0 * w, h - p.1 * h);
                                        // Grid lines at the ticks.
                                        for k in 0..4 {
                                            let y = h - h * k as f32 / 3.;
                                            if let Some(path) =
                                                stroke(&[(0., y), (w, y)], (ox, oy), 0.6, None)
                                            {
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
                                            if let Some(path) =
                                                stroke(&[(0., y), (w, y)], (ox, oy), 1.0, Some(4.))
                                            {
                                                window.paint_path(path, faint);
                                            }
                                        }
                                        if let Some(fx) = x
                                            && let Some(path) = stroke(
                                                &[(fx * w, 0.), (fx * w, h)],
                                                (ox, oy),
                                                0.8,
                                                None,
                                            )
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
