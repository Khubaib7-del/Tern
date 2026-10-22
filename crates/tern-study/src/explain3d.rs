//! Explain in 3D: a concept drawn as a small isometric machine.
//!
//! The model picks a concept from the session or the materials and
//! returns a typed [`Scene`]: parts (blocks, cylinders, plates, arrows,
//! floating labels) placed on a 12 × 12 floor grid, the connections
//! between them, and ordered steps, each highlighting some parts with a
//! caption and the passage it came from. [`parse_scene`] reads the reply
//! leniently; [`fallback_scene`] builds a scene from the passages' key
//! terms when there is no model, deterministically.
//!
//! The drawing is computed here too, so the desktop renderer, the SVG
//! snapshot, and the tests all see the same picture: [`project`] turns a
//! scene into shaded faces (the solid, Anatomy-style machine), hairline
//! edges, the floor grid, dashed links, and label points, sorted back to
//! front for a given view (the isometric angle plus a small pointer tilt).
//! [`hit`] finds the part under the pointer; [`to_svg`] writes the
//! snapshot.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::material::{Anchor, Source, sources_block};
use crate::text::{
    capitalise, clip, field, json_in, keywords, list, normalise, one_line, overlap, sentences, slug,
};
use crate::topics::definition;

/// The floor is GRID × GRID units.
pub const GRID: f32 = 12.0;
pub const MAX_PARTS: usize = 10;
pub const MAX_STEPS: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// A box: a stage, component, or actor.
    #[default]
    Block,
    /// A store, pool, or reservoir.
    Cylinder,
    /// A flat base or layer.
    Plate,
    /// A flow; its size is the direction and length on the floor.
    Arrow,
    /// A floating note, no solid.
    Label,
}

impl Shape {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase();
        Some(match s.as_str() {
            "block" | "box" | "cube" | "machine" | "stage" | "component" | "node" | "unit" => {
                Self::Block
            }
            "cylinder" | "tank" | "drum" | "store" | "db" | "database" | "pool" | "silo"
            | "reservoir" => Self::Cylinder,
            "plate" | "slab" | "base" | "platform" | "layer" | "floor" => Self::Plate,
            "arrow" | "flow" | "pipe" | "conveyor" => Self::Arrow,
            "label" | "text" | "note" | "tag" => Self::Label,
            _ => return None,
        })
    }

    fn default_size(self) -> [f32; 3] {
        match self {
            Self::Block => [2.0, 2.0, 1.6],
            Self::Cylinder => [2.0, 2.0, 1.8],
            Self::Plate => [4.0, 3.0, 0.4],
            Self::Arrow => [2.5, 0.0, 0.3],
            Self::Label => [0.0, 0.0, 0.0],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub id: String,
    pub label: String,
    pub shape: Shape,
    /// The corner nearest the origin (x, y on the floor; z up).
    pub at: [f32; 3],
    /// Width, depth, height; for arrows, the direction and length (x, y)
    /// and the thickness.
    pub size: [f32; 3],
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub caption: String,
    /// Part ids this step highlights.
    #[serde(default)]
    pub parts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
}

impl Step {
    /// `graphs.pdf p. 4`.
    pub fn cite(&self) -> Option<String> {
        self.anchor.as_ref().map(Anchor::label)
    }
}

/// One explainer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub id: String,
    pub concept: String,
    #[serde(default)]
    pub summary: String,
    pub parts: Vec<Part>,
    #[serde(default)]
    pub links: Vec<Link>,
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// `model` or `fallback`.
    pub written_by: String,
    pub created_at: i64,
}

impl Scene {
    pub fn part_index(&self, id: &str) -> Option<usize> {
        self.parts.iter().position(|p| p.id == id)
    }

