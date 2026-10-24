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
