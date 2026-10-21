# 03 · Architecture

> Status: phase 3 in progress, 21 October 2026. All eleven crates exist; `tern-core`, `tern-platform` (Windows keychain), `tern-store`, `tern-providers`, and in `tern-agent` the doctor, provider setup, bounded turn loop, policy gate, tool registry, and chat session have code and tests, as does the minimal ratatui TUI, and the rest are stubs. Chosen so far: `rusqlite` (bundled SQLite), `keyring-core` with `windows-native-keyring-store`, `directories`, `fs4`, `ulid`, `time`, `regex`, `clap`, `reqwest` (rustls), `tokio`, `rpassword`, `ratatui` 0.30 with its `crossterm` backend; own provider adapters per [ADR 0003](decisions/0003-own-provider-adapters.md); turn loop and policy gate per [ADR 0004](decisions/0004-turn-loop-and-policy-gate.md). Other library names are candidates to evaluate in the phase that needs them, not commitments. The founding decision is [ADR 0001](decisions/0001-rust-gpui-windows-first.md).

## 1. Shape

One Rust workspace. A headless core owns state, execution, and security. The TUI and the desktop app are clients that subscribe to the core's events and send it commands; neither runs agent logic of its own.

```text
            tern-tui (CLI + TUI)        tern-desktop (GPUI)        ACP clients (editors)
                     │                          │                          │
                     └──────── commands ▼  ▲ versioned events ─────────────┘
                                        │
   ┌────────────────────────────────── tern-agent ───────────────────────────────────┐
   │ turn engine · planner · tool registry · policy gate · workers · watcher · steering │
   │ checkpoints · budget & usage · loop / drift detection · MCP client · ACP server     │
   └───────┬──────────────┬───────────────┬──────────────┬───────────────┬─────────────┘
           │              │               │              │               │
     tern-research   tern-study     tern-providers  tern-decision    tern-crawl
     (pipeline,      (tutor, cards, (BYOK + local    (choice scorer)  (fetch, extract,
      citations)      quizzes, viva) models)                           network guard)
           │              │               │              │               │
           └──────────────┴──────── tern-store ─────────┴───────────────┘
                     (SQLite, full-text, vectors, event log, artifacts, vault)
                                        │
                     tern-core (types, IDs, events, state machine, redaction)
                     tern-platform (keychain, paths, OS integration)
```

## 2. Crates

| Crate | Responsibility | Candidate dependencies |
| --- | --- | --- |
| `tern-core` | Shared types and contracts: stable IDs (`Q12`, `Q12.3`, source and claim IDs), the versioned event envelope, the turn state machine, effect classes, verdict and status enums, config schema, error types, and the secret redactor | `serde`, `thiserror`, `uuid` or `ulid`, `time` |
| `tern-agent` | The turn engine: bounded model and tool loop, planner, typed tool registry, policy gate, worker and watcher agents, live steering, checkpoints, budget and usage ledger, loop and drift detection. Hosts the MCP client and ACP server until they need their own crates | `tokio`, `schemars`, `jsonschema`, `rmcp`, `agent-client-protocol` |
| `tern-providers` | BYOK provider layer for remote and local models: capability probing, model listing, role assignment, streaming, token accounting. No silent provider fallback | Own adapters ([ADR 0003](decisions/0003-own-provider-adapters.md)), `reqwest`, `eventsource-stream` |
| `tern-decision` | The decision-model slot: `score(context, question, choices) → probabilities`. Remote decision-model APIs and a local cross-encoder or reranker fallback. Hard checks always override a score | `reqwest`; local inference crate to evaluate |
| `tern-crawl` | Tern's own reader and crawler: URL frontier, robots and rate limits, fetch with caching, SSRF and DNS-rebinding guard, content router (HTML, PDF, arXiv, DOI), headless-browser fallback for empty JS pages, readability extraction, HTML to Markdown, chunking with stable anchors, near-duplicate detection | `reqwest`, `hickory-resolver`, `scraper`, `pdfium-render`, `chromiumoxide` |
| `tern-store` | Persistence: SQLite for projects, sessions, ledgers, and the append-only event log; full-text index; vector index; hashed artifacts; checkpoints; the Markdown vault on disk | `rusqlite`, `tantivy`, `sqlite-vec` or LanceDB, `notify` |
| `tern-research` | The Research harness: clarify and plan, search, read, verify, write, record. Scholarly API clients, claim extraction and verification, evidence tables, citation formatting, run report, export | `hayagriva` or `citeproc`; pandoc detected, not bundled |
| `tern-study` | The Study harness: tutor, flashcards with spaced repetition, quizzes, viva practice, study plans; grading with the shared verdict vocabulary; Anki export | — |
| `tern-tui` | The `tern` binary: CLI commands (`tern ask`, `tern doctor`, `tern run --json`), the ratatui TUI, terminal capability probing | `clap`, `ratatui`, `crossterm` |
| `tern-desktop` | The GPUI desktop app: shell, sessions, transcript, right rail, vault editor, Scout | `gpui` via a pinned component kit |
| `tern-platform` | OS integration behind traits so ports stay small: keychain, app data paths, file dialogs, notifications, process spawning. Windows implementation first | `keyring`, `directories`, `windows` |

