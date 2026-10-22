//! The study library: materials, topics, mastery, and the quiz and viva
//! history, as one serialisable value. `tern-agent` saves it in the data
//! directory; material texts and full quiz and viva records live in their
//! own files beside it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::mastery::Mastery;
use crate::material::{Chunk, Material, Source};
use crate::quiz::Difficulty;
use crate::text::{normalise, overlap};
use crate::topics::Topic;

pub const LIBRARY_VERSION: u32 = 1;

/// A quiz in the history list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuizSummary {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    pub taken_at: i64,
    pub score: f32,
    pub total: usize,
}

/// A viva in the history list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VivaSummary {
    pub id: String,
    pub title: String,
    pub taken_at: i64,
    /// Out of 100.
    pub overall: u32,
    pub questions: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StudyLibrary {
    pub version: u32,
    pub materials: Vec<Material>,
    pub topics: Vec<Topic>,
    /// By topic id.
    #[serde(default)]
    pub mastery: BTreeMap<String, Mastery>,
    #[serde(default)]
    pub quizzes: Vec<QuizSummary>,
    #[serde(default)]
    pub vivas: Vec<VivaSummary>,
    /// The topics were written by the model (`model`) or by headings and
    /// key words (`fallback`).
    #[serde(default)]
    pub topics_by: String,
    /// The topics are older than the materials (one was added or removed).
    #[serde(default)]
    pub topics_stale: bool,
    /// The next material number.
    #[serde(default)]
    pub next_material: u32,
}

impl StudyLibrary {
    /// A fresh material id: `m1`, `m2`, …
    pub fn new_material_id(&mut self) -> String {
        self.next_material = self.next_material.max(self.materials.len() as u32) + 1;
        format!("m{}", self.next_material)
    }

    /// A material with the same origin, if one was added before.
    pub fn material_from(&self, origin: &str) -> Option<&Material> {
        self.materials.iter().find(|m| m.origin == origin)
    }

    pub fn add_material(&mut self, m: Material) {
        self.materials.retain(|x| x.id != m.id);
        self.materials.push(m);
        self.topics_stale = true;
    }

    /// Removes a material by id, number (`2`), or name. Returns it.
    pub fn remove_material(&mut self, which: &str) -> Result<Material, String> {
        let i = self.material_index(which)?;
        self.topics_stale = true;
        Ok(self.materials.remove(i))
    }

    pub fn material_index(&self, which: &str) -> Result<usize, String> {
        let w = which.trim();
        if let Ok(n) = w.parse::<usize>()
            && (1..=self.materials.len()).contains(&n)
        {
            return Ok(n - 1);
        }
        self.materials
            .iter()
            .position(|m| m.id == w || m.name == w)
            .or_else(|| {
                let lw = w.to_lowercase();
                let hits: Vec<usize> = self
                    .materials
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.name.to_lowercase().contains(&lw))
                    .map(|(i, _)| i)
                    .collect();
                (hits.len() == 1).then(|| hits[0])
            })
            .ok_or_else(|| format!("No material matches \"{w}\". `tern study list` shows them."))
    }

    /// Replaces the topics, keeping mastery for topics that kept their id.
    pub fn set_topics(&mut self, topics: Vec<Topic>, by: &str) {
        self.topics = topics;
        self.topics_by = by.to_owned();
        self.topics_stale = false;
    }

    /// A topic by id, number (`3`), or title (exact, then a unique part).
    pub fn topic(&self, which: &str) -> Option<&Topic> {
        let w = which.trim();
        if let Ok(n) = w.parse::<usize>() {
            return self.topics.get(n.checked_sub(1)?);
        }
        let nw = normalise(w);
        self.topics
            .iter()
            .find(|t| t.id == w || normalise(&t.title) == nw)
            .or_else(|| {
                let hits: Vec<&Topic> = self
                    .topics
                    .iter()
                    .filter(|t| normalise(&t.title).contains(&nw) || t.id.contains(w))
                    .collect();
                (hits.len() == 1).then(|| hits[0])
            })
            .or_else(|| {
                // Best word overlap, when it is clear.
                self.topics
                    .iter()
                    .map(|t| (overlap(w, &t.title), t))
                    .filter(|(o, _)| *o >= 0.5)
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, t)| t)
            })
    }

    pub fn mastery_of(&self, topic: &str) -> Mastery {
        self.mastery.get(topic).cloned().unwrap_or_default()
    }

    /// Applies a graded answer to a topic's mastery.
    pub fn record(&mut self, topic: &str, credit: f32, difficulty: Difficulty, now: i64) {
        self.mastery
            .entry(topic.to_owned())
            .or_default()
            .record(credit, difficulty, now);
    }

    pub fn touch(&mut self, topic: &str, now: i64) {
        self.mastery.entry(topic.to_owned()).or_default().touch(now);
    }

    /// The topic the user most needs: due for review first, then the
    /// lowest rating, then never studied.
    pub fn weakest(&self, now: i64) -> Option<&Topic> {
        self.topics.iter().min_by(|a, b| {
            let (ma, mb) = (self.mastery_of(&a.id), self.mastery_of(&b.id));
            mb.is_due(now)
                .cmp(&ma.is_due(now))
                .then(ma.rating.total_cmp(&mb.rating))
        })
    }

    /// Average rating over every topic (0 to 1).
    pub fn overall(&self) -> f32 {
        if self.topics.is_empty() {
            return 0.0;
        }
        self.topics
            .iter()
            .map(|t| self.mastery_of(&t.id).rating)
            .sum::<f32>()
            / self.topics.len() as f32
    }
}

