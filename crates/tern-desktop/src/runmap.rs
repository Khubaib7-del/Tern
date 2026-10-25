//! The right panel's Graph tab, GPUI-free so it is unit tested: "FIG 1 ·
//! CHAT MAP", an isometric hairline figure of the active chat's run. The
//! question is the hub; around it, on rings, sit what the run touched:
//! deep-research workers, the answer's checked claims, the sources read
//! (solid) or only seen (faint), and the answer's key terms
//! (`crate::chatgraph`), each joined by curved hairlines (dotted when
//! weak). Placement comes from the chat's own data, so two chats look
//! different. The run's six phases are a small strip along the bottom
//! (dotted where not reached, the live one in the accent with a dot
//! travelling into it). Under the figure: the spend-over-time chart
//! (the v2 concept's s12).
//!
//! Everything comes from the run's own events (`tern_agent::Run`): which
//! phase each step belongs to, counts, and tokens. Tern has no price
//! table yet, so "spend" is measured tokens and the budget line is the
//! deep plan's token budget when there is one; no dollar figure is made up.

use std::time::Duration;

use tern_agent::{Run, StepKind};

use crate::chatgraph::{ChatGraph, NodeKind};

/// The phases, in the order a Tern turn goes through them.
pub const PHASES: [&str; 6] = ["search", "screen", "read", "synthesize", "write", "verify"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseState {
    /// Not reached: drawn dotted, in the faint colour.
    Pending,
    /// Where the run is now: its link in the accent, with the dot.
    Live,
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase {
    pub name: &'static str,
    pub state: PhaseState,
    /// What the phase did: results found, sources ranked, pages read,
    /// model calls, claims checked.
    pub count: u64,
    pub unit: &'static str,
    /// Tokens spent while it was the phase.
    pub tokens: u64,
}

/// One point of the spend line: time since the turn started, and tokens
/// spent by then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpendPoint {
    pub at: Duration,
    pub tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunMap {
    pub phases: Vec<Phase>,
    /// The live phase, or the last one reached once the run ended.
    pub current: Option<usize>,
    pub ended: bool,
    /// Search results seen, and pages read ("kept").
    pub sources: u64,
    pub kept: u64,
    pub tokens: u64,
    pub spend: Vec<SpendPoint>,
    /// The deep plan's token budget, summed over its workers.
    pub budget: Option<u64>,
}

/// Which phase a step belongs to; `None` keeps the phase it is in (a model
/// call belongs to whatever phase is going on).
fn phase_of(kind: StepKind, event: &str) -> Option<usize> {
    if event.starts_with("claim") || event.starts_with("verify") || event.contains("checked") {
        return Some(5);
    }
    match kind {
        StepKind::Search => Some(0),
        StepKind::Decision | StepKind::Plan => Some(1),
        StepKind::Read | StepKind::Worker => Some(2),
        _ => None,
    }
}

/// `12.4k`, `980`, `1.2M`.
pub fn tokens_label(n: u64) -> String {
    tern_agent::run::compact(n)
}

impl RunMap {
    pub fn from_run(run: &Run) -> Self {
        let mut phases: Vec<Phase> = PHASES
            .iter()
            .map(|&name| Phase {
                name,
                state: PhaseState::Pending,
                count: 0,
                unit: match name {
                    "search" => "found",
                    "screen" => "ranked",
                    "read" => "read",
                    "synthesize" => "model calls",
                    "write" => "answer",
                    _ => "claims",
                },
                tokens: 0,
            })
            .collect();
        let mut seen = [false; 6];
        let mut cur: Option<usize> = None;
        let mut spent = 0u64;
        let mut spend = vec![SpendPoint {
            at: Duration::ZERO,
            tokens: 0,
        }];
        let mut sources = 0u64;
        let mut kept = 0u64;
        for s in &run.steps {
            let mut ph = phase_of(s.kind, &s.event);
            if s.kind == StepKind::Model {
                // Thinking over what was read is synthesis; before any
                // reading it belongs to the phase going on.
                ph = match cur {
                    Some(c) if c >= 2 => Some(3),
                    // Before any search it is no phase yet: only a real
                    // search lights the search phase.
                    c => c,
                };
            }
            if let Some(p) = ph {
                seen[p] = true;
                cur = Some(cur.map_or(p, |c| if p == 5 || p >= c { p } else { c }));
                match s.kind {
                    // Only a search that ran adds sources: its result
                    // count, or the hits it carries.
                    StepKind::Search if s.ok => {
                        let n = s
                            .payload
                            .pointer("/meta/results")
                            .and_then(|v| v.as_u64())
                            .or_else(|| {
                                s.payload
                                    .get("hits")
                                    .and_then(|h| h.as_array())
                                    .map(|a| a.len() as u64)
                            })
                            .unwrap_or(0);
                        phases[0].count += n;
                        sources += n;
                    }
                    StepKind::Read if s.ok => {
                        phases[2].count += 1;
                        kept += 1;
                    }
                    StepKind::Decision | StepKind::Plan => phases[1].count += 1,
                    // A model call is counted only as synthesis; one made
                    // before any reading is not a search.
                    StepKind::Model if p == 3 => phases[3].count += 1,
                    _ => {}
                }
            }
            if let Some((i, o)) = s.tokens {
                spent += i + o;
                if let Some(p) = cur {
                    phases[p].tokens += i + o;
                }
                spend.push(SpendPoint {
                    at: s.at,
                    tokens: spent,
                });
            }
        }
        // Deep runs count reads per worker.
        if let Some(d) = &run.deep {
            let read: u64 = d.workers.iter().map(|w| w.pages.len() as u64).sum();
            if read > kept {
                phases[2].count = read;
                kept = read;
            }
            if !d.workers.is_empty() {
                seen[2] = true;
            }
        }
        let ended = run.end.is_some();
        if ended {
            seen[4] = true;
            phases[4].count = 1;
            if cur.is_none_or(|c| c < 4) {
                cur = Some(4);
            }
        }
        if !run.checks.is_empty() {
            seen[5] = true;
            phases[5].count = run.checks.len() as u64;
            cur = Some(5);
        }
        for (i, ph) in phases.iter_mut().enumerate() {
            ph.state = if Some(i) == cur && !ended {
                PhaseState::Live
            } else if seen[i] || cur.is_some_and(|c| i < c && i != 1) {
                PhaseState::Done
            } else {
                PhaseState::Pending
            };
        }
        let budget = run
            .deep
            .as_ref()
            .and_then(|d| d.plan.as_ref())
            .map(|p| p.budget.tokens)
            .filter(|t| *t > 0);
        Self {
            phases,
            current: cur,
            ended,
            sources,
            kept,
            tokens: spent,
            spend,
            budget,
        }
    }

    /// The corner readout, two lines: the run, or the phase pointed at.
    pub fn readout(&self, hover: Option<usize>) -> (String, String) {
        if let Some(ph) = hover.and_then(|i| self.phases.get(i)) {
            let what = if ph.state == PhaseState::Pending {
                "not reached".to_owned()
            } else if ph.name == "write" {
                if self.ended { "written" } else { "writing" }.to_owned()
            } else {
                format!("{} {}", ph.count, ph.unit)
            };
            return (
                ph.name.to_owned(),
                format!("{what} · {} tok", tokens_label(ph.tokens)),
            );
        }
        let first = match self.current {
            Some(c) => format!("phase {}/{} · {}", c + 1, PHASES.len(), PHASES[c]),
            None => "not started".to_owned(),
        };
        (
            first,
            format!(
                "{} sources · {} kept · {} tok",
                self.sources,
                self.kept,
                tokens_label(self.tokens)
            ),
        )
    }
}

// ---- The figure ---------------------------------------------------------

/// Floor units to the page: the v2 isometric projection.
pub fn iso(x: f32, y: f32, z: f32) -> (f32, f32) {
    ((x - y) * 0.866, (x + y) * 0.5 - z)
}

/// An isometric box's outline as polylines: the top face (closed) and the
/// two visible sides.
pub fn tile(c: (f32, f32), s: f32, h: f32) -> Vec<Vec<(f32, f32)>> {
    let p = |x: f32, y: f32, z: f32| {
        let (a, b) = iso(x, y, z);
        (c.0 + a, c.1 + b)
    };
    vec![
        vec![
            p(-s, -s, h),
            p(s, -s, h),
            p(s, s, h),
            p(-s, s, h),
            p(-s, -s, h),
        ],
        vec![
            p(-s, s, h),
            p(-s, s, 0.),
            p(s, s, 0.),
            p(s, -s, 0.),
            p(s, -s, h),
        ],
        vec![p(s, s, h), p(s, s, 0.)],
    ]
}

/// What a line is part of, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    /// The hub (the question), in the text colour.
    Hub,
    /// The inner slab on the hub.
    HubInner,
    /// A phase tile on the progress strip, and the link into it.
    Phase(usize),
    Link(usize),
    /// A node of the chat's graph (`crate::chatgraph`), and an edge into
    /// it.
    Node(usize),
    Edge(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub pts: Vec<(f32, f32)>,
    pub ink: Ink,
    pub dotted: bool,
}

/// What the pointer can point at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Hub,
    Phase(usize),
    Node(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Figure {
    pub lines: Vec<Line>,
    /// What can be pointed at: its centre and radius.
    pub targets: Vec<(Target, (f32, f32), f32)>,
    /// The live curve (from, control, to), for the travelling dot: the
    /// edge to the newest node while the run goes, else the strip's link
    /// into the live phase.
    pub live: Option<[(f32, f32); 3]>,
    /// Nothing in the chat to map: the view shows its empty state.
    pub empty: bool,
}

/// A point on a quadratic curve.
pub fn bezier(a: (f32, f32), c: (f32, f32), b: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1. - t;
    (
        u * u * a.0 + 2. * u * t * c.0 + t * t * b.0,
        u * u * a.1 + 2. * u * t * c.1 + t * t * b.1,
    )
}

/// The curve from the hub to a box bends a little sideways, as in v2.
fn control(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    (
        (a.0 + b.0) / 2. + (b.1 - a.1) * 0.18,
        (a.1 + b.1) / 2. - (b.0 - a.0) * 0.12,
    )
}

/// Ring radii (floor units) in the order kinds are placed: workers,
/// claims, sources, terms. Only the kinds present take a ring.
const RINGS: [f32; 4] = [62., 92., 122., 148.];

fn ring_of(k: &NodeKind) -> usize {
    match k {
        NodeKind::Worker { .. } => 0,
        NodeKind::Claim { .. } => 1,
        NodeKind::Source { .. } => 2,
        NodeKind::Entity { .. } => 3,
    }
}

/// `0..1` from a hash, for stable jitter.
fn unit(h: u64, shift: u32) -> f32 {
    ((h >> shift) & 0xffff) as f32 / 65535.
}

/// Each node's place on the floor (x, y) around the hub at (0, 0): its
/// kind's ring, spread evenly, rotated by a hash of the chat and nudged by
/// a hash of the node, so the figure is the chat's own and stable.
pub fn layout(graph: &ChatGraph) -> Vec<(f32, f32)> {
    let mut rings: [Vec<usize>; 4] = Default::default();
    for (i, n) in graph.nodes.iter().enumerate() {
        rings[ring_of(&n.kind)].push(i);
    }
    let mut radius = [0f32; 4];
    let mut next = 0;
    for (r, members) in rings.iter().enumerate() {
        if !members.is_empty() {
            radius[r] = RINGS[next];
            next += 1;
        }
    }
    let seed = crate::chatgraph::stable_hash(&graph.question);
    let mut at = vec![(0f32, 0f32); graph.nodes.len()];
    // A worker's angle, for the pages hanging off it.
    let mut angle_of: Vec<Option<f32>> = vec![None; graph.nodes.len()];
    for (r, members) in rings.iter().enumerate() {
        let n = members.len();
        if n == 0 {
            continue;
        }
        let rot = unit(seed, (r as u32) * 13 % 48) * std::f32::consts::TAU;
        let step = std::f32::consts::TAU / n as f32;
        // Pages under a worker fan out around it instead of the ring.
        let mut fans: std::collections::HashMap<usize, Vec<usize>> = Default::default();
        for &i in members {
            if r == 2
                && let Some(w) = graph
                    .edges
                    .iter()
                    .find(|e| e.to == i)
                    .and_then(|e| e.from)
                    .filter(|f| matches!(graph.nodes[*f].kind, NodeKind::Worker { .. }))
            {
                fans.entry(w).or_default().push(i);
            }
        }
        for (k, &i) in members.iter().enumerate() {
            let h = crate::chatgraph::stable_hash(&graph.nodes[i].key);
            let mut a = rot + k as f32 * step + (unit(h, 0) - 0.5) * step * 0.35;
            if let Some((w, fan)) = fans.iter().find(|(_, f)| f.contains(&i)) {
                if let Some(wa) = angle_of[*w] {
                    let j = fan.iter().position(|x| *x == i).unwrap_or(0) as f32;
                    let spread = (0.9 / fan.len().max(1) as f32).min(0.28);
                    a = wa + (j - (fan.len() as f32 - 1.) / 2.) * spread;
                }
            }
            let rr = radius[r] * (0.92 + unit(h, 16) * 0.16);
            at[i] = (rr * a.cos(), rr * a.sin());
            angle_of[i] = Some(a);
        }
    }
    at
}

/// An isometric ellipse on the floor (a term's ring).
fn ellipse(c: (f32, f32), r: f32) -> Vec<(f32, f32)> {
    (0..=16)
        .map(|j| {
            let t = j as f32 / 16. * std::f32::consts::TAU;
            let (a, b) = iso(r * t.cos(), r * t.sin(), 0.);
            (c.0 + a, c.1 + b)
        })
        .collect()
}

/// A claim's standing diamond.
fn diamond(c: (f32, f32), s: f32) -> Vec<(f32, f32)> {
    vec![
        (c.0, c.1 - 2. * s),
        (c.0 + s, c.1 - s),
        (c.0, c.1),
        (c.0 - s, c.1 - s),
        (c.0, c.1 - 2. * s),
    ]
}

/// Lays the figure out in a `w` × `h` card (the v2 card is 410 × 270),
/// scaled so it fits: the question as the hub, the chat's graph around
/// it, and the run's six phases as a small strip along the bottom.
pub fn figure(map: &RunMap, graph: &ChatGraph, w: f32, h: f32) -> Figure {
    let k = (w / 410.).min(h / 270.).clamp(0.5, 2.5);
    let hub = (w / 2., h / 2. - 4. * k);
    let mut lines = vec![];
    let mut targets = vec![];
    let mut live = None;
    let empty = graph.is_empty();
    for pts in tile(hub, 18. * k, 7. * k) {
        lines.push(Line {
            pts,
            ink: Ink::Hub,
            dotted: empty,
        });
    }
    for pts in tile((hub.0, hub.1 - 7. * k), 9. * k, 4. * k) {
        lines.push(Line {
            pts,
            ink: Ink::HubInner,
            dotted: empty,
        });
    }
    targets.push((Target::Hub, (hub.0, hub.1 - 6. * k), 20. * k));

    // Nodes, in screen space.
    let floor = layout(graph);
    // Squash the rings a little when the card is short.
    // The outer ring's farthest point lies 0.707 × 1.08 × its radius
        // below the hub; keep it above the phase strip.
        let outer = graph
            .nodes
            .iter()
            .map(|n| ring_of(&n.kind))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let reach = RINGS[outer.max(1) - 1] * 0.765 * k;
        let fit = ((h / 2. - 34. * k) / reach).clamp(0.5, 1.);
    let place: Vec<(f32, f32)> = floor
        .iter()
        .map(|&(x, y)| {
            let (a, b) = iso(x * k * fit, y * k * fit, 0.);
            (hub.0 + a, hub.1 + b)
        })
        .collect();
    let centre = |from: Option<usize>| from.map_or(hub, |f| place[f]);
    // Edges first, so nodes draw over them.
    for e in &graph.edges {
        let a = centre(e.from);
        let b = place[e.to];
        let c = control(a, b);
        let curve: Vec<(f32, f32)> = (0..=16).map(|j| bezier(a, c, b, j as f32 / 16.)).collect();
        lines.push(Line {
            pts: curve,
            ink: Ink::Edge(e.to),
            dotted: e.weak,
        });
        if graph.nodes[e.to].fresh && !map.ended {
            live = Some([a, c, b]);
        }
    }
    for (i, n) in graph.nodes.iter().enumerate() {
        let c = place[i];
        let (shapes, top): (Vec<Vec<(f32, f32)>>, f32) = match &n.kind {
            NodeKind::Source { read, .. } => {
                let hgt = if *read { 6. } else { 1.5 } * k;
                (tile(c, if *read { 5. } else { 4. } * k, hgt), hgt)
            }
            NodeKind::Worker { .. } => (tile(c, 8. * k, 7. * k), 7. * k),
            NodeKind::Claim { .. } => (vec![diamond(c, 5. * k)], 10. * k),
            NodeKind::Entity { mentions, .. } => {
                let r = (3.5 + (*mentions).min(5) as f32 * 0.6) * k;
                (vec![ellipse(c, r)], 0.)
            }
        };
        for pts in shapes {
            lines.push(Line {
                pts,
                ink: Ink::Node(i),
                dotted: false,
            });
        }
        targets.push((Target::Node(i), (c.0, c.1 - top / 2.), 10. * k));
    }

    // The phase strip, bottom left.
    let y = h - 18. * k;
    let x0 = 18. * k;
    let gap = 15. * k;
    let mut prev: Option<(f32, f32)> = None;
    for (i, ph) in map.phases.iter().enumerate() {
        let c = (x0 + i as f32 * gap, y);
        let hgt = if ph.state == PhaseState::Live { 5. } else { 2.5 } * k;
        if let Some(p) = prev {
            let ctl = ((p.0 + c.0) / 2., (p.1 + c.1) / 2. - 3. * k);
            let curve: Vec<(f32, f32)> = (0..=8).map(|j| bezier(p, ctl, c, j as f32 / 8.)).collect();
            lines.push(Line {
                pts: curve,
                ink: Ink::Link(i),
                dotted: ph.state == PhaseState::Pending,
            });
            if ph.state == PhaseState::Live && live.is_none() {
                live = Some([p, ctl, c]);
            }
        }
        for pts in tile(c, 3.6 * k, hgt) {
            lines.push(Line {
                pts,
                ink: Ink::Phase(i),
                dotted: false,
            });
        }
        targets.push((Target::Phase(i), (c.0, c.1 - hgt / 2.), 7. * k));
        prev = Some(c);
    }
    Figure {
        lines,
        targets,
        live,
        empty,
    }
}

/// What is under a point in the card, if anything (the nearest target
/// within its radius).
pub fn hit(fig: &Figure, x: f32, y: f32) -> Option<Target> {
    fig.targets
        .iter()
        .map(|(t, c, r)| (*t, (c.0 - x).hypot(c.1 - y), *r))
        .filter(|(_, d, r)| d <= r)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, _, _)| t)
}

/// The corner readout for what is pointed at.
pub fn readout(map: &RunMap, graph: &ChatGraph, hover: Option<Target>) -> (String, String) {
    match hover {
        Some(Target::Phase(i)) => map.readout(Some(i)),
        Some(Target::Node(i)) => graph
            .readout(i)
            .unwrap_or_else(|| map.readout(None)),
        Some(Target::Hub) => (
            "question".to_owned(),
            if graph.question.is_empty() {
                "—".to_owned()
            } else {
                graph.question.clone()
            },
        ),
        None => (
            map.readout(None).0,
            format!("{} · {} tok", graph.summary(), tokens_label(map.tokens)),
        ),
    }
}

/// A polyline cut to the first `frac` of its length, for the draw-in.
pub fn trim(pts: &[(f32, f32)], frac: f32) -> Vec<(f32, f32)> {
    let frac = frac.clamp(0., 1.);
    if frac >= 1. || pts.len() < 2 {
        return pts.to_vec();
    }
    let segs: Vec<f32> = pts
        .windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .collect();
    let mut left = segs.iter().sum::<f32>() * frac;
    let mut out = vec![pts[0]];
    for (i, len) in segs.iter().enumerate() {
        if left >= *len {
            out.push(pts[i + 1]);
            left -= len;
            // The cut landed on this corner: stop, so it is not repeated.
            if left <= f32::EPSILON {
                break;
            }
        } else {
            let t = if *len > 0. { left / len } else { 0. };
            if t <= 0. {
                break;
            }
            let (a, b) = (pts[i], pts[i + 1]);
            out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            break;
        }
    }
    out
}

// ---- The spend chart ---------------------------------------------------

/// The chart's top value and its four labels from 0 (a round number at or
/// above the largest of spend and budget).
pub fn chart_ticks(max: u64) -> (u64, [u64; 4]) {
    let max = max.max(1);
    let raw = max as f64 / 3.;
    let mag = 10f64.powi(raw.log10().floor() as i32);
    let step = [1., 2., 2.5, 5., 10.]
        .iter()
        .map(|m| m * mag)
        .find(|s| s * 3. >= max as f64)
        .unwrap_or(10. * mag)
        .ceil() as u64;
    let step = step.max(1);
    (step * 3, [0, step, step * 2, step * 3])
}

/// The spend line at time `t`: tokens spent by then (the last point at or
/// before it).
pub fn spend_at(spend: &[SpendPoint], t: Duration) -> u64 {
    spend
        .iter()
        .take_while(|p| p.at <= t)
        .last()
        .map_or(0, |p| p.tokens)
}

/// `45s`, `6m`, `1h 5m`.
pub fn time_label(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    }
}

