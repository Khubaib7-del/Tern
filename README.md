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

It is being built as the research tool for its author's final-year project. **Nothing described below is shipped.** This repository holds the plan (docs, design rules, brand assets) the core skeleton, and provider setup. The first cited answer is next.

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
| 3 | Providers and the first cited answer in the TUI | In progress |
| 4 | Crawler and claim verification | Planned |
| 5 | Deep research: plans, workers, checkpoints, steering | Planned |
| 6 | Desktop app | Planned |
| 7 | Study harness | Planned |
| 8 | Decision model | Planned |
| 9 | Continuity and library | Planned |
| 10 | Extensibility: MCP, ACP, skills | Planned |
| 11 | Evaluation | Planned |
| 12 | Linux and macOS | Planned |

Phase detail and exit checks: [docs/04-roadmap.md](docs/04-roadmap.md).

## Getting Tern

Not yet released. There are no builds, installers, or packages.

## Repository layout

```text
.
├── .github/              issue forms, PR template, code owners
├── assets/
│   └── brand/            mark, lockups, app icon, animated dither hero
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

The Cargo workspace lives in `crates/`. Build and check it with `cargo build`, `cargo test --workspace`, `cargo run -p tern-tui -- doctor`, `cargo run -p tern-tui -- provider add groq`, and `cargo run -p tern-tui` to open the chat TUI (`-- chat --provider ollama` for a local model). Nothing is released yet.

## Documentation

| Doc | Covers |
| --- | --- |
| [Vision](docs/00-vision.md) | Why Tern exists and what success looks like |
| [Product spec](docs/01-product-spec.md) | Harnesses, features, chat behaviour, vault |
| [UI definition](docs/02-ui-definition.md) | Layouts, streaming, palette, motion, Scout |
| [Architecture](docs/03-architecture.md) | Crates, contracts, memory, platforms |
| [Roadmap](docs/04-roadmap.md) | Phases and exit checks |
| [Security model](docs/05-security-model.md) | Threats, controls, trust order |
| [Contributing](CONTRIBUTING.md) · [Agent rules](AGENTS.md) · [Security policy](SECURITY.md) | How work happens here |

## Licence

Copyright (c) 2026 Khubaib Nazeer. All rights reserved. Tern is proprietary software; no licence is granted to use, copy, modify, or distribute it. The Tern name, the diving-tern mark, the wordmark, and the Scout mascot are reserved brand assets. See [LICENSE](LICENSE).
