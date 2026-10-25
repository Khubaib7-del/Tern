//! The Graph tab's data, GPUI-free so it is unit tested: what the active
//! chat's run actually touched, as nodes and edges around the question.
//!
//! Before this, the Graph tab drew one fixed six-box template for every
//! chat (only the shading of the phase boxes changed), so two chats with
//! different sources looked the same. Now the figure is built from:
//!
//! - **sources**: pages read (solid) and search results seen (faint), one
//!   node per URL, linked to the question or to the deep-research worker
//!   that found or read them;
//! - **workers**: a deep run's sub-questions, between the question and
//!   their pages;
//! - **claims**: the answer's checked claims, coloured by verdict and
//!   linked to the sources they cite or were checked against;
//! - **entities**: key terms of the question and answer (bold spans,
//!   acronyms, names), linked to the claims and sources that mention them,
//!   so a quick answer with no sources still maps to something of its own.
//!
//! The layout is a function of this data: every node's angle comes from a
//! stable hash of what it is (its URL, its text), so different chats give
//! different shapes and the same chat gives the same shape every time.
//! [`graph_key`] says when the graph must be rebuilt: when the chat, the
//! run (a new step, a new check, the end) or the answer changes.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use tern_agent::{Run, StepKind, Verdict, WorkerStatus};

/// At most this many of each kind of node, so the figure stays legible.
pub const MAX_SOURCES: usize = 14;
pub const MAX_CLAIMS: usize = 10;
pub const MAX_ENTITIES: usize = 8;
pub const MAX_WORKERS: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// A page: `read` when Tern read it, else a search result it saw.
    Source {
        url: String,
        site: String,
        title: String,
        read: bool,
    },
    /// A deep-research worker (its sub-question).
    Worker {
        n: usize,
        question: String,
        running: bool,
    },
    /// A checked claim from the answer.
    Claim { text: String, verdict: Verdict },
    /// A key term from the question or answer.
    Entity { name: String, mentions: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub kind: NodeKind,
    /// Stable identity (URL, text) the layout hashes.
    pub key: String,
    /// Added by the latest step of a run that is still going.
    pub fresh: bool,
}

/// What an edge joins; `from: None` is the question at the centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: Option<usize>,
    pub to: usize,
    /// Dotted: a weak link (a result only seen, a claim with no source).
    pub weak: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// The question's first words, for the readout on the hub.
    pub question: String,
}

impl ChatGraph {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn count(&self, f: impl Fn(&NodeKind) -> bool) -> usize {
        self.nodes.iter().filter(|n| f(&n.kind)).count()
    }

