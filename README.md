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

It is being built as the research tool for its author's final-year project. It is pre-alpha: the core, provider setup, cited web answers, a terminal chat, the desktop app, Tern Crawl, claim checks with run reports, deep research with parallel workers, the Study harness (tutor, quizzes, viva, flashcards with spaced repetition and Anki export, Explain in 3D), the decision model, continuity (message IDs and sub-sessions, cleaning a chat by ID range, handoff packets, the Library, highlight actions, and the side chat), and extensibility (MCP servers, Tern as an ACP agent in editors such as Zed, skills, `TERN.md` project rules, and project folders) work today (see below); study plans, evaluation, and the Linux and macOS ports are still ahead.

## What works today

| Area | Working now |
| --- | --- |
| Core | Stable IDs, versioned events, turn state machine, secret redaction, append-only SQLite event log with replay, `tern doctor` |
| Providers | 13 providers (11 remote, Ollama and LM Studio local) on Tern's own adapters; keys tested before they are saved to Windows Credential Manager; streaming replies |
| Turn loop | Bounded by model calls, tool calls, and tokens; loop detection; cancel; policy gate with Allow, Ask, Deny and inline approvals |
| Web answers | `tern ask` and chat search through Tavily, Brave, or SearXNG, read pages behind a network guard, and cite them with a citation check |
| Crawler | **Tern Crawl**, Tern's own crawler and reader, with its own colour logo on every "Read N pages" row: network guard, robots.txt, per-site pacing, a day-long page cache, Readability-style main text, and PDFs read page by page with `[page N]` markers |
| PDFs and pictures | Attach a PDF (up to 32 MB) and its text is read page by page; attach a PNG, JPEG, GIF, or WebP (up to 5 MB) and it goes to the model as a picture (OpenAI-compatible, Anthropic, and Gemini formats), staying in the chat for follow-ups |
| Claim checks | Every cited sentence of an answer is checked against the text Tern Crawl read for its source: Supported, Partly supported, Not supported, or No source, with the quoted passage and a link to it. The chat's model judges; a word-overlap check works offline and fills gaps |
| Deep research | A plan first: the model splits the question into sub-questions (searches, expected source types, budget share) and you run, edit, skip, or cancel it before anything is searched. Workers then research the parts in parallel (3 at once by default), each with its own slice of tokens, pages, and time, reading through Tern Crawl; one answer is written from their notes with one set of citations, then checked and reported like any answer. `tern research --deep "question"` in the terminal; the composer's **Deep research** mode on desktop |
| Run reports | Each answer writes a Markdown and JSON report (question, steps, sources with robots and cache status, claim verdicts, budget and tokens, errors, and a section per worker for deep research); `tern report` prints or exports one |
| Resume | Research runs save a checkpoint at safe points (each tool result; for deep research the approved plan, each worker's reads and notes, each worker's end, and before the answer). A run that crashed, was force-quit, or was stopped picks up where it was, without repeating finished workers or reading their pages again: `tern resume [chat]`, or **Resume** on desktop |
| Steering | Send a note while a run goes: "focus on 2024 sources" guides the rest of the run, "drop sub-question 3", "add: cost comparison", and "redirect 2: …" change a deep run's workers. Notes land at the next safe point and the log says what each did. Type during `tern research`, use `tern steer <chat> "…"` from another terminal, or press Enter in the desktop composer while a run goes |
| Watcher | Put a question on watch (daily, weekly, or any interval). The desktop app re-runs due watches while it is open, and `tern watch run` does from the terminal; each run is diffed against the last report: new sources, changed verdicts, new and dropped claims. There is no operating-system scheduler, so nothing runs while Tern is closed. `tern watch add\|list\|rm\|run` |
| Ledgers | Each run keeps a source ledger (every URL considered, read live or from the cache, or skipped for robots.txt, the budget, a duplicate, or the policy gate) and a budget ledger (tokens, pages, searches, and time per step and per worker), in the run report, `tern ledger`, and the desktop run panel's **Ledger** view |
| Study | Add your materials (PDFs read page by page, notes, folders, vault notes, a research report, links) and Tern splits them into topics with learning objectives and page anchors, and keeps a mastery score per topic (a rating moved by each graded answer, a Leitner box for when it is due, both explained in words). **Tutor**: Explain, Socratic, or Explain like I'm new, citing the pages it teaches from and flagging anything beyond your materials. **Quizzes**: multiple choice, true or false, fill in the blank, and short answer, with an answer key and source page per question; keyed answers are graded exactly, short answers by the model with a word-overlap fallback, in the verdict glyphs; results move mastery and are kept in a history. **Viva practice**: an examiner asks harder questions level by level, follows up on thin answers, and ends with a rubric report (clarity, correctness, depth, use of sources) saved as Markdown. Everything works offline from your materials when no model is set. `tern study add\|list\|rm\|topics\|history`, `tern tutor`, `tern quiz`, `tern viva`; the **Study** harness on desktop |
| Flashcards | Cards written from your materials per topic (basic, cloze `{{c1::…}}`, and "explain why"), each with the page or section it came from; by the model, or offline from definitions, key sentences, and stated reasons. Repeats are skipped; you can edit, suspend, and delete cards. Reviews run on SM-2, the scheduler Anki uses (Again, Hard, Good, Easy; learning steps of 1 and 10 minutes; ease and interval per card), with a daily queue (learning, then reviews due today, then up to 20 new cards), deck statistics, and each answer nudging its topic's mastery. Export a real Anki package (`.apkg`: SQLite collection and media, "Tern Basic" and "Tern Cloze" note types, one deck, tags per topic, the source in its own field, and review cards keeping their schedule), CSV, or Anki-ready TSV, and import CSV or TSV back. `tern cards gen\|review\|list\|stats\|edit\|suspend\|rm\|export\|import`; **Flashcards** on the desktop Study home, with a review screen (the card turns over on the motion curve, keys 1 to 4, Scout holds up a card and reacts) and a deck screen |
| Explain in 3D | A concept from your materials (a topic, a chat answer, or one you name) built as a small isometric machine: blocks, cylinders, plates, and arrows on a floor grid, linked in order, with 3 to 7 steps that light the parts they are about, each captioned and citing its page. Solid shaded faces with a hairline overlay in the monochrome palette and the one accent. On desktop it opens as a tab: hover lifts a part and the machine tilts toward the pointer, click a part to jump to its step, step with the arrows or play them (Space); reduced motion keeps it still. Saved to the study library as JSON with an SVG snapshot, or to the vault as a note. Offline, the scene is built from your materials' key terms. `tern explain3d "concept" --svg FILE`; the cube button on a topic or an answer on desktop |
| Decision model | A fast scorer beside the chat model for fixed choices, with hard checks that always override it. It ranks search results before Tern reads them (quick asks, chat searches, and every deep worker), hints the first tool for a chat message, pre-scores each claim before the model judge (which keeps the final word), starts deep workers and due watches most pressing first, and logs every decision with its model, probabilities, latency, and cost. **Local** (the default) is Tern's own offline scorer: word statistics and a few rules, free and deterministic. **Remote** is bring-your-own-key: TypeSafe **Jev** or Subquadratic **SubQ Snap** (the same System One API, every decision point) or **Voyage** rerank (source ranking and now-or-later), with a 4 s timeout and the local scorer as fallback. `tern decide "question" --choices a,b,c [--context FILE]`, `tern decide mode`, `tern decide key add`, `tern decide bench`; the desktop **Decision model** page and the run panel's **Decisions** view. Offline benchmark on the bundled 50 fixtures: fixed order 52%, local scorer 86% (in-sample; see [ADR 0007](docs/decisions/0007-decision-model.md)) |
| Continuity | Every turn has a stable ID (`Q12`), and sub-sessions hung under a message add a sub-ID (`Q12.3`) that resolves in `@` mentions, the CLI, `tern://chat/<id>/Q12.3` links, and Library search. **Clean a chat by ID range**: remove `Q3-Q5` to the chat's trash (restorable), or move messages with the context before them to a new session or a sub-session, with the original kept until you confirm (or undo). **Sub-sessions**: branch from a message, plan one with a question, or run a background branch; when it finishes it posts a summary back to the parent, which the parent's model reads too. **Handoff packets**: goal, decisions, open questions, key sources and claims, a ledger snapshot, and next steps as Markdown and JSON, and a new session that starts from the packet alone. `tern range`, `tern sub`, `tern show`, `tern handoff`, `tern new --from-handoff` |
| Library | Reports, sources, notes, decks, files, Explain-in-3D scenes, and sessions in one index with search (titles, details, and message IDs), filters, sort, paging, bulk delete to a trash that restores, and empty-trash; quick with thousands of items. `tern library list\|search\|rm\|restore\|empty-trash`; the **Library** page on desktop |
| Highlights and side chat | Select text in an answer for **Explain**, **Verify claim** (checked against the pages that answer read), **Add to notes** (the vault's Highlights note), **Make flashcard** (a cloze on its key term), or **Ask in side chat**. The side chat reads the thread and writes nothing back. `tern side "question"`; docked beside the chat on desktop or popped out into its own window |
| MCP servers | Add MCP servers over stdio or Streamable HTTP (or import another app's config). Tern speaks the current spec (`2026-07-28`) and falls back to the older `initialize` handshake for servers still on `2025-11-25` back to `2024-11-05`. A new server stays off until you read its **trust preview**: the command, its environment, every tool and its description, and anything suspicious flagged (invisible characters, hidden-instruction tags, text that hides actions from you or reaches for secrets). Each tool has its own switch and approval (Ask, Allow, Deny) behind the policy gate, and its output reaches the model as untrusted data that cannot give instructions. Health checks start, ping, and list each server and time each step. `tern mcp add\|import\|list\|preview\|enable\|disable\|health\|rm`; the desktop **Customize → MCP servers** tab with health dots |
| ACP (editors) | `tern acp` runs Tern as an agent inside editors that speak the Agent Client Protocol (v1), such as Zed: the editor's folder becomes the project (its `TERN.md` and skills apply), replies and tool steps stream into the editor, past chats load again, and approvals appear as the editor's permission prompt. Setup: [docs/zed.md](docs/zed.md) |
| Skills | Folders with a `SKILL.md` (yours in Tern's data folder, a project's in `.tern/skills/`). Only names and descriptions go into the prompt; the model loads a skill's full instructions when a task needs it. Install from a folder or a git URL after a preview with the same suspicious-text scan, switch on and off. `tern skills list\|install\|enable\|disable\|show\|rm`; **Customize → Skills** |
| Rules and instructions | `TERN.md` files (yours, folders above the project, and the project's own) merged in order with a later file winning. **Effective instructions** shows exactly what the model is told, piece by piece. `tern instructions`; **Customize → Instructions** |
| Project folders | Projects hold chats and their sub-sessions; a project can link a folder on disk so its `TERN.md` and skills apply to its chats. Deleting a project moves its chats to the Inbox. `tern project new\|list\|mv\|rename\|root\|rm`; the desktop sidebar's project tree (drag a chat onto a folder or Move to…, collapse saved) |
| Terminal | `tern` / `tern chat`: an 80×24 ratatui chat |
| Desktop app | Windows build on GPUI: chat with streaming answers, chat list (reopen, rename, archive), providers screen, source rows with site icons, context ring, run panel (Summary, Detailed, Raw, Report, Ledger, Workers for deep research, and Decisions), a Decision model page (Off, Local, or Remote; Jev, SubQ Snap, and Voyage rows with their logos and keys; a Test button), a plan card for deep research (Approve, Edit, Skip plan, Cancel), steering from the composer while a run goes, a Resume banner for a stopped run, a Watches list in the sidebar with a Watches screen showing what changed, verdict badges on cited claims, attachments (files, folders, nested `.zip`, long paste), `@` references (attachments and sources, `@prompt:` / `@context:`), message actions (copy, reply with a quote, branch into a new chat, `Branch ▾` for sub-sessions and background branches, ask in side chat), message IDs with multi-select and range actions (move to a sub-session or a new session, remove, confirm or undo, the chat's trash), a sidebar sub-session tree (`↳ Q12` rows, planned and running state) and a handoff action, a highlight popover on answer text, the side chat (docked or popped out with the tern mark), the Library page (Reports, Sources, Notes, Decks, Files, Explain in 3D, Sessions, All, and Trash; grid or list; `New ▾`; search, sort, bulk select, paging), project folders in the sidebar (Projects, chats, `↳` sub-sessions, the Inbox; drag to file, collapse saved), the Customize page (Skills, MCP servers with the trust preview and health dots, Providers, Instructions), session tabs, a responsive layout (panels float over the chat on narrow windows), the locked design (palette C Mono & Lichen, IBM Plex Sans and Mono, Instrument Serif wordmark, Tern glyphs and built-in service logos, an Appearance screen) and Scout, a live mascot rig in the sidebar that reacts to the run, Git for the vault (checkpoints, branches, pull/push, open a pull request on GitHub, baseline files, branch chip in the title bar), a built-in browser (pages as readable Markdown beside the chat, citations and sources open there, Ask about this page), the Editor and vault (Markdown notes, `[[links]]` and backlinks, autosave, Save to vault, agent vault tools), and the motion pass (working line with the moving tern mark and shimmer, sliding panels, the soft light/dark switch), and the Study harness (a Study home with materials and their icons, topics with mastery rings, Tutor, Quiz, Viva, and Flashcards with the due count; a tutor chat with mode chips; a quiz card with instant feedback and a score summary; a viva screen with a timer and the rubric report; a flashcard review and deck screen; Study settings), and Explain in 3D tabs |

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
| Models | More OpenAI-compatible endpoints and capability probing (13 providers, local Ollama and LM Studio, and keychain storage are built) |
| Decision model | A small local cross-encoder for claim checks, and measured numbers for the chat model and the remote decision models on real runs (the local scorer, the Jev, SubQ Snap, and Voyage adapters, and `tern decide bench` are built) |
| Control | Safe pause at budget limits, drift detection, and a scheduler that runs watches while Tern is closed (loop detection, steering, run checkpoints, and the in-app watcher are built) |
| Memory | Stable IDs for files and sources, and a goal and decision ledger that survives compaction (message IDs, sub-IDs, and handoff packets are built) |
| Extensibility | MCP prompts and resources (MCP tools, the ACP server, skills, `TERN.md` rules, and project folders are built) |
| Workspace | The Study playground built on Explain in 3D (Explain in 3D and the built-in browser are built) |
| Vault | Agent edits shown as diffs (plain Markdown notes, `[[links]]`, backlinks, and Git for the vault are built) |
| Study | Study plans, and FSRS scheduling once a deck has enough reviews to fit it (flashcards with SM-2 and Anki export, and Explain in 3D, are built) |
| Library | A model-written handoff summary (chat ranges, sub-sessions, handoffs, the Library with bulk delete, highlights, the side chat, and project folders are built) |
| Evaluation | Benchmarks for citation accuracy, cost, and reliability |
| Platforms | Linux and macOS builds |

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

## Architecture

A single Rust workspace. The headless core owns execution and security; the TUI and desktop app subscribe to its events.

| Crate | Role |
| --- | --- |
| `tern-core` | Shared types, stable IDs, versioned events, turn state machine, secret redaction |
| `tern-agent` | Turn engine, planner, tool registry, policy gate, workers, watcher, steering, checkpoints, budgets, message IDs, range cleaning, sub-sessions, handoff packets, Library sync, side chat, MCP client, ACP server, skills, `TERN.md` rules |
| `tern-providers` | BYOK remote providers and local models, capability probing, usage accounting, MCP's HTTP transport |
| `tern-decision` | The decision model: `score(context, question, choices)`, a local offline scorer, Jev, SubQ Snap, and Voyage adapters, hard checks, cache, decision log, benchmark fixtures |
| `tern-crawl` | Fetch, extract, and chunk web pages and PDFs, behind an SSRF and DNS-rebinding guard |
| `tern-store` | SQLite, append-only event log, sub-sessions, project folders, the Library index (search, filter, sort, paging, trash), Anki package export; full-text and vector indexes next |
| `tern-research` | Research pipeline, scholarly sources, claim verification, citations, run reports |
| `tern-study` | Materials and anchors, topics, mastery, tutor, quizzes and grading, viva practice, flashcards and SM-2 scheduling, CSV and TSV, Explain in 3D scenes and their projection; study plans next |
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
| 1 | Design lock: palette, fonts, concept screens, SVG mark | Done |
| 2 | Core skeleton, keychain, store, `tern doctor` | Done |
| 3 | Providers and the first cited answer in the TUI | Done |
| 4 | Crawler and claim verification | Mostly done |
| 5 | Deep research: plans, workers, checkpoints, steering | Mostly done |
| 6 | Desktop app | In progress |
| 7 | Study harness | Mostly done |
| 8 | Decision model | Mostly done |
| 9 | Continuity and library | Mostly done |
| 10 | Extensibility: MCP, ACP, skills | Mostly done |
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
│   ├── zed.md            using Tern as an ACP agent in Zed
│   └── decisions/        architecture decision records (0001 to 0007)
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
| `cargo run -p tern-tui -- research --deep "your question"` | Deep research: a plan to approve, then parallel workers |
| `cargo run -p tern-tui -- report` | Prints the last run report (`--json`, `--out FILE`, `--list`) |
| `cargo run -p tern-tui -- resume <chat>` | Resumes a stopped research run |
| `cargo run -p tern-tui -- watch add "question" --every weekly` | Puts a question on watch (`watch list\|rm\|run`) |
| `cargo run -p tern-tui -- study add notes.pdf` | Adds study materials (`study topics`, `tutor`, `quiz`, `viva`) |
| `cargo run -p tern-tui -- cards gen` | Writes flashcards from your materials (`cards review`, `list`, `stats`, `edit`, `suspend`, `rm`) |
| `cargo run -p tern-tui -- cards export --apkg deck.apkg` | Exports the deck for Anki (`--csv FILE`, `--tsv FILE`; `cards import FILE` reads CSV or TSV) |
| `cargo run -p tern-tui -- explain3d "concept" --svg out.svg` | Builds an Explain in 3D scene and writes a snapshot (`--topic`, `--step N`, `--vault`) |
| `cargo run -p tern-tui -- decide "Which source first?" --choices a,b,c` | Asks the decision model (`--context FILE`, `--kind source\|tool\|claim\|triage`, `--json`) |
| `cargo run -p tern-tui -- decide mode remote --provider typesafe` | Chooses the decision model (`off`, `local`, `remote`; `decide key add typesafe\|subq\|voyage`, `decide status`) |
| `cargo run -p tern-tui -- decide bench` | Scores the bundled fixtures: fixed order and the local scorer (`--reasoning` adds the chat model, `--remote` the remote decision model) |
| `cargo run -p tern-tui -- range rm Q3-Q5` | Removes messages to the chat's trash (`range mv Q3-Q5 [--sub]`, `confirm`, `undo`, `restore`, `trash`; `--chat <id>`) |
| `cargo run -p tern-tui -- sub new Q12 "question"` | Plans a sub-session under Q12 (`sub list`, `sub run Q12.1` runs it and reports back) |
| `cargo run -p tern-tui -- show Q12.3` | Prints a message by its ID or `tern://` link |
| `cargo run -p tern-tui -- handoff --out packet` | Writes `packet.md` and `packet.json`; `new --from-handoff packet.md` starts a session from it alone |
| `cargo run -p tern-tui -- library list` | Lists the Library (`--kind`, `--sort`, `--page`, `--trash`; `library search`, `rm`, `restore`, `empty-trash`) |
| `cargo run -p tern-tui -- side "question"` | A throwaway question about the last chat; nothing is saved |
| `cargo run -p tern-tui -- mcp add notes -- npx -y notes-mcp` | Adds an MCP server, off until reviewed (`--url` for HTTP; `mcp import FILE`, `list`, `preview NAME --trust`, `enable`, `disable`, `health`, `rm`) |
| `cargo run -p tern-tui -- acp` | Serves the Agent Client Protocol on stdio for an editor (see [docs/zed.md](docs/zed.md)) |
| `cargo run -p tern-tui -- skills list` | Lists skills (`install PATH\|URL`, `enable`, `disable`, `show`, `rm`) |
| `cargo run -p tern-tui -- instructions` | Prints exactly what the model is told, piece by piece |
| `cargo run -p tern-tui -- project new "Thesis" --root D:/thesis` | Makes a project folder (`project list`, `mv <chat> <project>`, `rename`, `root`, `rm`) |
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
