# ADR 0001 · Rust core, GPUI desktop, ratatui TUI, Windows first

| Field | Value |
| --- | --- |
| Status | Accepted |
| Date | 2026-10-21 |
| Decider | Khubaib Nazeer |

## Context

Tern needs two surfaces, a terminal UI and a desktop app, over one agent engine. The engine streams many events, runs a crawler and local indexes, holds API keys, and must stay responsive during long research runs on a student laptop. The author works on Windows, so Windows is where Tern is used first. A modular design should make Linux and macOS ports cheap later.

## Decision

| Area | Choice |
| --- | --- |
| Language | Rust for the whole stack, one Cargo workspace |
| Shape | Headless core; the TUI and desktop app are event subscribers |
| TUI | `ratatui` with `crossterm` |
| Desktop | GPUI (the framework behind the Zed editor), through a component kit pinned to a known GPUI revision |
| Platform order | Windows first, then Linux, then macOS, with OS differences behind `tern-platform` traits |

## Alternatives considered

| Option | Why not |
| --- | --- |
| Tauri with a web front end | Mature and cross-platform, but splits the stack into Rust and TypeScript and adds a webview; GPUI keeps one language and native rendering |
| Electron | Heavy memory footprint for a long-running local tool |
| Python engine with a native shell | A sidecar process, a second runtime to ship, and weaker guarantees for a long-running concurrent engine |
| egui or Iced | Viable, but GPUI's editor-grade text, docks, and performance fit a vault editor and a dense agent UI better |
| POSIX first | The primary user is on Windows; problems found late on Windows are costlier than porting a well-abstracted core |

## Consequences

| Positive | Negative |
| --- | --- |
| One language, one toolchain, shared types end to end | GPUI is pre-1.0 and changes often; the kit must be pinned and upgrades planned |
| Native performance for streaming, crawling, and indexing | GPUI's Windows backend is newer than its macOS one; expect platform bugs |
| UI risk isolated from the engine by the subscriber rule | Fewer ready-made UI components than web stacks |
| Windows problems surface first, where the user is | CI must cover Windows from phase 2 |

## Revisit when

- GPUI's Windows backend blocks a phase for more than two weeks.
- The pinned component kit is abandoned or cannot follow GPUI.
- A second primary user on another OS joins the project.