    /// The part indexes step `i` highlights.
    pub fn lit(&self, step: usize) -> BTreeSet<usize> {
        self.steps
            .get(step)
            .map(|s| {
                s.parts
                    .iter()
                    .filter_map(|id| self.part_index(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The scene as Markdown: the concept, summary, and numbered steps
    /// with their sources, for the vault.
    pub fn to_markdown(&self, svg_name: Option<&str>) -> String {
        let mut s = format!("# Explain in 3D: {}\n\n", self.concept);
        if !self.summary.is_empty() {
            s.push_str(&format!("{}\n\n", self.summary));
        }
        if let Some(svg) = svg_name {
            s.push_str(&format!("![{}]({svg})\n\n", self.concept));
        }
        s.push_str("## Parts\n\n");
        for p in &self.parts {
            if p.note.is_empty() {
                s.push_str(&format!("- **{}**\n", p.label));
            } else {
                s.push_str(&format!("- **{}**: {}\n", p.label, p.note));
            }
        }
        s.push_str("\n## Steps\n\n");
        for (i, st) in self.steps.iter().enumerate() {
            s.push_str(&format!("{}. {}", i + 1, st.caption));
            if let Some(c) = st.cite() {
                s.push_str(&format!(" ({c})"));
            }
            s.push('\n');
        }
        s.push_str(&format!("\n_Written by {}._\n", self.written_by));
        s
    }
}

// ---- The model's scene -------------------------------------------------

/// The prompt asking a model for a scene: (system, user). An empty
/// `concept` lets the model pick one from the passages.
pub fn scene_prompt(concept: &str, sources: &[Source]) -> (String, String) {
    let system = format!(
        "You design an isometric explainer diagram that teaches one concept to a student, using only the numbered passages from their own materials. Build the concept as a small machine of 3 to {max} parts on a {g} × {g} floor grid: \"at\" is [x, y, z] in grid units (the viewer looks from the corner where x and y are large; z is up), \"size\" is [width, depth, height], parts 1 to 4 units wide and 0.3 to 4 tall, not overlapping. Shapes: \"block\" (a stage, component, or actor), \"cylinder\" (a store, pool, or reservoir), \"plate\" (a flat base or layer; other parts may stand on it), \"arrow\" (a flow; its size is [dx, dy, thickness]), \"label\" (a floating note). Connect parts that hand something to each other. Then write 3 to {steps} steps that walk through how it works, in order: each highlights the parts it is about, has a caption of one or two sentences, and names the passage it comes from as \"source\": \"S2\". Use only what the passages say. Reply with JSON only: {{\"concept\": \"…\", \"summary\": \"one sentence\", \"parts\": [{{\"id\": \"p1\", \"label\": \"…\", \"shape\": \"block\", \"at\": [1, 1, 0], \"size\": [2, 2, 1.5], \"note\": \"what it is\"}}], \"links\": [{{\"from\": \"p1\", \"to\": \"p2\", \"label\": \"what moves\"}}], \"steps\": [{{\"caption\": \"…\", \"parts\": [\"p1\"], \"source\": \"S1\"}}]}}",
        max = MAX_PARTS.min(8),
        g = GRID as u32,
        steps = MAX_STEPS - 1,
    );
    let ask = if concept.trim().is_empty() {
        "Pick the single most important mechanism or idea in these passages.".to_owned()
    } else {
        format!("Concept: {}", one_line(concept))
    };
    let user = format!("{ask}\n\nPassages:\n\n{}", sources_block(sources));
    (system, user)
}

fn num(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| f as f32),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|f: &f32| f.is_finite())
}

/// `[x, y, z]`, `[x, y]`, or `{"x":…, "y":…, "z":…}` (or w, d, h).
fn vec3(item: &Value, keys: &[&str]) -> Option<[f32; 3]> {
    let v = keys.iter().find_map(|k| item.get(*k))?;
    match v {
        Value::Array(a) if a.len() >= 2 => Some([
            num(&a[0])?,
            num(&a[1])?,
            a.get(2).and_then(num).unwrap_or(0.0),
        ]),
        Value::Object(_) => {
            let get = |ks: &[&str]| ks.iter().find_map(|k| v.get(*k).and_then(num));
            Some([
                get(&["x", "w", "width"])?,
                get(&["y", "d", "depth"])?,
                get(&["z", "h", "height"]).unwrap_or(0.0),
            ])
        }
        _ => None,
    }
}

fn source_of<'a>(item: &Value, sources: &'a [Source]) -> Option<&'a Source> {
    let raw = match item.get("source").or_else(|| item.get("passage"))? {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => match a.first()? {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => return None,
        },
        _ => return None,
    };
    let n: usize = raw
        .trim()
        .trim_start_matches(['[', 'S', 's'])
        .trim_end_matches(']')
        .trim()
        .parse()
        .ok()?;
    sources.iter().find(|s| s.n == n)
}

/// Keeps a part inside the floor and its size sensible.
fn tidy(p: &mut Part) {
    for v in p.at.iter_mut() {
        *v = v.clamp(0.0, GRID);
    }
    match p.shape {
        Shape::Arrow => {
            p.size[0] = p.size[0].clamp(-8.0, 8.0);
            p.size[1] = p.size[1].clamp(-8.0, 8.0);
            if p.size[0].abs() + p.size[1].abs() < 0.5 {
                p.size[0] = 2.0;
            }
            p.size[2] = p.size[2].abs().clamp(0.1, 1.0);
        }
        Shape::Label => p.size = [0.0; 3],
        _ => {
            p.size[0] = p.size[0].abs().clamp(0.3, 8.0);
            p.size[1] = p.size[1].abs().clamp(0.3, 8.0);
            p.size[2] = p.size[2].abs().clamp(0.1, 6.0);
        }
    }
}

/// Reads a model's scene. `None` when fewer than two parts came back.
pub fn parse_scene(
    text: &str,
    concept: &str,
    sources: &[Source],
    topic: Option<&str>,
    now: i64,
) -> Option<Scene> {
    let v = json_in(text)?;
    let v = v.get("scene").cloned().unwrap_or(v);
    let items = v.get("parts").and_then(Value::as_array)?;
    let mut parts: Vec<Part> = Vec::new();
    for (i, item) in items.iter().take(MAX_PARTS).enumerate() {
        let Some(label) = field(item, &["label", "name", "title", "text"]) else {
            continue;
        };
        let shape = field(item, &["shape", "type", "kind"])
            .and_then(Shape::parse)
            .unwrap_or_default();
        let mut id = field(item, &["id", "key"])
            .map(str::to_owned)
            .unwrap_or_else(|| slug(label));
        while parts.iter().any(|p| p.id == id) {
            id.push('_');
        }
        let mut p = Part {
            id,
            label: clip(label, 40),
            shape,
            at: vec3(item, &["at", "position", "pos", "origin"]).unwrap_or([
                1.0 + 3.0 * i as f32,
                4.0,
                0.0,
            ]),
            size: vec3(item, &["size", "dims", "dimensions"])
                .unwrap_or_else(|| shape.default_size()),
            note: field(item, &["note", "description", "role", "what"])
                .map(|s| clip(s, 160))
                .unwrap_or_default(),
        };
        tidy(&mut p);
        parts.push(p);
    }
    if parts.len() < 2 {
        return None;
    }
    let find = |r: &str| -> Option<String> {
        let n = normalise(r);
        parts
            .iter()
            .find(|p| p.id == r.trim())
            .or_else(|| parts.iter().find(|p| normalise(&p.label) == n))
            .map(|p| p.id.clone())
    };
    let mut links = Vec::new();
    for l in v
        .get("links")
        .or_else(|| v.get("connections"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(a), Some(b)) = (
            field(l, &["from", "source_part", "a"]).and_then(find),
            field(l, &["to", "target", "b"]).and_then(find),
        ) else {
            continue;
        };
        if a != b && !links.iter().any(|x: &Link| x.from == a && x.to == b) {
            links.push(Link {
                from: a,
                to: b,
                label: field(l, &["label", "what", "carries"])
                    .map(|s| clip(s, 40))
                    .unwrap_or_default(),
            });
        }
    }
    let mut steps = Vec::new();
    for s in v
        .get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_STEPS)
    {
        let Some(caption) = field(s, &["caption", "text", "description", "explanation"]) else {
            continue;
        };
        let lit: Vec<String> = list(s, &["parts", "highlight", "highlights", "focus"])
            .iter()
            .filter_map(|r| find(r))
            .collect();
        steps.push(Step {
            caption: clip(caption, 320),
            parts: lit,
            anchor: source_of(s, sources).map(|x| x.anchor.clone()),
        });
    }
    if steps.is_empty() {
        steps = parts
            .iter()
            .map(|p| Step {
                caption: if p.note.is_empty() {
                    p.label.clone()
                } else {
                    format!("{}: {}", p.label, p.note)
                },
                parts: vec![p.id.clone()],
                anchor: None,
            })
            .collect();
    }
    let concept = field(&v, &["concept", "title", "name"])
        .map(str::to_owned)
        .unwrap_or_else(|| one_line(concept));
    Some(Scene {
        id: String::new(),
        concept: clip(&concept, 80),
        summary: field(&v, &["summary", "one_line", "description"])
            .map(|s| clip(s, 240))
            .unwrap_or_default(),
        parts,
        links,
        steps,
        topic: topic.map(str::to_owned),
        written_by: "model".into(),
        created_at: now,
    })
}

