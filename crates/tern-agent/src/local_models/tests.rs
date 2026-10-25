use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const A: &[u8] = b"first model file, some bytes of weights";
const B: &[u8] = b"second file: the vocabulary";

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Serves a body, honouring `Range: bytes=N-`.
struct Ranged(&'static [u8]);
impl Respond for Ranged {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let from = req
            .headers
            .get("range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("bytes="))
            .and_then(|v| v.trim_end_matches('-').parse::<usize>().ok());
        match from {
            Some(n) if n >= self.0.len() => ResponseTemplate::new(416),
            Some(n) => ResponseTemplate::new(206).set_body_bytes(&self.0[n..]),
            None => ResponseTemplate::new(200).set_body_bytes(self.0),
        }
    }
}

fn manifest(sha_b: Option<String>) -> Manifest {
    let json = serde_json::json!({
        "version": 1,
        "models": [{
            "id": "tiny-ocr",
            "task": "ocr",
            "name": "Tiny OCR",
            "hf_repo": "acme/tiny-ocr",
            "revision": "abc123",
            "licence": "MIT",
            "runtime": "onnx",
            "files": [
                {"name": "det.onnx", "size": A.len(), "sha256": hex(A)},
                {"name": "dict/keys.txt", "size": B.len(), "sha256": sha_b},
            ]
        }, {
            "id": "tiny-stt",
            "task": "speech_to_text",
            "name": "Tiny STT",
            "hf_repo": "acme/tiny-stt",
            "licence": "Apache-2.0",
            "runtime": "whisper_cpp",
            "files": [{"name": "m.bin", "size": A.len(), "sha256": hex(A)}]
        }]
    });
    Manifest::parse(&json.to_string()).unwrap()
}

async fn server() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/acme/tiny-ocr/resolve/abc123/det.onnx"))
        .respond_with(Ranged(A))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/acme/tiny-ocr/resolve/abc123/dict/keys.txt"))
        .respond_with(Ranged(B))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/acme/tiny-stt/resolve/main/m.bin"))
        .respond_with(Ranged(A))
        .mount(&s)
        .await;
    s
}

#[test]
fn the_builtin_manifest_is_valid_and_marked_placeholder() {
    let m = Manifest::builtin();
    assert!(m.models.len() >= 2);
    for e in &m.models {
        assert!(e.placeholder, "{} should be marked placeholder", e.id);
        assert!(e.size() > 0);
        assert!(e.files.iter().all(|f| f.sha256.is_some()));
        assert!(!e.licence.is_empty());
    }
    assert!(m.for_task(Task::Ocr).next().is_some());
    assert!(m.for_task(Task::SpeechToText).next().is_some());
}

#[test]
fn manifest_rejects_unsafe_paths_duplicates_and_bad_hashes() {
    let entry = |id: &str, file: &str, sha: &str| {
        serde_json::json!({
            "id": id, "task": "ocr", "name": "x", "hf_repo": "a/b",
            "licence": "MIT", "runtime": "onnx",
            "files": [{"name": file, "size": 1, "sha256": sha}]
        })
    };
    let ok = "a".repeat(64);
    let doc = |models: Vec<serde_json::Value>| {
        serde_json::json!({"version": 1, "models": models}).to_string()
    };
    assert!(Manifest::parse(&doc(vec![entry("a", "x.bin", &ok)])).is_ok());
    for bad in ["../x", "/etc/passwd", "a\\b", "C:/x", "a//b", RECORD] {
        assert!(
            Manifest::parse(&doc(vec![entry("a", bad, &ok)])).is_err(),
            "{bad}"
        );
    }
    assert!(Manifest::parse(&doc(vec![entry("../a", "x", &ok)])).is_err());
    assert!(Manifest::parse(&doc(vec![entry("a", "x", "zz")])).is_err());
    assert!(Manifest::parse(&doc(vec![entry("a", "x", &ok), entry("a", "y", &ok)])).is_err());
}

