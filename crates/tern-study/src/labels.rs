//! Where the Explain in 3D labels go on screen, free of any UI so it is
//! tested: no two labels overlap, the focused nodes always get their full
//! label, nearby nodes get theirs when there is room, and the rest show a
//! short label (the node's step number) or nothing. A label that had to
//! move away from its node keeps a leader line back to it.
//!
//! The desktop computes this once per camera position and step, not every
//! frame (`crate::camera` changes, steps change; nothing else moves them).

/// How much a label matters, most first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rank {
    /// The step on screen, the node under the pointer or picked: always
    /// shown in full.
    Focus,
    /// Linked to the focus or in the same level: full when it fits.
    Near,
    /// Everything else: the short label when it fits.
    Rest,
}

/// One label to place.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelIn {
    pub part: usize,
    /// The point it labels, in stage pixels.
    pub anchor: (f32, f32),
    pub full: String,
    /// What shows when the full label does not fit or is not wanted (a
    /// step number); empty hides the label then.
    pub short: String,
    pub rank: Rank,
}

/// A label placed: its box (top-left, size) in stage pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub part: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
    pub full: bool,
    pub rank: Rank,
    /// From the node to the label, when the label moved away from it.
    pub leader: Option<((f32, f32), (f32, f32))>,
}

/// Text metrics: an average character's width and a line's height, in
/// pixels, at the label size in use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub char_w: f32,
    pub line_h: f32,
    /// Widest a full label gets before it is clipped with `…`.
    pub max_w: f32,
}

impl Metrics {
    /// For a font of `size` pixels.
    pub fn for_size(size: f32) -> Self {
        let size = if size.is_finite() {
            size.clamp(8.0, 32.0)
        } else {
            12.0
        };
        Self {
            char_w: size * 0.56,
            line_h: size * 1.45,
            max_w: size * 18.0,
        }
    }
}

/// The label size for a zoom: 12 px at 1×, growing and shrinking with the
/// zoom but kept between 11 and 15 px so it stays readable.
pub fn label_size(zoom: f32) -> f32 {
    if !zoom.is_finite() || zoom <= 0.0 {
        return 12.0;
    }
    (12.0 * zoom.sqrt()).clamp(11.0, 15.0)
}

/// Room kept between labels, in pixels.
const GAP: f32 = 3.0;
/// Padding inside a label's box.
const PAD: f32 = 8.0;

fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.0 + b.2 + GAP && b.0 < a.0 + a.2 + GAP && a.1 < b.1 + b.3 + GAP && b.1 < a.1 + a.3 + GAP
}

