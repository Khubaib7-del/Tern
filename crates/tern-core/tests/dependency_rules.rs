//! Enforces the dependency rules in `docs/03-architecture.md`, section 3, by
//! reading every crate manifest in the workspace.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn workspace_crates() -> BTreeMap<String, BTreeSet<String>> {
    let crates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(&crates_dir).unwrap() {
        let manifest = entry.unwrap().path().join("Cargo.toml");
        if !manifest.exists() {
            continue;
        }
        let doc: toml::Table = std::fs::read_to_string(&manifest).unwrap().parse().unwrap();
        let name = doc["package"]["name"].as_str().unwrap().to_owned();
        let mut deps = BTreeSet::new();
        let mut collect = |t: Option<&toml::Value>| {
            if let Some(t) = t.and_then(|v| v.as_table()) {
                deps.extend(t.keys().cloned());
            }
        };
        collect(doc.get("dependencies"));
        collect(doc.get("build-dependencies"));
        if let Some(targets) = doc.get("target").and_then(|v| v.as_table()) {
            for t in targets.values() {
                collect(t.get("dependencies"));
            }
        }
        out.insert(name, deps);
    }
    out
}

fn tern_deps(deps: &BTreeSet<String>) -> BTreeSet<&str> {
    deps.iter()
        .filter(|d| d.starts_with("tern-"))
        .map(String::as_str)
        .collect()
}

#[test]
fn all_thirteen_crates_exist() {
    let names: BTreeSet<_> = workspace_crates().into_keys().collect();
    let expected: BTreeSet<String> = [
        "tern-core",
        "tern-agent",
        "tern-providers",
        "tern-decision",
        "tern-crawl",
        "tern-store",
        "tern-research",
        "tern-study",
        "tern-eval",
        "tern-tui",
        "tern-desktop",
        "tern-platform",
        "tern-browser",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(names, expected);
}

#[test]
fn core_depends_on_nothing_heavy() {
    let all = workspace_crates();
    let core = &all["tern-core"];
    assert!(
        tern_deps(core).is_empty(),
        "tern-core depends on {:?}",
        tern_deps(core)
    );
    let forbidden = [
        "reqwest",
        "hyper",
        "tokio",
        "rusqlite",
        "sqlx",
        "gpui",
        "ratatui",
        "crossterm",
        "genai",
        "keyring-core",
        "windows",
        "windows-sys",
    ];
    for f in forbidden {
        assert!(!core.contains(f), "tern-core must not depend on {f}");
    }
}

#[test]
fn platform_depends_only_on_core() {
    let all = workspace_crates();
    assert_eq!(
        tern_deps(&all["tern-platform"]),
        BTreeSet::from(["tern-core"])
    );
}

#[test]
fn front_ends_use_only_agent_and_core() {
    let all = workspace_crates();
    for ui in ["tern-tui", "tern-desktop"] {
        let deps = tern_deps(&all[ui]);
        let allowed = BTreeSet::from(["tern-agent", "tern-core"]);
        assert!(deps.is_subset(&allowed), "{ui} depends on {deps:?}");
        for f in ["reqwest", "rusqlite", "keyring-core"] {
            assert!(!all[ui].contains(f), "{ui} must not depend on {f}");
        }
    }
}

#[test]
fn the_browser_helper_holds_the_engine_alone() {
    let all = workspace_crates();
    // The helper speaks the protocol from tern-core and nothing else of Tern.
    assert_eq!(
        tern_deps(&all["tern-browser"]),
        BTreeSet::from(["tern-core"])
    );
    // The web engine never links into a front end: a page that hangs or
    // crashes takes down the helper, not Tern (ADR 0011).
    for ui in ["tern-tui", "tern-desktop"] {
        for f in ["wry", "tao", "webview2-com", "webkit2gtk"] {
            assert!(!all[ui].contains(f), "{ui} must not depend on {f}");
        }
    }
}

#[test]
fn harnesses_never_import_each_other() {
    let all = workspace_crates();
    assert!(!all["tern-research"].contains("tern-study"));
    assert!(!all["tern-study"].contains("tern-research"));
}

#[test]
fn only_providers_and_crawl_may_reach_the_network() {
    let all = workspace_crates();
    for (name, deps) in &all {
        if name == "tern-providers" || name == "tern-crawl" || name == "tern-decision" {
            continue;
        }
        for f in ["reqwest", "hyper", "ureq", "isahc", "surf"] {
            assert!(!deps.contains(f), "{name} must not depend on {f}");
        }
    }
}

#[test]
fn eval_harness_is_headless_and_below_the_agent() {
    let all = workspace_crates();
    let eval = &all["tern-eval"];
    for f in [
        "tern-agent",
        "tern-store",
        "tern-crawl",
        "tern-providers",
        "tern-platform",
        "gpui",
        "gpui-kit",
        "ratatui",
        "crossterm",
        "rusqlite",
    ] {
        assert!(!eval.contains(f), "tern-eval must not depend on {f}");
    }
}

#[test]
fn the_windows_icon_is_embedded_at_build_time_only() {
    // tern-desktop, tern and tern-browser carry the Tern icon and version
    // information through build.rs; the resource compiler never links in.
    let crates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    for name in ["tern-desktop", "tern-tui", "tern-browser"] {
        let dir = crates_dir.join(name);
        let doc: toml::Table = std::fs::read_to_string(dir.join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let windows = &doc["target"]["cfg(windows)"];
        assert!(
            windows["build-dependencies"].get("winresource").is_some(),
            "{name}"
        );
        assert!(
            windows
                .get("dependencies")
                .is_none_or(|d| d.get("winresource").is_none())
        );
        assert!(doc["dependencies"].get("winresource").is_none(), "{name}");
        let build = std::fs::read_to_string(dir.join("build.rs")).unwrap();
        assert!(build.contains("assets/brand/tern.ico"), "{name}");
    }
    assert!(crates_dir.join("../assets/brand/tern.ico").exists());
}

#[test]
fn the_windows_desktop_gets_an_eight_megabyte_main_stack() {
    // Windows gives a main thread 1 MB; GPUI lays out and paints on it
    // recursively, and a stack overflow ends the app with nothing logged.
    let build = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tern-desktop/build.rs"),
    )
    .unwrap();
    assert!(build.contains("cargo:rustc-link-arg-bins=/STACK:8388608"));
    assert!(build.contains("cargo:rustc-link-arg-bins=-Wl,--stack,8388608"));
}

/// Every `.rs` file under `crates/*/src`, with its path from `crates/`.
fn crate_sources() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let crates_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates_dir).unwrap() {
        let src = entry.unwrap().path().join("src");
        if src.is_dir() {
            walk(&src, &mut files);
        }
    }
    files
        .into_iter()
        .map(|p| {
            let name = p
                .strip_prefix(&crates_dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            (name, std::fs::read_to_string(&p).unwrap())
        })
        .collect()
}