## 3. Dependency rules

| Rule | Why |
| --- | --- |
| `tern-core` depends on no other Tern crate and on no network, UI, provider, or database library | Keeps the contracts portable and testable |
| `tern-platform` depends only on `tern-core` | OS code stays in one place for porting |
| `tern-tui` and `tern-desktop` depend on `tern-agent` and `tern-core` only; they never call providers, the crawler, or the store directly | UIs are subscribers; the GPUI pre-1.0 risk stays out of the engine |
| `tern-research` and `tern-study` never import each other | Harnesses share the core, not each other |
| Only `tern-providers` talks to model APIs; only `tern-crawl` fetches arbitrary URLs | One place to enforce keys, budgets, and the network guard |
| Every side effect passes the policy gate in `tern-agent` | The model never holds permissions |

## 4. Runtime contracts

### 4.1 Event envelope

Every state change is an event. The log is append-only, strictly ordered per session, and replayable from any acknowledged sequence number. UIs may drop animation frames but never semantic events. Secrets are redacted before an event is persisted.

| Field | Meaning |
| --- | --- |
| `version` | Envelope schema version |
| `event_id` | Unique ID |
| `session_id`, `turn_id` | Where it belongs |
| `seq` | Monotonic per session |
| `ts` | Timestamp, UTC |
| `type` | For example `plan.created`, `source.fetched`, `claim.extracted`, `claim.verified`, `approval.requested`, `usage.update`, `checkpoint.created`, `loop.suspected`, `run.paused` |
| `user_summary` | One plain-language line for the Summary view |
| `effect_class` | Read, analysis, write, network, credentials, external side effect, destructive |
| `evidence_refs` | IDs of sources, spans, and artifacts |
| `usage_delta` | Tokens and cost, each marked measured or estimated |
| `payload` | Typed body |

### 4.2 Turn state machine

```text
Created → Preparing → ModelStreaming → ToolRequested → PolicyCheck → (ApprovalRequested)
        → Executing → RecordResult → … → Verifying → Completed
Terminal: Completed · CompletedWithGaps · Cancelled · Failed · Blocked · BudgetExceeded
```

For Research, `Verifying` is the claim-to-source check. For Study, it is the answer-key or rubric check. Paused is not completed, and cancelled is not rolled back.

### 4.3 Tool contract

| Field | Meaning |
| --- | --- |
| Name and version | Stable identity |
| Input and output schema | Generated JSON Schema, validated on both sides |
| Human summary | What it does, in one line |
| Effect class and scope | What it can touch |
| Timeout and idempotency | Whether a retry is safe |
| Normalised errors | Output truncation never hides an error |

### 4.4 Worker contract

| In | Out |
| --- | --- |
| Task, references by ID, forbidden scope, permissions, budget, output schema, stop condition | Summary, evidence references, assumptions, usage |

Only a worker's final report returns to the parent context.

## 5. Context and memory

| Layer | Contents | Trust |
| --- | --- | --- |
| Context window | The current turn | Assembled per turn from IDs |
| Session summary | Rolling summary citing event IDs | Rebuilt after compaction |
| Goal and decision ledger | Goals, constraints verbatim, non-goals, decisions, rejected options | User-owned; survives compaction |
| Project memory | Facts and preferences for the project | Tagged observed, user decision, inference, temporary, sensitive |
| Library retrieval | Full-text and vector search over sources | Untrusted content; never instructions |
| Global profile | Cross-project preferences | Inspectable and deletable |

Compaction thresholds are per model. The log is persisted first, the summary is split into verified facts, decisions, assumptions, failures, done, and next, and the user is told when it happened.

## 6. Storage layout (planned)

| Location | Contents |
| --- | --- |
| OS keychain | Provider and search API keys |
| App data directory (per OS, via `tern-platform`) | SQLite database, indexes, artifact store, crawl cache, checkpoints |
| Vault directory (user-chosen) | Plain Markdown notes and reports, Obsidian-compatible; optional git-backed history |

## 7. Platform order

| Order | Platform | Notes |
| --- | --- | --- |
| 1 | Windows 10 and 11 | First target for the TUI and desktop app. Windows Terminal is the reference terminal. Credential Manager for keys |
| 2 | Linux | Secret Service for keys; X11 and Wayland for GPUI |
| 3 | macOS | Keychain; GPUI's most mature backend |

Porting cost is kept small by putting every OS difference behind `tern-platform` traits and testing path, shell, and line-ending assumptions on Windows first.

## 8. Design references, not dependencies

| Reference | What Tern takes | What Tern does not take |
| --- | --- | --- |
| Open-source desktop agent harnesses | Bounded turn loop, permission gate in the engine, approval inbox, read-only explorer workers | Python sidecars; Tern is all Rust |
| Firecrawl | The shape of its endpoints (search, scrape, map, crawl, extract) | Code. It is AGPL-3.0 (re-verify before relying on this); Tern copies nothing |
| Zed and Obsidian | Shell layout, docks, vault structure | — |
