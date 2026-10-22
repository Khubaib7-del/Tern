# Changelog

All notable changes to Tern are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Tern has no released versions yet; once it does, versions will follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added (phase 6, started early)

- `tern-desktop`: first GPUI window on `gpui-kit` 0.8 (ADR 0006). Title bar with the Research or Study switch, icon rail, chat list, transcript with Markdown answers, working line, inline approvals, composer (Enter sends, Shift+Enter new line, stop), tokens used of the context window, and a run panel. Neutral greys until ADR 0002.
- CI uploads `tern-desktop.exe` and `tern.exe` (release, Windows x64) on every PR as the `tern-windows-x64` artifact.
- `tern-store`: `recent_sessions` and `rename_session`; a chat takes its title from the first message.
- Run panel views: **Summary** (rollup line, pages read, problems), **Detailed** (each step verb first with its target, note, and time), and **Raw** (every logged event with seq, type, time, and its JSON payload on click; filters All, Tools, Approvals, Search). All three read one `tern_agent::run::Run`, built live or from the log on reopen. "N steps" under an answer opens that answer's run.
- `web_search` results carry `query` and `results` in their event meta.
- Attachments: paperclip button (files and folders), drag and drop onto the composer, and long paste. Text over 4,000 characters becomes a **Pasted text** chip. Chips show name, lines or files and characters, and a **Context** or **Prompt** role (click to switch); Prompt attachments are sent first as the user's instructions, Context is marked as material with no instructions to follow. `tern_agent::attach` reads text files, folders (README first, then path order; hidden folders, `.git`, `node_modules`, `target` skipped; symlinks not followed), and `.zip` archives in memory (zips inside zips up to 3 deep). Limits: 8 MB per file, 200k characters per file, 400k per attachment, 500 files. PDFs, images, and Office files are listed as not read yet. Sent messages show their chips, also after reopening a chat.
- `@` references: typing `@` in the composer opens a menu of this message's attachments, sources read earlier in the chat (with their site), and **Attach a file or folder…**. Letters filter in order (`@rd` finds `README.md`); Up and Down move, Enter or Tab picks, Escape closes. `@prompt:name` sends that attachment as instructions and `@context:name` as material, whether picked or typed by hand. Picking a source adds it as a link the model can read again. Logic in `tern_agent::mention` (GPUI-free, tested).
- Message actions: every message and answer has **Copy** (a check confirms it) and **Reply**, which puts a quote bar over the composer; the quote rides ahead of the text in an `<in_reply_to>` block (`tern_agent::reply`, cut to 600 characters) and shows over the sent bubble, also after reopening. Answers also have **Branch**: a new chat, "Branch of …", that starts with a copy of every turn up to that answer (`tern_agent::branch`; the source chat's log is untouched).
- Session tabs: open chats show as tabs over the transcript once there are two or more (close button on each, **+** for a fresh chat). A chat-list row's **Open in new tab** button opens it beside the current one; clicking a row focuses its tab if open, else shows it in the active tab. Branches open in a new tab; archiving a chat closes its tab. Switching is held while a turn runs.
- Responsive layout (`crates/tern-desktop/src/layout.rs`): at 1100 px and wider both side panels sit beside the chat; from 900 px the run panel floats over the chat; under 900 px the sidebar floats too and closes when the window narrows past it.
- Editor and vault: the **Editor** rail item opens the vault (a folder of plain Markdown notes, `<data>/vault` or `vault` in preferences, so Obsidian can open it too). Note list with folders, **New note**, text editing that saves 700 ms after typing stops (and on blur or switching), a **Preview** toggle, `[[links]]` (click to open, or create a missing note) and **Linked from** backlinks. Answers get a **Save to vault** action that writes the question, answer, and sources as a note. The agent gets `vault_search` and `vault_read` (read) and `vault_write` (write, approved per call). `tern_agent::vault` keeps every path inside the vault, `.md` only, hidden folders skipped, atomic writes.
- The mic button is shown locked until a transcription model is set up.
- Motion pass (`docs/02-ui-definition.md` §2.7 and §6), on one curve `cubic-bezier(.32,.72,0,1)`: the working line shows the tern mark moving by state (a 2px glide every 2 s while thinking, quick wingbeats while searching or running a tool, a head dip per page read, small beats with streamed text, still and full-strength when it needs you, dim when stopping) next to a phrase with a 1.6 s shimmer ("Searching · 2 queries", "Reading page 3") and elapsed seconds. The sidebar and run panel slide and fade over 200 ms without reflowing; screens fade in; light and dark cross over in 600 ms with every colour token moving at once and a soft circle from the toggle. Reduced motion (`reduced_motion` in preferences or `TERN_REDUCED_MOTION=1`) turns all of it off.
- `assets/brand/tern-mark.svg`: the mark traced to one-colour SVG; the app embeds it.

### Fixed (desktop)

- Icons outside the components' default set (Pencil, Archive, run-panel icons) rendered blank: the app now registers the full Lucide catalog (`AllAssets`, about 750 KB).

### Added (phase 3, in progress)

- `tern-providers`: catalog of 13 providers (11 remote, Ollama and LM Studio local), own adapters for OpenAI-compatible, Anthropic, and Gemini APIs (ADR 0003), model listing with kinds and context windows, a no-token probe, non-streaming chat, and a usage ledger that marks estimates.
- `tern provider list | add | test | models | remove`. `add` reads the key from a hidden prompt, tests it, and saves it to the keychain only if the provider accepts it.
- ADR 0003: own provider adapters.
- `tern-agent` turn loop: bounded by model calls, tool calls, and tokens; every step moves the turn state machine and is written to the event log (memory or SQLite) before the next; repeated identical calls stop as a suspected loop; cancel flag; no silent retry or fallback.
- Policy gate: Allow, Ask, or Deny per effect class; once and session grants scoped to tool and target, session grants expire; deny rules beat grants; approvals recorded as `approval.requested` and `approval.answered`.
- Typed tool registry with JSON Schema input checks, timeouts, and a fenced `tool` block protocol that works on every provider (ADR 0004).
- ADR 0004: turn loop and policy gate.
- `tern ask "question"`: one search, up to 4 pages read in parallel, one model call, then a citation check; prints the answer, numbered sources (clickable in terminals that support links), unreadable pages, and any citation that matches no source. `--json` for the full report. ADR 0005.
- `tern search list/add/test/remove`: Tavily and Brave keys (tested before saving, kept in the keychain) and self-hosted SearXNG.
- `tern-crawl`: network guard (no private, loopback, or link-local addresses; redirects re-checked; 2 MiB cap), HTML-to-text reader, and source icons (built-in glyph ids for known services, favicon otherwise).
- Chat TUI gets `web_search` and `read_url` when a search service is set up; the launch card says which.
- Chat TUI (`tern` or `tern chat [--provider] [--model] [--base-url]`): unboxed transcript, working line with spinner and timer, composer with Shift+Enter and bracketed paste, footer with harness, provider, model, and tokens (`~` when estimated), boxed approval dialog (`y` once, `s` session, `n` no), help on `?`, Esc to stop. Works at 80×24; no hue until the palette decision.
- `tern-agent::ChatSession`: opens provider, model, and a stored session for front ends, so UIs never touch providers, the keychain, or the database.

### Changed

- `SecretString` moved to `tern-core` so providers never depend on the platform crate.
- `UsageDelta.cost_micros` is optional: no cost is shown until it is known.
- Turn state machine: a tool request rejected before the gate (unknown tool, bad input) moves straight to `RecordResult`.

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
