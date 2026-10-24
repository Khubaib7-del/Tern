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

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use tern_providers::ChatMessage;

/// The camera (orbit, zoom, pan, focus), GPUI-free.
pub use tern_study::camera;
pub use tern_study::explain_answer::{
    LEVELED_PITCH, LEVELED_YAW, LEVELS, citations, is_question_turn, pick_explanatory,
};
pub use tern_study::explain3d::{
    Drawing, Edge, Face, FaceKind, Fit, GRID, Link, Part, Scene, Shape, Step, SvgStyle, View, hit,
    ink_amount, mix, part_centre, parts_centre, project, sanitize, to_svg,
};
/// Where labels go on screen, GPUI-free.
pub use tern_study::labels;
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

/// Where a build is, for the loading line: shown as "Asking the model…",
/// "Reading the reply…", "Laying out the scene…".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    /// Gathering the passages, before any model call.
    #[default]
    Starting,
    /// Waiting on model call `part` (from 1) of `of`.
    Asking { part: u8, of: u8 },
    /// Reading the model's reply.
    Reading,
    /// Laying out and tidying the scene.
    Laying,
}

impl Stage {
    fn pack(self) -> u32 {
        match self {
            Stage::Starting => 0,
            Stage::Asking { part, of } => 1 | (u32::from(part) << 8) | (u32::from(of) << 16),
            Stage::Reading => 2,
            Stage::Laying => 3,
        }
    }

    fn unpack(v: u32) -> Self {
        match v & 0xff {
            1 => Stage::Asking {
                part: ((v >> 8) & 0xff) as u8,
                of: ((v >> 16) & 0xff) as u8,
            },
            2 => Stage::Reading,
            3 => Stage::Laying,
            _ => Stage::Starting,
        }
    }

    /// The loading line's words.
    pub fn text(self) -> String {
        match self {
            Stage::Starting => "Gathering your sources…".into(),
            Stage::Asking { part, of } if of > 1 => {
                format!("Asking the model… (part {part} of {of})")
            }
            Stage::Asking { .. } => "Asking the model…".into(),
            Stage::Reading => "Reading the reply…".into(),
            Stage::Laying => "Laying out the scene…".into(),
        }
    }
}

/// Stops a scene being built: checked after every model call, so a
/// closed tab or a new build never waits on (or receives) the old one.
/// It also carries the build's [`Stage`], so the tab can say where it is.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>, Arc<AtomicU32>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Whether `other` is this same token (not a copy of its state).
    pub fn same(&self, other: &Cancel) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Where the build is.
    pub fn stage(&self) -> Stage {
        Stage::unpack(self.1.load(Ordering::Relaxed))
    }

    pub fn set_stage(&self, s: Stage) {
        self.1.store(s.pack(), Ordering::Relaxed);
    }
}

/// What a cancelled build returns.
pub const CANCELLED: &str = "Cancelled.";

/// Longest one model call for a scene may take; past it Tern maps the
/// answer itself, so a slow local model never leaves the tab spinning.
pub const MODEL_WAIT: Duration = Duration::from_secs(150);

/// Runs `f`, turning a panic into an error: building a scene must never
/// take the app down.
fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        let why = p
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "an internal error".into());
        format!("Tern could not {what} ({why}).")
    })
}

/// One model call with [`MODEL_WAIT`].
async fn ask_model(
    e: &StudyEngine,
    system: String,
    user: String,
    max_tokens: u32,
) -> Result<String, String> {
    match tokio::time::timeout(
        MODEL_WAIT,
        e.ask(system, vec![ChatMessage::user(user)], max_tokens, 0.3, None),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => Err(format!("it took longer than {} s", MODEL_WAIT.as_secs())),
    }
}

/// Builds a scene. Returns it (not yet saved) and, when the model was
/// tried and not used, why.
pub async fn build(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    engine: Option<&StudyEngine>,
    ask: &Ask,
) -> Result<(Scene, Option<String>), String> {
    build_with(store, lib, engine, ask, &Cancel::new()).await
}

/// [`build`] that stops when `cancel` is set, and whose scene is always
/// safe to draw ([`sanitize`]): every number finite, everything on the
/// floor, steps naming real parts. A panic while building comes back as
/// an error, never as a crash.
pub async fn build_with(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    engine: Option<&StudyEngine>,
    ask: &Ask,
    cancel: &Cancel,
) -> Result<(Scene, Option<String>), String> {
    let (mut scene, note) = build_inner(store, lib, engine, ask, cancel).await?;
    if cancel.is_cancelled() {
        return Err(CANCELLED.into());
    }
    cancel.set_stage(Stage::Laying);
    guard("tidy the scene", || sanitize(&mut scene))?;
    Ok((scene, note))
}