// ---- The fallback scene ------------------------------------------------

const STORE_WORDS: &[&str] = &[
    "store",
    "storage",
    "memory",
    "cache",
    "database",
    "queue",
    "pool",
    "buffer",
    "tank",
    "reservoir",
    "stack",
    "heap",
    "table",
    "repository",
    "library",
    "index",
    "register",
];

/// A scene without a model: the concept's key terms in the passages as a
/// row of parts on a base, linked in reading order, with one step per
/// part captioned by the sentence that names it. Deterministic.
pub fn fallback_scene(concept: &str, sources: &[Source], topic: Option<&str>, now: i64) -> Scene {
    // Candidate terms: (term, sentence, source).
    let mut cands: Vec<(String, String, Option<&Source>)> = Vec::new();
    for s in sources {
        let body: String = s
            .text
            .lines()
            .filter(|l| crate::material::heading(l).is_none())
            .collect::<Vec<_>>()
            .join("\n");
        for sent in sentences(&body) {
            if let Some((t, _)) = definition(&sent) {
                cands.push((t, sent.clone(), Some(s)));
            }
        }
    }
    let defs_found = cands.len();
    let bodies: Vec<String> = sources
        .iter()
        .map(|s| {
            s.text
                .lines()
                .filter(|l| crate::material::heading(l).is_none())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    let joined = bodies.join("\n");
    let all_words = crate::text::words(&joined);
    // Key words the passages come back to (twice or more), each with the
    // first sentence that names it.
    for k in keywords(&joined, 12) {
        if all_words.iter().filter(|w| **w == k).count() < 2 {
            continue;
        }
        if let Some((s, sent)) = sources.iter().zip(&bodies).find_map(|(s, b)| {
            sentences(b)
                .into_iter()
                .find(|x| x.to_lowercase().contains(&k))
                .map(|x| (s, x))
        }) {
            cands.push((k, sent, Some(s)));
        }
    }
    // Definitions before bare key words, then closest to the concept;
    // reading order breaks ties.
    let defs = defs_found;
    if !concept.trim().is_empty() {
        let mut scored: Vec<(f32, usize)> = cands
            .iter()
            .enumerate()
            .map(|(i, c)| (overlap(concept, &c.1) + if i < defs { 1.0 } else { 0.0 }, i))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        cands = scored.into_iter().map(|(_, i)| cands[i].clone()).collect();
    }
    let cn = normalise(concept);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut said: BTreeSet<String> = BTreeSet::new();
    let mut picked: Vec<(String, String, Option<&Source>)> = Vec::new();
    for c in cands {
        let n = normalise(&c.0);
        if n.is_empty() || n == cn || !seen.insert(n.clone()) {
            continue;
        }
        // One part per sentence, so no two steps say the same thing.
        if said.contains(&normalise(&c.1)) {
            continue;
        }
        // Skip a term that is part of one already picked (or holds it).
        if picked.iter().any(|p| {
            let pn = normalise(&p.0);
            pn.contains(&n) || n.contains(&pn)
        }) {
            continue;
        }
        said.insert(normalise(&c.1));
        picked.push(c);
        if picked.len() >= 5 {
            break;
        }
    }
    // Nothing in the materials: the concept's own words, said plainly.
    if picked.is_empty() {
        for w in crate::text::words(concept).into_iter().take(3) {
            picked.push((
                w,
                "Your materials do not describe this part yet. Add a source, or connect a model, for a fuller explainer.".into(),
                None,
            ));
        }
        if picked.len() < 2 {
            picked.push((
                "idea".into(),
                "Your materials do not describe this concept yet.".into(),
                None,
            ));
            if picked.len() < 2 {
                picked.push((
                    "use".into(),
                    "Add a source to see how it is used.".into(),
                    None,
                ));
            }
        }
    }
    let concept = if concept.trim().is_empty() {
        sources
            .first()
            .and_then(|s| s.anchor.section.clone())
            .unwrap_or_else(|| capitalise(&picked[0].0))
    } else {
        one_line(concept)
    };
    let n = picked.len();
    let pitch = 2.6f32;
    let x0 = ((GRID - pitch * n as f32) / 2.0).max(0.2);
    let mut parts = vec![Part {
        id: "base".into(),
        label: clip(&concept, 40),
        shape: Shape::Plate,
        at: [x0 - 0.4, 4.2, 0.0],
        size: [pitch * n as f32 + 0.3, 3.4, 0.35],
        note: String::new(),
    }];
    let mut steps = Vec::new();
    let lead = picked
        .iter()
        .find(|p| p.2.is_some() && overlap(&concept, &p.1) > 0.0);
    let summary = lead
        .map(|p| clip(&p.1, 220))
        .unwrap_or_else(|| format!("How {concept} fits together, part by part."));
    steps.push(Step {
        caption: if lead.is_some() {
            format!(
                "{concept}, part by part: {}.",
                picked
                    .iter()
                    .map(|p| capitalise(&p.0))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            summary.clone()
        },
        parts: Vec::new(),
        anchor: None,
    });
    for (i, (term, sent, src)) in picked.iter().enumerate() {
        let lower = term.to_lowercase();
        let shape = if STORE_WORDS.iter().any(|w| lower.contains(w)) {
            Shape::Cylinder
        } else {
            Shape::Block
        };
        let h = 1.1 + (i % 3) as f32 * 0.45;
        let id = format!("p{}", i + 1);
        parts.push(Part {
            id: id.clone(),
            label: clip(&capitalise(term), 28),
            shape,
            at: [x0 + i as f32 * pitch, 4.9, 0.35],
            size: [1.8, 1.8, h],
            note: clip(sent, 160),
        });
        steps.push(Step {
            caption: clip(sent, 240),
            parts: vec![id],
            anchor: src.map(|s| s.anchor.clone()),
        });
    }
    steps[0].parts = parts.iter().skip(1).map(|p| p.id.clone()).collect();
    let links = (1..n)
        .map(|i| Link {
            from: format!("p{i}"),
            to: format!("p{}", i + 1),
            label: String::new(),
        })
        .collect();
    steps.truncate(MAX_STEPS);
    Scene {
        id: String::new(),
        concept: clip(&concept, 80),
        summary,
        parts,
        links,
        steps,
        topic: topic.map(str::to_owned),
        written_by: "fallback".into(),
        created_at: now,
    }
}

// ---- Drawing -----------------------------------------------------------

/// The camera: the isometric angle plus a small tilt from the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct View {
    /// Extra turn about the vertical, radians.
    pub yaw: f32,
    /// Extra tilt, radians.
    pub pitch: f32,
    /// A part raised a little (the one under the pointer).
    pub lift: Option<usize>,
}

impl View {
    /// The view for a pointer at `(px, py)`, each -1 to 1 from the centre:
    /// at most 8° of turn and 4° of tilt.
    pub fn tilted(px: f32, py: f32) -> Self {
        Self {
            yaw: px.clamp(-1.0, 1.0) * 8f32.to_radians(),
            pitch: py.clamp(-1.0, 1.0) * 4f32.to_radians(),
            lift: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceKind {
    Top,
    Side,
}

/// A shaded face of a part.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub part: usize,
    pub kind: FaceKind,
    pub pts: Vec<(f32, f32)>,
    /// 0 (darkest side) to 1 (a lit top).
    pub tone: f32,
}

/// A hairline: a part's edge, or the floor grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub part: Option<usize>,
    pub pts: Vec<(f32, f32)>,
    pub closed: bool,
}

/// A link drawn as a dashed hairline with a head at the target.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkLine {
    pub from: usize,
    pub to: usize,
    pub a: (f32, f32),
    pub b: (f32, f32),
    pub head: [(f32, f32); 3],
    pub label: String,
}

/// Where a part's label goes.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelPt {
    pub part: usize,
    pub at: (f32, f32),
    pub text: String,
}

/// The scene as flat shapes, back to front, in unscaled screen units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawing {
    pub grid: Vec<Edge>,
    pub faces: Vec<Face>,
    pub edges: Vec<Edge>,
    pub links: Vec<LinkLine>,
    pub labels: Vec<LabelPt>,
    /// (min x, min y, max x, max y).
    pub bounds: (f32, f32, f32, f32),
}

