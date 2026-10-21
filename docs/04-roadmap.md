# 04 · Roadmap

> Status: draft, 21 October 2026. Every phase after phase 0 is **planned**. Each phase starts only when the owner approves it. Dates are not committed; order is.

## Phase summary

| Phase | Name | Outcome | Status |
| --- | --- | --- | --- |
| 0 | Repository baseline | Licence, docs, brand assets, process files | Done (PR #1) |
| 1 | Design lock | Palette, fonts, concept screens, SVG mark, Scout rig sketch | Planned |
| 2 | Core skeleton | Cargo workspace, `tern-core` contracts, `tern-platform` keychain on Windows, `tern-store` basics, `tern doctor` | Done |
| 3 | Providers and first slice | BYOK and local models; a quick cited answer end to end in the TUI | In progress |
| 4 | Crawler and verification | `tern-crawl` with the network guard; claim verdicts; run report | Planned |
| 5 | Deep research | Plan-first runs, workers, checkpoints, budget pause, steering, watcher | Planned |
| 6 | Desktop app | GPUI shell, transcript, right rail, vault editor | Planned |
| 7 | Study harness | Tutor, flashcards, quizzes, viva practice | Planned |
| 8 | Decision model | Decision-model slot wired into source ranking, tool choice, and claim checks | Planned |
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
| Palette choice (C, A, or B) and font choice | Recorded as ADR 0002 |
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
| `tern-providers`: a first set of remote providers plus Ollama and LM Studio; capability probing; usage ledger | Paste a key, test, list models |
| Minimal `tern-agent` turn loop with the policy gate | Bounded loop; approvals recorded |
| Minimal TUI: composer, transcript, footer | Works at 80×24 in Windows Terminal |
| First slice: `tern ask` with web search and a cited answer | Every citation links to a fetched source |

### Phase 4 · Crawler and verification

| Deliverable | Exit check |
| --- | --- |
| `tern-crawl`: fetch, robots, rate limits, cache, content router, readability, Markdown, anchors | Fixture pages parse to stable anchors |
| Network guard: private, loopback, link-local, metadata ranges blocked; redirects re-checked; DNS pinning | SSRF test matrix passes |
| PDF and scholarly sources | arXiv, DOI, open-access PDF paths work |
| Claim extraction and verdicts | Seeded fixtures: supported, contradicted, misquoted, retracted, unreachable |
| Run report and methods log | One screen per run, saved to the vault |

### Phase 5 · Deep research

| Deliverable | Exit check |
| --- | --- |
| Clarify and plan with an editable plan and preflight cost range | No paid step runs before approval |
| Workers per sub-question with their own budgets | Only final reports return to the parent |
| File intake: any parseable file, images by model capability, archives unpacked and folders read recursively in order | A nested zip of mixed files is read completely and in a stable order |
| Checkpoints, safe pause, resume | Resume after a forced quit without repeating paid work |
| Live steering and the watcher | Steer lands at the next safe point |
| Parallel sub-researchers with a Subagents tab | Two workers run at once without sharing budget or context |
| Loop and drift detection | Synthetic loops are paused; legitimate pagination is not |
| Goal and decision ledger with loss-aware compaction | An early constraint survives several compactions |

### Phase 6 · Desktop app

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

### Phase 8 · Decision model

| Deliverable | Exit check |
| --- | --- |
| `tern-decision` interface and a local reranker fallback | Works offline |
| First remote decision-model integration, after verifying its API and pricing | Behind BYOK like any provider |
| Wired into source ranking, next-tool choice, claim pass or fail, now-or-later | Measured against the reasoning-model-only baseline |

### Phase 9 · Continuity and library

| Deliverable | Exit check |
| --- | --- |
| Clean a chat by ID range: remove, or move to a new session with context | Context carries over; original preserved until confirmed |
| Planned sub-sessions; background branching that reports back | Sub-IDs (`Q12.3`) resolve everywhere |
| Session handoff packets | New session starts from the packet alone |
| Library of everything; bulk delete | Thousands of items remain responsive |
| Highlight actions and throwaway side chat | Side chat writes nothing back |

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
