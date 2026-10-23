# Tern documentation

Tern is pre-alpha. These documents describe what is planned; nothing in them is shipped unless it says so.

| Doc | What it covers |
| --- | --- |
| [00 · Vision](00-vision.md) | Why Tern exists, who it is for, principles, what success looks like |
| [01 · Product spec](01-product-spec.md) | Research and Study harnesses, shared core features, chat behaviour, vault editor |
| [02 · UI definition](02-ui-definition.md) | Desktop and terminal layouts, activity streaming, palette, type, motion, Scout |
| [03 · Architecture](03-architecture.md) | Planned crates, dependency rules, event and tool contracts, memory, platform order |
| [04 · Roadmap](04-roadmap.md) | Phases 0 to 12 with deliverables and exit checks |
| [05 · Security model](05-security-model.md) | Assets, threats, controls, trust order, effect classes, data handling |
| [Evaluation](evaluation.md) | The test ladder, suites, metrics and intervals, first scores, and how to add cases |
| [Using Tern in Zed](zed.md) | Running Tern as an ACP agent inside Zed |
| [Assets](assets.md) | Third-party brand logos (Lobe Icons, MIT), their sources, and the trademark note |

## Decisions

Architecture decision records live in [decisions/](decisions/). Each has a status, the alternatives considered, consequences, and a revisit trigger.

| ADR | Title | Status |
| --- | --- | --- |
| [0001](decisions/0001-rust-gpui-windows-first.md) | Rust core, GPUI desktop, ratatui TUI, Windows first | Accepted |
| 0002 | Palette and type | Open |
| [0003](decisions/0003-own-provider-adapters.md) | Own provider adapters | Accepted |
| [0004](decisions/0004-turn-loop-and-policy-gate.md) | Bounded turn loop, policy gate, text tool protocol | Accepted |
| [0005](decisions/0005-quick-ask-search-reader-source-icons.md) | Quick ask: BYOK search, guarded reader, checked citations, source icons | Accepted |
| [0006](decisions/0006-desktop-shell-gpui-kit.md) | Desktop shell on GPUI Kit, brought forward | Accepted |
| [0007](decisions/0007-decision-model.md) | The decision model: local scorer, System One and rerank adapters, hard checks first | Accepted |

## Conventions

| Convention | Rule |
| --- | --- |
| Status words | Planned, In progress, Done, Parked (see [AGENTS.md](../AGENTS.md)) |
| Numbering | Top-level docs are numbered `NN-name.md`; ADRs are `NNNN-title.md` |
| Updates | A doc changes in the same PR as the decision or code that changes it |
| Style | Plain language, tables where they help, no emoji |