struct Cam {
    ca: f32,
    sa: f32,
    se: f32,
    ce: f32,
}

impl Cam {
    fn new(v: View) -> Self {
        let a = 45f32.to_radians() + v.yaw;
        let e = (30f32.to_radians() + v.pitch).clamp(0.2, 1.2);
        Self {
            ca: a.cos(),
            sa: a.sin(),
            se: e.sin(),
            ce: e.cos(),
        }
    }

    /// Screen point and depth (larger is nearer the viewer).
    fn p(&self, x: f32, y: f32, z: f32) -> ((f32, f32), f32) {
        let (dx, dy) = (x - GRID / 2.0, y - GRID / 2.0);
        let rx = dx * self.ca - dy * self.sa;
        let ry = dx * self.sa + dy * self.ca;
        ((rx, ry * self.se - z * self.ce), ry * self.ce + z * self.se)
    }

    /// How much an outward floor normal faces the viewer, and how far it
    /// points to the screen's right.
    fn facing(&self, nx: f32, ny: f32) -> (f32, f32) {
        (nx * self.sa + ny * self.ca, nx * self.ca - ny * self.sa)
    }
}

/// A part's floor outline, counter-clockwise.
fn outline(p: &Part) -> Vec<(f32, f32)> {
    let [x, y, _] = p.at;
    let [w, d, _] = p.size;
    let mut pts = match p.shape {
        Shape::Block | Shape::Plate => vec![(x, y), (x + w, y), (x + w, y + d), (x, y + d)],
        Shape::Cylinder => {
            let (cx, cy, rx, ry) = (x + w / 2.0, y + d / 2.0, w / 2.0, d / 2.0);
            (0..20)
                .map(|i| {
                    let a = i as f32 / 20.0 * std::f32::consts::TAU;
                    (cx + rx * a.cos(), cy + ry * a.sin())
                })
                .collect()
        }
        Shape::Arrow => {
            let (dx, dy) = (p.size[0], p.size[1]);
            let len = dx.hypot(dy).max(0.5);
            let (ux, uy) = (dx / len, dy / len);
            let (nx, ny) = (-uy, ux);
            let head = (len * 0.4).min(1.2);
            let (sw, hw) = (0.25, 0.6);
            let shaft = len - head;
            let at = |u: f32, n: f32| (x + ux * u + nx * n, y + uy * u + ny * n);
            vec![
                at(0.0, -sw),
                at(shaft, -sw),
                at(shaft, -hw),
                at(len, 0.0),
                at(shaft, hw),
                at(shaft, sw),
                at(0.0, sw),
            ]
        }
        Shape::Label => Vec::new(),
    };
    let area: f32 = pts
        .iter()
        .zip(pts.iter().cycle().skip(1))
        .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
        .sum();
    if area < 0.0 {
        pts.reverse();
    }
    pts
}

fn height(p: &Part) -> f32 {
    match p.shape {
        Shape::Label => 0.0,
        _ => p.size[2],
    }
}

/// A part's top centre, in floor units.
fn top_centre(p: &Part, lift: f32) -> (f32, f32, f32) {
    let o = outline(p);
    let (cx, cy) = if o.is_empty() {
        (p.at[0], p.at[1])
    } else {
        let n = o.len() as f32;
        (
            o.iter().map(|q| q.0).sum::<f32>() / n,
            o.iter().map(|q| q.1).sum::<f32>() / n,
        )
    };
    (cx, cy, p.at[2] + height(p) + lift)
}

