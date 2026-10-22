//! Topics and learning objectives.
//!
//! The model reads an outline of the chunks and groups them into topics,
//! each with a few learning objectives. When there is no model, or its
//! reply cannot be read, [`fallback`] groups chunks by their headings, or
//! by runs of chunks named after their key words.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::material::{Anchor, Chunk};
use crate::text::{capitalise, clip, field, json_in, keywords, list, sentences, slug};

/// Most topics kept from one build.
pub const MAX_TOPICS: usize = 16;

/// A topic to study, with the chunks it is grounded in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topic {
    /// A slug of the title, unique in the library.
    pub id: String,
    pub title: String,
    /// What the user should be able to do: `Explain why …`.
    pub objectives: Vec<String>,
    /// Chunk ids, in reading order.
    pub chunks: Vec<String>,
    /// Where the topic sits in the materials (first chunk of each page or
    /// section), for display.
    pub anchors: Vec<Anchor>,
}

impl Topic {
    /// `graphs.pdf pp. 2–5` style summary of where the topic is.
    pub fn where_label(&self) -> String {
        let Some(first) = self.anchors.first() else {
            return String::new();
        };
        let pages: Vec<u32> = self
            .anchors
            .iter()
            .filter(|a| a.material == first.material)
            .filter_map(|a| a.page)
            .collect();
        let others = self
            .anchors
            .iter()
            .map(|a| &a.material)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            - 1;
        let mut s = first.file.clone();
        match (pages.iter().min(), pages.iter().max()) {
            (Some(a), Some(b)) if a == b => s.push_str(&format!(" p. {a}")),
            (Some(a), Some(b)) => s.push_str(&format!(" pp. {a}–{b}")),
            _ => {}
        }
        if others > 0 {
            s.push_str(&format!(" + {others} more"));
        }
        s
    }
}

/// The prompt asking a model for topics: (system, user).
pub fn topics_prompt(chunks: &[Chunk], max: usize) -> (String, String) {
    let system = format!(
        "You organise a student's study materials into topics. Group the numbered chunks into at most {max} topics in reading order. Every chunk belongs to exactly one topic. Give each topic a short title (2 to 6 words) and 2 to 4 learning objectives that start with a verb (Explain, Compare, Apply, Define, Derive) and that the chunks can actually support. Use only the materials. Reply with JSON only: {{\"topics\": [{{\"title\": \"…\", \"objectives\": [\"…\"], \"chunks\": [\"c1\", \"c2\"]}}]}}"
    );
    let mut user = String::from("Chunks:\n");
    let budget = 24_000usize;
    let per = (budget / chunks.len().max(1)).clamp(80, 320);
    for (i, c) in chunks.iter().enumerate() {
        user.push_str(&format!(
            "c{} ({}): {}\n",
            i + 1,
            c.anchor.label(),
            clip(&c.text, per)
        ));
    }
    (system, user)
}

