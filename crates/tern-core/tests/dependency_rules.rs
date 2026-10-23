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
fn all_twelve_crates_exist() {
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
