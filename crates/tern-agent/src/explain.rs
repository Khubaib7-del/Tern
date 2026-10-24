//! Explain in 3D for front ends: build a scene for a concept (from a
//! topic, the whole library, or a chat message), save it to the study
//! library with an SVG snapshot, and save it to the vault as a note.
//!
//! ```text
//! <data>/study/explain/<id>.json   the scene
//! <data>/study/explain/<id>.svg    its snapshot
//! ```
//!
//! The scene model, its parsing and fallback, the projection, and the SVG
//! writer are in `tern-study`.

use std::path::{Path, PathBuf};

use tern_providers::ChatMessage;

/// The camera (orbit, zoom, pan, focus), GPUI-free.
pub use tern_study::camera;
pub use tern_study::explain_answer::{LEVELS, citations};
pub use tern_study::explain3d::{
    Drawing, Edge, Face, FaceKind, Fit, GRID, Link, Part, Scene, Shape, Step, SvgStyle, View, hit,
    ink_amount, mix, part_centre, parts_centre, project, to_svg,
};
use tern_study::material::{Anchor, Source};

use crate::study::{
    StudyEngine, StudyLibrary, StudyStore, ensure_topics, now, read_json, select_sources,
    write_atomic,
};

/// Characters of passages a scene is built from.
pub const EXPLAIN_CHARS: usize = 10_000;
/// Snapshot size.
pub const SVG_W: u32 = 960;
pub const SVG_H: u32 = 640;

fn dir(store: &StudyStore) -> PathBuf {
    store.root().join("explain")
}

/// What to explain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ask {
    /// The concept; empty lets the model (or the fallback) pick one.
    pub concept: String,
    /// A topic id, number, or title to draw the passages from.
    pub topic: Option<String>,
    /// Text from the session (a chat message) to explain from, cited as
    /// "This chat".
    pub context: Option<String>,
}

/// Builds a scene. Returns it (not yet saved) and, when the model was
/// tried and not used, why.
pub async fn build(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    engine: Option<&StudyEngine>,
    ask: &Ask,
) -> Result<(Scene, Option<String>), String> {
    use tern_study::explain3d::{fallback_scene, parse_scene, scene_prompt};
    // A chat answer is mapped whole, basics to advanced.
    if ask.topic.is_none()
        && let Some(text) = ask.context.as_deref().filter(|t| !t.trim().is_empty())
    {
        let at = now();
        let (mut scene, note) = build_answer(engine, text, ask.concept.trim(), at).await;
        scene.id = new_id(store, at);
        return Ok((scene, note));
    }
    if !lib.materials.is_empty() {
        ensure_topics(store, lib)?;
    }
    let topic =
        match &ask.topic {
            Some(t) => Some(lib.topic(t).cloned().ok_or_else(|| {
                format!("No topic matches \"{t}\". `tern study topics` lists them.")
            })?),
            None => None,
        };
    let concept = if ask.concept.trim().is_empty() {
        topic.as_ref().map(|t| t.title.clone()).unwrap_or_default()
    } else {
        ask.concept.trim().to_owned()
    };
    let mut sources: Vec<Source> = Vec::new();
    let mut budget = EXPLAIN_CHARS;
    if let Some(text) = ask.context.as_deref().filter(|t| !t.trim().is_empty()) {
        let text: String = text.chars().take(EXPLAIN_CHARS / 2).collect();
        budget -= text.len().min(budget / 2);
        sources.push(Source {
            n: 1,
            chunk: "chat".into(),
            anchor: Anchor {
                material: "chat".into(),
                file: "This chat".into(),
                page: None,
                section: None,
            },
            text,
        });
    }
    let chunks = store.chunks(lib);
    let query = if concept.is_empty() {
        ask.context.clone().unwrap_or_default()
    } else {
        concept.clone()
    };
    let from_lib = select_sources(&chunks, topic.as_ref(), &query, budget);
    let offset = sources.len();
    sources.extend(from_lib.into_iter().map(|mut s| {
        s.n += offset;
        s
    }));
    if sources.is_empty() && concept.is_empty() {
        return Err("Name a concept, or add materials first: `tern study add <files>`.".into());
    }
    let tid = topic.as_ref().map(|t| t.id.as_str());
    let at = now();
    let mut note = None;
    let mut scene = None;
    if let Some(e) = engine {
        let (system, user) = scene_prompt(&concept, &sources);
        match e
            .ask(system, vec![ChatMessage::user(user)], 2500, 0.3, None)
            .await
        {
            Ok(text) => {
                scene = parse_scene(&text, &concept, &sources, tid, at);
                if scene.is_none() {
                    note = Some("The model's scene could not be read; Tern built one from your materials instead.".into());
                }
            }
            Err(err) => {
                note = Some(format!(
                    "The model could not be reached ({err}); Tern built a scene from your materials instead."
                ))
            }
        }
    }
    let mut scene = scene.unwrap_or_else(|| fallback_scene(&concept, &sources, tid, at));
    scene.id = new_id(store, at);
    Ok((scene, note))
}