    /// Builds the graph of one run, with the question that started it and
    /// the answer it gave (the draft while it streams).
    pub fn build(run: &Run, question: &str, answer: &str) -> Self {
        let mut g = Self {
            question: clip(question.trim(), 60),
            ..Self::default()
        };
        let live = run.end.is_none() && !run.steps.is_empty();
        let last_seq = run.steps.last().map(|s| s.seq);
        // url -> node index
        let mut by_url: HashMap<String, usize> = HashMap::new();
        let mut worker_node: HashMap<usize, usize> = HashMap::new();

        // Workers first, so their sources can hang off them.
        if let Some(d) = &run.deep {
            for w in d.workers.iter().take(MAX_WORKERS) {
                let i = g.nodes.len();
                g.nodes.push(Node {
                    kind: NodeKind::Worker {
                        n: w.n,
                        question: w.question.clone(),
                        running: w.status == WorkerStatus::Running,
                    },
                    key: format!("worker:{}:{}", w.n, w.question),
                    fresh: false,
                });
                g.edges.push(Edge {
                    from: None,
                    to: i,
                    weak: false,
                });
                worker_node.insert(w.n, i);
            }
        }

        let mut add_source = |g: &mut ChatGraph,
                              url: &str,
                              site: &str,
                              title: &str,
                              read: bool,
                              worker: Option<usize>,
                              fresh: bool| {
            let url = url.trim();
            if url.is_empty() {
                return;
            }
            if let Some(&i) = by_url.get(url) {
                if let NodeKind::Source { read: r, .. } = &mut g.nodes[i].kind
                    && read
                    && !*r
                {
                    *r = true;
                    for e in g.edges.iter_mut().filter(|e| e.to == i) {
                        e.weak = false;
                    }
                }
                g.nodes[i].fresh |= fresh;
                return;
            }
            if g.count(|k| matches!(k, NodeKind::Source { .. })) >= MAX_SOURCES {
                return;
            }
            let i = g.nodes.len();
            g.nodes.push(Node {
                kind: NodeKind::Source {
                    url: url.to_owned(),
                    site: if site.is_empty() {
                        site_of(url)
                    } else {
                        site.to_owned()
                    },
                    title: title.trim().to_owned(),
                    read,
                },
                key: url.to_owned(),
                fresh,
            });
            g.edges.push(Edge {
                from: worker.and_then(|n| worker_node.get(&n).copied()),
                to: i,
                weak: !read,
            });
            by_url.insert(url.to_owned(), i);
        };

        // Reads before results, so a page both seen and read counts once,
        // as read, and the cap keeps the pages that mattered.
        for s in &run.steps {
            if s.kind == StepKind::Read
                && s.ok
                && let Some(pg) = &s.page
            {
                let worker = worker_of(&s.payload);
                let fresh = live && Some(s.seq) == last_seq;
                add_source(&mut g, &pg.url, &pg.site, &pg.title, true, worker, fresh);
            }
        }
        if let Some(d) = &run.deep {
            for w in &d.workers {
                for pg in &w.pages {
                    add_source(&mut g, &pg.url, &pg.site, &pg.title, true, Some(w.n), false);
                }
            }
        }
        for s in &run.steps {
            if s.kind != StepKind::Search || !s.ok {
                continue;
            }
            let worker = worker_of(&s.payload);
            let fresh = live && Some(s.seq) == last_seq;
            let hits = s
                .payload
                .pointer("/meta/hits")
                .or_else(|| s.payload.get("hits"))
                .and_then(|h| h.as_array());
            for h in hits.into_iter().flatten() {
                let url = h.get("url").and_then(|v| v.as_str()).unwrap_or_default();
                let site = h.get("site").and_then(|v| v.as_str()).unwrap_or_default();
                let title = h.get("title").and_then(|v| v.as_str()).unwrap_or_default();
                add_source(&mut g, url, site, title, false, worker, fresh);
            }
        }

        // Claims, linked to the sources they cite.
        let sources_in_order: Vec<usize> = g
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| matches!(n.kind, NodeKind::Source { read: true, .. }))
            .map(|(i, _)| i)
            .collect();
        for c in run.checks.iter().take(MAX_CLAIMS) {
            let i = g.nodes.len();
            g.nodes.push(Node {
                kind: NodeKind::Claim {
                    text: c.text.clone(),
                    verdict: c.verdict,
                },
                key: format!("claim:{}", c.text),
                fresh: false,
            });
            let mut linked = false;
            if let Some(ev) = &c.evidence
                && let Some(&s) = by_url.get(ev.url.trim())
            {
                g.edges.push(Edge {
                    from: Some(s),
                    to: i,
                    weak: false,
                });
                linked = true;
            }
            for &n in &c.citations {
                // Citations are 1-based numbers into the pages read.
                if let Some(&s) = n.checked_sub(1).and_then(|k| sources_in_order.get(k))
                    && !g.edges.iter().any(|e| e.from == Some(s) && e.to == i)
                {
                    g.edges.push(Edge {
                        from: Some(s),
                        to: i,
                        weak: false,
                    });
                    linked = true;
                }
            }
            if !linked {
                g.edges.push(Edge {
                    from: None,
                    to: i,
                    weak: true,
                });
            }
        }

