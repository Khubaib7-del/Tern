# 04 · Roadmap

> Status: 31 October 2026, roadmap v3. #82 (scholarly search) and #83 (evidence chain) are merged; #84 is this roadmap (a docs-only pull request); **#85, the engine, is next**. Work lands one pull request at a time, and each starts only when the owner approves it. Order is committed; dates are not. The phase tables at the end record how Tern got here.

Roadmap v3 replaces the list of #84–#104 that shipped with #83. The old plan put context, memory, cost and the run UI into one pull request (old #84), deep research and evaluation into another (old #85), and left study and packaging near the end. v3 splits those into pull requests that can each be tested in one sitting, puts reliability and evaluation before the research pipelines that depend on them, moves study up to follow the research engine, and adds a private beta early. The full old → new mapping is at the end of this page.

The roadmap follows GitHub's numbers: #84 is this docs pull request, which moved every planned pull request up by one (the engine is #85). If a closed PR or an issue takes a number again (as #81 did), this page and the README are renumbered.

## Phases at a glance

| Phase | PRs | Scope | Ends with | Status |
| --- | --- | --- | --- | --- |
| 1 · Foundations and work surface | #72–#80 | Rich answers, folder projects, work surface and sandbox, planner and code mode, Act-mode safety | — | Shipped |
| 2 · Trustworthy core and private beta | #81–#90 | Scholarly search, evidence chain, never losing a turn, context cascade and bootstrap prompt, run-state UI, memory and an explicit profile, a Store beta | A private beta on signed builds | #82–#85 shipped · #86 docs · **#87 next** |
| 3 · Research engine v2 and published proof | #91–#93 | Evaluation harness and consistency gate, router with Quick and Scholarly pipelines, Deep v2 and Long-run mode | Published citation-fidelity numbers | Planned |
| 4 · Study and show | #94–#100 | Generative UI, grounded study, conflicting-sources resolver, local RAG, learner model, guarded tutor and viva, charts | The final-year-project demo | Planned |
| 5 · Your files and reach | #101–#105 | Lecture video, Obsidian and chat import, export, security, connectors | — | Planned |
| 6 · Learning, hardening, launch and handbooks | #106–#112 | Learning core, adaptive user model v2, three buffers, the v1.0 release, the handbook bundle | v1.0 and the handbooks | Planned |
| After v1.0 | #113–#116 | Computer use, figures and Explain in 3D v2, git panel, shared rooms | — | Backlog |

**Final-year-project demo path:** #83 (every citation resolves to a quote Tern read) → **#85** (a run survives rate limits, crashes and network loss) → **#87** (fresh chats behave the same; "continue" keeps the thread) → **#92** (the Scholarly pipeline with evidence ids owned by code; its numbers measured by **#91**) → **#95** (research becomes quote-anchored cards and a quiz on FSRS, pushed to Anki) → **#96** (the conflicting-sources resolver) → **#99** (a viva that defends the claims of your own report).

- Minimum demo: through #92, plus #95.
- Full demo: add #96 and #99.

**Cut line: after #100.** #85 to #100 is the final-year-project build. If time runs short, drop in this order: #100 charts → #98 learner model (keep the FSRS queue) → #96 shrinks to the resolver without the actions router. #91 (it produces the measured numbers) and #99's viva are never dropped. #101 to #111 is the work for v1.0, and #112 completes the handbooks.

## Phase 1 · Foundations and work surface (#72–#80) — shipped

| PR | What merged |
| --- | --- |
| #71 | Per-chat state: chats run at the same time, each with its own composer, run and approvals |
| #72 | Rich Markdown answers (math, Mermaid, coloured code, citation chips, wrapping tables); hardening (retries with `Retry-After`, spending caps not retried, long tool output kept head and tail with `read_result`) |
| #73 | Math and Mermaid blocks with export; the `@` menu by keyboard; folder logos |
| #74 | Folder projects; the Files tab (Outputs and a live tree); math in `latex` fences |
| #75 | Work surface (tool cards, Terminal tab, streamed write previews with diffs), approval modes and permission rules, sandbox v1, `run_python` |
| #76 | Block focus view, the first-run intro, the Interrupted note, `file_rename` |
| #77 | Plan card, `run_tasks` task graph, code mode, tool result cache |
| #78 | Planner fixes and `file_edit` |
| #79 | Act mode hardening |
| #80 | Act mode guards: folder chats start in the folder's mode, "This needs Act mode", Chat mode never plans file work |

## Phase 2 · Trustworthy core and private beta (#81–#90)

| PR | What merged | Status |
| --- | --- | --- |
| #81 | A CI-only pull request | Closed without merging |
| #82 | Scholarly search: Semantic Scholar, OpenAlex, arXiv and Crossref (CORE with a key, Unpaywall for open-access PDFs); one paper per DOI; rerank funnel; an honest "not enough evidence"; passage locators; citation snowballing; loop and redo guards | Merged |
| #83 | Evidence chain: minted quote ids, unforgeable citations, a stance per source and agreement per independent site, counter-search, Report › Review claims, `[n]` hover cards, the `evidence` suite; scholarly follow-ups (academic questions search the literature first, snowball ranked by influence, abstract fallback, reference numbers removed from quotes) | Merged |

### #85 · Engine: durable turns, rate governor, honest budgets — merged

**Goal:** no turn is ever lost; a rate limit is a wait, not a failure; budgets are honest.

- An event contract first (run state, error class, kept summary, turn receipt, context trimmed, model switched), so #87–#89 build against fixtures.
- Interrupted turns stay in history with their partial text, sources and a tool digest; streamed text is checkpointed every second or 200 characters.
- A SQLite step journal keyed by turn: retries, fallbacks and Continue append to the same turn, and Continue replays recorded results instead of running tools again.
- A rate governor: requests-per-minute, tokens-per-minute and requests-per-day buckets seeded from a dated limits file and corrected from the provider's rate-limit headers and 429 bodies; a daily quota is never retried (its reset time is shown); a request bigger than the window is split before it is sent, within one call envelope per model.
- Per-model token calibration: one shared estimator corrected by what each provider reports, seeded per script, so Urdu and other non-Latin text is no longer overcounted.
- Cost accounting: cached tokens read on every wire; tool schemas no longer sent twice when native tools are on.
- Fallback per turn only, within the same provider by default, across providers only by explicit choice, never sticky.
- Budgets in tokens and money, with a reserved wrap-up (one writing call and one light verify) and failed calls not counted.
- Fixes from the #83 Windows test: argument unwrapping, a cache of failed URLs, `paper_read` dedupe, search prefetch per chat and skipped for follow-ups that refer back, Stop within 2 s, snowball by arXiv id or DOI, scholarly read batches sized by the governor, the Quick path reading the top three web results, reference markers stripped from every passage, an acronym guard, skip reasons in the Ledger, the code-mode path fix.
- A deterministic tool list per mode, folder and MCP set, pinned sampling, and a turn fingerprint (prompt version, fragment hashes, tool hash, model, sampling, cascade decisions).
- Learning core, step 1: a signal tap and an incident store with deterministic signatures, snapshots of the harness and a kill switch for learned entries (empty for now).
- Developer chaos switches (`TERN_DEV=1`): short and daily 429s, a dropped stream, malformed arguments, a kill mid-step; a fake-provider replay of a failed #83 turn.

