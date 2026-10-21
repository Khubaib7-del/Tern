# Changelog

All notable changes to Tern are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Tern has no released versions yet; once it does, versions will follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added (phase 3, in progress)

- `tern-providers`: catalog of 13 providers (11 remote, Ollama and LM Studio local), own adapters for OpenAI-compatible, Anthropic, and Gemini APIs (ADR 0003), model listing with kinds and context windows, a no-token probe, non-streaming chat, and a usage ledger that marks estimates.
- `tern provider list | add | test | models | remove`. `add` reads the key from a hidden prompt, tests it, and saves it to the keychain only if the provider accepts it.
- ADR 0003: own provider adapters.

### Changed

- `SecretString` moved to `tern-core` so providers never depend on the platform crate.
- `UsageDelta.cost_micros` is optional: no cost is shown until it is known.

### Added (phase 2, done)

- Cargo workspace with all eleven crates; dependency rules from `docs/03-architecture.md` enforced by a test that reads every manifest.
- `tern-core`: prefixed ULID IDs, `Q12` / `Q12.3` message references, effect classes, the versioned event envelope, the turn state machine with safe pause and resume, and the secret redactor.
- `tern-platform`: app data paths (`TERN_HOME` override), disk space, and the `SecretStore` trait with Windows Credential Manager. Other OS keychains report as unsupported until phase 12.
- `tern-store`: SQLite schema v1 for projects, sessions, and the append-only event log (redacted on append, update and delete blocked by triggers, replay from any sequence number).
- `tern doctor` (`--json` too): keychain round trip, data directory, disk space, database integrity, in plain language.
- Windows CI: fmt, clippy with warnings denied, tests, and `tern doctor`.

### Changed

- README header is now an animated dither scene (dark and light, 16 s loop): a procedural 3D tern with jointed wings flies around the wordmark, in front of the text and behind it, then lands in the logo position and becomes the original mark, which shimmers while the sky stays live. Replaces the static lockup and the text-art banner; generator script in `assets/brand/src/`.
- UI definition: live working line (tern mark animated per agent state beside a shimmering status phrase), message actions (swipe or button reply, copy, multi-branch), and composer input (attachments, long paste to `.txt`, `@` prompt or context roles, locked mic).
- Product spec: topic branching into several sub-sessions at once, file intake for any parseable file, recursive archive and folder reading, voice input planned with a transcription model.
- UI definition revised after the v2 concept review: ChatGPT-style title bar, icon rail and sidebar; Research │ Study toggle at the top of the chat area; session tabs; responsive composer rules; providers screen with logo rows, `Add` and toggles; Hoplite-style model picker; context ring and popover in the composer; Git and GitHub panel; Explain in 3D as its own mode; motion and theme-switch benchmarks.
- Product spec and roadmap: tabs, parallel sub-researchers, built-in browser, Explain in 3D, Git and GitHub, scheduled research; computer use moved to later, off by default. Phase 0 marked done.

### Removed

- Text-art banner from the README and the `tern-banner-*.png` renders.

### Added

- Repository baseline (roadmap phase 0). No application code exists yet.
- Proprietary licence (all rights reserved) with a brand clause covering the Tern name, mark, wordmark, and the Scout mascot.
- README with logo lockup, text-art banner, harness overview, principles, planned architecture, platform order, and roadmap summary.
- Project documentation under `docs/`: vision, product spec, UI definition, planned architecture, phased roadmap, security model, and ADR 0001 (Rust core, GPUI desktop, Windows first).
- Policy and process files: `SECURITY.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), and `AGENTS.md` for AI coding agents.
- Repository config: `.editorconfig`, `.gitattributes`, `.gitignore`, issue forms, pull request template, and `CODEOWNERS`.
- Brand assets under `assets/brand/`: mark, dark and light lockups, app icon, and rendered banner images.