/// Reads a model's topics. Unknown chunk numbers are dropped; chunks no
/// topic named join the topic before them. `None` when nothing usable came
/// back.
pub fn parse_topics(text: &str, chunks: &[Chunk], max: usize) -> Option<Vec<Topic>> {
    let v = json_in(text)?;
    let items = match &v {
        Value::Array(a) => a.clone(),
        _ => v
            .get("topics")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    let mut owner: Vec<Option<usize>> = vec![None; chunks.len()];
    let mut drafts: Vec<(String, Vec<String>)> = Vec::new();
    for item in items.iter().take(max) {
        let Some(title) = field(item, &["title", "name", "topic"]) else {
            continue;
        };
        let objectives = list(item, &["objectives", "learning_objectives", "goals"]);
        let mut any = false;
        for r in list(item, &["chunks", "chunk_ids", "sources"]) {
            let n = r
                .trim()
                .trim_start_matches(['c', 'C'])
                .trim()
                .parse::<usize>()
                .ok();
            if let Some(n) = n.filter(|n| (1..=chunks.len()).contains(n))
                && owner[n - 1].is_none()
            {
                owner[n - 1] = Some(drafts.len());
                any = true;
            }
        }
        if any {
            drafts.push((clip(title, 80), objectives));
        }
    }
    if drafts.is_empty() {
        return None;
    }
    // Unclaimed chunks follow the topic before them (or the first topic).
    let mut last = 0;
    for o in owner.iter_mut() {
        match o {
            Some(t) => last = *t,
            None => *o = Some(last),
        }
    }
    let mut topics: Vec<Topic> = Vec::new();
    for (t, (title, mut objectives)) in drafts.into_iter().enumerate() {
        let mine: Vec<&Chunk> = chunks
            .iter()
            .zip(&owner)
            .filter(|(_, o)| **o == Some(t))
            .map(|(c, _)| c)
            .collect();
        objectives.truncate(5);
        if objectives.is_empty() {
            objectives = fallback_objectives(&title, &mine);
        }
        topics.push(make(&title, objectives, &mine));
    }
    dedupe_ids(&mut topics);
    Some(topics)
}

/// Topics without a model: one per heading when the materials have
/// headings, otherwise runs of about four chunks named by key words.
pub fn fallback(chunks: &[Chunk], max: usize) -> Vec<Topic> {
    if chunks.is_empty() {
        return Vec::new();
    }
    let mut groups: Vec<(String, Vec<&Chunk>)> = Vec::new();
    let sections = chunks
        .iter()
        .filter_map(|c| c.anchor.section.as_deref())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    if sections >= 2 {
        for c in chunks {
            let title = c
                .anchor
                .section
                .clone()
                .unwrap_or_else(|| format!("{} (opening)", c.anchor.file));
            match groups.last_mut() {
                Some((t, g)) if *t == title => g.push(c),
                _ => groups.push((title, vec![c])),
            }
        }
    } else {
        // Per material, runs of chunks.
        let mut by_material: Vec<(String, Vec<&Chunk>)> = Vec::new();
        for c in chunks {
            match by_material.last_mut() {
                Some((m, g)) if *m == c.anchor.material => g.push(c),
                _ => by_material.push((c.anchor.material.clone(), vec![c])),
            }
        }
        for (_, cs) in by_material {
            let size = cs.len().div_ceil(cs.len().div_ceil(4).max(1));
            for part in cs.chunks(size.max(1)) {
                let joined: String = part
                    .iter()
                    .map(|c| c.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let kw = keywords(&joined, 3);
                let title = if kw.is_empty() {
                    part[0].anchor.file.clone()
                } else {
                    capitalise(&kw.join(", "))
                };
                groups.push((title, part.to_vec()));
            }
        }
    }
    // Too many: merge the smallest neighbours until it fits.
    while groups.len() > max {
        let i = (0..groups.len() - 1)
            .min_by_key(|&i| groups[i].1.len() + groups[i + 1].1.len())
            .unwrap_or(0);
        let (_, next) = groups.remove(i + 1);
        groups[i].1.extend(next);
    }
    let mut topics: Vec<Topic> = groups
        .into_iter()
        .map(|(title, cs)| {
            let obj = fallback_objectives(&title, &cs);
            make(&title, obj, &cs)
        })
        .collect();
    dedupe_ids(&mut topics);
    topics
}

/// Objectives without a model: explain the topic, define what its
/// sentences define, and recall its key terms.
fn fallback_objectives(title: &str, chunks: &[&Chunk]) -> Vec<String> {
    let mut out = vec![format!("Explain {} in your own words", lower_title(title))];
    let joined: String = chunks
        .iter()
        .flat_map(|c| c.text.lines())
        .filter(|l| crate::material::heading(l).is_none())
        .collect::<Vec<_>>()
        .join("\n");
    for s in sentences(&joined) {
        if out.len() >= 3 {
            break;
        }
        if let Some((term, _)) = definition(&s) {
            let o = format!("Define {term}");
            if !out.contains(&o) {
                out.push(o);
            }
        }
    }
    let kw = keywords(&joined, 3);
    if !kw.is_empty() && out.len() < 4 {
        out.push(format!("Recall the key terms: {}", kw.join(", ")));
    }
    out
}

fn lower_title(t: &str) -> String {
    let mut c = t.chars();
    match (c.next(), c.clone().next()) {
        // Keep acronyms and names: `TCP`, `Dijkstra`.
        (Some(f), Some(s)) if f.is_uppercase() && s.is_lowercase() => {
            f.to_lowercase().collect::<String>() + c.as_str()
        }
        _ => t.to_owned(),
    }
}

/// A sentence of the form `X is/are (a|an|the) Y`: (X, the whole sentence).
pub fn definition(s: &str) -> Option<(String, String)> {
    for verb in [
        " is a ",
        " is an ",
        " is the ",
        " are ",
        " refers to ",
        " means ",
    ] {
        if let Some((head, _)) = s.split_once(verb) {
            let head = head.trim().trim_start_matches(['-', '*', '•']).trim();
            let n = head.split_whitespace().count();
            if (1..=5).contains(&n)
                && head.chars().next().is_some_and(char::is_alphabetic)
                && !head.contains(',')
            {
                let head = head
                    .strip_prefix("The ")
                    .or_else(|| head.strip_prefix("A "))
                    .or_else(|| head.strip_prefix("An "))
                    .unwrap_or(head);
                return Some((head.to_owned(), s.to_owned()));
            }
        }
    }
    None
}

fn make(title: &str, objectives: Vec<String>, chunks: &[&Chunk]) -> Topic {
    let mut anchors: Vec<Anchor> = Vec::new();
    for c in chunks {
        let a = &c.anchor;
        if !anchors
            .iter()
            .any(|x| x.material == a.material && x.page == a.page && x.section == a.section)
        {
            anchors.push(a.clone());
        }
    }
    Topic {
        id: slug(title),
        title: title.to_owned(),
        objectives,
        chunks: chunks.iter().map(|c| c.id.clone()).collect(),
        anchors,
    }
}

fn dedupe_ids(topics: &mut [Topic]) {
    let mut seen = std::collections::BTreeSet::new();
    for t in topics.iter_mut() {
        let base = t.id.clone();
        let mut n = 2;
        while !seen.insert(t.id.clone()) {
            t.id = format!("{base}-{n}");
            n += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::chunk_text;

    fn sample() -> Vec<Chunk> {
        let md = "# Graphs\nA graph is a set of vertices joined by edges. Graphs model networks.\n# Shortest paths\nDijkstra's algorithm is a greedy method for shortest paths with non-negative weights.\n# Spanning trees\nA minimum spanning tree is the cheapest set of edges connecting every vertex.";
        chunk_text("m1", "algo.md", md)
    }

    #[test]
    fn fallback_uses_headings() {
        let t = fallback(&sample(), 16);
        assert_eq!(t.len(), 3);
        assert_eq!(t[1].title, "Shortest paths");
        assert_eq!(t[1].id, "shortest-paths");
        assert!(
            t[0].objectives.iter().any(|o| o == "Define graph"),
            "{:?}",
            t[0].objectives
        );
        assert_eq!(t[2].chunks, vec!["m1#3"]);
    }

    #[test]
    fn fallback_merges_down_to_the_cap() {
        let t = fallback(&sample(), 2);
        assert_eq!(t.len(), 2);
        assert_eq!(t.iter().map(|x| x.chunks.len()).sum::<usize>(), 3);
    }

    #[test]
    fn fallback_without_headings_names_runs_by_keywords() {
        let text = (1..=8)
            .map(|i| format!("[page {i}]\nEntropy measures uncertainty in a distribution and entropy drives compression bounds in coding."))
            .collect::<Vec<_>>()
            .join("\n");
        let cs = chunk_text("m1", "info.pdf", &text);
        let t = fallback(&cs, 16);
        assert_eq!(t.len(), 2);
        assert!(t[0].title.starts_with("Entropy"), "{}", t[0].title);
        assert_eq!(t[0].where_label(), "info.pdf pp. 1–4");
    }

    #[test]
    fn parses_model_topics_leniently() {
        let cs = sample();
        let reply = "Here you go:\n```json\n{\"topics\": [{\"name\": \"Graph basics\", \"objectives\": [\"Define a graph\"], \"chunks\": [\"c1\"]}, {\"title\": \"Optimisation on graphs\", \"chunks\": [2, \"C9\"]}]}\n```";
        let t = parse_topics(reply, &cs, 16).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].objectives, vec!["Define a graph"]);
        // Chunk 3 was not named: it joins the topic before it.
        assert_eq!(t[1].chunks, vec!["m1#2", "m1#3"]);
        assert!(!t[1].objectives.is_empty());
        assert!(parse_topics("no idea", &cs, 16).is_none());
        assert!(
            parse_topics(
                "{\"topics\": [{\"title\": \"x\", \"chunks\": [\"c42\"]}]}",
                &cs,
                16
            )
            .is_none()
        );
    }

    #[test]
    fn prompt_lists_every_chunk() {
        let (_, user) = topics_prompt(&sample(), 8);
        assert!(user.contains("c3 (algo.md · Spanning trees)"));
    }
}