**Exit check:** the replayed #83 turn finishes, or ends complete with gaps, never failed; no request exceeds the envelope; the Sources count never drops; Continue re-runs no tool; an Urdu estimate is within 20% of the provider's count after three calls.
**Depends on:** #83.

### #86 · Docs: desktop sources and renumbering

A docs-only pull request. It plans two optional pieces in #105, a desktop-sources connector (reading papers from Zotero, PDF readers and signed-in library portals through the OS accessibility tree, built on agent-desktop) and a browser for agents (the agent drives the built-in browser, built on agent-browser), names both as the base for computer use in #113, and moves every planned pull request after #85 up by one.

### #87 · Context cascade v2 and bootstrap prompt v1

**Goal:** "continue" keeps the thread; quality stays high in long chats; fresh chats in the same folder start the same way.

- A context cascade that trims the cheapest things first:
  - large results spill to a store that is never swept, recoverable with `read_result`;
  - each finished turn's tool results become short structured stub cards by tool class (a source's title, venue, year, read or abstract-only, its quote ids and first claim; a search's hits; a command's first and last line; errors kept longer);
  - tool output inside a long turn is stubbed with hysteresis, keeping the last rounds and freeing a large block at once so the prompt cache breaks rarely;
  - the request is fitted to the call envelope: oldest stubs dropped as call and result pairs, then whole oldest turns, never orphaning a tool call;
  - compaction runs before quality drops, keeps the last turns word for word, takes an editable "Keep: …" focus, and can be undone.