/// Maps a whole chat answer from the basics to the advanced parts (4 to
/// 20 nodes in five levels). The model writes the map when there is one:
/// a long answer goes in chunks and the maps are merged, so nothing is cut
/// off. Without a model, or when its map cannot be read, Tern maps the
/// answer itself. Returns the scene (no id yet) and, when the model was
/// tried and not used, why.
pub async fn build_answer(
    engine: Option<&StudyEngine>,
    answer: &str,
    title: &str,
    at: i64,
) -> (Scene, Option<String>) {
    use tern_study::explain_answer::{
        answer_prompt, answer_scene, chunk_prompt, chunks, merge, parse_answer_scene, passages,
        target_nodes,
    };
    let concept = if title.is_empty() {
        "This answer"
    } else {
        title
    };
    let fallback = || answer_scene(answer, title, at);
    let Some(e) = engine else {
        return (fallback(), None);
    };
    let parts = chunks(answer);
    let mut maps = Vec::new();
    let total = answer.chars().count().max(1);
    let n_all = {
        let ps = passages(answer);
        let heads = ps.iter().filter(|p| p.heading.is_some()).count();
        target_nodes(answer, ps.len(), heads)
    };
    for (k, part) in parts.iter().enumerate() {
        let (system, user) = if parts.len() == 1 {
            answer_prompt(part, concept)
        } else {
            let share = (n_all * part.chars().count()).div_ceil(total);
            chunk_prompt(part, concept, k, parts.len(), share)
        };
        match e
            .ask(system, vec![ChatMessage::user(user)], 4000, 0.3, None)
            .await
        {
            Ok(text) => match parse_answer_scene(&text, concept, at) {
                Some(s) => maps.push(s),
                None => {
                    return (
                        fallback(),
                        Some("The model's map could not be read; Tern mapped the answer itself instead.".into()),
                    );
                }
            },
            Err(err) => {
                return (
                    fallback(),
                    Some(format!(
                        "The model could not be reached ({err}); Tern mapped the answer itself instead."
                    )),
                );
            }
        }
    }
    match merge(maps, at) {
        Some(s) => (s, None),
        None => (fallback(), None),
    }
}

fn new_id(store: &StudyStore, at: i64) -> String {
    let base = format!("x{at}");
    let mut id = base.clone();
    let mut n = 2;
    while dir(store).join(format!("{id}.json")).exists() {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// Saves a scene to the study library with its SVG snapshot (the whole
/// machine, summary underneath). Returns (json, svg) paths.
pub fn save(store: &StudyStore, scene: &Scene) -> Result<(PathBuf, PathBuf), String> {
    let d = dir(store);
    let json = d.join(format!("{}.json", scene.id));
    write_atomic(
        &json,
        &serde_json::to_string_pretty(scene).unwrap_or_default(),
    )?;
    let svg = d.join(format!("{}.svg", scene.id));
    write_atomic(
        &svg,
        &to_svg(scene, None, SvgStyle::default(), SVG_W, SVG_H),
    )?;
    Ok((json, svg))
}

pub fn load(store: &StudyStore, id: &str) -> Result<Scene, String> {
    read_json(&dir(store).join(format!("{id}.json")))?
        .ok_or_else(|| format!("No saved explainer {id}."))
}

/// Saved scenes, newest first: (id, concept, created).
pub fn list(store: &StudyStore) -> Vec<(String, String, i64)> {
    let mut out: Vec<(String, String, i64)> = std::fs::read_dir(dir(store))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let s: Scene = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            Some((s.id, s.concept, s.created_at))
        })
        .collect();
    out.sort_by_key(|x| std::cmp::Reverse(x.2));
    out
}