        // Entities from the question and answer.
        let text = format!("{question}\n{answer}");
        for (name, mentions) in entities(&text).into_iter().take(MAX_ENTITIES) {
            let i = g.nodes.len();
            let lower = name.to_lowercase();
            g.nodes.push(Node {
                kind: NodeKind::Entity {
                    name: name.clone(),
                    mentions,
                },
                key: format!("entity:{lower}"),
                fresh: false,
            });
            let mut linked = false;
            for j in 0..i {
                let hay = match &g.nodes[j].kind {
                    NodeKind::Claim { text, .. } => text.to_lowercase(),
                    NodeKind::Source { title, .. } => title.to_lowercase(),
                    NodeKind::Worker { question, .. } => question.to_lowercase(),
                    NodeKind::Entity { .. } => continue,
                };
                if contains_word(&hay, &lower) {
                    g.edges.push(Edge {
                        from: Some(j),
                        to: i,
                        weak: true,
                    });
                    linked = true;
                }
            }
            if !linked {
                g.edges.push(Edge {
                    from: None,
                    to: i,
                    weak: true,
                });
            }
        }
        g
    }

    /// The readout for a node: what it is, and its detail.
    pub fn readout(&self, i: usize) -> Option<(String, String)> {
        let n = self.nodes.get(i)?;
        Some(match &n.kind {
            NodeKind::Source {
                site, title, read, ..
            } => (
                format!("{} · {site}", if *read { "read" } else { "seen" }),
                clip(if title.is_empty() { site } else { title }, 44),
            ),
            NodeKind::Worker {
                n,
                question,
                running,
                ..
            } => (
                format!("worker {n}{}", if *running { " · running" } else { "" }),
                clip(question, 44),
            ),
            NodeKind::Claim { text, verdict } => (
                format!("claim · {}", verdict_label(*verdict)),
                clip(text, 44),
            ),
            NodeKind::Entity { name, mentions } => (format!("term · {mentions}×"), clip(name, 44)),
        })
    }

    /// The readout with nothing pointed at.
    pub fn summary(&self) -> String {
        let read = self.count(|k| matches!(k, NodeKind::Source { read: true, .. }));
        let seen = self.count(|k| matches!(k, NodeKind::Source { read: false, .. }));
        let claims = self.count(|k| matches!(k, NodeKind::Claim { .. }));
        let terms = self.count(|k| matches!(k, NodeKind::Entity { .. }));
        let mut parts = vec![];
        if read > 0 {
            parts.push(format!("{read} read"));
        }
        if seen > 0 {
            parts.push(format!("{seen} seen"));
        }
        if claims > 0 {
            parts.push(format!("{claims} claims"));
        }
        if terms > 0 {
            parts.push(format!("{terms} terms"));
        }
        if parts.is_empty() {
            "nothing mapped yet".to_owned()
        } else {
            parts.join(" · ")
        }
    }
}

fn verdict_label(v: Verdict) -> &'static str {
    match v {
        Verdict::Supported => "supported",
        Verdict::Partly => "partly",
        Verdict::NotSupported => "not supported",
        Verdict::NoSource => "no source",
    }
}

fn worker_of(p: &serde_json::Value) -> Option<usize> {
    p.get("worker")
        .or_else(|| p.pointer("/meta/worker"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
}

fn site_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.trim_start_matches("www.").to_owned()
}

fn clip(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= n {
        s
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>().trim_end())
    }
}

fn contains_word(hay: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    hay.match_indices(word).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + word.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// Capitalised words that start sentences or are too common to be terms.
const COMMON: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "can", "do", "does", "each", "example",
    "for", "from", "here", "how", "i", "if", "in", "is", "it", "its", "key", "let", "may", "more",
    "most", "no", "not", "note", "of", "on", "one", "or", "so", "some", "such", "summary", "that",
    "the", "their", "then", "there", "these", "they", "this", "those", "to", "use", "uses", "we",
    "what", "when", "where", "which", "while", "who", "why", "with", "would", "yes", "you", "your",
    "also", "both", "all", "any", "has", "have", "than", "into", "over", "about", "would", "like",
];

