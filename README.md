<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/tern-hero-dark.gif">
    <source media="(prefers-color-scheme: light)" srcset="assets/brand/tern-hero-light.gif">
    <img alt="Tern, a diving tern drawn in moving dither beside the TERN Research wordmark" src="assets/brand/tern-hero-light.gif" width="100%">
  </picture>
</p>

<p align="center"><b>A local-first agent harness for research and study, with every claim cited and checked.</b></p>

<p align="center">Pre-alpha · private · not yet released</p>

---

Tern turns a rough question into a verified, cited report, and turns what you read into something you can learn from. It runs on your machine, with your own API keys or local models, and shows you what it is doing and what leaves your computer.

It is being built as the research tool for its author's final-year project. It is pre-alpha: the core, provider setup, cited web answers, a terminal chat, and a first desktop app work today (see below); the deep research pipeline, claim verification, and the Study harness are still ahead.

## What works today

| Area | Working now |
| --- | --- |
| Core | Stable IDs, versioned events, turn state machine, secret redaction, append-only SQLite event log with replay, `tern doctor` |
| Providers | 13 providers (11 remote, Ollama and LM Studio local) on Tern's own adapters; keys tested before they are saved to Windows Credential Manager; streaming replies |
| Turn loop | Bounded by model calls, tool calls, and tokens; loop detection; cancel; policy gate with Allow, Ask, Deny and inline approvals |
| Web answers | `tern ask` and chat search through Tavily, Brave, or SearXNG, read pages behind a network guard, and cite them with a citation check |
| Terminal | `tern` / `tern chat`: an 80×24 ratatui chat |
| Desktop app | Windows build on GPUI: chat with streaming answers, chat list (reopen, rename, archive), providers screen, source rows with site icons, context ring, run panel (Summary, Detailed, Raw), attachments (files, folders, nested `.zip`, long paste), `@` references (attachments and sources, `@prompt:` / `@context:`), message actions (copy, reply with a quote, branch into a new chat), session tabs, a responsive layout (panels float over the chat on narrow windows), the Editor and vault (Markdown notes, `[[links]]` and backlinks, autosave, Save to vault, agent vault tools), and the motion pass (working line with the moving tern mark and shimmer, sliding panels, the soft light/dark switch) |

Each pull request builds `tern-desktop.exe` and `tern.exe` for Windows x64 as a CI artifact. There are no releases or installers yet.

## Two harnesses, one core

| | Research | Study |
| --- | --- | --- |
| For | Finding out what is true and who said it | Learning it well enough to explain it |
| Produces | Cited reports, literature reviews, evidence tables, fact-check boards, comparisons | Tutoring, flashcard decks, quizzes, viva practice, study plans |
| Method | Plan → search → read → verify → write, with a run report | Explain → check understanding → practise → grade |
| Checked by | Claim verdicts against exact source spans: ■ supported · ✕ contradicted · ○ unverified · ⌀ unreachable · — waived | The same verdict vocabulary, applied to answers |
| Looks like | Cooler surface, square markers, mono labels | Warmer surface, round markers, italic serif labels |

Both share the same projects, library, vault, memory, providers, and permissions. Switch harness per session.

## Planned capabilities

| Area | Planned |
| --- | --- |
| Models | Bring your own key for many providers; local models through Ollama, LM Studio, or any OpenAI-compatible endpoint; keys kept in the OS keychain |
| Decision model | A fast scorer beside the main model that decides fixed choices (which source first, which tool next, does a claim pass) while hard checks keep the final word |
| Crawler | Tern's own reader and crawler, with a network guard and stable quote anchors |
| Control | Editable plans before spend, live steering, a watcher you can ask what the agent is doing, checkpoints, safe pause at budget limits |
| Memory | Stable IDs for every query, file, and source; a goal and decision ledger that survives compaction |
| Surfaces | A terminal UI and a desktop app over the same core; MCP client and ACP server |
| Workspace | Session tabs, parallel sub-researchers, a built-in browser for reading sources, and Explain in 3D for interactive concept models |
| Git | Project vaults as git repositories: commit, branch, push, and open pull requests from the app |
| Vault | Plain Markdown, Obsidian-compatible, with agent edits as diffs |

Full detail: [docs/01-product-spec.md](docs/01-product-spec.md).

## Principles

| # | Principle |
| --- | --- |
| 1 | **Evidence before claims.** A claim stays unverified until a source or check confirms it. |
| 2 | **Local-first.** Your vault, library, and logs live on your machine. |
| 3 | **Your keys, your models.** Remote or local, your choice; no silent provider switching. |
| 4 | **Show what leaves the machine.** Before data goes to a provider, Tern says what and to whom. |
| 5 | **The process folds away, the result stays open.** One quiet line per turn; the full log one click away. |
| 6 | **Calm by default.** Black and white, one restrained accent, motion only on real events. |
| 7 | **The user steers.** Plans first; pause, redirect, and resume at any point. |
| 8 | **One core, many surfaces.** The core owns state and security; UIs only render its events. |

## Planned architecture

A single Rust workspace. The headless core owns execution and security; the TUI and desktop app subscribe to its events.

