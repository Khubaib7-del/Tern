//! Explain in 3D for a whole chat answer: the answer mapped from the
//! basics to the advanced parts, as levels of nodes on the floor
//! (Foundations, Core idea, Mechanism, Details and edge cases, Advanced
//! and implications), with sub-nodes for key terms and links for what
//! builds on what. Each node keeps a short explanation and the passage of
//! the answer it comes from, citations included.
//!
//! The number of nodes follows the answer's length and structure (4 to
//! [`MAX_NODES`]), so a long answer comparing approaches gets a map of its
//! own size, not a fixed handful of steps. [`answer_scene`] builds one
//! without a model (deterministic); [`answer_prompt`] and
//! [`parse_answer_scene`] let a model write the nodes, and [`lay_out`]
//! places either on the floor by level.

use serde_json::Value;

use crate::explain3d::{GRID, Link, Part, Scene, Shape, Step};
use crate::material::Anchor;
use crate::text::{clip, field, json_in, one_line, sentences, slug};

/// The levels, basic to advanced.
pub const LEVELS: [&str; 5] = [
    "Foundations",
    "Core idea",
    "Mechanism",
    "Details and edge cases",
    "Advanced and implications",
];

/// Most nodes (main nodes and key-term sub-nodes together).
pub const MAX_NODES: usize = 20;
/// Fewest main nodes.
pub const MIN_NODES: usize = 4;
/// Most characters of the answer read.
pub const ANSWER_CHARS: usize = 40_000;

/// One passage of the answer: a heading's section, a paragraph, or a list
/// item, with the heading it sits under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Passage {
    pub heading: Option<String>,
    pub text: String,
    /// Where it starts, for keeping reading order.
    pub at: usize,
}

/// Splits an answer into passages at headings, blank lines, and list
/// items. Code fences stay with their paragraph.
pub fn passages(answer: &str) -> Vec<Passage> {
    let mut out: Vec<Passage> = Vec::new();
    let mut heading: Option<String> = None;
    let mut cur = String::new();
    let mut fence = false;
    let mut start = 0;
    let flush =
        |cur: &mut String, out: &mut Vec<Passage>, heading: &Option<String>, start: usize| {
            let t = cur.trim();
            if t.chars().filter(|c| c.is_alphanumeric()).count() >= 3 {
                out.push(Passage {
                    heading: heading.clone(),
                    text: t.to_owned(),
                    at: start,
                });
            }
            cur.clear();
        };
    for (i, line) in answer.lines().enumerate() {
        let l = line.trim();
        if l.starts_with("```") {
            fence = !fence;
            cur.push_str(line);
            cur.push('\n');
            continue;
        }
        if fence {
            cur.push_str(line);
            cur.push('\n');
            continue;
        }
        if let Some(h) = l.strip_prefix('#') {
            flush(&mut cur, &mut out, &heading, start);
            heading = Some(h.trim_start_matches('#').trim().to_owned()).filter(|h| !h.is_empty());
            start = i + 1;
            continue;
        }
        let item = l.starts_with("- ")
            || l.starts_with("* ")
            || l.starts_with("• ")
            || l.split_once(". ").is_some_and(|(n, _)| {
                !n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit())
            });
        if l.is_empty() || (item && !cur.trim().is_empty()) {
            flush(&mut cur, &mut out, &heading, start);
            start = i;
            if l.is_empty() {
                continue;
            }
        }
        if cur.is_empty() {
            start = i;
        }
        cur.push_str(line);
        cur.push('\n');
    }
    flush(&mut cur, &mut out, &heading, start);
    out
}

fn has(text: &str, words: &[&str]) -> bool {
    let t = format!(" {} ", text.to_lowercase());
    words.iter().any(|w| t.contains(w))
}