/// Key terms of a text, most mentioned first: bold spans (`**SIMD**`),
/// acronyms and mixed-case names (`GPU`, `NVLink`), and runs of
/// capitalised words that do not start a sentence (`Pixel Shader`).
pub fn entities(text: &str) -> Vec<(String, usize)> {
    // name lowercased -> (display, score, mentions, first position)
    let mut found: HashMap<String, (String, usize, usize, usize)> = HashMap::new();
    let mut pos = 0usize;
    let mut note = |name: &str, weight: usize, found: &mut HashMap<_, _>, pos: &mut usize| {
        let name = name
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '+' && c != '#')
            .trim();
        if name.chars().count() < 2 || name.chars().count() > 40 || name.split(' ').count() > 4 {
            return;
        }
        if COMMON.contains(&name.to_lowercase().as_str()) {
            return;
        }
        if name
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
        {
            return;
        }
        *pos += 1;
        let e = found
            .entry(name.to_lowercase())
            .or_insert_with(|| (name.to_owned(), 0, 0, *pos));
        e.1 += weight;
        e.2 += 1;
    };
    // Bold spans.
    let mut rest = text;
    while let Some(a) = rest.find("**") {
        let after = &rest[a + 2..];
        let Some(b) = after.find("**") else { break };
        note(&after[..b], 3, &mut found, &mut pos);
        rest = &after[b + 2..];
    }
    // Words, by sentence.
    for line in text.lines() {
        let line = line.trim_start_matches(|c: char| {
            c == '#'
                || c == '-'
                || c == '*'
                || c == '>'
                || c.is_ascii_digit()
                || c == '.'
                || c == ' '
        });
        for sentence in line.split(['.', '!', '?', ':', ';', '(', ')', '[', ']', '|', '"']) {
            let words: Vec<&str> = sentence
                .split(|c: char| c.is_whitespace() || c == ',' || c == '/')
                .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '+'))
                .filter(|w| !w.is_empty())
                .collect();
            let mut run: Vec<&str> = vec![];
            let flush = |run: &mut Vec<&str>, found: &mut HashMap<_, _>, pos: &mut usize| {
                if !run.is_empty() {
                    let name = run.join(" ");
                    note(&name, 1, found, pos);
                    run.clear();
                }
            };
            for (k, w) in words.iter().enumerate() {
                let upper = w.chars().filter(|c| c.is_uppercase()).count();
                let acronym = upper >= 2 && w.chars().count() <= 12;
                let capital = w.chars().next().is_some_and(char::is_uppercase);
                if acronym {
                    flush(&mut run, &mut found, &mut pos);
                    note(w, 2, &mut found, &mut pos);
                } else if capital && (k > 0 || !run.is_empty()) {
                    run.push(w);
                } else if capital && k == 0 && !COMMON.contains(&w.to_lowercase().as_str()) {
                    // A capital at the start of a sentence counts only if
                    // the next word is capitalised too (a name).
                    if words
                        .get(1)
                        .is_some_and(|n| n.chars().next().is_some_and(char::is_uppercase))
                    {
                        run.push(w);
                    }
                } else {
                    flush(&mut run, &mut found, &mut pos);
                }
            }
            flush(&mut run, &mut found, &mut pos);
        }
    }
    let mut out: Vec<(String, usize, usize, usize)> = found
        .into_values()
        // A plain capitalised word seen once is noise; keep it when it
        // is bold, an acronym, or repeated.
        .filter(|(_, score, mentions, _)| *score >= 2 || *mentions >= 2)
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.3.cmp(&b.3)));
    out.into_iter().map(|(n, _, m, _)| (n, m)).collect()
}