/// The source with `//` comments blanked, so prose can name a call.
fn code_only(src: &str) -> String {
    src.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The bodies of `cx.listener(...)` and `.on_*(...)` calls (a click,
/// mouse, wheel, pinch, key or action listener), found by matching
/// parentheses. A heuristic, not a parser: good enough to catch a frame
/// request written into a listener.
fn listener_bodies(code: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < code.len() {
        let rest = &code[i..];
        let start = if rest.starts_with("cx.listener(") {
            Some("cx.listener(".len())
        } else if rest.starts_with(".on_") {
            rest.find('(')
                .filter(|&p| {
                    rest[4..p]
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                })
                .map(|p| p + 1)
        } else {
            None
        };
        if let Some(open) = start {
            let from = i + open;
            let mut depth = 1usize;
            let mut j = from;
            while j < code.len() && depth > 0 {
                match bytes[j] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            out.push(&code[from..j.saturating_sub(1)]);
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    out
}

#[test]
fn the_listener_scan_finds_a_frame_request_in_a_listener() {
    let bad = "div().on_click(cx.listener(|this, _, window, cx| {\n    this.step += 1;\n    window.request_animation_frame();\n}))";
    assert!(
        listener_bodies(bad)
            .iter()
            .any(|b| b.contains("request_animation_frame"))
    );
    let good =
        "div().on_click(cx.listener(|this, _, window, cx| {\n    next_frame(window, cx);\n}))";
    assert!(
        !listener_bodies(good)
            .iter()
            .any(|b| b.contains("request_animation_frame"))
    );
}

#[test]
fn no_frame_request_reads_gpuis_rendered_view_outside_a_render() {
    // `Window::request_animation_frame` and `Window::current_view` unwrap
    // GPUI's rendered-view stack, which is empty in a listener, a timer,
    // an action or a task. Called from a click or wheel listener they
    // panicked inside the Windows window procedure, which cannot unwind,
    // and Tern closed (0xC0000409, PR #57: Explain in 3D's Next, Play,
    // zoom and Reset). Tern asks for frames with
    // `view::frame::next_frame`, which names its view itself; the two
    // calls appear nowhere else, not even where they would work, and
    // `view/frame.rs` uses `request_animation_frame` only in its tests.
    let mut offenders = Vec::new();
    let mut in_listeners = Vec::new();
    for (name, src) in crate_sources() {
        let code = code_only(&src);
        for b in listener_bodies(&code) {
            if b.contains("request_animation_frame") || b.contains("current_view(") {
                in_listeners.push(name.clone());
            }
        }
        if name == "tern-desktop/src/view/frame.rs" {
            let tests = code.find("#[cfg(test)]").expect("frame.rs has its tests");
            assert!(!code[..tests].contains("request_animation_frame"));
            assert!(!code[..tests].contains("current_view("));
            assert!(code[..tests].contains("on_next_frame"));
            continue;
        }
        for (n, line) in code.lines().enumerate() {
            if line.contains("request_animation_frame") || line.contains("current_view(") {
                offenders.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(in_listeners.is_empty(), "in a listener: {in_listeners:?}");
    assert!(
        offenders.is_empty(),
        "use view::frame::next_frame (or next_frame_for in a paint callback):\n{}",
        offenders.join("\n")
    );
}

#[test]
fn a_cached_view_never_takes_the_window_buttons_with_it() {
    // GPUI copies a cached view's hitboxes, listeners and scene into the
    // next frame but not its window-control areas, which Windows' hit
    // test reads for the caption and the minimize, maximize and close
    // buttons (`WM_NCHITTEST` → `HTMINBUTTON`/`HTMAXBUTTON`/`HTCLOSE`).
    // With the workspace drawn cached, every frame only Scout redrew had
    // none and the three buttons did nothing (PR #61). Whoever draws a
    // view cached also draws `caption_layer` outside it, under it.
    let mut cached = Vec::new();
    for (name, src) in crate_sources() {
        let code = code_only(&src);
        // A view drawn cached: `entity.clone().cached(style)` (not the
        // providers' listing cache, `providers.cached(id)`).
        if !name.starts_with("tern-desktop/src/") || !code.contains("clone().cached(") {
            continue;
        }
        cached.push(name.clone());
        let layer = code.find(".children(caption_layer(window))");
        let view = code.find("clone().cached(");
        assert!(
            matches!((layer, view), (Some(l), Some(v)) if l < v),
            "{name} draws a view cached without caption_layer under it"
        );
    }
    assert!(
        cached.iter().any(|n| n.ends_with("view/pet_view.rs")),
        "the scan finds the workspace's Shell: {cached:?}"
    );
}