/// The level a passage teaches, from its words and where it sits (a
/// fraction 0 to 1 through the answer).
pub fn level_of(p: &Passage, frac: f32) -> u8 {
    let t = format!("{} {}", p.heading.clone().unwrap_or_default(), p.text);
    if has(
        &t,
        &[
            " in short",
            " tl;dr",
            " the key idea",
            " the main idea",
            " core idea",
            " in a nutshell",
            " summary",
        ],
    ) {
        return 1;
    }
    if has(
        &t,
        &[
            " advanced",
            " future",
            " implication",
            " at scale",
            " in production",
            " open question",
            " research direction",
            " compar",
            " versus ",
            " vs ",
            " vs. ",
            " which to choose",
            " when to use",
            " trade-offs between",
        ],
    ) {
        return 4;
    }
    if has(
        &t,
        &[
            " however",
            " limitation",
            " edge case",
            " caveat",
            " trade-off",
            " tradeoff",
            " fails ",
            " failure",
            " pitfall",
            " risk",
            " note that",
            " drawback",
            " downside",
            " cost ",
        ],
    ) {
        return 3;
    }
    if has(
        &t,
        &[
            " how it works",
            " works by",
            " step ",
            " steps",
            " process",
            " pipeline",
            " mechanism",
            " algorithm",
            " first,",
            " then ",
            " finally",
            " under the hood",
            " implementation",
            " method",
        ],
    ) {
        return 2;
    }
    if frac < 0.12
        || has(
            &t,
            &[
                " is a ",
                " is an ",
                " refers to",
                " means ",
                " defined as",
                " basics",
                " background",
                " definition",
                " what is",
            ],
        )
    {
        return 0;
    }
    (1. + frac * 3.5).floor().clamp(1., 4.) as u8
}

/// How many main nodes an answer gets: 4 for a short answer, more with
/// length and headings, at most [`MAX_NODES`] less room for terms.
pub fn target_nodes(answer: &str, passages: usize, headings: usize) -> usize {
    let words = answer.split_whitespace().count();
    let want = MIN_NODES + words / 90 + headings;
    want.min(passages.max(MIN_NODES))
        .clamp(MIN_NODES, MAX_NODES - 4)
}