/// Writes the snapshot at `step` (or the whole machine) to `path`.
pub fn export_svg(scene: &Scene, step: Option<usize>, path: &Path) -> Result<(), String> {
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, to_svg(scene, step, SvgStyle::default(), SVG_W, SVG_H))
        .map_err(|e| format!("Tern could not write {}: {e}", path.display()))
}

/// Saves the scene into the vault as `Explain/<concept>.md` with its SVG
/// beside it. Returns the note's path in the vault.
pub fn save_to_vault(vault: &crate::vault::Vault, scene: &Scene) -> Result<String, String> {
    let base = crate::vault::clean_name(&scene.concept);
    let mut n = 1;
    let name = loop {
        let name = if n == 1 {
            format!("Explain/{base}.md")
        } else {
            format!("Explain/{base} {n}.md")
        };
        if vault.resolve(&name).is_ok_and(|p| !p.exists()) {
            break name;
        }
        n += 1;
    };
    let stem = Path::new(&name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Explainer")
        .to_owned();
    let svg_name = format!("{stem}.svg");
    let note_path = vault.resolve(&name)?;
    let svg_path = note_path.with_file_name(&svg_name);
    if let Some(d) = svg_path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &svg_path,
        to_svg(scene, None, SvgStyle::default(), SVG_W, SVG_H),
    )
    .map_err(|e| e.to_string())?;
    vault.write(&name, &scene.to_markdown(Some(&svg_name)))?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tern_providers::{ChatRequest, ChatResponse, ProviderResult};

    struct Script(Mutex<String>);

    impl crate::engine::Model for Script {
        fn provider_id(&self) -> &str {
            "test"
        }
        fn complete<'a>(
            &'a self,
            _req: &'a ChatRequest,
        ) -> crate::tools::BoxFuture<'a, ProviderResult<ChatResponse>> {
            Box::pin(async move {
                Ok(ChatResponse {
                    text: self.0.lock().unwrap().clone(),
                    usage: tern_providers::Usage::measured(10, 5),
                    finish_reason: None,
                    model: None,
                })
            })
        }
    }

    const NOTES: &str = "# Paging\nA page table is a map from virtual pages to physical frames. A TLB is a small cache of recent translations. A page fault is a trap raised when a page is not in memory.";

    fn setup() -> (tempfile::TempDir, StudyStore, StudyLibrary) {
        let dir = tempfile::tempdir().unwrap();
        let store = StudyStore::at(dir.path().join("study"));
        let file = dir.path().join("os.md");
        std::fs::write(&file, NOTES).unwrap();
        let mut lib = store.load().unwrap();
        crate::study::add_path(&store, &mut lib, &file).unwrap();
        store.save(&lib).unwrap();
        (dir, store, lib)
    }

    #[tokio::test]
    async fn builds_saves_and_lists_offline() {
        let (dir, store, mut lib) = setup();
        let ask = Ask {
            concept: "virtual memory".into(),
            ..Ask::default()
        };
        let (scene, note) = build(&store, &mut lib, None, &ask).await.unwrap();
        assert!(note.is_none());
        assert_eq!(scene.written_by, "fallback");
        assert!(scene.id.starts_with('x'));
        assert!(scene.parts.len() >= 3);
        let (json, svg) = save(&store, &scene).unwrap();
        assert!(json.exists() && svg.exists());
        assert_eq!(load(&store, &scene.id).unwrap(), scene);
        assert_eq!(list(&store)[0].1, "virtual memory");
        let out = dir.path().join("snap/vm.svg");
        export_svg(&scene, Some(1), &out).unwrap();
        assert!(std::fs::read_to_string(&out).unwrap().contains("Step 2 of"));
        let vault = crate::vault::Vault::at(dir.path().join("vault"));
        let note = save_to_vault(&vault, &scene).unwrap();
        assert_eq!(note, "Explain/virtual memory.md");
        assert!(vault.read(&note).unwrap().contains("virtual memory.svg"));
        assert!(dir.path().join("vault/Explain/virtual memory.svg").exists());
        // From a topic with no concept: the topic's title.
        let (t, _) = build(
            &store,
            &mut lib,
            None,
            &Ask {
                topic: Some("1".into()),
                ..Ask::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(t.concept, lib.topics[0].title);
    }

    #[tokio::test]
    async fn chat_answers_map_whole_from_basics_to_advanced() {
        let (_d, store, mut lib) = setup();
        let reply = r#"{"concept": "TLB lookup", "summary": "How a lookup goes.", "nodes": [
            {"label": "Pages", "level": 0, "explain": "Memory comes in pages.", "quote": "Memory is split into pages [1].", "terms": ["page"]},
            {"label": "The TLB", "level": 1, "explain": "A cache of translations.", "quote": "The CPU first checks the TLB.", "needs": ["Pages"]},
            {"label": "Page walk", "level": 2, "explain": "On a miss it walks the table.", "quote": "Before walking the page table [2].", "needs": ["The TLB"]},
            {"label": "Huge pages", "level": 4, "explain": "Bigger pages, fewer misses.", "quote": "Huge pages cut misses."}
        ]}"#;
        let e = StudyEngine::with_model(Arc::new(Script(Mutex::new(reply.into()))), "m", "T");
        let ask = Ask {
            concept: String::new(),
            topic: None,
            context: Some("The CPU first checks the TLB before walking the page table.".into()),
        };
        let (s, note) = build(&store, &mut lib, Some(&e), &ask).await.unwrap();
        assert!(note.is_none());
        assert_eq!(s.written_by, "model");
        assert!(s.id.starts_with('x'));
        assert_eq!(s.steps[0].cite().as_deref(), Some("This answer"));
        assert_eq!(s.steps.len(), 4);
        assert!(s.steps.windows(2).all(|w| w[0].level <= w[1].level));
        assert!(s.steps[0].passage.contains("[1]"));
        let bad = StudyEngine::with_model(Arc::new(Script(Mutex::new("hmm".into()))), "m", "T");
        let (s, note) = build(&store, &mut lib, Some(&bad), &ask).await.unwrap();
        assert_eq!(s.written_by, "fallback");
        assert!(note.unwrap().contains("could not be read"));
        // No 5,000-character cut: the end of a long answer is on the map.
        let mut long = String::new();
        for i in 0..60 {
            long.push_str(&format!(
                "## Part {i}\nSection {i} covers one more mechanism of paging in detail.\n\n"
            ));
        }
        long.push_str("## Last\nThe final point is about TLB shootdowns across cores.\n");
        assert!(long.len() > 5_000);
        let ask = Ask {
            context: Some(long),
            ..Ask::default()
        };
        let (s, _) = build(&store, &mut lib, None, &ask).await.unwrap();
        assert!(
            s.steps
                .iter()
                .any(|st| st.passage.contains("shootdowns") || st.caption.contains("shootdowns")),
            "the end of the answer is mapped"
        );
        assert!(s.parts.len() <= tern_study::explain_answer::MAX_NODES);
        // Nothing at all to go on.
        let empty = StudyStore::at(_d.path().join("none"));
        let mut lib2 = StudyLibrary::default();
        assert!(
            build(&empty, &mut lib2, None, &Ask::default())
                .await
                .is_err()
        );
        let (s, _) = build(
            &empty,
            &mut lib2,
            None,
            &Ask {
                concept: "entropy".into(),
                ..Ask::default()
            },
        )
        .await
        .unwrap();
        assert!(s.steps[1].caption.contains("do not describe"));
    }
}