| Crate | Role |
| --- | --- |
| `tern-core` | Shared types, stable IDs, versioned events, turn state machine, secret redaction |
| `tern-agent` | Turn engine, planner, tool registry, policy gate, workers, watcher, steering, checkpoints, budgets |
| `tern-providers` | BYOK remote providers and local models, capability probing, usage accounting |
| `tern-decision` | Decision-model slot with a local reranker fallback |
| `tern-crawl` | Fetch, extract, and chunk web pages and PDFs, behind an SSRF and DNS-rebinding guard |
| `tern-store` | SQLite, full-text and vector indexes, append-only event log, artifacts, the Markdown vault |
| `tern-research` | Research pipeline, scholarly sources, claim verification, citations, run reports |
| `tern-study` | Tutor, flashcards, quizzes, viva practice, study plans |
| `tern-tui` | The `tern` CLI and ratatui terminal UI |
| `tern-desktop` | The GPUI desktop app |
| `tern-platform` | Keychain, paths, and OS integration behind traits |

Details and dependency rules: [docs/03-architecture.md](docs/03-architecture.md). Founding decision: [ADR 0001](docs/decisions/0001-rust-gpui-windows-first.md).

## Platform order

| Order | Platform |
| --- | --- |
| 1 | Windows |
| 2 | Linux |
| 3 | macOS |

Every OS difference sits behind `tern-platform`, so ports stay small.

## Roadmap

| Phase | Focus | Status |
| --- | --- | --- |
| 0 | Repository baseline | Done |
| 1 | Design lock: palette, fonts, concept screens, SVG mark | Planned |
| 2 | Core skeleton, keychain, store, `tern doctor` | Done |
| 3 | Providers and the first cited answer in the TUI | Done |
| 4 | Crawler and claim verification | Planned |
| 5 | Deep research: plans, workers, checkpoints, steering | Planned |
| 6 | Desktop app | In progress |
| 7 | Study harness | Planned |
| 8 | Decision model | Planned |
| 9 | Continuity and library | Planned |
| 10 | Extensibility: MCP, ACP, skills | Planned |
| 11 | Evaluation | Planned |
| 12 | Linux and macOS | Planned |

Phase detail and exit checks: [docs/04-roadmap.md](docs/04-roadmap.md).

## Getting Tern

Not yet released. There are no installers or packages; test builds come from CI (the `tern-windows-x64` artifact on each pull request).

## Repository layout

```text
.
├── .github/              issue forms, PR template, code owners
├── assets/
│   └── brand/            mark (PNG and SVG), lockups, app icon, animated dither hero
├── crates/               the Rust workspace (11 crates, see Planned architecture)
├── docs/
│   ├── 00-vision.md
│   ├── 01-product-spec.md
│   ├── 02-ui-definition.md
│   ├── 03-architecture.md
│   ├── 04-roadmap.md
│   ├── 05-security-model.md
│   └── decisions/        architecture decision records
├── AGENTS.md             rules for AI coding agents
├── CHANGELOG.md
├── CODE_OF_CONDUCT.md
├── CONTRIBUTING.md
├── LICENSE
├── README.md
└── SECURITY.md
```

## Building from source

| Command | Does |
| --- | --- |
| `cargo build` | Builds the workspace |
| `cargo test --workspace` | Runs the tests |
| `cargo run -p tern-desktop` | Opens the desktop app |
| `cargo run -p tern-tui -- doctor` | Checks paths, keychain, and store |
| `cargo run -p tern-tui -- provider add groq` | Adds a provider key (hidden prompt, tested before saving) |
| `cargo run -p tern-tui -- search add tavily` | Adds a web search key |
| `cargo run -p tern-tui -- ask "your question"` | A cited answer from the web |
| `cargo run -p tern-tui` | The terminal chat (`-- chat --provider ollama` for a local model) |

Set `TERN_REDUCED_MOTION=1` (or `"reduced_motion": true` in `preferences.json`) to turn motion off in the desktop app.

## Documentation

| Doc | Covers |
| --- | --- |
| [Vision](docs/00-vision.md) | Why Tern exists and what success looks like |
| [Product spec](docs/01-product-spec.md) | Harnesses, features, chat behaviour, vault |
| [UI definition](docs/02-ui-definition.md) | Layouts, streaming, palette, motion, Scout |
| [Architecture](docs/03-architecture.md) | Crates, contracts, memory, platforms |
| [Changelog](CHANGELOG.md) | Everything merged so far |
| [Roadmap](docs/04-roadmap.md) | Phases and exit checks |
| [Security model](docs/05-security-model.md) | Threats, controls, trust order |
| [Contributing](CONTRIBUTING.md) · [Agent rules](AGENTS.md) · [Security policy](SECURITY.md) | How work happens here |

## Licence

Copyright (c) 2026 Khubaib Nazeer. All rights reserved. Tern is proprietary software; no licence is granted to use, copy, modify, or distribute it. The Tern name, the diving-tern mark, the wordmark, and the Scout mascot are reserved brand assets. See [LICENSE](LICENSE).