/// Picks passages for a prompt: the topic's chunks (or all chunks when
/// `topic` is `None`), the ones that best match `query` first when there
/// are too many, kept in reading order, up to `max_chars`. Numbered from 1.
pub fn select_sources<'a>(
    chunks: impl IntoIterator<Item = &'a Chunk>,
    topic: Option<&Topic>,
    query: &str,
    max_chars: usize,
) -> Vec<Source> {
    let pool: Vec<&Chunk> = chunks
        .into_iter()
        .filter(|c| topic.is_none_or(|t| t.chunks.contains(&c.id)))
        .collect();
    let mut order: Vec<usize> = (0..pool.len()).collect();
    let total: usize = pool.iter().map(|c| c.text.len()).sum();
    if total > max_chars && !query.trim().is_empty() {
        // Best matches first; ties keep reading order.
        let scores: Vec<f32> = pool.iter().map(|c| overlap(query, &c.text)).collect();
        order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]).then(a.cmp(b)));
    } else if total > max_chars {
        // No query: spread over the whole topic.
        let step = (total / max_chars.max(1)).max(1);
        let mut spread: Vec<usize> = (0..pool.len()).step_by(step + 1).collect();
        let rest: Vec<usize> = (0..pool.len()).filter(|i| !spread.contains(i)).collect();
        spread.extend(rest);
        order = spread;
    }
    let mut picked: Vec<usize> = Vec::new();
    let mut used = 0usize;
    for i in order {
        let len = pool[i].text.len();
        if used + len > max_chars && !picked.is_empty() {
            continue;
        }
        used += len;
        picked.push(i);
    }
    picked.sort();
    picked
        .into_iter()
        .enumerate()
        .map(|(n, i)| Source {
            n: n + 1,
            chunk: pool[i].id.clone(),
            anchor: pool[i].anchor.clone(),
            text: if pool[i].text.len() > max_chars {
                pool[i].text.chars().take(max_chars).collect()
            } else {
                pool[i].text.clone()
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{MaterialKind, build};
    use crate::topics::fallback;

    fn lib() -> (StudyLibrary, Vec<Chunk>) {
        let mut lib = StudyLibrary::default();
        let id = lib.new_material_id();
        let md = "# Graphs\nA graph is a set of vertices joined by edges in a network.\n# Shortest paths\nDijkstra's algorithm finds shortest paths with non-negative weights.\n# Spanning trees\nA minimum spanning tree connects every vertex at least cost.";
        let (m, t) = build(&id, "algo.md", MaterialKind::Note, "/a/algo.md", md, 0);
        lib.add_material(m);
        assert!(lib.topics_stale);
        let topics = fallback(&t.chunks, 16);
        lib.set_topics(topics, "fallback");
        (lib, t.chunks)
    }

    #[test]
    fn finds_topics_and_materials_loosely() {
        let (lib, _) = lib();
        assert_eq!(lib.topic("2").unwrap().title, "Shortest paths");
        assert_eq!(lib.topic("spanning").unwrap().id, "spanning-trees");
        assert_eq!(lib.topic("Shortest Paths").unwrap().id, "shortest-paths");
        assert!(lib.topic("quantum").is_none());
        assert_eq!(lib.material_index("algo").unwrap(), 0);
        assert!(lib.material_index("nope").is_err());
        assert_eq!(lib.material_from("/a/algo.md").unwrap().id, "m1");
    }

    #[test]
    fn mastery_records_and_weakest_topic() {
        let (mut lib, _) = lib();
        lib.record("graphs", 1.0, Difficulty::Medium, 0);
        lib.record("shortest-paths", 1.0, Difficulty::Hard, 0);
        assert_eq!(lib.weakest(10).unwrap().id, "spanning-trees");
        // Due topics come first.
        assert_eq!(lib.weakest(10 * 86_400).unwrap().id, "graphs");
        assert!(lib.overall() > 0.0);
    }

    #[test]
    fn select_sources_by_topic_query_and_budget() {
        let (lib, chunks) = lib();
        let t = lib.topic("graphs").unwrap();
        let s = select_sources(&chunks, Some(t), "", 10_000);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].n, 1);
        let all = select_sources(&chunks, None, "spanning tree cost", 120);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].chunk, "m1#3");
        let all = select_sources(&chunks, None, "", 100_000);
        assert_eq!(all.iter().map(|s| s.n).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn library_round_trips() {
        let (mut lib, _) = lib();
        lib.record("graphs", 0.5, Difficulty::Easy, 5);
        lib.version = LIBRARY_VERSION;
        let json = serde_json::to_string(&lib).unwrap();
        let back: StudyLibrary = serde_json::from_str(&json).unwrap();
        assert_eq!(back, lib);
        let mut l2 = lib.clone();
        assert_eq!(l2.remove_material("1").unwrap().id, "m1");
        assert!(l2.topics_stale);
        assert_eq!(l2.new_material_id(), "m2");
    }
}