/// A stable 64-bit hash (FNV-1a), the same on every machine and run, so
/// the layout never jumps between frames or launches.
pub fn stable_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// When the Graph tab must rebuild: a different chat (its first message
/// and length), a different run (steps, checks, workers' pages, its end),
/// or a different answer. Hover and frames reuse the cached graph.
pub fn graph_key(chat: &str, entries: usize, run: &Run, question: &str, answer: &str) -> u64 {
    struct Fnv(u64);
    impl Hasher for Fnv {
        fn finish(&self) -> u64 {
            self.0
        }
        fn write(&mut self, bytes: &[u8]) {
            for b in bytes {
                self.0 ^= *b as u64;
                self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    chat.hash(&mut h);
    entries.hash(&mut h);
    run.steps.len().hash(&mut h);
    run.steps.last().map(|s| s.seq).hash(&mut h);
    run.checks.len().hash(&mut h);
    for c in &run.checks {
        c.text.hash(&mut h);
    }
    run.end.is_some().hash(&mut h);
    if let Some(d) = &run.deep {
        d.workers.len().hash(&mut h);
        for w in &d.workers {
            w.pages.len().hash(&mut h);
            (w.status == WorkerStatus::Running).hash(&mut h);
        }
    }
    question.hash(&mut h);
    answer.len().hash(&mut h);
    stable_hash(answer).hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tern_agent::{ClaimCheck, PageRef, RunStep};

    fn step(
        seq: u64,
        kind: StepKind,
        payload: serde_json::Value,
        page: Option<PageRef>,
    ) -> RunStep {
        RunStep {
            seq,
            event: "tool.executed".into(),
            summary: String::new(),
            kind,
            verb: "",
            target: String::new(),
            note: String::new(),
            at: Duration::from_secs(seq),
            tokens: None,
            ok: true,
            detailed: true,
            page,
            payload,
        }
    }

    fn page(url: &str, title: &str) -> PageRef {
        PageRef {
            title: title.into(),
            url: url.into(),
            site: site_of(url),
            builtin: None,
            favicon: None,
        }
    }

    fn claim(text: &str, citations: Vec<usize>, verdict: Verdict) -> ClaimCheck {
        serde_json::from_value(serde_json::json!({
            "claim": 1, "text": text, "citations": citations,
            "verdict": verdict, "overlap": 50
        }))
        .unwrap()
    }

    fn run_with(urls: &[(&str, &str)], read: &[usize]) -> Run {
        let mut r = Run::new();
        let hits: Vec<_> = urls
            .iter()
            .map(|(u, t)| serde_json::json!({"url": u, "title": t, "site": site_of(u)}))
            .collect();
        r.steps.push(step(
            1,
            StepKind::Search,
            serde_json::json!({"tool": "web_search", "meta": {"results": hits.len(), "hits": hits}}),
            None,
        ));
        for (k, &i) in read.iter().enumerate() {
            r.steps.push(step(
                2 + k as u64,
                StepKind::Read,
                serde_json::json!({}),
                Some(page(urls[i].0, urls[i].1)),
            ));
        }
        r
    }

    #[test]
    fn sources_come_from_the_run_once_each_read_before_seen() {
        let r = run_with(
            &[
                ("https://a.org/x", "GPU memory hierarchy"),
                ("https://b.com/y", "SIMD explained"),
                ("https://c.net/z", "Warp scheduling"),
            ],
            &[1],
        );
        let g = ChatGraph::build(&r, "How do GPUs work?", "");
        let sources: Vec<_> = g
            .nodes
            .iter()
            .filter_map(|n| match &n.kind {
                NodeKind::Source { url, read, .. } => Some((url.as_str(), *read)),
                _ => None,
            })
            .collect();
        assert_eq!(
            sources,
            vec![
                ("https://b.com/y", true),
                ("https://a.org/x", false),
                ("https://c.net/z", false)
            ]
        );
        // Read pages hang off the question with solid edges; results seen
        // are weak.
        let e = g.edges.iter().find(|e| e.to == 0).unwrap();
        assert_eq!((e.from, e.weak), (None, false));
        assert!(g.edges.iter().find(|e| e.to == 1).unwrap().weak);
        assert_eq!(g.summary(), "1 read · 2 seen · 1 terms");
    }

    #[test]
    fn claims_link_to_the_sources_they_cite() {
        let mut r = run_with(
            &[("https://a.org/x", "Alpha"), ("https://b.com/y", "Beta")],
            &[0, 1],
        );
        r.checks = vec![
            claim("Beta is fast", vec![2], Verdict::Supported),
            claim("Nobody said this", vec![], Verdict::NoSource),
        ];
        let g = ChatGraph::build(&r, "q", "");
        let c0 = g
            .nodes
            .iter()
            .position(|n| matches!(&n.kind, NodeKind::Claim { text, .. } if text == "Beta is fast"))
            .unwrap();
        let e: Vec<_> = g.edges.iter().filter(|e| e.to == c0).collect();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].from, Some(1), "cited [2] = second page read");
        let c1 = c0 + 1;
        let e1 = g.edges.iter().find(|e| e.to == c1).unwrap();
        assert_eq!((e1.from, e1.weak), (None, true));
        assert_eq!(g.readout(c0).unwrap().0, "claim · supported");
    }

    #[test]
    fn a_chat_without_sources_still_maps_its_key_terms() {
        let answer = "GPUs use **SIMD** and the **Pipeline**. Vertex Processing runs first, \
                      then Rasterization. NVLink joins two GPUs. Vertex Processing again.";
        let g = ChatGraph::build(&Run::new(), "How does a GPU work?", answer);
        let names: Vec<String> = g
            .nodes
            .iter()
            .filter_map(|n| match &n.kind {
                NodeKind::Entity { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert!(names.iter().any(|n| n == "SIMD"), "{names:?}");
        assert!(names.iter().any(|n| n == "NVLink"), "{names:?}");
        assert!(names.iter().any(|n| n == "Vertex Processing"), "{names:?}");
        assert!(
            !names.iter().any(|n| n == "Then" || n == "The"),
            "{names:?}"
        );
        assert!(names.len() <= MAX_ENTITIES);
        assert!(!g.is_empty());
        // Nothing at all: empty.
        assert!(ChatGraph::build(&Run::new(), "hi", "hello there").is_empty());
    }

    #[test]
    fn deep_workers_sit_between_the_question_and_their_pages() {
        let mut r = Run::new();
        let mut w = tern_agent::WorkerRow {
            n: 1,
            question: "What is SIMD?".into(),
            status: WorkerStatus::Done,
            ..Default::default()
        };
        w.pages = vec![page("https://simd.dev/a", "SIMD")];
        r.deep = Some(Box::new(tern_agent::DeepRun {
            workers: vec![w],
            ..Default::default()
        }));
        let g = ChatGraph::build(&r, "q", "");
        assert!(matches!(g.nodes[0].kind, NodeKind::Worker { n: 1, .. }));
        let e = g.edges.iter().find(|e| e.to == 1).unwrap();
        assert_eq!(e.from, Some(0), "page hangs off its worker");
    }

    #[test]
    fn the_key_changes_with_the_chat_and_the_run_but_not_otherwise() {
        let r = run_with(&[("https://a.org/x", "A")], &[]);
        let k = graph_key("chat one", 4, &r, "q", "answer");
        assert_eq!(k, graph_key("chat one", 4, &r, "q", "answer"), "stable");
        assert_ne!(
            k,
            graph_key("chat two", 4, &r, "q", "answer"),
            "switching chats"
        );
        assert_ne!(k, graph_key("chat one", 6, &r, "q", "answer"), "a new turn");
        assert_ne!(
            k,
            graph_key("chat one", 4, &r, "q", "answer!"),
            "answer streams"
        );
        let mut more = r.clone();
        more.steps.push(step(
            9,
            StepKind::Read,
            serde_json::json!({}),
            Some(page("https://a.org/x", "A")),
        ));
        assert_ne!(
            k,
            graph_key("chat one", 4, &more, "q", "answer"),
            "new source"
        );
        let mut checked = r.clone();
        checked.checks.push(claim("x", vec![], Verdict::Partly));
        assert_ne!(
            k,
            graph_key("chat one", 4, &checked, "q", "answer"),
            "new check"
        );
    }

    #[test]
    fn two_chats_give_two_different_graphs() {
        let a = ChatGraph::build(
            &run_with(&[("https://a.org/x", "A"), ("https://b.org/y", "B")], &[0]),
            "q1",
            "",
        );
        let b = ChatGraph::build(&run_with(&[("https://c.org/z", "C")], &[0]), "q2", "");
        assert_ne!(a, b);
        assert_ne!(a.nodes.len(), b.nodes.len());
    }

    #[test]
    fn the_source_cap_holds() {
        let urls: Vec<(String, String)> = (0..40)
            .map(|i| (format!("https://s{i}.org/p"), format!("t{i}")))
            .collect();
        let refs: Vec<(&str, &str)> = urls.iter().map(|(u, t)| (u.as_str(), t.as_str())).collect();
        let g = ChatGraph::build(&run_with(&refs, &[0, 5]), "q", "");
        assert_eq!(
            g.count(|k| matches!(k, NodeKind::Source { .. })),
            MAX_SOURCES
        );
        assert_eq!(
            g.count(|k| matches!(k, NodeKind::Source { read: true, .. })),
            2
        );
    }

    #[test]
    fn helpers() {
        assert_eq!(site_of("https://www.example.com/a?b"), "example.com");
        assert!(contains_word("the simd unit", "simd"));
        assert!(!contains_word("simdx", "simd"));
        assert_eq!(stable_hash("a"), stable_hash("a"));
        assert_ne!(stable_hash("a"), stable_hash("b"));
    }
}