#[test]
fn user_entries_replace_and_extend_the_builtin_list() {
    let dir = tempfile::tempdir().unwrap();
    let mut extra = manifest(None);
    extra.models[0].id = "whisper-base-en".into();
    std::fs::write(
        dir.path().join(OVERRIDES),
        serde_json::to_string(&extra).unwrap(),
    )
    .unwrap();
    let (m, warn) = Manifest::load(dir.path());
    assert!(warn.is_none());
    assert_eq!(m.find("whisper-base-en").unwrap().name, "Tiny OCR");
    assert!(m.find("tiny-stt").is_some());
    std::fs::write(dir.path().join(OVERRIDES), "{not json").unwrap();
    let (m, warn) = Manifest::load(dir.path());
    assert!(warn.unwrap().contains("ignored"));
    assert_eq!(m, Manifest::builtin());
}

#[test]
fn tasks_round_trip_their_words() {
    for t in Task::ALL {
        assert_eq!(Task::parse(t.key()), Some(t));
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            format!("\"{}\"", t.key())
        );
    }
    assert_eq!(Task::parse("stt"), Some(Task::SpeechToText));
    assert_eq!(Task::parse("doc-parse"), Some(Task::DocParse));
    assert_eq!(Task::parse("nope"), None);
}

#[tokio::test]
async fn downloads_verifies_and_records_a_model() {
    let s = server().await;
    let dir = tempfile::tempdir().unwrap();
    let lm = LocalModels::new(dir.path(), manifest(Some(hex(B)))).with_endpoint(&s.uri());
    let entry = lm.manifest().find("tiny-ocr").unwrap().clone();
    assert_eq!(lm.state(&entry), State::NotInstalled);
    let mut events = Vec::new();
    let rec = lm
        .download("tiny-ocr", &CancelDownload::new(), &mut |e| events.push(e))
        .await
        .unwrap();
    assert_eq!(rec.files.len(), 2);
    assert_eq!(rec.files[1].sha256, hex(B));
    assert_eq!(lm.state(&entry), State::Installed);
    assert_eq!(
        std::fs::read(lm.dir("tiny-ocr").join("dict/keys.txt")).unwrap(),
        B
    );
    let total = (A.len() + B.len()) as u64;
    assert!(events.contains(&DownloadEvent::Progress { done: total, total }));
    assert!(events.contains(&DownloadEvent::Verifying("det.onnx".into())));
    // Progress never goes backwards.
    let dones: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            DownloadEvent::Progress { done, .. } => Some(*done),
            _ => None,
        })
        .collect();
    assert!(dones.windows(2).all(|w| w[0] <= w[1]), "{dones:?}");
    // A second download is a no-op.
    lm.download("tiny-ocr", &CancelDownload::new(), &mut |_| {})
        .await
        .unwrap();
}

#[tokio::test]
async fn a_bad_checksum_deletes_the_file_and_installs_nothing() {
    let s = server().await;
    let dir = tempfile::tempdir().unwrap();
    let lm = LocalModels::new(dir.path(), manifest(Some("0".repeat(64)))).with_endpoint(&s.uri());
    let err = lm
        .download("tiny-ocr", &CancelDownload::new(), &mut |_| {})
        .await
        .unwrap_err();
    assert_eq!(err, ModelError::Checksum("dict/keys.txt".into()));
    assert!(!lm.dir("tiny-ocr").join("dict/keys.txt").exists());
    assert!(lm.installed("tiny-ocr").is_none());
    // The good first file stays, so a retry only fetches what is missing.
    let entry = lm.manifest().find("tiny-ocr").unwrap().clone();
    assert_eq!(lm.state(&entry), State::Partial(A.len() as u64));
}