/// Projects the scene for `view`.
pub fn project(scene: &Scene, view: View) -> Drawing {
    let cam = Cam::new(view);
    let mut d = Drawing::default();
    // The floor grid, every two units.
    let mut g = 0.0;
    while g <= GRID + 0.01 {
        d.grid.push(Edge {
            part: None,
            pts: vec![cam.p(g, 0.0, 0.0).0, cam.p(g, GRID, 0.0).0],
            closed: false,
        });
        d.grid.push(Edge {
            part: None,
            pts: vec![cam.p(0.0, g, 0.0).0, cam.p(GRID, g, 0.0).0],
            closed: false,
        });
        g += 2.0;
    }
    // Plates first (others stand on them), then by depth.
    let mut order: Vec<(bool, f32, usize)> = scene
        .parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (x, y, z) = top_centre(p, 0.0);
            let depth = cam.p(x, y, z - height(p) / 2.0).1;
            (p.shape != Shape::Plate, depth, i)
        })
        .collect();
    order.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    for (_, _, i) in order {
        let p = &scene.parts[i];
        let lift = if view.lift == Some(i) { 0.25 } else { 0.0 };
        let base = outline(p);
        if base.is_empty() {
            continue;
        }
        let z0 = p.at[2] + lift;
        let z1 = z0 + height(p);
        let n = base.len();
        let mut sides: Vec<(f32, Face, bool)> = Vec::new();
        for k in 0..n {
            let (a, b) = (base[k], base[(k + 1) % n]);
            let (ex, ey) = (b.0 - a.0, b.1 - a.1);
            let len = ex.hypot(ey).max(1e-6);
            let (toward, right) = cam.facing(ey / len, -ex / len);
            if toward <= 1e-4 {
                continue;
            }
            let pts = vec![
                cam.p(a.0, a.1, z0).0,
                cam.p(b.0, b.1, z0).0,
                cam.p(b.0, b.1, z1).0,
                cam.p(a.0, a.1, z1).0,
            ];
            let depth = cam.p((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0, z0).1;
            sides.push((
                depth,
                Face {
                    part: i,
                    kind: FaceKind::Side,
                    pts,
                    // Left-facing sides lit, right-facing in shade.
                    tone: 0.5 - 0.16 * right,
                },
                p.shape != Shape::Cylinder,
            ));
        }
        sides.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, f, outline_it) in &sides {
            if *outline_it {
                d.edges.push(Edge {
                    part: Some(i),
                    pts: f.pts.clone(),
                    closed: true,
                });
            }
        }
        if p.shape == Shape::Cylinder {
            // The visible lower rim and the two silhouette lines.
            let vis: Vec<bool> = (0..n)
                .map(|k| {
                    let (a, b) = (base[k], base[(k + 1) % n]);
                    let (ex, ey) = (b.0 - a.0, b.1 - a.1);
                    cam.facing(ey, -ex).0 > 1e-4
                })
                .collect();
            for k in 0..n {
                if vis[k] {
                    let (a, b) = (base[k], base[(k + 1) % n]);
                    d.edges.push(Edge {
                        part: Some(i),
                        pts: vec![cam.p(a.0, a.1, z0).0, cam.p(b.0, b.1, z0).0],
                        closed: false,
                    });
                }
                if vis[k] != vis[(k + n - 1) % n] {
                    let a = base[k];
                    d.edges.push(Edge {
                        part: Some(i),
                        pts: vec![cam.p(a.0, a.1, z0).0, cam.p(a.0, a.1, z1).0],
                        closed: false,
                    });
                }
            }
        }
        d.faces.extend(sides.into_iter().map(|s| s.1));
        let top: Vec<(f32, f32)> = base.iter().map(|q| cam.p(q.0, q.1, z1).0).collect();
        d.edges.push(Edge {
            part: Some(i),
            pts: top.clone(),
            closed: true,
        });
        d.faces.push(Face {
            part: i,
            kind: FaceKind::Top,
            pts: top,
            tone: if p.shape == Shape::Plate { 0.8 } else { 0.92 },
        });
    }
    // Keep edges in the same back-to-front order as their faces: draw them
    // after all faces as the line overlay.
    for (i, p) in scene.parts.iter().enumerate() {
        let lift = if view.lift == Some(i) { 0.25 } else { 0.0 };
        let at = if p.shape == Shape::Plate {
            // A base is named under its nearest corner, clear of what
            // stands on it.
            let (sx, sy) = cam
                .p(p.at[0] + p.size[0], p.at[1] + p.size[1], p.at[2] + lift)
                .0;
            (sx, sy + 0.9)
        } else {
            let (x, y, z) = top_centre(p, lift);
            let up = if p.shape == Shape::Label { 0.0 } else { 0.55 };
            cam.p(x, y, z + up).0
        };
        d.labels.push(LabelPt {
            part: i,
            at,
            text: p.label.clone(),
        });
    }
    for l in &scene.links {
        let (Some(fi), Some(ti)) = (scene.part_index(&l.from), scene.part_index(&l.to)) else {
            continue;
        };
        let (fp, tp) = (&scene.parts[fi], &scene.parts[ti]);
        let (ax, ay, az) = top_centre(fp, 0.0);
        let (bx, by, bz) = top_centre(tp, 0.0);
        let a = cam.p(ax, ay, az + 0.15).0;
        let b = cam.p(bx, by, bz + 0.15).0;
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy).max(1e-6);
        let (ux, uy) = (dx / len, dy / len);
        // Stop short of the target's centre, with a small head.
        let tip = (b.0 - ux * len * 0.18, b.1 - uy * len * 0.18);
        let start = (a.0 + ux * len * 0.18, a.1 + uy * len * 0.18);
        let hs = 0.2;
        let head = [
            tip,
            (
                tip.0 - ux * hs - uy * hs * 0.6,
                tip.1 - uy * hs + ux * hs * 0.6,
            ),
            (
                tip.0 - ux * hs + uy * hs * 0.6,
                tip.1 - uy * hs - ux * hs * 0.6,
            ),
        ];
        d.links.push(LinkLine {
            from: fi,
            to: ti,
            a: start,
            b: tip,
            head,
            label: l.label.clone(),
        });
    }
    let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut grow = |q: (f32, f32)| {
        b.0 = b.0.min(q.0);
        b.1 = b.1.min(q.1);
        b.2 = b.2.max(q.0);
        b.3 = b.3.max(q.1);
    };
    for e in d.grid.iter().chain(d.edges.iter()) {
        e.pts.iter().copied().for_each(&mut grow);
    }
    for l in &d.labels {
        grow((l.at.0, l.at.1 - 0.6));
    }
    if b.0 > b.2 {
        b = (0.0, 0.0, 1.0, 1.0);
    }
    d.bounds = b;
    d
}

