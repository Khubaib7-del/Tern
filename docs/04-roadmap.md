# 04 · Roadmap

> Status: draft, 21 October 2026. Every phase after phase 0 is **planned**. Each phase starts only when the owner approves it. Dates are not committed; order is.

## Phase summary

| Phase | Name | Outcome | Status |
| --- | --- | --- | --- |
| 0 | Repository baseline | Licence, docs, brand assets, process files | Done (PR #1) |
| 1 | Design lock | Palette, fonts, concept screens, SVG mark, Scout rig sketch | Done (PR #30; Tern Crawl logo PR #32) |
| 2 | Core skeleton | Cargo workspace, `tern-core` contracts, `tern-platform` keychain on Windows, `tern-store` basics, `tern doctor` | Done |
| 3 | Providers and first slice | BYOK and local models; a quick cited answer end to end in the TUI | Done |
| 4 | Crawler and verification | `tern-crawl` with the network guard; claim verdicts; run report | Mostly built (PRs #31 to #33); scholarly sources and reports in the vault remain |
| 5 | Deep research | Plan-first runs, workers, checkpoints, budget pause, steering, watcher | Mostly built (PR #34: plans and parallel workers; PR #35: checkpoints, steering, the in-app watcher, ledgers); budget pause and an OS scheduler remain |
| 6 | Desktop app | GPUI shell, transcript, right rail, vault editor | In progress (shell brought forward, ADR 0006) |
| 7 | Study harness | Tutor, flashcards, quizzes, viva practice | In progress (PR #36: materials, topics, mastery, tutor, quizzes, viva); flashcards with Anki export and Explain in 3D next |
| 8 | Decision model | Decision-model slot wired into source ranking, tool choice, and claim checks | Mostly built (PR #38: local scorer, Jev, SubQ Snap, and Voyage adapters, wiring, `tern decide bench`); the reasoning-model and remote benchmark rows need keys |
| 9 | Continuity and library | Chat range clean and move, sub-sessions, handoff, library, bulk delete | Planned |
| 10 | Extensibility | MCP client, ACP server, skills, project rules | Planned |
| 11 | Evaluation | Benchmarks for citation accuracy, cost, and reliability | Planned |
| 12 | Linux and macOS | Ports through `tern-platform` | Planned |

Distribution (installers, releases, auto-update, a website) is deliberately not on this list. It will be planned after phase 12, or earlier only if the owner decides.

## Phase detail

### Phase 0 · Repository baseline

| Deliverable | Exit check |
| --- | --- |
| Proprietary licence with brand clause | `LICENSE` on `main` |
| README, docs 00–05, ADR 0001 | Docs agree with each other; nothing described as shipped |
| Security policy, contributing guide, code of conduct, agent rules, changelog | Present and linked from the README |
| Editor and git config, issue forms, PR template, code owners | Present |
| Brand assets and README hero | In `assets/brand/` |

### Phase 1 · Design lock

| Deliverable | Exit check |
| --- | --- |
| Palette choice (C, A, or B) and font choice | Done: palette C, IBM Plex, ADR 0002 |
| Concept screens in the chosen palette: desktop, TUI, Study, approval, budget pause | Approved by the owner |
| SVG trace of the mark and wordmark; small-size mark | In `assets/brand/` |
| Scout rig parts and state table | Sketch approved |

### Phase 2 · Core skeleton

| Deliverable | Exit check |
| --- | --- |
| Cargo workspace with all eleven crates as stubs and the dependency rules enforced | `cargo build` and `cargo clippy -D warnings` pass on Windows |
| `tern-core`: IDs, event envelope, turn state machine, effect classes, redactor | Unit tests, including redaction of key-shaped strings |
| `tern-platform`: keychain on Windows Credential Manager, app data paths | Store, read, and delete a test key |
| `tern-store`: SQLite schema for projects, sessions, append-only event log | Replay from a sequence number in tests |
| `tern doctor` | Reports keychain, data directory, and disk state in plain language |
| CI on Windows | fmt, clippy, test on every PR |

### Phase 3 · Providers and first slice

| Deliverable | Exit check |
| --- | --- |
| `tern-providers`: a first set of remote providers plus Ollama and LM Studio; capability probing; usage ledger | Paste a key, test, list models (done: PR #5) |
| Minimal `tern-agent` turn loop with the policy gate | Bounded loop; approvals recorded (done: ADR 0004) |
| Minimal TUI: composer, transcript, footer | Works at 80×24 in Windows Terminal (built; Windows Terminal check by the owner) |
| First slice: `tern ask` with web search and a cited answer | Every citation links to a fetched source (built, ADR 0005; invalid citations are reported) |

### Phase 4 · Crawler and verification

| Deliverable | Exit check |
| --- | --- |
| `tern-crawl`: fetch, robots, rate limits, cache, content router, readability, Markdown, anchors | Fixture pages parse to stable anchors |
| Network guard: private, loopback, link-local, metadata ranges blocked; redirects re-checked; DNS pinning | SSRF test matrix passes |
| PDF and scholarly sources | arXiv, DOI, open-access PDF paths work |
| Claim extraction and verdicts | Seeded fixtures: supported, contradicted, misquoted, retracted, unreachable (built, PR #33: supported, partly, not supported, no source; model judge with an offline word-overlap fallback; retraction checks remain) |
| Run report and methods log | One screen per run, saved to the vault (built, PR #33: Report view in the run panel, Markdown and JSON per turn in the data folder, `tern report`; saving into the vault remains) |

### Phase 5 · Deep research

| Deliverable | Exit check |
| --- | --- |
| Clarify and plan with an editable plan and preflight cost range | No paid step runs before approval (built, PR #34: the model drafts a typed plan with sub-questions, searches, source types, and budget shares, or Tern splits the question itself; run, edit, skip, or cancel it; a preflight token range is shown. Drafting the plan is the one model call before approval; no search, read, or worker runs until the user says so. Clarifying questions remain) |
| Workers per sub-question with their own budgets | Only final reports return to the parent (built, PR #34: each worker gets its own slice of tokens, pages, and time and hands back only its notes and their pages) |
| File intake: any parseable file, images by model capability, archives unpacked and folders read recursively in order | A nested zip of mixed files is read completely and in a stable order |
| Checkpoints, safe pause, resume | Resume after a forced quit without repeating paid work (built, PR #35: run checkpoints at every safe point of quick and deep runs; a killed run resumes without re-running finished workers or re-reading their pages, tested by dropping a run mid-flight; `tern resume`, desktop Resume banner. Safe pause at budget limits remains) |
| Live steering and the watcher | Steer lands at the next safe point (built, PR #35: notes guide a run or drop, add, and redirect deep workers at the next safe point, with `steer.received`/`steer.applied`; watched questions re-run while the app is open or by `tern watch run`, diffed against the last report. Source and budget ledgers in every report. An OS-level scheduler remains) |
| Parallel sub-researchers with a Subagents tab | Two workers run at once without sharing budget or context (built, PR #34: up to 3 at once by default, 1 to 6 in preferences, each cancellable; desktop Workers view in the run panel, `tern research --deep` in the terminal) |
| Loop and drift detection | Synthetic loops are paused; legitimate pagination is not |
| Goal and decision ledger with loss-aware compaction | An early constraint survives several compactions |

### Phase 6 · Desktop app

Started early (ADR 0006): the first slice is the window shell on the phase 3 chat engine, built for Windows by CI on every PR.

| Deliverable | Exit check |
| --- | --- |
| GPUI shell: title bar, icon rail, sidebar, harness toggle, session tabs, transcript, composer, right rail | Matches [02-ui-definition.md](02-ui-definition.md) |
| Responsive composer | Nothing overflows at 1280, 1440, and 1920px with both side panels open |
| Motion and theme switch | Panel motion on one curve at 60 fps; the circular light/dark wipe matches the concept video |
| Providers screen, model picker, context ring | Rows with logos, `Add` and toggles; context popover matches the sidebar view |
| Working line and message actions | Mark animates per real event; swipe or button reply, copy, multi-branch |
| Composer input | Attachments, long paste to `.txt`, `@` prompt or context roles, locked mic with tooltip |
| Built-in browser tab | Page text is isolated from instructions; network guard applies |
| Git and GitHub panel | Commit, branch, push, and open a PR from a project vault |
| Summary, Detailed, and Raw views of the same run | Same event stream renders all three |
| Vault editor: Markdown, wikilinks, backlinks, outline, agent edits as diffs | Vault opens in Obsidian unchanged |
| Scout states driven by real events | Reduced-motion still frame works |

### Phase 7 · Study harness

| Deliverable | Exit check |
| --- | --- |
| Tutor grounded in project sources | Answers cite the library |
| Flashcards with spaced repetition and Anki export | Deck imports into Anki |
| Quizzes graded with the verdict vocabulary | Deterministic grading where an answer key exists |
| Viva practice on the user's own report | Follow-up questions reference report sections |

Built in PR #36: study materials (files, folders, PDFs with page anchors, vault notes, research reports, links) split into topics with learning objectives and an explainable mastery model (an Elo-style rating and a Leitner box per topic); the tutor (Explain, Socratic, Explain like I'm new) citing material anchors and flagging anything beyond them; quizzes (multiple choice, true or false, fill in the blank, short answer) graded exactly against the key, or by the model judge with a word-overlap fallback, in the verdict glyphs; and viva practice with levelled questions, follow-ups, and a rubric report. Built in PR #37: flashcards (basic, cloze, and "explain why", each with its source anchor, written by the model or offline from the materials, deduplicated, editable, suspendable) scheduled with SM-2 (Anki's scheduler, so an exported deck keeps its schedule; FSRS is left until a deck has the reviews to fit it), a daily review queue and deck statistics, reviews nudging topic mastery; a real Anki `.apkg` export (verified by importing it with Anki's own collection library), CSV and Anki-ready TSV export, and CSV and TSV import; and Explain in 3D, a typed isometric scene with ordered, cited steps, drawn in the desktop app as a session tab (solid faces with a hairline overlay, pointer tilt and hover, step-through with autoplay) and saved as JSON, an SVG snapshot, or a vault note. Study plans, and the Study playground built on Explain in 3D, remain.

### Phase 8 · Decision model

| Deliverable | Exit check |
| --- | --- |
| `tern-decision` interface and a local reranker fallback | Works offline (built, PR #38: `local-lexical-1`, BM25 and per-kind rules, deterministic, no download; a local cross-encoder remains) |
| First remote decision-model integration, after verifying its API and pricing | Behind BYOK like any provider (built, PR #38: TypeSafe Jev and Subquadratic SubQ Snap on one System One adapter, Voyage rerank for ranking; keys tested before saving; API and prices checked on 23 October 2026, ADR 0007. Not yet run against the live services from CI) |
| Wired into source ranking, next-tool choice, claim pass or fail, now-or-later | Measured against the reasoning-model-only baseline (built, PR #38: ranking in quick asks, chat search, and deep workers; a first-tool hint; a claim pre-score under hard checks with the model judge keeping the final word; deep worker and watch order. `tern decide bench` on 50 bundled fixtures: fixed order 52%, local scorer 86%, in-sample. The reasoning-model row (`--reasoning`) and the remote row (`--remote`) need keys and have not been run) |

### Phase 9 · Continuity and library

| Deliverable | Exit check |
| --- | --- |
| Clean a chat by ID range: remove, or move to a new session with context | Context carries over; original preserved until confirmed (built, PR #39: `tern_agent::range`. Remove appends `range.removed` and the turns go to the chat's trash, restorable; a move copies the turns into a new session or a sub-session with a `session.context` summary of the turns before them, and the original is untouched until Confirm (the originals then go to the chat's trash) or Undo (the copy is archived). The log stays append-only and the turns that stay keep their numbers. Tested in `range.rs`) |
| Planned sub-sessions; background branching that reports back | Sub-IDs (`Q12.3`) resolve everywhere (built, PR #39: schema v3 adds a session's parent, anchor message, and planned question; `tern_agent::sub` makes, lists, and runs sub-sessions and posts a summary back to the parent as `sub.reported`, which the parent's transcript shows and its model reads; `tern_agent::refs` numbers sub-session messages across all sub-sessions of one parent message in start order and resolves IDs in `@` mentions, the CLI, `tern://chat/<id>/Q12.3` links, and Library search. Tested in `refs.rs`, `sub.rs`, and `tests/continuity.rs`) |
| Session handoff packets | New session starts from the packet alone (built, PR #39: `tern_agent::handoff` drafts goal, decisions, open questions, key sources and claims, a ledger snapshot, and next steps as Markdown and JSON; `tern new --from-handoff` starts a session whose only event is the packet. `tests/continuity.rs` starts one in a fresh database from the Markdown alone and checks the model reads the packet and nothing of the old chat) |
| Library of everything; bulk delete | Thousands of items remain responsive (built, PR #39: one index in `tern_store::library` over reports, sources, notes, decks, files, Explain-in-3D scenes, and sessions, with search, filter, sort, and paging in SQLite; bulk delete to a restorable trash, empty-trash. A 5,000-item test indexes, queries three ways, and bulk-moves in well under the bounds; the desktop page reads 60 rows at a time) |
| Highlight actions and throwaway side chat | Side chat writes nothing back (built, PR #39: Explain, Verify claim, Add to notes, Make flashcard, and Ask in side chat on selected answer text; `tern_agent::side` reads the thread and keeps its exchanges in memory. Tested: the parent's log and title are unchanged after side questions) |

Built in PR #39: message IDs on the transcript with multi-select, range actions with Confirm and Undo, a `Branch ▾` menu (new sub-session from here, run the composer text in a background branch, move to a sub-session), a sidebar sub-session tree, handoff, the Library page and trash, the highlight popover, and the side chat docked or popped out. Folders inside Projects in the sidebar (projects are still one Inbox) and a model-written handoff summary remain.

### Phase 10 · Extensibility

| Deliverable | Exit check |
| --- | --- |
| MCP client with trust preview and health checks | MCP output treated as untrusted data |
| ACP server so Tern runs inside compatible editors | Session round-trip works |
| Skills loaded on demand; project rules in `TERN.md` | Effective instructions visible to the user |

### Phase 11 · Evaluation

| Deliverable | Exit check |
| --- | --- |
| Test ladder: unit, synthetic, pinned real tasks, public benchmark, human usability | Each level runnable from one command |
| Metrics: claim precision and recall, unsupported claims per 100 runs, citation accuracy, constraint retention, forecast error, cost per verified report | At least three trials, confidence intervals, failures published |

### Phase 12 · Linux and macOS

| Deliverable | Exit check |
| --- | --- |
| `tern-platform` for Linux (Secret Service, XDG paths) | CI green on Linux |
| `tern-platform` for macOS (Keychain) | CI green on macOS |

## Later and parked

| Item | Status |
| --- | --- |
| Import chats from other assistants with provenance | Later |
| Universal connector for services without MCP | Later |
| Learning from frustration signals as reviewable preferences | Later |
| Skill discovery and repository reading without cloning | Later |
| Lecture video and slide intake | Later |
| Interactive explainer export | Later |
| Explain in 3D mode | Planned with the Study harness (phase 7) |
| Voice input and a Tern transcription model | After v1; the mic ships locked until then |
| Computer use, scoped to research chores | Later, off by default |
| Shared study and research rooms | Parked |
| Hardware research pack | Parked |