/// The hover tooltip at fraction `x` (0 to 1) across the chart: "6m",
/// "Spend 12.4k tok", and "Budget 40k tok" when there is a budget.
pub fn tooltip(map: &RunMap, x: f32) -> (String, String, Option<String>) {
    let end = map.spend.last().map_or(Duration::ZERO, |p| p.at);
    let t = end.mul_f32(x.clamp(0., 1.));
    (
        time_label(t),
        format!("Spend {} tok", tokens_label(spend_at(&map.spend, t))),
        map.budget
            .map(|b| format!("Budget {} tok", tokens_label(b))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_agent::RunStep;
    use tern_core::{EffectClass, EventEnvelope, SessionId};

    fn run(events: &[(&str, serde_json::Value)]) -> Run {
        let sid = SessionId::new();
        let mut r = Run::new();
        for (kind, p) in events {
            r.push(&EventEnvelope::new(
                sid,
                *kind,
                EffectClass::Analysis,
                "x",
                p.clone(),
            ));
        }
        r
    }

    #[test]
    fn an_empty_run_has_nothing_reached() {
        let m = RunMap::from_run(&Run::new());
        assert!(m.phases.iter().all(|p| p.state == PhaseState::Pending));
        assert_eq!(m.readout(None).0, "not started");
        assert_eq!(m.phases.len(), 6);
    }

    #[test]
    fn phases_light_up_in_order_with_counts_and_spend() {
        let mut r = Run::new();
        let step = |kind: StepKind, event: &str, at: u64, tokens: Option<(u64, u64)>| RunStep {
            seq: at,
            event: event.into(),
            summary: String::new(),
            kind,
            verb: "",
            target: String::new(),
            note: String::new(),
            at: Duration::from_secs(at),
            tokens,
            ok: true,
            detailed: true,
            page: None,
            payload: serde_json::json!({"hits": [1, 2, 3, 4]}),
        };
        r.steps = vec![
            step(StepKind::Model, "model.replied", 2, Some((100, 20))),
            step(StepKind::Search, "search.done", 5, None),
            step(StepKind::Read, "source.read", 30, None),
            step(StepKind::Read, "source.read", 40, None),
            step(StepKind::Model, "model.replied", 360, Some((900, 300))),
        ];
        let m = RunMap::from_run(&r);
        assert_eq!(m.phases[0].count, 4);
        assert_eq!(m.phases[0].state, PhaseState::Done);
        assert_eq!(m.phases[2].count, 2);
        assert_eq!(m.phases[3].state, PhaseState::Live, "thinking after reads");
        assert_eq!(m.phases[5].state, PhaseState::Pending);
        assert_eq!(m.current, Some(3));
        assert_eq!(m.sources, 4);
        assert_eq!(m.kept, 2);
        assert_eq!(m.tokens, 1320);
        assert_eq!(
            m.readout(None),
            (
                "phase 4/6 · synthesize".to_owned(),
                "4 sources · 2 kept · 1.3k tok".to_owned()
            )
        );
        assert_eq!(m.readout(Some(2)).0, "read");
        assert!(m.readout(Some(2)).1.starts_with("2 read"));
        assert_eq!(m.readout(Some(5)).1, "not reached · 0 tok");
        // Spend: a point per model reply, rising.
        assert_eq!(spend_at(&m.spend, Duration::from_secs(10)), 120);
        assert_eq!(spend_at(&m.spend, Duration::from_secs(400)), 1320);
        let (t, s, b) = tooltip(&m, 1.0);
        assert_eq!((t.as_str(), s.as_str(), b), ("6m", "Spend 1.3k tok", None));
    }

    #[test]
    fn only_a_search_that_ran_lights_the_search_phase() {
        let mut r = Run::new();
        let mut s = RunStep {
            seq: 1,
            event: "model.replied".into(),
            summary: String::new(),
            kind: StepKind::Model,
            verb: "",
            target: String::new(),
            note: String::new(),
            at: Duration::from_secs(1),
            tokens: Some((10, 5)),
            ok: true,
            detailed: true,
            page: None,
            payload: serde_json::json!({}),
        };
        r.steps = vec![s.clone()];
        let m = RunMap::from_run(&r);
        assert_eq!(m.phases[0].count, 0);
        assert_eq!(m.phases[0].state, PhaseState::Pending);
        assert_eq!(m.sources, 0);
        s.kind = StepKind::Search;
        s.event = "tool.executed".into();
        s.payload = serde_json::json!({"meta": {"results": 7}});
        s.ok = false;
        r.steps.push(s.clone());
        assert_eq!(RunMap::from_run(&r).sources, 0, "a failed search adds none");
        s.ok = true;
        r.steps.push(s);
        assert_eq!(RunMap::from_run(&r).phases[0].count, 7);
    }

    #[test]
    fn a_finished_run_has_no_live_phase() {
        let r = run(&[("turn.finished", serde_json::json!({"reason": "done"}))]);
        let m = RunMap::from_run(&r);
        if m.ended {
            assert!(m.phases.iter().all(|p| p.state != PhaseState::Live));
            assert_eq!(m.phases[4].state, PhaseState::Done);
        }
    }

    fn graph_of(urls: &[&str]) -> ChatGraph {
        let mut r = Run::new();
        let hits: Vec<_> = urls
            .iter()
            .map(|u| serde_json::json!({"url": u, "title": u, "site": ""}))
            .collect();
        r.steps.push(RunStep {
            seq: 1,
            event: "tool.executed".into(),
            summary: String::new(),
            kind: StepKind::Search,
            verb: "",
            target: String::new(),
            note: String::new(),
            at: Duration::from_secs(1),
            tokens: None,
            ok: true,
            detailed: true,
            page: None,
            payload: serde_json::json!({"meta": {"hits": hits}}),
        });
        ChatGraph::build(&r, urls.first().copied().unwrap_or(""), "")
    }

    #[test]
    fn the_figure_maps_the_chat_and_keeps_a_phase_strip() {
        let m = RunMap::from_run(&Run::new());
        let empty = figure(&m, &ChatGraph::default(), 410., 270.);
        assert!(empty.empty);
        let phases = empty
            .targets
            .iter()
            .filter(|t| matches!(t.0, Target::Phase(_)))
            .count();
        assert_eq!(phases, 6);
        assert!(empty.live.is_none());
        let g = graph_of(&["https://a.org/1", "https://b.org/2", "https://c.org/3"]);
        let f = figure(&m, &g, 410., 270.);
        assert!(!f.empty);
        let nodes: Vec<_> = f
            .targets
            .iter()
            .filter(|t| matches!(t.0, Target::Node(_)))
            .collect();
        assert_eq!(nodes.len(), 3);
        // Every node inside the card.
        for (_, c, _) in &nodes {
            assert!(c.0 > 0. && c.0 < 410. && c.1 > 0. && c.1 < 270., "{c:?}");
        }
        let (_, c, _) = nodes[1];
        assert_eq!(hit(&f, c.0, c.1), Some(Target::Node(1)));
        assert_eq!(hit(&f, -500., -500.), None);
        assert_eq!(hit(&f, 205., 270. / 2. - 10.), Some(Target::Hub));
        let edges = f.lines.iter().filter(|l| matches!(l.ink, Ink::Edge(_))).count();
        assert_eq!(edges, 3);
        assert!(f.lines.iter().filter(|l| matches!(l.ink, Ink::Edge(_))).all(|l| l.dotted));
        // Bigger card, bigger figure.
        let big = figure(&m, &g, 820., 540.);
        assert!(big.targets[1].2 > f.targets[1].2);
    }

    #[test]
    fn two_chats_draw_two_different_figures_and_one_chat_the_same_twice() {
        let m = RunMap::from_run(&Run::new());
        let a = graph_of(&["https://a.org/1", "https://b.org/2"]);
        let b = graph_of(&["https://x.org/9", "https://y.org/8", "https://z.org/7"]);
        let fa = figure(&m, &a, 410., 270.);
        let fb = figure(&m, &b, 410., 270.);
        assert_ne!(fa.targets, fb.targets);
        assert_eq!(fa, figure(&m, &a, 410., 270.), "stable");
        // Same count, different chat: different placement.
        let c = graph_of(&["https://q.org/1", "https://r.org/2"]);
        assert_ne!(layout(&a), layout(&c));
    }

    #[test]
    fn the_readout_follows_the_pointer() {
        let m = RunMap::from_run(&Run::new());
        let g = graph_of(&["https://a.org/1"]);
        assert_eq!(readout(&m, &g, None).1, "1 seen · 0 tok");
        assert!(readout(&m, &g, Some(Target::Node(0))).0.starts_with("seen"));
        assert_eq!(readout(&m, &g, Some(Target::Hub)).0, "question");
        assert_eq!(readout(&m, &g, Some(Target::Phase(5))).0, "verify");
    }

    #[test]
    fn iso_tiles_curves_and_trim() {
        assert_eq!(iso(0., 0., 0.), (0., 0.));
        assert_eq!(iso(1., 1., 0.), (0., 1.));
        let t = tile((0., 0.), 1., 1.);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].first(), t[0].last());
        assert_eq!(bezier((0., 0.), (5., 5.), (10., 0.), 0.), (0., 0.));
        assert_eq!(bezier((0., 0.), (5., 5.), (10., 0.), 1.), (10., 0.));
        let line = [(0., 0.), (10., 0.), (10., 10.)];
        assert_eq!(trim(&line, 0.5), vec![(0., 0.), (10., 0.)]);
        assert_eq!(trim(&line, 0.25), vec![(0., 0.), (5., 0.)]);
        assert_eq!(trim(&line, 1.), line.to_vec());
        assert_eq!(trim(&line, 0.), vec![(0., 0.)]);
        let t = trim(&line, 0.75);
        assert_eq!(t, vec![(0., 0.), (10., 0.), (10., 5.)]);
        assert!(t.windows(2).all(|w| w[0] != w[1]), "no repeated point");
    }

    #[test]
    fn chart_ticks_are_round_and_cover_the_max() {
        assert_eq!(chart_ticks(3), (3, [0, 1, 2, 3]));
        let (top, ticks) = chart_ticks(1320);
        assert!(top >= 1320);
        assert_eq!(ticks[3], top);
        assert_eq!(chart_ticks(0).0, 3);
        assert_eq!(time_label(Duration::from_secs(45)), "45s");
        assert_eq!(time_label(Duration::from_secs(3900)), "1h 5m");
    }
}