/// `[1]`, `[2][3]`, `[12]` in a passage, in order, without repeats.
pub fn citations(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' {
            let j = i + 1;
            let mut k = j;
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            if k > j && k < b.len() && b[k] == b']' && k - j <= 3 {
                let c = text[i..=k].to_owned();
                if !out.contains(&c) {
                    out.push(c);
                }
                i = k + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Key terms in a passage: **bold** and `code` spans, at most `n`.
pub fn key_terms(text: &str, n: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (open, close) in [("**", "**"), ("`", "`")] {
        let mut rest = text;
        while let Some(a) = rest.find(open) {
            let after = &rest[a + open.len()..];
            let Some(b) = after.find(close) else { break };
            let term = one_line(&after[..b]);
            let len = term.chars().count();
            if (2..=40).contains(&len) && !out.iter().any(|t| t.eq_ignore_ascii_case(&term)) {
                out.push(term);
            }
            rest = &after[b + close.len()..];
        }
    }
    out.truncate(n);
    out
}

fn strip_marks(s: &str) -> String {
    one_line(&s.replace("**", "").replace('`', "").replace("__", ""))
        .trim_start_matches(['-', '*', '•', ' '])
        .to_owned()
}

/// A node's label: its heading, else its first bold term, else the first
/// words of its first sentence.
fn label_for(p: &Passage, used: &[String]) -> String {
    if let Some(h) = &p.heading
        && !used.iter().any(|u| u == h)
    {
        return clip(&strip_marks(h), 40);
    }
    if let Some(t) = key_terms(&p.text, 1).into_iter().next()
        && !used.contains(&t)
    {
        return clip(&t, 40);
    }
    let first = sentences(&strip_marks(&p.text))
        .into_iter()
        .next()
        .unwrap_or_default();
    let words: Vec<&str> = first.split_whitespace().take(6).collect();
    clip(&words.join(" "), 40)
}

/// Fits `ps` to `n` passages: adjacent passages of the same level merge
/// (the shortest pair first) while there are too many, and the longest
/// splits at a sentence while there are too few.
fn fit_count(mut ps: Vec<(u8, Passage)>, n: usize) -> Vec<(u8, Passage)> {
    while ps.len() > n {
        let best = (0..ps.len() - 1)
            .filter(|&i| ps[i].0 == ps[i + 1].0)
            .min_by_key(|&i| ps[i].1.text.len() + ps[i + 1].1.text.len())
            .or_else(|| {
                (0..ps.len() - 1).min_by_key(|&i| ps[i].1.text.len() + ps[i + 1].1.text.len())
            });
        let Some(i) = best else { break };
        let next = ps.remove(i + 1);
        ps[i].1.text = format!("{}\n\n{}", ps[i].1.text, next.1.text);
        ps[i].0 = ps[i].0.min(next.0);
    }
    while ps.len() < n {
        let Some(i) = (0..ps.len())
            .filter(|&i| sentences(&ps[i].1.text).len() >= 2)
            .max_by_key(|&i| ps[i].1.text.len())
        else {
            break;
        };
        let ss = sentences(&ps[i].1.text);
        let half = ss.len() / 2;
        let (a, b) = (ss[..half].join(" "), ss[half..].join(" "));
        let mut second = ps[i].1.clone();
        second.text = b;
        second.heading = None;
        ps[i].1.text = a;
        let lvl = ps[i].0;
        ps.insert(i + 1, ((lvl + 1).min(4), second));
    }
    ps
}

fn chat_anchor(section: Option<String>) -> Anchor {
    Anchor {
        material: "chat".into(),
        file: "This answer".into(),
        page: None,
        section,
    }
}

/// The scene for a whole answer without a model: passages become nodes,
/// put in level order (basics first, keeping reading order inside a
/// level), with key-term sub-nodes, links from each level to the one
/// before it, and one step per node.
pub fn answer_scene(answer: &str, concept: &str, now: i64) -> Scene {
    let answer: String = answer.chars().take(ANSWER_CHARS).collect();
    let ps = passages(&answer);
    let headings = ps
        .iter()
        .filter_map(|p| p.heading.as_ref())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let total = ps.len().max(1) as f32;
    let leveled: Vec<(u8, Passage)> = ps
        .iter()
        .enumerate()
        .map(|(i, p)| (level_of(p, i as f32 / total), p.clone()))
        .collect();
    let n = target_nodes(&answer, leveled.len(), headings);
    let mut nodes = fit_count(leveled, n);
    // Basics first; reading order inside a level.
    nodes.sort_by_key(|(l, p)| (*l, p.at));
    let mut parts: Vec<Part> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    let room_for_terms = MAX_NODES.saturating_sub(nodes.len());
    let mut terms_left = room_for_terms;
    for (k, (level, p)) in nodes.iter().enumerate() {
        let label = label_for(p, &labels);
        labels.push(label.clone());
        let id = format!("n{}", k + 1);
        let plain = strip_marks(&p.text);
        let first = sentences(&plain)
            .into_iter()
            .next()
            .unwrap_or(plain.clone());
        let shape = match level {
            0 => Shape::Plate,
            3 => Shape::Cylinder,
            _ => Shape::Block,
        };
        parts.push(Part {
            id: id.clone(),
            label: label.clone(),
            shape,
            level: *level,
            note: clip(&first, 160),
            ..Part::default()
        });
        let mut lit = vec![id.clone()];
        for (t, term) in key_terms(&p.text, 2.min(terms_left))
            .into_iter()
            .enumerate()
        {
            if labels.iter().any(|l| l.eq_ignore_ascii_case(&term)) {
                continue;
            }
            let tid = format!("{id}t{}", t + 1);
            parts.push(Part {
                id: tid.clone(),
                label: clip(&term, 32),
                shape: Shape::Label,
                level: *level,
                note: format!("A key term in {label}"),
                ..Part::default()
            });
            links.push(Link {
                from: id.clone(),
                to: tid.clone(),
                label: String::new(),
            });
            lit.push(tid);
            terms_left -= 1;
        }
        let cites = citations(&p.text);
        let mut caption = clip(&first, 220);
        if !cites.is_empty() && !cites.iter().any(|c| caption.contains(c.as_str())) {
            caption = format!("{caption} {}", cites.join(""));
        }
        steps.push(Step {
            caption,
            parts: lit,
            anchor: Some(chat_anchor(p.heading.clone())),
            level: *level,
            passage: clip(&p.text, 900),
        });
    }
    // What builds on what: each node needs the last node of the level
    // before it; inside a level, nodes follow each other.
    let mains: Vec<(String, u8)> = parts
        .iter()
        .filter(|p| p.shape != Shape::Label)
        .map(|p| (p.id.clone(), p.level))
        .collect();
    for (i, (id, lvl)) in mains.iter().enumerate() {
        if let Some((prev, _)) = mains[..i].iter().rev().find(|(_, l)| l < lvl) {
            links.push(Link {
                from: prev.clone(),
                to: id.clone(),
                label: "builds on".into(),
            });
        } else if i > 0 && mains[i - 1].1 == *lvl {
            links.push(Link {
                from: mains[i - 1].0.clone(),
                to: id.clone(),
                label: String::new(),
            });
        }
    }
    let concept = if concept.trim().is_empty() {
        labels
            .first()
            .cloned()
            .unwrap_or_else(|| "This answer".into())
    } else {
        clip(concept, 80)
    };
    let mut scene = Scene {
        id: String::new(),
        concept,
        summary: format!(
            "This answer from the basics to the advanced parts: {} nodes in {} levels.",
            mains.len(),
            levels_used(&parts)
        ),
        parts,
        links,
        steps,
        topic: None,
        written_by: "fallback".into(),
        created_at: now,
    };
    lay_out(&mut scene);
    scene
}

fn levels_used(parts: &[Part]) -> usize {
    parts
        .iter()
        .map(|p| p.level)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

/// Places nodes on the floor by level: each level a row from the front
/// (basics) to the back (advanced), its nodes spread across the row, a
/// node's key terms floating beside it. Steps are put in level order.
pub fn lay_out(scene: &mut Scene) {
    let mains: Vec<usize> = (0..scene.parts.len())
        .filter(|&i| scene.parts[i].shape != Shape::Label)
        .collect();
    let used: Vec<u8> = {
        let mut v: Vec<u8> = mains.iter().map(|&i| scene.parts[i].level.min(4)).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let rows = used.len().max(1) as f32;
    let row_d = GRID / rows;
    for (r, lvl) in used.iter().enumerate() {
        let here: Vec<usize> = mains
            .iter()
            .copied()
            .filter(|&i| scene.parts[i].level.min(4) == *lvl)
            .collect();
        let cols = here.len().max(1) as f32;
        let col_w = GRID / cols;
        // Advanced at the back (small y is far from the viewer).
        let y = GRID - (r as f32 + 1.) * row_d + row_d * 0.2;
        for (c, &i) in here.iter().enumerate() {
            let p = &mut scene.parts[i];
            let w = (col_w * 0.6).clamp(0.8, 2.6);
            let d = (row_d * 0.55).clamp(0.6, 2.2);
            p.at = [c as f32 * col_w + (col_w - w) / 2., y, 0.];
            p.size = match p.shape {
                Shape::Plate => [w, d, 0.4],
                Shape::Cylinder => [w.min(d) * 1.1, w.min(d) * 1.1, 1.2 + *lvl as f32 * 0.2],
                _ => [w, d, 0.9 + *lvl as f32 * 0.35],
            };
        }
    }
    // Terms float just above and beside their node.
    let ids: Vec<(String, [f32; 3], [f32; 3])> = scene
        .parts
        .iter()
        .map(|p| (p.id.clone(), p.at, p.size))
        .collect();
    for p in scene.parts.iter_mut().filter(|p| p.shape == Shape::Label) {
        let owner = ids
            .iter()
            .filter(|(id, _, _)| p.id.starts_with(id.as_str()) && p.id != *id)
            .max_by_key(|(id, _, _)| id.len());
        if let Some((_, at, size)) = owner {
            let k =
                p.id.chars()
                    .last()
                    .and_then(|c| c.to_digit(10))
                    .unwrap_or(1) as f32;
            p.at = [
                (at[0] + size[0] * 0.5 + (k - 1.5) * 1.2).clamp(0., GRID),
                (at[1] - 0.3).clamp(0., GRID),
                size[2] + 0.8 + 0.5 * (k - 1.),
            ];
            p.size = [0.; 3];
        }
    }
    let level_of_step = |s: &Step| s.level;
    scene.steps.sort_by_key(level_of_step);
}

/// The prompt asking a model to map an answer: (system, user).
pub fn answer_prompt(answer: &str, title: &str) -> (String, String) {
    let answer: String = answer.chars().take(ANSWER_CHARS).collect();
    let ps = passages(&answer);
    let headings = ps.iter().filter(|p| p.heading.is_some()).count();
    let n = target_nodes(&answer, ps.len(), headings);
    let system = format!(
        "You turn an answer into a teaching map that goes from the basics to the advanced parts. Read the WHOLE answer. Write about {n} nodes (at least {MIN_NODES}, at most {max}), each at one level: 0 Foundations (what you must know first), 1 Core idea, 2 Mechanism (how it works), 3 Details and edge cases (limits, trade-offs, comparisons of approaches), 4 Advanced and implications. Cover every major section, approach and technique the answer compares. Each node: a label of at most 5 words, a level, \"explain\" (one or two plain sentences a newcomer understands), \"quote\" (the exact passage of the answer it comes from, keeping any citations like [2]), up to 2 key \"terms\", and \"needs\" (the labels of earlier nodes it builds on). Use only what the answer says. Reply with JSON only: {{\"concept\": \"…\", \"summary\": \"one sentence\", \"nodes\": [{{\"label\": \"…\", \"level\": 0, \"explain\": \"…\", \"quote\": \"…\", \"terms\": [\"…\"], \"needs\": [\"…\"]}}]}}",
        max = MAX_NODES - 4
    );
    let user = format!("Question: {}\n\nAnswer:\n\n{answer}", one_line(title));
    (system, user)
}

/// Reads a model's map of an answer into a scene laid out by level.
/// `None` when fewer than [`MIN_NODES`] nodes came back.
pub fn parse_answer_scene(text: &str, concept: &str, now: i64) -> Option<Scene> {
    let v = json_in(text)?;
    let items = v.get("nodes").and_then(Value::as_array)?;
    let mut parts: Vec<Part> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let mut terms_left = MAX_NODES.saturating_sub(items.len().min(MAX_NODES - 4));
    for item in items.iter().take(MAX_NODES - 4) {
        let Some(label) = field(item, &["label", "name", "title"]) else {
            continue;
        };
        let level = item
            .get("level")
            .and_then(|l| {
                l.as_u64()
                    .or_else(|| l.as_str().and_then(|s| s.trim().parse().ok()))
            })
            .unwrap_or(1)
            .min(4) as u8;
        let mut id = format!("n{}", parts.len() + 1);
        while parts.iter().any(|p| p.id == id) {
            id.push('_');
        }
        let explain = field(item, &["explain", "explanation", "caption", "note"]).unwrap_or(label);
        let quote = field(item, &["quote", "passage", "source"]).unwrap_or_default();
        let shape = match level {
            0 => Shape::Plate,
            3 => Shape::Cylinder,
            _ => Shape::Block,
        };
        parts.push(Part {
            id: id.clone(),
            label: clip(label, 40),
            shape,
            level,
            note: clip(explain, 160),
            ..Part::default()
        });
        let mut lit = vec![id.clone()];
        let terms: Vec<String> = item
            .get("terms")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|t| t.as_str().map(str::to_owned))
            .take(2)
            .collect();
        for (t, term) in terms.into_iter().enumerate() {
            if terms_left == 0 {
                break;
            }
            let tid = format!("{id}t{}", t + 1);
            parts.push(Part {
                id: tid.clone(),
                label: clip(&term, 32),
                shape: Shape::Label,
                level,
                note: format!("A key term in {}", clip(label, 40)),
                ..Part::default()
            });
            links.push(Link {
                from: id.clone(),
                to: tid.clone(),
                label: String::new(),
            });
            lit.push(tid);
            terms_left -= 1;
        }
        let mut caption = clip(explain, 260);
        let cites = citations(quote);
        if !cites.is_empty() && !cites.iter().any(|c| caption.contains(c.as_str())) {
            caption = format!("{caption} {}", cites.join(""));
        }
        steps.push(Step {
            caption,
            parts: lit,
            anchor: Some(chat_anchor(None)),
            level,
            passage: clip(quote, 900),
        });
        for need in item
            .get("needs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let n = slug(need);
            if let Some(from) = parts
                .iter()
                .find(|p| p.shape != Shape::Label && slug(&p.label) == n && p.id != id)
            {
                links.push(Link {
                    from: from.id.clone(),
                    to: id.clone(),
                    label: "builds on".into(),
                });
            }
        }
    }
    if parts.iter().filter(|p| p.shape != Shape::Label).count() < MIN_NODES {
        return None;
    }
    let concept = field(&v, &["concept", "title"])
        .map(str::to_owned)
        .unwrap_or_else(|| one_line(concept));
    let mut scene = Scene {
        id: String::new(),
        concept: clip(&concept, 80),
        summary: field(&v, &["summary"])
            .map(|s| clip(s, 240))
            .unwrap_or_default(),
        parts,
        links,
        steps,
        topic: None,
        written_by: "model".into(),
        created_at: now,
    };
    lay_out(&mut scene);
    Some(scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_answer() -> String {
        let mut s = String::from(
            "Retrieval-augmented generation (RAG) is a way to give a model **documents** it can cite [1].\n\n",
        );
        s.push_str("## In short\nThe key idea is to retrieve first and generate second [2].\n\n");
        s.push_str("## How it works\nFirst, a **retriever** finds passages. Then the generator writes with them in its context. Finally the answer cites them [3].\n\n");
        s.push_str(
            "- Dense retrieval uses `embeddings`.\n- Sparse retrieval uses **BM25** [4].\n\n",
        );
        s.push_str("## Limitations\nHowever, retrieval fails when the query is vague, and long contexts cost more [5].\n\n");
        s.push_str("## Comparing approaches\nAgentic RAG versus single-shot RAG: the agent plans several searches, which helps on multi-hop questions at a higher cost [6]. ");
        s.push_str(
            &"In production teams also cache results and evaluate faithfulness. ".repeat(20),
        );
        s
    }

    #[test]
    fn passages_split_at_headings_paragraphs_and_items() {
        let ps = passages(&long_answer());
        assert!(ps.len() >= 6, "{ps:#?}");
        assert_eq!(ps[1].heading.as_deref(), Some("In short"));
        assert!(ps.iter().any(|p| p.text.starts_with("- Sparse")));
        assert!(passages("").is_empty());
    }

    #[test]
    fn node_count_scales_with_the_answer() {
        let short = answer_scene(
            "RAG retrieves passages, then writes with them. It cites them.",
            "",
            0,
        );
        let long = answer_scene(&long_answer(), "", 0);
        let mains = |s: &Scene| s.parts.iter().filter(|p| p.shape != Shape::Label).count();
        assert!(mains(&short) <= MIN_NODES);
        assert!(mains(&long) > mains(&short));
        assert!(mains(&long) >= 6, "{}", mains(&long));
        assert!(long.parts.len() <= MAX_NODES);
        // One step per main node, each with its passage.
        assert_eq!(long.steps.len(), mains(&long));
        assert!(long.steps.iter().all(|s| !s.passage.is_empty()));
    }

    #[test]
    fn levels_go_from_basics_to_advanced_and_keep_citations() {
        let s = answer_scene(&long_answer(), "RAG", 0);
        let levels: Vec<u8> = s.steps.iter().map(|st| st.level).collect();
        let mut sorted = levels.clone();
        sorted.sort_unstable();
        assert_eq!(levels, sorted, "steps are in level order");
        assert_eq!(levels.first(), Some(&0));
        assert!(levels.iter().any(|&l| l >= 3), "{levels:?}");
        assert!(
            s.steps
                .iter()
                .any(|st| st.caption.contains("[2]") || st.passage.contains("[2]"))
        );
        // Key terms are sub-nodes linked to their node.
        let terms: Vec<&Part> = s.parts.iter().filter(|p| p.shape == Shape::Label).collect();
        assert!(!terms.is_empty());
        assert!(terms.iter().all(|t| s.links.iter().any(|l| l.to == t.id)));
        // Prerequisite links point from a lower level to a higher one.
        let lvl = |id: &str| s.parts.iter().find(|p| p.id == id).map(|p| p.level);
        for l in s.links.iter().filter(|l| l.label == "builds on") {
            assert!(lvl(&l.from) < lvl(&l.to));
        }
    }

    #[test]
    fn layout_puts_basics_in_front_and_everything_on_the_floor() {
        let s = answer_scene(&long_answer(), "", 0);
        for p in &s.parts {
            assert!(p.at.iter().all(|v| (0.0..=GRID).contains(v)), "{p:?}");
        }
        let y_of = |lvl: u8| {
            s.parts
                .iter()
                .filter(|p| p.level == lvl && p.shape != Shape::Label)
                .map(|p| p.at[1])
                .next()
        };
        if let (Some(a), Some(b)) = (y_of(0), y_of(4)) {
            assert!(a > b, "foundations nearer the viewer");
        }
    }

    #[test]
    fn a_model_map_parses_with_levels_terms_and_needs() {
        let json = r#"{"concept":"RAG","summary":"s","nodes":[
            {"label":"Documents","level":0,"explain":"The model reads documents.","quote":"give a model documents [1]","terms":["corpus"]},
            {"label":"Retrieve then generate","level":1,"explain":"Retrieve first.","quote":"retrieve first [2]","needs":["Documents"]},
            {"label":"Retriever","level":2,"explain":"Finds passages.","quote":"a retriever finds passages","needs":["Retrieve then generate"]},
            {"label":"Vague queries","level":3,"explain":"Fails when vague.","quote":"fails when vague [5]"},
            {"label":"Agentic RAG","level":"4","explain":"Plans searches.","quote":"agent plans [6]"}
        ]}"#;
        let s = parse_answer_scene(json, "x", 0).expect("parses");
        assert_eq!(s.steps.len(), 5);
        assert_eq!(s.steps[4].level, 4);
        assert!(s.steps[0].caption.ends_with("[1]"));
        assert!(s.links.iter().any(|l| l.label == "builds on"));
        assert!(s.parts.iter().any(|p| p.label == "corpus"));
        assert!(parse_answer_scene(r#"{"nodes":[{"label":"a"}]}"#, "x", 0).is_none());
    }

    #[test]
    fn citations_and_terms_are_found() {
        assert_eq!(
            citations("a [1] b [2][3] c [1] [x] [1234]"),
            vec!["[1]", "[2]", "[3]"]
        );
        assert_eq!(
            key_terms("use **BM25** and `faiss`, **BM25** again", 5),
            vec!["BM25", "faiss"]
        );
    }
}
