# Changelog

All notable changes to Tern are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Tern has no released versions yet; once it does, versions will follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- README header is now an animated dither scene (dark and light, 8 s loop): the tern flies a figure-eight around the wordmark, beating its wings, banking and turning, passing in front of the text and behind it. Replaces the static lockup and the text-art banner; generator script in `assets/brand/src/`.
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