/// Scales a drawing into a box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub scale: f32,
    pub dx: f32,
    pub dy: f32,
}

impl Fit {
    /// Fits `bounds` into `w` × `h` with `margin` on every side, centred.
    pub fn new(bounds: (f32, f32, f32, f32), w: f32, h: f32, margin: f32) -> Self {
        let (bw, bh) = (
            (bounds.2 - bounds.0).max(1e-3),
            (bounds.3 - bounds.1).max(1e-3),
        );
        let scale = ((w - 2.0 * margin) / bw)
            .min((h - 2.0 * margin) / bh)
            .max(0.01);
        Self {
            scale,
            dx: (w - bw * scale) / 2.0 - bounds.0 * scale,
            dy: (h - bh * scale) / 2.0 - bounds.1 * scale,
        }
    }

    pub fn at(&self, p: (f32, f32)) -> (f32, f32) {
        (p.0 * self.scale + self.dx, p.1 * self.scale + self.dy)
    }
}

fn inside(pts: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut c = false;
    let n = pts.len();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + n - 1) % n]);
        if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
    }
    c
}

/// The part under `(x, y)` (in fitted pixels): the front-most face hit,
/// or a label's text near the point.
pub fn hit(d: &Drawing, fit: &Fit, x: f32, y: f32) -> Option<usize> {
    for f in d.faces.iter().rev() {
        let pts: Vec<(f32, f32)> = f.pts.iter().map(|p| fit.at(*p)).collect();
        if inside(&pts, x, y) {
            return Some(f.part);
        }
    }
    d.labels.iter().find_map(|l| {
        let (lx, ly) = fit.at(l.at);
        let half = l.text.chars().count() as f32 * 3.4 + 6.0;
        ((x - lx).abs() <= half && (y - ly).abs() <= 10.0).then_some(l.part)
    })
}

/// How much ink goes into a face of `tone` (mixed over the background):
/// in the dark theme lit faces are lighter, in the light theme lit faces
/// are paler, so the top always reads as the lit one.
pub fn ink_amount(tone: f32, dark: bool) -> f32 {
    let t = tone.clamp(0.0, 1.0);
    if dark {
        0.10 + t * 0.30
    } else {
        0.05 + (1.0 - t) * 0.34
    }
}

/// Colours for the SVG snapshot, as `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SvgStyle {
    pub background: u32,
    pub ink: u32,
    pub faint: u32,
    pub accent: u32,
    pub dark: bool,
}

impl Default for SvgStyle {
    /// Palette C, light.
    fn default() -> Self {
        Self {
            background: 0xFBFBFA,
            ink: 0x141414,
            faint: 0x8C8C87,
            accent: 0x56603F,
            dark: false,
        }
    }
}

/// `a` mixed toward `b` by `t`.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |s: u32| {
        let x = ((a >> s) & 255) as f32;
        let y = ((b >> s) & 255) as f32;
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn path(pts: &[(f32, f32)], fit: &Fit, closed: bool) -> String {
    let mut s = String::new();
    for (i, p) in pts.iter().enumerate() {
        let (x, y) = fit.at(*p);
        s.push_str(&format!("{}{x:.1} {y:.1} ", if i == 0 { "M" } else { "L" }));
    }
    if closed {
        s.push('Z');
    }
    s.trim_end().to_owned()
}