- Every trim is an event written before the call; resume and Continue replay those decisions, so a reopened chat sends the same request bytes.
- Scopes: global (`TERN.md` and a small core), project (brief, memory index, search over the project's chats) and chat; `@chat` brings another chat's handoff digest; no silent recall across chats.
- A versioned, layered bootstrap prompt ordered from stable to volatile (tools, Tern's core prompt, workflow and model-family deltas, user prompts, the user's stated profile, `TERN.md` and the project brief, then messages and a per-turn context block), with cache breakpoints between the layers.
- Tern core v1: about 1,000 tokens of instructions (authority order, evidence, working, adapting, format, honesty) and a lite core for small context windows. Short workflow deltas (Quick, Scholarly, Deep, Study, Act, Plan) replace scattered harness strings; model-family deltas for native and text tool protocols, reasoning models and small local models.
- Prompts as versioned fragment files with CI lints: a cap on instructions per fragment and per prompt, a token budget per window class, no clock, path or username in the stable layers, sorted tool JSON.
- Interim formatting rules (answer first; a table when several items share attributes) until #94.
- Settings edits apply to new chats, with a note and **Apply to this chat** in open chats.

**Exit check:** no orphaned tool call and no stub naming a deleted file; replay after a restart gives identical request bytes; ten unchanged turns give an identical prompt prefix; fresh chats in one folder start identically.
**Depends on:** #85 (journal, estimator, envelope).

### #88 · Run-state UI, presentation v1, panel stability

**Goal:** every state of a run is visible and distinct; nothing on screen is replaced or emptied; the right panel stops moving.

- One run-state model drives the working indicator, status line, sidebar glyph, taskbar and notifications: needs you, failed, waiting, working, streaming, paused, done, each distinguishable in greyscale.
- A working puck with the tern mark: a sweep per real event, a countdown ring while waiting, a lichen ring only when Tern needs you; Full, Calm and Off.
- A status line and an activity line ("Worked 2m 40s · searched 6 · read 3 · 2 skipped · verified 4 claims"), and a collapsible live timeline.
- One error is one row, updated in place; a banner only for a daily quota or a sign-in problem.
- A "Stopped here · cause · Kept: …" receipt with Continue; typing "continue" routes to it.
- A live context ring with a tick at the quality threshold and a breakdown popover; a compaction divider with "Freed N tokens", View summary and Undo.
- The model shown per turn (the one chosen and the one that answered); a model picker without truncation.
- Links and citation chips with AA contrast (lichen for links only); favicon chips, `Author Year` for papers, an "(abstract only)" badge, the quote on hover.
- The right panel keeps its tab order and widths, and per-chat width, tab and scroll; closing the browser leaves the panel open; Graph and Spend empty states say why.

**Exit check:** every state passes a greyscale test; the tab strip does not move between states; link contrast is at least 4.5:1 in both themes; the UI tour records every new state.
**Depends on:** #85 (events); #87 (context events).

### #89 · Memory v1 and how Tern adapts to you (explicit only)

**Goal:** Tern remembers what you tell it, in the right scope, and can show why; it adapts to what you state, never to who you seem to be.

- Memory v1: facts with provenance, supersession (a change keeps its history) and validity windows; an audit log and tombstones; full-text recall; explicit remember and forget; a **Save this instruction** chip; Temporary chat; deleting a chat deletes memories learned only from it; a sensitive-data blocklist; an inbox.
- An explicit profile per domain: depth, citation style, your stated level per field, a project's method and glossary. Name, gender, age, nationality, location, language or dialect, spelling, grammar, fluency, device and time of day are never used as evidence; mixed Urdu and English never lowers a level.
- A short profile block in the stable prompt and a per-domain cue on every turn.
- A skippable first-chat setup card (pitch, citation style, main field), and an "Adapted: …" chip under answers with More technical, Simpler and Don't adapt this chat; answers never announce the level.
- Settings › Memory (Global, This project, Inbox, History, How Tern adapts to you), with **Why?** opening the turn a memory came from.
- Reference lists formatted by a citation renderer with CSL styles, not by the model (may move to #92).

**Exit check:** a stated instruction persists over long chats; memories never leak between projects; deleting a chat removes what was learned from it; answers keep the same verdicts and numbers at every stated level.
**Depends on:** #85, #87. #90 can ship without it; only #98 and #107 need it.

### #90 · Private beta release (Microsoft Store MSIX)

**Goal:** real users on signed builds, without SmartScreen warnings.

- An MSIX package through the Microsoft Store, with a private listing for the beta; an optional signed ZIP for testers outside the Store.
- In-place upgrades keep chats, keys and memory.
- Setup on first run with no cost to the user: a local model, a free API tier, keyless Wikipedia, and the user's own scholarly keys (OpenAlex, optionally Semantic Scholar and CORE, an Unpaywall email) in the OS keychain; under ten minutes from install to the first verified report.
- Opt-in anonymous counters only (a verified report done, a crash), off until the user says yes; local crash reports; in-app feedback and the existing usability (SUS) runner.

**Exit check:** the Store build installs on a clean Windows 11 machine with no SmartScreen prompt; an upgrade keeps data; testers reach a verified report in ten minutes or less; nothing is sent before opt-in.
**Depends on:** #85–#88 (#89 optional).

## Phase 3 · Research engine v2 and published proof (#91–#93)

### #91 · Evaluations, consistency gate, observability

**Goal:** every claim about quality is a number, and every prompt or pipeline change is checked before it merges.

- A cassette format in `tern-eval` that records and replays tool and model responses; a paired before and after runner with pinned sampling; pass^k, three trials and 95% intervals.
- Evaluation layers: unit on every commit, recorded replay on every PR (each PR adds cases), chaos weekly from #85's switches; live benchmarks wired in and run from #92.
- A consistency suite for "the same model behaves differently in different chats": several chats per arm compared by fingerprint and by behaviour (tool counts, cited-source overlap, structure, verdict), never by identical text.
- A contradiction lint and a prompt change gate: a PR that changes a prompt attaches its consistency and replay results.
- Judge calibration, and the gate as a library the learning core reuses.
- A three-axis citation evaluation: the link works, it is relevant, and the claim checks against the cited text.
- Traces in SQLite, a **Cost** panel in Run details, `tern usage`, headless JSONL output; "Report this turn as a test case" in developer mode.

**Exit check:** the consistency cases fail on the #83 behaviour and pass after #85–#89; a planted prompt contradiction fails CI; the Cost panel is within 2% of the provider's reported usage.
**Depends on:** #85–#89.

### #92 · Research pipelines v2: router, evidence store, Quick and Scholarly

**Goal:** research answers whose citations are owned by code, at a known request cost, that stay accurate on small models.

- A router in code with a reason chip and a depth override: **Quick** (up to 3 requests), **Scholarly** (7 frugal, 12–16 standard), **Deep** (25–40), **Long-run** (in #93); a preflight "you have N requests left today".
- An evidence-card store with offset-checked quotes and evidence ids owned by code: the model never writes a URL or a reference, and unknown ids are stripped.
- Retrieval funnel v2: recall from several services, dedupe, BM25 plus dense retrieval, a local cross-encoder, an optional model screen; Semantic Scholar snippet search.
- The Scholarly pipeline: a criteria card, a query plan, a gap loop, a batched reader or an extractive mode for small models, an outline, one section-by-section writer, then verify and repair.
- The Quick pipeline with stated confidence.
- Verifier v2: deterministic checks, then a batched claim judge; quoted and attributed spans are not split into claims; conflict detection (contradicting quote pairs with an independence check) that feeds #96.
- Neutral sub-questions, gatherers that do not see the thesis, a verifier that does; a clarify card with numbered questions and a recommended answer for each.
- Report sections "What didn't work", "Where sources disagree", "Checks" and "Activity"; a source-scope chip (all, prioritise mine, only these); funnel statistics and exclusion reasons; a plan card you can edit while it runs; a second-opinion verifier on another provider when two keys exist.

**Exit check:** citation validity 100%; quote verification at least 98%; citation precision at least 0.90 on strong models and 0.80 on small ones; Scholarly within 7 requests frugal and 16 standard; no lost work under injected faults; the router right on at least 95% of a labelled set. These are the numbers published with the public beta.
**Depends on:** #85 (governor), #87 (workflow deltas), #91 (harness).

### #93 · Deep v2 and Long-run mode

**Goal:** sourced reports that hold up, and research that runs unattended for hours on free quotas.

- Deep v2: worker rounds with a short rewritten notes state, a lead review with an evolving outline, one hierarchical writer, self-feedback on mid-tier models and up; a single agent by default, parallel workers only for independent sub-questions or a deadline.
- Long-run mode: a goal card and a criteria card, a time budget, cycles of a few requests, one central report, a finish gated on evidence, sleeping through a quota until the provider's reset, a live summary ("Updated N min ago"), the best report so far, and run notes read on resume.
- A steering queue (edit, delete, apply now, reorder), pause, interrupt, stop and discard, a Discarded list, sub-agent rows and a run dashboard.

**Exit check:** Deep stays within 40 requests; a budget hit at 70% still yields a report with its gaps named; a daily quota running out mid-run waits and resumes after the reset.
**Depends on:** #92.

## Phase 4 · Study and show (#94–#100)

### #94 · Generative UI core and Depth

**Goal:** the agent chooses prose, a list, a table, a card or a surface by rule, and every surface is bound to evidence.

- A `tern-ui` crate with a typed UI document, a `show_ui` tool and a compact DSL that Tern compiles and validates.
- An evidence binder: a component that cites an id Tern never minted is not drawn, and verdicts set by the model are stripped.
- Three tiers: deterministic from the ledger, controlled typed decisions, and composed surfaces; form-router rules, with an optional bring-your-own-key UI router off by default.
- Components: CompareTable, ClaimBoard, SourceTriage, DecisionCard, ClarifyCard, Timeline, Quiz and Flashcard; a Markdown fallback for ACP and headless use; a Depth toggle; one dotted reveal per generated component (a fade with reduced motion); "Create from report".

**Exit check:** form choice precision at least 0.85; valid output at least 95% on strong models and 85% on small ones; evidence integrity 100%; no layout shift.
**Depends on:** #88, #92.

### #95 · Study a: grounded study core

**Goal:** research becomes quote-anchored cards and quizzes on FSRS, pushed to Anki, with every item traceable to a quote.

- **Study this** under any verified answer or report.
- Quote-first generation with four gates: grounding (a minted quote id, a verbatim answer span, entailment checked offline then by a model, every distractor refuted or not covered by the source), item rules (one fact per card, short answers, options of similar length, the key's position randomised in code), dedupe, and a difficulty tag that is stored but not trusted.
- New card kinds: Contrast, Apply and Worked step.
- FSRS-6 from the first review, desired retention 0.90 (adjustable), the optimiser only once a deck has enough reviews and only if it improves held-out predictions; SM-2 kept only to migrate imported decks.
- Card-quality marks (`✓ verified · p. 12`, `◌ check`) and a "Not added (n)" drawer.
- AnkiConnect push and review pull with dedupe, and `.apkg` import; fixed-width Study segments; keyboard shortcuts.

**Exit check:** at least 97% of items grounded and no forged ids; at least 85% of cards usable and at most 2% duplicates; no question with two correct options; a re-push to Anki creates no duplicates.
**Depends on:** #83, #85, #91, #94.

### #96 · Generative UI b: conflicting-sources resolver and actions

**Goal:** when sources disagree, Tern shows the disagreement piece by piece and lets you resolve it, like a merge conflict.

- A conflicting-sources resolver: each disagreement with its bound quotes, visible checks (date, primary or secondary, method, retraction, sample), a preview of what each resolution keeps and drops, and Apply into the report with Undo. It asks first in Chat, Plan and Deep; it applies on its own (with Undo) only in Long-run, under the criteria card.
- An evidence matrix and an inline paper map with a Graph handoff.
- An actions router through the policy gate with idempotent action ids.
- One approval inbox, shared by memory, the resolver and later the learning core (diffs, badges, history and revert); an editable compaction summary card; an outline approval card for long runs.

**Exit check:** Undo restores the report byte for byte; a double Apply writes once; a stale preview is checked again before it applies.
**Depends on:** #92 (conflict detection), #94.

### #97 · Local RAG and knowledge bases

**Goal:** answers and study items cite passages of your own files, with page locators.

- Knowledge bases per project, on #92's retrieval funnel: hybrid BM25, vectors and reranking on by default.
- Quote minting and page locators for files, so files can feed study.
- Memory vectors only if they beat full-text recall.

**Exit check:** retrieval recall beats dense-only retrieval on a literature-search slice; every chunk citation opens its page.
**Depends on:** #92.

### #98 · Study b: learner model and deadline plans

**Goal:** Tern knows what you have mastered and plans study around your deadline.

- Concept mastery with a simple knowledge-tracing model (replacing the Leitner box), Elo kept for matching difficulty.
- A misconception pipeline and a "Misconceptions to clear" rail; confidence ratings.
- Adapting to your stated level per domain, from #89, plus a short diagnostic.
- Rubric grading v2 that abstains when unsure, with appeals.
- A deadline planner: spaced sessions, FSRS raising retention in the last week, honest warnings when the load does not fit, and `.ics` export; a concept map in Explain in 3D.

**Exit check:** grading agrees with human marks on the answers it does not abstain on, and wrongly marks a right answer incorrect at most 2% of the time; every plan is feasible or says why not.
**Depends on:** #89, #95, #97.

### #99 · Study c: guarded tutor and viva over your own report

**Goal:** a tutor that teaches without giving the answer away, and a viva that makes you defend your own report.

- A guarded tutor as a state machine in code: orient, explain a short chunk, ask a check question, a hint ladder, reveal, re-ask; hidden answer keys; an answer-leak guard; one question per turn.
- A viva over the claims of your own report: warm-up, claim defence (claims picked in code, weighted toward contested or thinly sourced ones), method and limitations, counter-evidence from #83's Refutes cards, extension; a rubric in which every score cites your transcript and the source quote; optional grading by two or three of your models.
- Local voice (speech in and out on your machine; audio not kept unless you opt in); mistakes become cards.

**Exit check:** the tutor leaks an answer at most 2% of the time; every citation is bound; viva grading agrees within one point with recorded human grading on at least 80% of transcripts.
**Depends on:** #95, #98.

### #100 · Charts and analysis sandbox

**Goal:** charts drawn from real data, bound to the run.

- Chart specs as UI-document components; Long-run progress charts (best so far, the headline metric, baseline, target and now); stance and research-gap charts; a ranked source table.
- The Graph built from the ledger; deterministic analysis helpers in the sandbox.

**Exit check:** a chart is drawn from a data file inside the sandbox with no network, and every value traces to its source.
**Depends on:** #93, #94.

— **Cut line** —

## Phase 5 · Your files and reach (#101–#105)

### #101 · Lecture video intake

**Goal:** a lecture becomes study material whose citations are timestamps.

- Transcript segments with start and end times; quote ids per segment, so a citation opens the player at that moment; slide text read as passages at their time.
- Short segments with a few questions before watching; a watched segment enters the deadline plan.

**Depends on:** #95, #98.

### #102 · Obsidian and chat import

**Goal:** Tern works with the notes and chats you already have.

- Verified notes written into an Obsidian vault with each claim's status in the frontmatter; "Related in your vault".
- ChatGPT, Claude and Gemini import with provenance; imported memories go to the inbox.
- An editable Markdown view of memory: what Tern remembers, shown as Markdown you can read and edit.

**Depends on:** #89.

### #103 · Export and reference formats

**Goal:** every result leaves Tern in the format you need, citations kept.

- DOCX, PPTX, XLSX, PDF, LaTeX, BibTeX, RIS and CSV; a self-contained HTML evidence report; a PRISMA flow diagram.
- Reference lists through #89's citation renderer.

**Depends on:** #89, #92.

### #104 · Security and reliability

**Goal:** untrusted content cannot steer Tern, and nothing is lost when something fails.

- Taint tracked through traces; a redactor allow-list; poisoning suites for memory and sources; network egress per collection.
- Idempotency keys and a hardened journal; the chaos suite in CI; a quarantined tool profile for the learning researcher; an overseer watchdog.

**Exit check:** prompt-injection, poisoning and SSRF suites stay green; a crash resumes from its step journal.
**Depends on:** #85, #91. It comes before connectors and the learning core on purpose.

### #105 · Universal connector and your own corpus

**Goal:** any service can be a tool, behind the trust preview.

- ACP v1 brought up to date; the MCP Registry, OAuth 2.1 and OpenAPI tools behind the trust preview; presets for literature services you already use.
- Tern as an MCP server and `tern research --json --wait`; an A2A client behind a flag; MCP Apps in a separate webview; connector actions as journaled steps with idempotency keys.
- Desktop sources (optional, off by default): read papers from desktop apps such as Zotero, PDF readers, and a library portal the user is already signed in to, through the OS accessibility tree rather than screenshots. Built on [agent-desktop](https://github.com/vercel-labs/agent-desktop) (Apache-2.0, Rust, Windows, macOS and Linux), whose snapshot, ref, act and verify loop also shapes the built-in browser tool and the UI tours. Consent is asked per app, actions are read-only by default, and snapshots are never logged or sent to a model unfiltered.
- Browser for agents (optional, off by default): the research agent drives Tern's built-in browser itself, for pages that need JavaScript, clicks or the user's signed-in session (publisher pages, library portals, paywalled papers the user has access to). Built on [agent-browser](https://github.com/vercel-labs/agent-browser) (Apache-2.0, Rust) and the same snapshot, ref, act and verify loop: the page is read as an accessibility snapshot with stable refs, each click or form fill is a journaled step, actions are read-only by default, and page text stays isolated from instructions.

**Depends on:** #104.

## Phase 6 · Learning, hardening, launch and handbooks (#106–#112)

### #106 · Learning core (eval-gated)

**Goal:** Tern learns from its own failures, and no lesson is kept unless it passes an evaluation.

- Signals from #85 → incidents deduplicated by signature → a diagnosis that must first produce a failing evaluation case → quarantined, redacted research → the smallest fix that could work → a hidden evaluation gate (replays, regression and poisoning suites) → the approval inbox for lessons from the web or global ones; lessons local to a chat apply with rollback → a canary → keep, or roll back by snapshot.
- Calibrated confidence on resolved claims; a budget-aware router that learns; watches run by an OS scheduler; a daily learning spend cap in Settings, never on free-tier keys, local model first.

**Exit check (thesis metrics):** incidents per 100 turns by class; recurrence after a fix; falling false-positive proposals; learned tokens per turn; learning cost as a share of task spend; success before and after on a held-out suite.
**Depends on:** #85, #91, #96, #104.

### #107 · Adapts to you v2 and memory v2

**Goal:** Tern can infer how you work, but only where it is shown to help and to be fair.

- Levels per domain inferred from behaviour only, decaying over time, promoted only after consistent signals in more than one chat, and then through the inbox; learning from your edits; confirmations recorded so Tern does not drift into caution.
- Memory v2: a background extractor and consolidator, contradiction detection, decay weighted by use, source and claim memory; "What Tern assumes about you" with Fix and Forget.

**Exit check:** the same level and the same claims when only name, dialect (Roman Urdu or English), grammar or country change, in at least 95% of pairs; if blind raters do not prefer adapted answers, inferred adaptation ships off by default.
**Depends on:** #89, #104, #106.

### #108 · Buffer 1: performance

Startup, memory and token budgets measured and met; retrieval and curation off the main thread.

### #109 · Buffer 2: polish and the Windows test list

The owner's end-of-cycle Windows test list passes; fixes from beta feedback.

### #110 · Buffer 3: accessibility, Urdu, edge cases

Screen-reader labels on every control; the interface in Urdu; a quality check of study items generated in Urdu; Windows' reduced-motion setting.

### #111 · v1.0 release

Updates through Velopack, GitHub Releases and the Store listing; macOS notarisation at low priority. **Exit check:** a signed build installs and updates itself.

### #112 · Handbook bundle

**Goal:** fourteen handbooks that explain Tern to the people who use, examine and build it, shipped in the repository under `docs/handbooks/`, as a PDF set and as one ZIP. Each handbook stands alone and links to the others, and all are written from the real code, ADRs and pull request history.

| Handbook | For | Contents |
| --- | --- | --- |
| 1. User guide | Students and researchers | Install, your own keys and local models, modes (Quick, Scholarly, Deep, Long-run), folders, approvals, Study, export, privacy settings, troubleshooting |
| 2. Research paper reading | Students | Reading a paper in three passes, judging evidence, abstract versus full text, citation chasing, how the evidence chain and claim checks help, worked examples |
| 3. Research paper writing | Students and thesis writers | From question to outline, the literature review, argument and evidence, citing (APA, IEEE, Harvard), avoiding plagiarism and unsupported claims, using Tern honestly, viva preparation |
| 4. Study | Learners | The learning science (testing, spacing, interleaving), FSRS, quote-tied cards, tutor and viva, deadline plans |
| 5. Features | Everyone | Every feature with screenshots: rich blocks, math and Mermaid, generative UI, the conflict resolver, charts, the focus view, lecture video, Obsidian |
| 6. Technical architecture | Engineers and examiners | Crates, data flow, the turn loop, providers, the rate governor, the journal, the evidence store, the sandbox, diagrams |
| 7. Internals | Contributors | The context cascade, bootstrap prompt layers, memory schema, user model, learning core, event types, replay |
| 8. Engineering | Contributors | Repository layout, build, CI (four jobs), UI tours, evaluations and gates, coding rules, release and packaging |
| 9. Design | Designers | Design brief, palette, type, spacing, components, motion, brand (the Tern mark, Scout), accessibility |
| 10. Evaluation | Examiners and researchers | Every evaluation suite, metrics and targets, and how to reproduce the numbers |
| 11. Security and privacy | Everyone | Local-first data, key storage, sandbox and permissions, the taint gate, prompt-injection defences, what leaves the machine |
| 12. Prompts | Contributors | Versioned prompts, model-family deltas, lints, how to change a prompt safely |
| 13. Raw (decision log) | Future maintainers | Every ADR, a research digest with sources, the pull request history, what failed and why |
| 14. Research and sources | Thesis readers, examiners and contributors | Everything researched for Tern, topic by topic (context, memory, evidence, deep research, generative UI, study, adaptation, sandbox) with its findings, and every source: papers (with arXiv ids), repositories (with their licence, and whether ideas or code were used), posts and the people behind them, documentation and benchmarks; where each finding landed (the PR number), and which claims were verified against the source and which rest on an excerpt only |

**Built PR by PR, starting with #85:** every pull request adds or updates its section under `docs/handbooks/` (a checklist item in the PR template), so #112 edits and assembles rather than writing from scratch. #112 adds screenshots from the UI tours, diagrams, a shared glossary, a PDF build and a ZIP in the release.

**Exit check:** every handbook builds to PDF in CI; links and code references are checked; every feature in the README's What works today appears in the Features handbook; someone new can install Tern and reach a first verified report from the User guide alone.
**Depends on:** #111; sections grow from #85 on.

## After v1.0 (#113–#116) — backlog

| PR | Scope | Why it waits |
| --- | --- | --- |
| #113 | Computer use, off by default, built on agent-desktop (desktop apps) and agent-browser (the web): Tern acts for you, asking first, on research chores only (sign in to a library portal, download the PDF, file it in Zotero), each action a journaled step | Risky, and not needed for research and study |
| #114 | Interactive figures and Explain in 3D v2; image-occlusion cards from PDF figures | Polish; needs figure extraction |
| #115 | Git panel: status, diff, history | A coding feature, outside research and study |
| #116 | Shared rooms: read-only spectators over run events, shared libraries | Builds on the rest |

## Dependencies

| PR | Needs |
| --- | --- |
| #85 | #83 |
| #87 | #85 |
| #88 | #85, #87 |
| #89 | #85, #87 |
| #90 | #85–#88 |
| #91 | #85–#89 |
| #92 | #85, #87, #91 |
| #93 | #92 |
| #94 | #88, #92 |
| #95 | #83, #85, #91, #94 |
| #96 | #92, #94 |
| #97 | #92 |
| #98 | #89, #95, #97 |
| #99 | #95, #98 |
| #100 | #93, #94 |
| #101 | #95, #98 |
| #102 | #89 |
| #103 | #89, #92 |
| #104 | #85, #91 |
| #105 | #104 |
| #106 | #85, #91, #96, #104 |
| #107 | #89, #104, #106 |
| #112 | #111 (sections from #85 on) |

## Old → new numbering

The plan published with #83 listed #84–#103 (the Old column). Where each item went:

| Old | Old focus | New | New focus |
| --- | --- | --- | --- |
| #84 (part) | Context, memory and cost | #85 | Engine: durable turns, rate governor, honest budgets |
| #84 (part) | Context, memory and cost | #87 | Context cascade v2 and bootstrap prompt v1 |
| #84 (part) | Context, memory and cost | #88 | Run-state UI, presentation v1, panel stability |
| #84 (part) | Context, memory and cost | #89 | Memory v1 and how Tern adapts to you (explicit) |
| #103 (part) | Packaging | #90 | Private beta release (Microsoft Store MSIX) |
| #85 (part) | Deep research v2, evaluations and observability | #91 | Evaluations, consistency gate, observability |
| #85 (part) | Deep research v2 | #92 | Research pipelines v2: router, evidence store, Quick and Scholarly |
| #85 (part) | Deep research v2 | #93 | Deep v2 and Long-run mode |
| #87 (part) | Generative UI and a Depth toggle | #94 | Generative UI core and Depth |
| #92 (part) | Study suite | #95 | Study a: grounded study core |
| #87 (part) | Generative UI | #96 | Conflicting-sources resolver and actions |
| #90 | Local RAG and a knowledge graph | #97 | Local RAG and knowledge bases |
| #92 (part) | Study suite | #98 | Study b: learner model and deadline plans |
| #92 (part) | Study suite | #99 | Study c: guarded tutor and viva |
| #86 | Charts and an analysis sandbox | #100 | Charts and analysis sandbox |
| #91 | Lecture video intake | #101 | Lecture video intake |
| #88 | Markdown folders, Obsidian, chat import | #102 | Obsidian and chat import |
| #89 | Export | #103 | Export and reference formats |
| #95 | Security and reliability | #104 | Security and reliability |
| #93 | Universal connector | #105 | Universal connector and your own corpus |
| #99 (part) | Learning loops | #106 | Learning core |
| #99 (part) | Learning loops | #107 | Adapts to you v2 and memory v2 |
| #100 | Buffer 1: performance | #108 | Buffer 1: performance |
| #101 | Buffer 2: polish and fixes from testing | #109 | Buffer 2: polish and the Windows test list |
| #102 | Buffer 3: accessibility, Urdu, edge cases | #110 | Buffer 3: accessibility, Urdu, edge cases |
| #103 (rest) | Packaging | #111 | v1.0 release |
| — | (new) | #112 | Handbook bundle |
| — | (new) | #86 | Docs: desktop sources and renumbering |
| #94 | Computer use | #113 | Computer use (after v1.0) |
| #96 | Interactive figures and Explain in 3D v2 | #114 | Figures and Explain in 3D v2 (after v1.0) |
| #97 | Git panel | #115 | Git panel (after v1.0) |
| #98 | Shared rooms | #116 | Shared rooms (after v1.0) |

Why the shape changed:

- **Old #84 became four PRs.** Context, memory and the run UI grew too large to test in one Windows session; smaller PRs keep work moving without adding testing.
- **Reliability first.** The failed turns in the #83 test came from rate limits, lost partial work and retries, so the engine (#85) comes before anything else.
- **Evaluation before pipelines.** The research pipelines, study and the learning core all need recorded-replay cases that fail on the old behaviour, so #91 sits before #92.
- **Study moved up and split in three.** Research into study is Tern's most distinctive half; it follows the research engine (#95, #98, #99). FSRS-6 is used from the first review instead of SM-2 until a deck has enough reviews.
- **Generative UI split.** Quiz and Flashcard components come first (#94); the resolver needs #92's conflict detection (#96).
- **A private beta early (#90)**, so real users test signed builds long before v1.0 (#111).
- **Security before connectors and the learning core (#104)**, because connectors bring untrusted content and the learning core must not run before taint and poisoning suites exist.
- **Handbooks added (#112)**, drafted PR by PR under `docs/handbooks/` from #85 on, so the last PR of the plan assembles them.
- **Computer use, figures v2, the git panel and shared rooms wait until after v1.0 (#113–#116).**

## Earlier phases

| Phase | Name | Outcome | Status |
| --- | --- | --- | --- |
| 0 | Repository baseline | Licence, docs, brand assets, process files | Done (PR #1) |
| 1 | Design lock | Palette, fonts, concept screens, SVG mark, Scout rig sketch | Done (PR #30; Tern Crawl logo PR #32) |
| 2 | Core skeleton | Cargo workspace, `tern-core` contracts, `tern-platform` keychain on Windows, `tern-store` basics, `tern doctor` | Done |
| 3 | Providers and first slice | BYOK and local models; a quick cited answer end to end in the TUI | Done |
| 4 | Crawler and verification | `tern-crawl` with the network guard; claim verdicts; run report | Done (PRs #31 to #33; scholarly sources in #82, evidence chain in #83) |
| 5 | Deep research | Plan-first runs, workers, checkpoints, budget pause, steering, watcher | Mostly built (PR #34: plans and parallel workers; PR #35: checkpoints, steering, the in-app watcher, ledgers); budget pause and an OS scheduler remain |
| 6 | Desktop app | GPUI shell, transcript, right rail, vault editor | Done (shell brought forward, ADR 0006) |
| 7 | Study harness | Tutor, flashcards, quizzes, viva practice | Mostly built (PR #36: materials, topics, mastery, tutor, quizzes, viva; flashcards and Explain in 3D since); FSRS in #95 and study plans in #98 |
| 8 | Decision model | Decision-model slot wired into source ranking, tool choice, and claim checks | Mostly built (PR #38: local scorer, Jev, SubQ Snap, and Voyage adapters, wiring, `tern decide bench`); the reasoning-model and remote benchmark rows need keys |
| 9 | Continuity and library | Chat range clean and move, sub-sessions, handoff, library, bulk delete | Mostly built (PR #39); a model-written handoff summary remains |
| 10 | Extensibility | MCP client, ACP server, skills, project rules | Mostly built (PR #40: MCP client, ACP server, skills, `TERN.md` and effective instructions, project folders, the Customize page); MCP prompts and resources remain |
| 11 | Evaluation | Benchmarks for citation accuracy, cost, and reliability | Mostly built (PR #42: the five-rung ladder, versioned suites, metrics with intervals over three trials, published failures, run comparison, the CI baseline, the Evaluation page); live, pinned, and public scores need a key and have not been run |
| 12 | Linux and macOS | Ports through `tern-platform` | Built (PR #43: Secret Service and Keychain, XDG and Application Support folders, OS reduced motion, the macOS menu and Cmd keys, CI jobs and test builds per OS); not yet tried by hand on a Linux desktop or a Mac |

Distribution is now planned as #90 (a private beta through the Microsoft Store) and #111 (the v1.0 release with updates).

## Earlier phase detail

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
| Research flow v2 (PR #59): queued steer rows, needs-you card, checkpoint strip, budget card, citation hover cards, claim review, report export, vault diffs, launch splash | Each step drawn from the run's real events; the UI tour records every one |

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
| Wired into source ranking, next-tool choice, claim pass or fail, now-or-later | Measured against the reasoning-model-only baseline (built, PR #38: ranking in quick asks, chat search, and deep workers; a first-tool hint; a claim pre-score under hard checks with the model judge keeping the final word; deep worker and watch order. `tern decide bench` on 50 bundled fixtures: fixed order 52%, local scorer 86%, in-sample; on the held-out evaluation split of PR #42, 45.5% and 72.7%. The reasoning-model row (`--reasoning`) and the remote row (`--remote`) need keys and have not been run) |

### Phase 9 · Continuity and library

| Deliverable | Exit check |
| --- | --- |
| Clean a chat by ID range: remove, or move to a new session with context | Context carries over; original preserved until confirmed (built, PR #39: `tern_agent::range`. Remove appends `range.removed` and the turns go to the chat's trash, restorable; a move copies the turns into a new session or a sub-session with a `session.context` summary of the turns before them, and the original is untouched until Confirm (the originals then go to the chat's trash) or Undo (the copy is archived). The log stays append-only and the turns that stay keep their numbers. Tested in `range.rs`) |
| Planned sub-sessions; background branching that reports back | Sub-IDs (`Q12.3`) resolve everywhere (built, PR #39: schema v3 adds a session's parent, anchor message, and planned question; `tern_agent::sub` makes, lists, and runs sub-sessions and posts a summary back to the parent as `sub.reported`, which the parent's transcript shows and its model reads; `tern_agent::refs` numbers sub-session messages across all sub-sessions of one parent message in start order and resolves IDs in `@` mentions, the CLI, `tern://chat/<id>/Q12.3` links, and Library search. Tested in `refs.rs`, `sub.rs`, and `tests/continuity.rs`) |
| Session handoff packets | New session starts from the packet alone (built, PR #39: `tern_agent::handoff` drafts goal, decisions, open questions, key sources and claims, a ledger snapshot, and next steps as Markdown and JSON; `tern new --from-handoff` starts a session whose only event is the packet. `tests/continuity.rs` starts one in a fresh database from the Markdown alone and checks the model reads the packet and nothing of the old chat) |
| Library of everything; bulk delete | Thousands of items remain responsive (built, PR #39: one index in `tern_store::library` over reports, sources, notes, decks, files, Explain-in-3D scenes, and sessions, with search, filter, sort, and paging in SQLite; bulk delete to a restorable trash, empty-trash. A 5,000-item test indexes, queries three ways, and bulk-moves in well under the bounds; the desktop page reads 60 rows at a time) |
| Highlight actions and throwaway side chat | Side chat writes nothing back (built, PR #39: Explain, Verify claim, Add to notes, Make flashcard, and Ask in side chat on selected answer text; `tern_agent::side` reads the thread and keeps its exchanges in memory. Tested: the parent's log and title are unchanged after side questions) |

Built in PR #39: message IDs on the transcript with multi-select, range actions with Confirm and Undo, a `Branch ▾` menu (new sub-session from here, run the composer text in a background branch, move to a sub-session), a sidebar sub-session tree, handoff, the Library page and trash, the highlight popover, and the side chat docked or popped out. Project folders in the sidebar came in PR #40; a model-written handoff summary remains.

### Phase 10 · Extensibility

| Deliverable | Exit check |
| --- | --- |
| MCP client with trust preview and health checks | MCP output treated as untrusted data (built, PR #40: `tern_agent::mcp`, stdio and Streamable HTTP, spec `2026-07-28` with the `initialize` fallback for `2025-11-25` back to `2024-11-05`; servers start off until their trust preview is read; per-tool switches and approvals behind the policy gate; results wrapped by `tern_agent::untrusted`; health checks time start, ping, and list. Tested in `tests/mcp.rs` against a mock server over stdio, modern and legacy, and HTTP, including a poisoned tool description and gated, wrapped output) |
| ACP server so Tern runs inside compatible editors | Session round-trip works (built, PR #40: `tern acp`, ACP v1 over stdio; `tests/acp.rs` makes a session, prompts it, reads the streamed updates, and loads it again. Setup for Zed: [docs/zed.md](zed.md)) |
| Skills loaded on demand; project rules in `TERN.md` | Effective instructions visible to the user (built, PR #40: `SKILL.md` folders loaded with `load_skill`; `TERN.md` merged global, parents, project; `tern instructions` and the desktop Customize page's Instructions tab show the exact prompt) |
| MCP prompts and resources | A server's prompt text is shown before anything is sent; a resource reaches the model only as untrusted Context (built, PR #41: `tern_agent::mcp::catalog`; `prompts/list` and `prompts/get` with arguments, `resources/list`, `resources/templates/list`, and `resources/read` for text and blobs; list changes from legacy notifications and the modern `subscriptions/listen` stream on stdio; the composer's `/` menu (Tern's commands, skills, and MCP prompts, each row with its logo) with an inline argument form, and `@` resources; the trust preview, the fingerprint, and the effective instructions cover them. Tested in `tests/mcp.rs` against the mock server over stdio, modern and legacy, and HTTP) |

Built in PR #40: project folders in the store (schema v4) with chats and sub-sessions filed under them and an optional linked folder on disk; the sidebar's project tree (Projects, then chats, then `↳` sub-sessions, the Inbox, drag a chat onto a folder or Move to…, collapse saved); the **Customize** rail item (Skills, MCP servers with trust preview and health dots, Providers, Instructions); `tern mcp`, `tern acp`, `tern skills`, `tern instructions`, `tern project`. Left out of PR #40: MCP prompts and resources (added in PR #41), and the model-written handoff summary. Left out of PR #41: watching single resources (`resources/subscribe` and `resourceSubscriptions`), a long-lived change stream over HTTP (the HTTP catalog is re-read after 30 seconds instead), and client capabilities (roots, sampling, elicitation).

### Phase 11 · Evaluation

| Deliverable | Exit check |
| --- | --- |
| Test ladder: unit, synthetic, pinned real tasks, public benchmark, human usability | Each level runnable from one command |
| Metrics: claim precision and recall, unsupported claims per 100 runs, citation accuracy, constraint retention, forecast error, cost per verified report | At least three trials, confidence intervals, failures published |

Built in PR #42 ([evaluation.md](evaluation.md)): a twelfth crate, `tern-eval`, with versioned JSON suites and one command per rung. **Unit**: `tern eval run --level unit` runs `cargo test --workspace` and reads it back as a run. **Synthetic**: `tern eval run`, offline and deterministic, on bundled suites of claim verdicts (27 cases), research answers from given pages (6 tasks with 3 recorded answers each and per-sentence labels), quiz grading (23), decision routing on a held-out split (22 cases written after the local scorer and never used to tune it, so `tern decide bench`'s in-sample 86% has an out-of-sample counterpart), and injection and tool safety (34: poisoned and benign MCP descriptions for the trust scan, injected page and server text for the untrusted wrapper, tool calls an injection could ask for at the policy gate, canary pages for a live model). **Pinned**: real questions over three pinned public pages, `--live`, with source drift reported. **Public**: SciFact from the user's copy (nothing bundled). **Human**: `tern eval usability`, five timed tasks and the System Usability Scale. Metrics: citation accuracy, claim precision and recall, unsupported claims per 100 runs, source precision and recall, constraint retention, verified reports, tokens (and, with `--price`, dollars) per verified report, preflight forecast error and range (live, measured tokens only), checker agreement, verdict and grading accuracy, top-choice accuracy, Brier score and calibration error, scan recall and false alarms, containment, gate correctness, canary resistance, task success, SUS. Three trials by default; a 95% interval on every metric (Wilson and t); every failure in the report; `tern eval compare` flags regressions; CI runs the offline suites against `crates/tern-eval/baseline.json` and uploads the report. First offline scores: citation accuracy 97.7%, claim verdicts 59.3%, quiz grading 87.0%, held-out routing 72.7% (fixed order 45.5%), safety 90.0%; the failures (wrong numbers called Partly supported, swapped entities called Supported, paraphrased short answers, a network read to a new domain allowed by default) are listed in [evaluation.md](evaluation.md). Not yet: live, pinned, and public runs with a key, and a larger human study.

### Phase 12 · Linux and macOS

| Deliverable | Exit check |
| --- | --- |
| `tern-platform` for Linux (Secret Service, XDG paths) | CI green on Linux |
| `tern-platform` for macOS (Keychain) | CI green on macOS |

Built in PR #43 ([linux.md](linux.md), [macos.md](macos.md)). **Linux**: keys in the Secret Service over D-Bus (GNOME Keyring, KWallet, KeePassXC; pure-Rust D-Bus, no system library), with `TERN_KEYRING=keyutils` for the kernel keyring on machines without one (keys last until reboot; nothing is ever written to a file); data, settings, and cache in the XDG folders; reduced motion read from GNOME or KDE; links and saved reports opened through the portal or `xdg-open` as `file://` links; the desktop app on X11 or Wayland through GPUI, with the window's app id matching a bundled `.desktop` entry; Ctrl+Q quits. **macOS**: keys in the login Keychain; data and settings in Application Support, the cache in Caches; reduced motion read from the Accessibility setting; the title bar leaves room for the traffic lights and keeps its controls out of the drag region (`occlude`); a menu bar (Tern, Window) and Cmd+Q, Cmd+W, Cmd+M, Cmd+H. **Windows** keeps its single-folder layout; its reduced-motion setting is still not read (it needs an `unsafe` system call, which `tern-platform` forbids). **CI**: a Linux job (fmt, clippy, tests, `tern doctor` on the kernel keyring, the offline evaluation with the baseline check, a release build, and the `tern-linux-x64` archive with both programs, the `.desktop` entry, and the icon) and a macOS arm64 job (clippy, tests including a Keychain round trip, the offline evaluation, a release build, and `tern-macos-arm64`, an unsigned `Tern.app` with `Info.plist` and icon, zipped with the CLI). The same PR fixed the two weak spots the evaluation found: the policy gate now tracks where web addresses came from, and the offline claim check catches wrong numbers, negations, antonyms, and swapped entities and roles ([evaluation.md](evaluation.md)). Not yet: hand testing on a Linux desktop and a Mac, signed or notarized builds, and packages (distribution comes after this phase).

## Later and parked

| Item | Status |
| --- | --- |
| Import chats from other assistants with provenance | Planned (#102) |
| Universal connector for services without MCP | Planned (#105) |
| Desktop sources through the accessibility tree (agent-desktop) | Planned (#105) |
| Browser for agents: the agent drives the built-in browser (agent-browser) | Planned (#105) |
| Learning from frustration signals, eval-gated | Planned (signal tap in #85, learning core in #106) |
| Lecture video and slide intake | Planned (#101) |
| Computer use, scoped to research chores, on agent-desktop and agent-browser | After v1.0 (#113), off by default |
| Shared study and research rooms | After v1.0 (#116) |
| Skill discovery and repository reading without cloning | Later |
| Handbooks (user, research reading and writing, study, features, architecture, internals, engineering, design, evaluation, security and privacy, prompts, decision log, research and sources) | Planned (#112), drafted from #85 on |
| Interactive explainer export | Later |
| Hardware research pack | Parked |
