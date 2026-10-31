# Engineering handbook (draft)

> Status: draft, started in PR #85 (31 October 2026). It grows pull request by pull request and is finished in #113, the handbook bundle. It covers how Tern is built, tested and merged. How the engine works inside is in [internals.md](internals.md).

## 1. How work lands

- One pull request at a time, in the order of the [roadmap](../04-roadmap.md); each starts when the owner approves it and merges when its acceptance tests pass and the owner has tried it.
- Every pull request updates the README's **What works today**, the CHANGELOG's **Unreleased** section, and the handbook drafts here when it changes how Tern works inside.
- A pull request that adds third-party code, data or assets adds it to `NOTICE` with its licence. Only permissive licences (MIT, Apache 2.0, BSD, ISC, Zlib, CC0, OFL for fonts and the like): no GPL, AGPL or unlicensed code.

## 2. CI is the gate

The `CI` workflow runs on every pull request and on `main`, with four jobs:

| Job | Runs |
| --- | --- |
| Linux | `cargo fmt --check`, clippy, the workspace tests, the offline evaluation suites (3 trials, baseline check), the desktop build |
| macOS arm64 | clippy, tests, the desktop build |
| Windows | `cargo fmt --check`, clippy, tests |
| Windows desktop build | a release build of `tern-desktop`, `tern` and the browser helper, uploaded to download and test |

- `RUSTFLAGS=-D warnings`: every warning fails the build, including `missing_debug_implementations` (every public type needs `Debug`, by derive or by hand) and clippy's default lints.
- A new push to a pull request cancels its running CI. Batch commits when a run is close to finishing.
- Read a failing job's log from `https://api.github.com/repos/Khubaib7-del/Tern/actions/jobs/<id>/logs`.

## 3. Tests

- **Acceptance tests** come from the plan of each pull request (for #85, A-T1 to A-T16) and run against scripted models: no live keys in CI. #85's are in `crates/tern-agent/tests/durable_turns.rs`; its scripted `Script` model replays a list of steps (a reply, an error, a stream that drops after some text, a call that never returns) and reports about four characters a token.
- **Time.** Tests that wait use `#[tokio::test(start_paused = true)]`, so a 10 s Retry-After takes no real time.
- **No shared state between tests.** Tests run in parallel: give each run its own `Estimator`, cache folder (`tempfile::tempdir()`) and checkpoints. Do not set environment variables in tests; pass the switch in (for example `learned_context(entries, enabled)`).
- **Unit tests** sit beside the code (`#[cfg(test)] mod tests`). Parsers and classifiers (`followup`, `quickread`, `signals`, `intent`) carry a list of real messages that must and must not match.
- **Offline evaluation** (`tern eval`) runs in the Linux job against a baseline; a drop fails the job.

## 4. Conventions

- Comments say what and why in plain words and name the pull request and plan item (`(#85, A13)`), so a reader can find the reasoning.
- User-facing text is plain: say what happened and what to do, never internal names.
- New behaviour that could surprise a test or a user is off by default in the engine (`RunHooks`, `ChatOptions`) and turned on in the app's own options (`Preferences::chat_options`), so scripted tests keep their exact calls.
- Settings go in `Preferences` with `#[serde(default)]`, so an older `preferences.json` still loads.

## 5. Trying a change on Windows

The Windows desktop build job uploads the `tern-windows-x64` artifact with `tern-desktop.exe`, `tern.exe` and the browser helper. Download it from the run's page, unzip it, and run `tern-desktop.exe`. Developer switches: set `TERN_DEV=1` and `TERN_CHAOS` (for example `429-daily`) in the shell before starting it; see [internals.md](internals.md#6-developer-switches).