/// `text` cut to fit `max_w` at `char_w` a character, with `…`.
fn fit_text(text: &str, m: &Metrics) -> String {
    let max_chars = (((m.max_w - PAD) / m.char_w).floor() as usize).max(4);
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max_chars - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Places `labels` in a `w` × `h` stage. Focus labels first, then near,
/// then the rest, each group front to back (larger `y` first). Labels
/// whose node is off the stage are left out.
pub fn place(labels: &[LabelIn], w: f32, h: f32, m: &Metrics) -> Vec<Placed> {
    if !(w.is_finite() && h.is_finite()) || w < 4.0 || h < 4.0 {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..labels.len())
        .filter(|&i| {
            let (x, y) = labels[i].anchor;
            x.is_finite() && y.is_finite() && x >= -4.0 && y >= -4.0 && x <= w + 4.0 && y <= h + 4.0
        })
        .collect();
    order.sort_by(|&a, &b| {
        let (la, lb) = (&labels[a], &labels[b]);
        la.rank
            .cmp(&lb.rank)
            .then(lb.anchor.1.total_cmp(&la.anchor.1))
            .then(a.cmp(&b))
    });
    let mut out: Vec<Placed> = Vec::new();
    let mut taken: Vec<(f32, f32, f32, f32)> = Vec::new();
    for i in order {
        let l = &labels[i];
        let (ax, ay) = l.anchor;
        let try_text = |text: &str, out: &mut Vec<Placed>, taken: &mut Vec<_>, force: bool| {
            let tw = (text.chars().count() as f32 * m.char_w + PAD).min(m.max_w);
            let th = m.line_h;
            // Above, below, right, left, then further up and out with a
            // leader line.
            let mut spots: Vec<(f32, f32)> = vec![
                (ax - tw / 2.0, ay - th - 4.0),
                (ax - tw / 2.0, ay + 6.0),
                (ax + 10.0, ay - th / 2.0),
                (ax - tw - 10.0, ay - th / 2.0),
            ];
            for k in 1..=3 {
                let up = ay - th - 4.0 - k as f32 * (th + GAP + 2.0);
                spots.push((ax - tw / 2.0, up));
                spots.push((ax + 12.0, up));
                spots.push((ax - tw - 12.0, up));
                let down = ay + 6.0 + k as f32 * (th + GAP + 2.0);
                spots.push((ax - tw / 2.0, down));
            }
            let inside = |x: f32, y: f32| x >= 0.0 && y >= 0.0 && x + tw <= w && y + th <= h;
            let free = |x: f32, y: f32, taken: &Vec<(f32, f32, f32, f32)>| {
                !taken.iter().any(|t| overlaps(*t, (x, y, tw, th)))
            };
            let pick = spots
                .iter()
                .enumerate()
                .find(|(_, s)| inside(s.0, s.1) && free(s.0, s.1, taken))
                .map(|(k, s)| (k, *s))
                .or_else(|| {
                    // A focus label always shows: the first spot on stage.
                    force.then(|| {
                        let (x, y) = spots[0];
                        (
                            0,
                            (
                                x.clamp(0.0, (w - tw).max(0.0)),
                                y.clamp(0.0, (h - th).max(0.0)),
                            ),
                        )
                    })
                });
            let Some((k, (x, y))) = pick else {
                return false;
            };
            // The first four spots sit next to the node; the rest moved.
            let leader = (k >= 4).then(|| {
                let tx = ax.clamp(x, x + tw);
                let ty = if ay < y { y } else { y + th };
                ((ax, ay), (tx, ty))
            });
            taken.push((x, y, tw, th));
            out.push(Placed {
                part: l.part,
                x,
                y,
                w: tw,
                h: th,
                text: text.to_owned(),
                full: text != l.short,
                rank: l.rank,
                leader,
            });
            true
        };
        let full = fit_text(&l.full, m);
        match l.rank {
            Rank::Focus => {
                try_text(&full, &mut out, &mut taken, true);
            }
            Rank::Near => {
                if !try_text(&full, &mut out, &mut taken, false) && !l.short.is_empty() {
                    try_text(&l.short, &mut out, &mut taken, false);
                }
            }
            Rank::Rest => {
                if !l.short.is_empty() {
                    try_text(&l.short, &mut out, &mut taken, false);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(part: usize, x: f32, y: f32, rank: Rank) -> LabelIn {
        LabelIn {
            part,
            anchor: (x, y),
            full: format!("Node number {part} label"),
            short: format!("{}", part + 1),
            rank,
        }
    }

    fn no_overlaps(p: &[Placed]) {
        for (i, a) in p.iter().enumerate() {
            for b in &p[i + 1..] {
                let hit = a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
                assert!(!hit, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn a_pile_of_labels_never_overlaps_and_focus_is_full() {
        // Twenty nodes piled into a small patch, as in a dense scene.
        let mut ls: Vec<LabelIn> = (0..20)
            .map(|i| {
                label(
                    i,
                    200.0 + (i % 5) as f32 * 9.0,
                    200.0 + (i / 5) as f32 * 7.0,
                    Rank::Rest,
                )
            })
            .collect();
        ls[3].rank = Rank::Focus;
        ls[7].rank = Rank::Near;
        let m = Metrics::for_size(12.0);
        let p = place(&ls, 600.0, 420.0, &m);
        no_overlaps(&p);
        let focus = p.iter().find(|x| x.part == 3).expect("focus shown");
        assert!(focus.full && focus.text.starts_with("Node number 3"));
        // The rest show their numbers where they fit; nothing is off stage.
        assert!(p.iter().filter(|x| !x.full).count() >= 5, "{p:#?}");
        for x in &p {
            assert!(x.x >= 0.0 && x.y >= 0.0 && x.x + x.w <= 600.0 && x.y + x.h <= 420.0);
        }
        // Moved labels keep a leader back to their node.
        assert!(p.iter().any(|x| x.leader.is_some()));
    }

    #[test]
    fn spread_labels_all_show_in_full_when_near() {
        let ls: Vec<LabelIn> = (0..6)
            .map(|i| {
                label(
                    i,
                    60.0 + i as f32 * 150.0,
                    300.0 - (i % 2) as f32 * 80.0,
                    Rank::Near,
                )
            })
            .collect();
        let p = place(&ls, 1000.0, 500.0, &Metrics::for_size(12.0));
        assert_eq!(p.len(), 6);
        assert!(p.iter().all(|x| x.full && x.leader.is_none()));
    }

    #[test]
    fn degenerate_input_is_safe() {
        let m = Metrics::for_size(f32::NAN);
        assert_eq!(m, Metrics::for_size(12.0));
        let ls = vec![
            label(0, f32::NAN, 10.0, Rank::Focus),
            label(1, 1e9, 10.0, Rank::Focus),
            label(2, 10.0, 10.0, Rank::Rest),
        ];
        assert!(place(&ls, 0.0, 0.0, &m).is_empty());
        assert!(place(&ls, f32::INFINITY, 100.0, &m).is_empty());
        let p = place(&ls, 100.0, 100.0, &m);
        assert_eq!(p.len(), 1, "only the node on stage: {p:?}");
        // A focus label wider than the stage still shows, clipped.
        let long = LabelIn {
            full: "x".repeat(400),
            ..label(0, 50.0, 50.0, Rank::Focus)
        };
        let p = place(&[long], 100.0, 100.0, &m);
        assert_eq!(p.len(), 1);
        assert!(p[0].text.ends_with('…'));
        assert_eq!(label_size(1.0), 12.0);
        assert_eq!(label_size(100.0), 15.0);
        assert_eq!(label_size(0.01), 11.0);
        assert_eq!(label_size(f32::NAN), 12.0);
    }
}