async fn build_inner(
    store: &StudyStore,
    lib: &mut StudyLibrary,
    engine: Option<&StudyEngine>,
    ask: &Ask,
    cancel: &Cancel,
) -> Result<(Scene, Option<String>), String> {
    use tern_study::explain3d::{fallback_scene, parse_scene, scene_prompt};
    // A chat answer is mapped whole, basics to advanced.
    if ask.topic.is_none()
        && let Some(text) = ask.context.as_deref().filter(|t| !t.trim().is_empty())
    {
        let at = now();
        let (mut scene, note) =
            build_answer_with(engine, text, ask.concept.trim(), at, cancel).await?;
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
        cancel.set_stage(Stage::Asking { part: 1, of: 1 });
        match ask_model(e, system, user, 2500).await {
            Ok(text) => {
                if cancel.is_cancelled() {
                    return Err(CANCELLED.into());
                }
                cancel.set_stage(Stage::Reading);
                scene = guard("read the model's scene", || {
                    parse_scene(&text, &concept, &sources, tid, at)
                })
                .ok()
                .flatten();
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
    if cancel.is_cancelled() {
        return Err(CANCELLED.into());
    }
    let mut scene = match scene {
        Some(s) => s,
        None => guard("build a scene from your materials", || {
            fallback_scene(&concept, &sources, tid, at)
        })?,
    };
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
) -> Result<(Scene, Option<String>), String> {
    build_answer_with(engine, answer, title, at, &Cancel::new()).await
}

async fn build_answer_with(
    engine: Option<&StudyEngine>,
    answer: &str,
    title: &str,
    at: i64,
    cancel: &Cancel,
) -> Result<(Scene, Option<String>), String> {
    use tern_study::explain_answer::{
        answer_prompt, answer_scene, chunk_prompt, chunks, merge, parse_answer_scene, passages,
        target_nodes,
    };
    let concept = if title.is_empty() {
        "This answer"
    } else {
        title
    };
    let fallback = |note: Option<String>| {
        guard("map this answer", || answer_scene(answer, title, at)).map(|s| (s, note))
    };
    let Some(e) = engine else {
        cancel.set_stage(Stage::Laying);
        return fallback(None);
    };
    let parts = guard("split this answer", || chunks(answer))?;
    let mut maps = Vec::new();
    let total = answer.chars().count().max(1);
    let n_all = guard("size the map", || {
        let ps = passages(answer);
        let heads = ps.iter().filter(|p| p.heading.is_some()).count();
        target_nodes(answer, ps.len(), heads)
    })?;
    for (k, part) in parts.iter().enumerate() {
        let (system, user) = if parts.len() == 1 {
            answer_prompt(part, concept)
        } else {
            let share = (n_all * part.chars().count()).div_ceil(total);
            chunk_prompt(part, concept, k, parts.len(), share)
        };
        cancel.set_stage(Stage::Asking {
            part: (k + 1).min(255) as u8,
            of: parts.len().min(255) as u8,
        });
        let reply = ask_model(e, system, user, 4000).await;
        if cancel.is_cancelled() {
            return Err(CANCELLED.into());
        }
        cancel.set_stage(Stage::Reading);
        match reply {
            Ok(text) => match guard("read the model's map", || {
                parse_answer_scene(&text, concept, at)
            })
            .ok()
            .flatten()
            {
                Some(s) => maps.push(s),
                None => {
                    return fallback(Some(
                        "The model's map could not be read; Tern mapped the answer itself instead."
                            .into(),
                    ));
                }
            },
            Err(err) => {
                return fallback(Some(format!(
                    "The model could not be reached ({err}); Tern mapped the answer itself instead."
                )));
            }
        }
    }
    match guard("merge the maps", || merge(maps, at)).ok().flatten() {
        Some(s) => Ok((s, None)),
        None => fallback(None),
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
        for i in 0..150 {
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

    #[tokio::test]
    async fn a_cancelled_build_stops_and_scenes_are_sanitized() {
        let (_d, store, mut lib) = setup();
        let reply = r#"{"concept": "x", "nodes": [{"label": "a"}, {"label": "b"}, {"label": "c"}, {"label": "d"}]}"#;
        let e = StudyEngine::with_model(Arc::new(Script(Mutex::new(reply.into()))), "m", "T");
        let ask = Ask {
            context: Some("Some answer text that explains a thing.".into()),
            ..Ask::default()
        };
        let c = Cancel::new();
        c.cancel();
        let r = build_with(&store, &mut lib, Some(&e), &ask, &c).await;
        assert_eq!(r.unwrap_err(), CANCELLED);
        let token = Cancel::new();
        assert_eq!(token.stage(), Stage::Starting);
        let (s, _) = build_with(&store, &mut lib, Some(&e), &ask, &token)
            .await
            .unwrap();
        assert_eq!(
            token.stage(),
            Stage::Laying,
            "the token says where it got to"
        );
        assert!(s.parts.iter().all(|p| p.at.iter().all(|v| v.is_finite())));
        // Small models' near-JSON is read, not thrown away.
        let sloppy = "```json\n{'concept': 'x', 'nodes': [{'label': 'a', 'level': 'Foundations'}, {'label': 'b'}, {'label': 'c'}, {'label': 'd'},]}\n```";
        let e = StudyEngine::with_model(Arc::new(Script(Mutex::new(sloppy.into()))), "m", "T");
        let (s, note) = build(&store, &mut lib, Some(&e), &ask).await.unwrap();
        assert!(note.is_none(), "{note:?}");
        assert_eq!(s.written_by, "model");
        assert_eq!(
            guard("x", || -> u8 { panic!("boom") }).unwrap_err(),
            "Tern could not x (boom)."
        );
    }

    #[test]
    fn stages_round_trip_and_read_plainly() {
        let c = Cancel::new();
        for st in [
            Stage::Starting,
            Stage::Asking { part: 2, of: 3 },
            Stage::Reading,
            Stage::Laying,
        ] {
            c.set_stage(st);
            assert_eq!(c.stage(), st);
        }
        assert_eq!(Stage::Asking { part: 1, of: 1 }.text(), "Asking the model…");
        assert_eq!(
            Stage::Asking { part: 2, of: 3 }.text(),
            "Asking the model… (part 2 of 3)"
        );
        assert_eq!(Stage::Laying.text(), "Laying out the scene…");
        // A copy shares the stage, as the bridge and the build do.
        let d = c.clone();
        d.set_stage(Stage::Reading);
        assert_eq!(c.stage(), Stage::Reading);
    }
}