#[tokio::test]
async fn cancel_then_resume_finishes_the_download() {
    let s = server().await;
    let dir = tempfile::tempdir().unwrap();
    let lm = LocalModels::new(dir.path(), manifest(None)).with_endpoint(&s.uri());
    // A half-written first file, as an interrupted download leaves it.
    let det = lm.dir("tiny-ocr").join("det.onnx");
    std::fs::create_dir_all(det.parent().unwrap()).unwrap();
    std::fs::write(tern_providers::download::part_path(&det), &A[..7]).unwrap();
    let entry = lm.manifest().find("tiny-ocr").unwrap().clone();
    assert_eq!(lm.state(&entry), State::Partial(7));

    let cancel = CancelDownload::new();
    cancel.cancel();
    let err = lm
        .download("tiny-ocr", &cancel, &mut |_| {})
        .await
        .unwrap_err();
    assert_eq!(err, ModelError::Cancelled);
    assert!(lm.installed("tiny-ocr").is_none());

    let mut first = None;
    lm.download("tiny-ocr", &CancelDownload::new(), &mut |e| {
        if let DownloadEvent::Progress { done, .. } = e {
            first.get_or_insert(done);
        }
    })
    .await
    .unwrap();
    assert_eq!(first, Some(7), "resumed from the part file");
    assert_eq!(std::fs::read(&det).unwrap(), A);
}

#[tokio::test]
async fn defaults_enable_and_delete() {
    let s = server().await;
    let dir = tempfile::tempdir().unwrap();
    let lm = LocalModels::new(dir.path(), manifest(None)).with_endpoint(&s.uri());
    let mut prefs = LocalModelPrefs::default();
    assert!(lm.usable(&prefs, Task::Ocr).is_none());
    lm.download("tiny-ocr", &CancelDownload::new(), &mut |_| {})
        .await
        .unwrap();
    assert_eq!(lm.usable(&prefs, Task::Ocr).unwrap().id, "tiny-ocr");
    assert!(lm.usable(&prefs, Task::SpeechToText).is_none());

    prefs.set_default(Task::Ocr, Some("tiny-ocr"));
    let st = lm.statuses(&prefs);
    let ocr = st.iter().find(|m| m.entry.id == "tiny-ocr").unwrap();
    assert!(ocr.installed() && ocr.enabled && ocr.is_default);

    prefs.set_enabled("tiny-ocr", false);
    assert!(lm.usable(&prefs, Task::Ocr).is_none());
    prefs.set_enabled("tiny-ocr", true);
    assert!(lm.usable(&prefs, Task::Ocr).is_some());

    lm.remove("tiny-ocr").unwrap();
    prefs.forget("tiny-ocr");
    assert!(!lm.dir("tiny-ocr").exists());
    assert!(lm.usable(&prefs, Task::Ocr).is_none());
    assert_eq!(prefs.default_for(Task::Ocr), None);
    // Deleting what was never downloaded is fine; an unknown id is not.
    lm.remove("tiny-stt").unwrap();
    assert!(matches!(lm.remove("nope"), Err(ModelError::Unknown(_))));
}

#[test]
fn prefs_survive_a_round_trip_in_preferences() {
    let mut p = crate::Preferences::default();
    p.local_models
        .set_default(Task::SpeechToText, Some("whisper-base-en"));
    p.local_models.set_enabled("rapidocr-ppocrv4-mobile", false);
    let back: crate::Preferences =
        serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(back.local_models, p.local_models);
    // Older preference files without the key still load.
    let old: crate::Preferences = serde_json::from_str("{}").unwrap();
    assert_eq!(old.local_models, LocalModelPrefs::default());
}

#[test]
fn sizes_read_like_people_write_them() {
    assert_eq!(human_size(147_964_211), "148 MB");
    assert_eq!(human_size(4_745_517), "4.7 MB");
    assert_eq!(human_size(585_532), "586 KB");
    assert_eq!(human_size(2_100_000_000), "2.1 GB");
}

#[test]
fn no_runtime_ships_yet() {
    for r in [Runtime::WhisperCpp, Runtime::Onnx, Runtime::LlamaCpp] {
        assert!(!runtime_available(r));
    }
}