/// Splits text into lines of about `n` characters.
fn wrap(s: &str, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for w in s.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > n {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The scene as a standalone SVG, `w` × `h`, at `step` (its parts in the
/// accent and its caption under the drawing), or the whole machine with
/// no step lit.
pub fn to_svg(scene: &Scene, step: Option<usize>, style: SvgStyle, w: u32, h: u32) -> String {
    let (wf, hf) = (w as f32, h as f32);
    let caption_h = 96.0;
    let d = project(scene, View::default());
    let fit = Fit::new(d.bounds, wf, hf - caption_h - 40.0, 28.0);
    let fit = Fit {
        dy: fit.dy + 40.0,
        ..fit
    };
    let lit = step.map(|s| scene.lit(s)).unwrap_or_default();
    let hex = |c: u32| format!("#{c:06X}");
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\" font-family=\"IBM Plex Sans, Helvetica, Arial, sans-serif\">\n<title>Explain in 3D: {}</title>\n<rect width=\"100%\" height=\"100%\" fill=\"{}\"/>\n",
        esc(&scene.concept),
        hex(style.background)
    );
    s.push_str(&format!(
        "<text x=\"24\" y=\"34\" font-size=\"18\" font-style=\"italic\" fill=\"{}\">{}</text>\n",
        hex(style.ink),
        esc(&scene.concept)
    ));
    s.push_str(&format!(
        "<g stroke=\"{}\" stroke-width=\"0.5\" opacity=\"0.45\">\n",
        hex(style.faint)
    ));
    for e in &d.grid {
        s.push_str(&format!("<path d=\"{}\"/>\n", path(&e.pts, &fit, false)));
    }
    s.push_str("</g>\n<g stroke=\"none\">\n");
    for f in &d.faces {
        let mut c = mix(style.background, style.ink, ink_amount(f.tone, style.dark));
        if lit.contains(&f.part) {
            c = mix(
                c,
                style.accent,
                if f.kind == FaceKind::Top { 0.55 } else { 0.4 },
            );
        }
        // The same colour as a thin stroke closes the seams between facets.
        s.push_str(&format!(
            "<path d=\"{}\" fill=\"{1}\" stroke=\"{1}\" stroke-width=\"0.6\"/>\n",
            path(&f.pts, &fit, true),
            hex(c)
        ));
    }
    s.push_str(&format!(
        "</g>\n<g fill=\"none\" stroke=\"{}\" stroke-width=\"0.8\" stroke-linejoin=\"round\">\n",
        hex(style.ink)
    ));
    for e in &d.edges {
        let on = e.part.is_some_and(|p| lit.contains(&p));
        s.push_str(&format!(
            "<path d=\"{}\"{}/>\n",
            path(&e.pts, &fit, e.closed),
            if on {
                format!(" stroke=\"{}\" stroke-width=\"1.4\"", hex(style.accent))
            } else {
                String::new()
            }
        ));
    }
    s.push_str("</g>\n");
    s.push_str(&format!(
        "<g stroke=\"{0}\" fill=\"{0}\" stroke-width=\"0.9\">\n",
        hex(style.ink)
    ));
    for l in &d.links {
        s.push_str(&format!(
            "<path d=\"{}\" fill=\"none\" stroke-dasharray=\"4 3\"/>\n<path d=\"{}\" stroke=\"none\"/>\n",
            path(&[l.a, l.b], &fit, false),
            path(&l.head, &fit, true)
        ));
    }
    s.push_str("</g>\n");
    for l in &d.labels {
        let (x, y) = fit.at(l.at);
        let on = lit.contains(&l.part);
        s.push_str(&format!(
            "<text x=\"{x:.1}\" y=\"{y:.1}\" font-size=\"12\" text-anchor=\"middle\" fill=\"{}\"{}>{}</text>\n",
            hex(if on { style.accent } else { style.ink }),
            if on { " font-weight=\"600\"" } else { "" },
            esc(&l.text)
        ));
    }
    if let Some(st) = step.and_then(|i| scene.steps.get(i).map(|s| (i, s))) {
        let (i, st) = st;
        let mut y = hf - caption_h + 18.0;
        s.push_str(&format!(
            "<text x=\"24\" y=\"{y:.1}\" font-size=\"11\" fill=\"{}\">Step {} of {}</text>\n",
            hex(style.faint),
            i + 1,
            scene.steps.len()
        ));
        for line in wrap(&st.caption, ((wf - 48.0) / 7.2) as usize)
            .into_iter()
            .take(3)
        {
            y += 18.0;
            s.push_str(&format!(
                "<text x=\"24\" y=\"{y:.1}\" font-size=\"14\" fill=\"{}\">{}</text>\n",
                hex(style.ink),
                esc(&line)
            ));
        }
        if let Some(c) = st.cite() {
            s.push_str(&format!(
                "<text x=\"24\" y=\"{:.1}\" font-size=\"11\" fill=\"{}\">{}</text>\n",
                y + 18.0,
                hex(style.faint),
                esc(&c)
            ));
        }
    } else if !scene.summary.is_empty() {
        for (k, line) in wrap(&scene.summary, ((wf - 48.0) / 7.2) as usize)
            .into_iter()
            .take(3)
            .enumerate()
        {
            s.push_str(&format!(
                "<text x=\"24\" y=\"{:.1}\" font-size=\"14\" fill=\"{}\">{}</text>\n",
                hf - caption_h + 36.0 + k as f32 * 18.0,
                hex(style.ink),
                esc(&line)
            ));
        }
    }
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(n: usize, text: &str) -> Source {
        Source {
            n,
            chunk: format!("m1#{n}"),
            anchor: Anchor {
                material: "m1".into(),
                file: "os.pdf".into(),
                page: Some(n as u32),
                section: Some("Memory".into()),
            },
            text: text.into(),
        }
    }

    fn sources() -> Vec<Source> {
        vec![
            src(
                1,
                "A page table is a map from virtual pages to physical frames. The processor checks the page table on every memory access.",
            ),
            src(
                2,
                "A TLB is a small cache of recent translations. A page fault is a trap raised when a page is not in memory, and the kernel loads it from disk.",
            ),
        ]
    }

    const REPLY: &str = r#"Sure. ```json
{"concept": "Virtual memory", "summary": "Addresses are translated page by page.",
 "parts": [
  {"id": "cpu", "label": "CPU", "shape": "box", "at": [1, 5, 0], "size": [2, 2, 1.5], "note": "issues virtual addresses"},
  {"label": "TLB", "shape": "cylinder", "position": {"x": 5, "y": 5}, "size": [1.5, 1.5, 1]},
  {"id": "pt", "label": "Page table", "shape": "slab", "at": [8, 99, -3], "size": [3, 3, 0.4]},
  {"label": "", "shape": "block"},
  {"id": "flow", "label": "lookup", "shape": "arrow", "at": [3, 6, 0.2], "size": [2, 0, 0.2]}
 ],
 "links": [{"from": "cpu", "to": "TLB", "label": "address"}, {"from": "cpu", "to": "ghost"}, {"from": "tlb", "to": "tlb"}],
 "steps": [
  {"caption": "The CPU asks the TLB first.", "parts": ["cpu", "TLB"], "source": "S2"},
  {"text": "On a miss, the page table answers.", "highlight": "pt", "source": 1},
  {"parts": ["cpu"]}
 ]}
```"#;

    #[test]
    fn parses_a_model_scene_leniently() {
        let s = parse_scene(REPLY, "virtual memory", &sources(), Some("memory"), 5).unwrap();
        assert_eq!(s.concept, "Virtual memory");
        assert_eq!(s.parts.len(), 4, "the unnamed part is dropped");
        assert_eq!(s.parts[0].shape, Shape::Block);
        assert_eq!(s.parts[1].id, "tlb");
        assert_eq!(s.parts[1].shape, Shape::Cylinder);
        assert_eq!(s.parts[1].at, [5.0, 5.0, 0.0]);
        assert_eq!(s.parts[2].shape, Shape::Plate);
        assert_eq!(s.parts[2].at, [8.0, GRID, 0.0], "clamped to the floor");
        assert_eq!(s.parts[3].shape, Shape::Arrow);
        assert_eq!(s.links.len(), 1, "unknown and self links dropped");
        assert_eq!(s.links[0].to, "tlb", "links resolve labels too");
        assert_eq!(s.steps.len(), 2, "a step with no caption is dropped");
        assert_eq!(s.steps[0].parts, vec!["cpu", "tlb"]);
        assert_eq!(s.steps[0].cite().as_deref(), Some("os.pdf p. 2 · Memory"));
        assert_eq!(s.steps[1].parts, vec!["pt"]);
        assert_eq!(s.lit(0).len(), 2);
        assert_eq!(s.topic.as_deref(), Some("memory"));
        assert!(parse_scene("nothing", "x", &sources(), None, 0).is_none());
        assert!(
            parse_scene(r#"{"parts": [{"label": "one"}]}"#, "x", &sources(), None, 0).is_none()
        );
        // No steps: one per part.
        let s = parse_scene(
            r#"{"parts": [{"label": "a"}, {"label": "b", "note": "second"}]}"#,
            "ab",
            &[],
            None,
            0,
        )
        .unwrap();
        assert_eq!(s.steps.len(), 2);
        assert_eq!(s.steps[1].caption, "b: second");
        assert_eq!(s.parts[1].at, [4.0, 4.0, 0.0], "laid out in a row");
        let (sys, user) = scene_prompt("", &sources());
        assert!(sys.contains("\"cylinder\""));
        assert!(user.starts_with("Pick the single"));
    }

    #[test]
    fn fallback_builds_a_cited_scene_deterministically() {
        let s = fallback_scene("page table", &sources(), None, 1);
        assert_eq!(s.written_by, "fallback");
        assert_eq!(s.parts[0].shape, Shape::Plate);
        assert!(s.parts.len() >= 3 && s.parts.len() <= 6, "{:?}", s.parts);
        assert!(
            s.parts.iter().any(|p| p.label == "TLB"),
            "{:?}",
            s.parts.iter().map(|p| &p.label).collect::<Vec<_>>()
        );
        // The step for each part cites the passage that names it.
        assert_eq!(s.steps.len(), s.parts.len());
        assert_eq!(
            s.steps[0].parts.len(),
            s.parts.len() - 1,
            "overview lights everything"
        );
        assert!(s.steps[1..].iter().all(|st| st.anchor.is_some()));
        assert_eq!(s.links.len(), s.parts.len() - 2);
        assert_eq!(s, fallback_scene("page table", &sources(), None, 1));
        // A store word makes a cylinder.
        let c = fallback_scene(
            "",
            &[src(
                1,
                "The cache is a small fast memory. The cache holds recent data near the processor.",
            )],
            None,
            0,
        );
        assert!(
            c.parts.iter().any(|p| p.shape == Shape::Cylinder),
            "{:?}",
            c.parts
        );
        assert_eq!(c.concept, "Memory");
        // No materials: still a scene, honest about it.
        let e = fallback_scene("quantum tunnelling", &[], None, 0);
        assert!(e.parts.len() >= 3);
        assert!(e.steps[1].caption.contains("do not describe"));
    }

    #[test]
    fn projection_shades_hides_and_hits() {
        let s = parse_scene(REPLY, "", &sources(), None, 0).unwrap();
        let d = project(&s, View::default());
        // A block shows its top and two sides; tops are the lightest.
        let cpu: Vec<&Face> = d.faces.iter().filter(|f| f.part == 0).collect();
        assert_eq!(cpu.len(), 3);
        let top = cpu.iter().find(|f| f.kind == FaceKind::Top).unwrap();
        assert!(cpu.iter().all(|f| f.tone <= top.tone));
        let sides: Vec<f32> = cpu
            .iter()
            .filter(|f| f.kind == FaceKind::Side)
            .map(|f| f.tone)
            .collect();
        assert!(
            (sides[0] - sides[1]).abs() > 0.1,
            "one side lit, one in shade"
        );
        // The plate is drawn before the parts standing on it.
        let first_plate = d.faces.iter().position(|f| f.part == 2).unwrap();
        let first_block = d.faces.iter().position(|f| f.part == 0).unwrap();
        assert!(first_plate < first_block);
        assert_eq!(d.links.len(), 1);
        assert_eq!(d.labels.len(), 4);
        let fit = Fit::new(d.bounds, 640.0, 420.0, 20.0);
        // A point just inside the CPU's top face.
        let c = d
            .faces
            .iter()
            .find(|f| f.part == 0 && f.kind == FaceKind::Top)
            .unwrap();
        let n = c.pts.len() as f32;
        let mid = (
            c.pts.iter().map(|p| p.0).sum::<f32>() / n,
            c.pts.iter().map(|p| p.1).sum::<f32>() / n,
        );
        let (mx, my) = fit.at(mid);
        assert_eq!(hit(&d, &fit, mx, my), Some(0));
        assert_eq!(hit(&d, &fit, -50.0, -50.0), None);
        // Everything fits in the box.
        for f in &d.faces {
            for p in &f.pts {
                let (x, y) = fit.at(*p);
                assert!((-0.5..=640.5).contains(&x) && (-0.5..=420.5).contains(&y));
            }
        }
        // Tilting and lifting move things.
        let t = project(
            &s,
            View {
                lift: Some(0),
                ..View::tilted(1.0, -1.0)
            },
        );
        assert_ne!(t.faces, d.faces);
        // A cylinder outlines its rim and silhouette, not every facet.
        let rim = d.edges.iter().filter(|e| e.part == Some(1)).count();
        assert!(rim < 20, "{rim}");
    }

    #[test]
    fn svg_snapshot_is_well_formed() {
        let s = parse_scene(REPLY, "", &sources(), None, 0).unwrap();
        let svg = to_svg(&s, Some(0), SvgStyle::default(), 800, 560);
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains("Step 1 of 2"));
        assert!(svg.contains("os.pdf p. 2 · Memory"));
        assert!(svg.contains("#56603F"), "the lit step uses the accent");
        assert_eq!(
            svg.matches("<path").count(),
            svg.matches("/>").count() - 1,
            "every path closes"
        );
        let plain = to_svg(&s, None, SvgStyle::default(), 800, 560);
        assert!(plain.contains("Addresses are translated"));
        assert!(!plain.contains("stroke=\"#56603F\""));
        assert_eq!(mix(0x000000, 0xFFFFFF, 0.5), 0x808080);
        assert!(ink_amount(0.9, false) < ink_amount(0.3, false));
        assert!(ink_amount(0.9, true) > ink_amount(0.3, true));
        let md = s.to_markdown(Some("vm.svg"));
        assert!(md.contains("1. The CPU asks the TLB first. (os.pdf p. 2 · Memory)"));
        assert!(md.contains("![Virtual memory](vm.svg)"));
    }
}
