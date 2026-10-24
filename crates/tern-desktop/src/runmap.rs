//! The right panel's Graph tab, GPUI-free so it is unit tested: "FIG 1 ·
//! RUN MAP", an isometric hairline figure of the run (a central slab for
//! the question, one small box per phase on a curved hairline, little
//! cubes beside phases that found or read sources, dotted links for
//! phases not reached, the live phase's link in the accent with a dot
//! travelling on it), its corner readout, and the spend-over-time chart
//! under it (the v2 concept's s12).
//!
//! Everything comes from the run's own events (`tern_agent::Run`): which
//! phase each step belongs to, counts, and tokens. Tern has no price
//! table yet, so "spend" is measured tokens and the budget line is the
//! deep plan's token budget when there is one; no dollar figure is made up.

use std::time::Duration;

use tern_agent::{Run, StepKind};

/// The phases, in the order a Tern turn goes through them.
pub const PHASES: [&str; 6] = ["search", "screen", "read", "synthesize", "write", "verify"];

/// Where each phase box sits around the hub, in floor units (x, y), from
/// the v2 figure.
const POS: [(f32, f32); 6] = [
    (-95., -10.),
    (-60., -80.),
    (15., -105.),
    (90., -60.),
    (100., 25.),
    (40., 90.),
];

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
    /// Little cubes beside it (sources it found or read), at most 6.
    pub cubes: usize,
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
                cubes: 0,
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
        phases[0].cubes = (phases[0].count as usize).min(6);
        phases[2].cubes = (phases[2].count as usize).min(6);
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
    /// The hub, in the text colour.
    Hub,
    /// The inner slab on the hub.
    HubInner,
    Phase(usize),
    Link(usize),
    Cube(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub pts: Vec<(f32, f32)>,
    pub ink: Ink,
    pub dotted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Figure {
    pub lines: Vec<Line>,
    /// Each phase box's centre and radius, for pointing.
    pub targets: Vec<((f32, f32), f32)>,
    /// The live link's curve (hub, control, box), for the travelling dot.
    pub live: Option<[(f32, f32); 3]>,
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

/// Lays the figure out in a `w` × `h` card (the v2 card is 410 × 270),
/// scaled so it fits.
pub fn figure(map: &RunMap, w: f32, h: f32) -> Figure {
    let k = (w / 410.).min(h / 270.).clamp(0.5, 2.5);
    let hub = (w / 2. - 6. * k, h / 2. + 10. * k);
    let mut lines = vec![];
    for pts in tile(hub, 24. * k, 8. * k) {
        lines.push(Line {
            pts,
            ink: Ink::Hub,
            dotted: false,
        });
    }
    for pts in tile((hub.0, hub.1 - 8. * k), 12. * k, 5. * k) {
        lines.push(Line {
            pts,
            ink: Ink::HubInner,
            dotted: false,
        });
    }
    let mut targets = vec![];
    let mut live = None;
    for (i, ph) in map.phases.iter().enumerate() {
        let (ax, ay) = iso(POS[i].0 * k, POS[i].1 * k, 0.);
        let node = (hub.0 + ax, hub.1 + ay);
        let c = control(hub, node);
        let curve: Vec<(f32, f32)> = (0..=24)
            .map(|j| bezier(hub, c, node, j as f32 / 24.))
            .collect();
        lines.push(Line {
            pts: curve,
            ink: Ink::Link(i),
            dotted: ph.state == PhaseState::Pending,
        });
        if ph.state == PhaseState::Live {
            live = Some([hub, c, node]);
        }
        let hgt = if ph.state == PhaseState::Live { 9. } else { 6. } * k;
        for pts in tile(node, 11. * k, hgt) {
            lines.push(Line {
                pts,
                ink: Ink::Phase(i),
                dotted: false,
            });
        }
        for j in 0..ph.cubes {
            let cx = node.0 + (18. + (j % 3) as f32 * 9.) * k;
            let cy = node.1 + (8. + (j / 3) as f32 * 6. - (j % 2) as f32 * 3.) * k;
            for pts in tile((cx, cy), 3.2 * k, 3. * k) {
                lines.push(Line {
                    pts,
                    ink: Ink::Cube(i),
                    dotted: false,
                });
            }
        }
        targets.push(((node.0, node.1 - hgt / 2.), 16. * k));
    }
    Figure {
        lines,
        targets,
        live,
    }
}

/// The phase box under a point in the card, if any (the nearest one
/// within its radius).
pub fn hit(fig: &Figure, x: f32, y: f32) -> Option<usize> {
    fig.targets
        .iter()
        .enumerate()
        .map(|(i, (c, r))| (i, (c.0 - x).hypot(c.1 - y), *r))
        .filter(|(_, d, r)| d <= r)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _, _)| i)
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
        assert_eq!(m.phases[0].cubes, 4);
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

    #[test]
    fn the_figure_has_a_box_per_phase_dotted_links_when_pending_and_hits() {
        let m = RunMap::from_run(&Run::new());
        let f = figure(&m, 410., 270.);
        assert_eq!(f.targets.len(), 6);
        let links: Vec<&Line> = f
            .lines
            .iter()
            .filter(|l| matches!(l.ink, Ink::Link(_)))
            .collect();
        assert_eq!(links.len(), 6);
        assert!(links.iter().all(|l| l.dotted));
        assert!(f.live.is_none());
        let (c, _) = f.targets[2];
        assert_eq!(hit(&f, c.0, c.1), Some(2));
        assert_eq!(hit(&f, -500., -500.), None);
        // Bigger card, bigger figure, same shape.
        let g = figure(&m, 820., 540.);
        assert!(g.targets[0].1 > f.targets[0].1);
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
