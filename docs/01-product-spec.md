# 01 · Product spec

> Status: draft, 21 October 2026. Every feature in this document is **planned**. Phase numbers refer to [04-roadmap.md](04-roadmap.md).

## 1. Shape of the product

| Concept | What it is |
| --- | --- |
| Workspace | Everything on one machine: projects, global memory, settings, keys (in the OS keychain) |
| Project | A body of work, such as an FYP. Holds sessions, a library, a vault, and project memory |
| Session | One conversation. Can have planned sub-sessions that inherit its context |
| Message / run | A user query and the agent's work on it. Every query has a stable ID (`Q12`), sub-sessions add a sub-ID (`Q12.3`) |
| Library | Every file, PDF, URL, DOI, image, and generated output in the project, with IDs |
| Vault | Plain Markdown on disk, Obsidian-compatible, for notes and reports |
| Harness | Research or Study. Chosen per session from the composer or title bar |

## 2. Research harness

### 2.1 Modes

| Mode | Output | Typical budget |
| --- | --- | --- |
| Quick ask | Short cited answer | One model call plus a few reads |
| Deep research | Structured report with numbered citations and a run report | Plan-first, budget-capped, sub-researchers |
| Literature review | Themed review, evidence table, bibliography | Scholarly sources first |
| Fact-check | Claim board: supported, contradicted, unverified | Per-claim verification |
| Compare | Side-by-side matrix of methods, tools, or papers | Primary sources |

### 2.2 Pipeline

| Stage | What happens | Shown to the user |
| --- | --- | --- |
| 1 Clarify and plan | Turn a rough prompt into scope, sub-questions, sources, verification plan, and a cost range | Editable plan in the right rail; nothing paid runs before approval |
| 2 Search | Parallel searchers across web and scholarly APIs; the decision model ranks what to read first | One rollup line: "Searched 14 sources" |
| 3 Read | Reader workers fetch, parse, and extract claims with quote anchors | "Read 6" |
| 4 Verify | Each claim mapped to a source span; DOI and URL resolve; retraction check; contradictions flagged | Verdict marks on claims |
| 5 Write | Outline, sections, inline numbered citations, bibliography | Report in the right rail and the vault |
| 6 Record | Methods log and one-screen run report saved | Run report card |

### 2.3 Claim verdicts

Shape first, colour second, so they read in monochrome and in the terminal.

| Mark | Verdict | Meaning |
| --- | --- | --- |
| ■ | Supported | A source span directly supports the claim |
| ✕ | Contradicted | A source span contradicts it |
| ○ | Unverified | Not checked, or checked without a conclusive result |
| ⌀ | Unreachable | The source could not be fetched or resolved |
| — | Waived | The user chose to accept it without verification |

Rule: a claim is never turned into Supported because a model is confident. The evidence order is direct source span, then deterministic check, then source-grounded review, then model critique.

### 2.4 Sources and tools

| Group | Planned tools |
| --- | --- |
| Discovery | Web search through BYOK search APIs or a self-hosted search engine |
| Scholarly | OpenAlex, Semantic Scholar, arXiv, Crossref, Unpaywall, Europe PMC, CORE |
| Reading | `tern-crawl` fetch and extract, PDF reader, open-access PDF finder |
| Verification | DOI resolver, retraction check, claim verifier, citation graph |
| Writing | Evidence tables, citation formatting (CSL styles, BibTeX), export to Markdown, DOCX, PDF, LaTeX |
| Project | Library search, notes, memory, ask the user, plan, spawn reader |

## 3. Study harness

| Feature | What it does |
| --- | --- |
| Tutor | Explains a topic or a paper at a chosen level, grounded in the project's sources, and checks understanding as it goes |
| Flashcards | Builds decks from sources and notes; spaced repetition; Anki export |
| Quizzes | Multiple choice and short answer, graded with the same verdict vocabulary as research claims |
| Viva practice | Plays examiner on the user's own report or thesis: asks, follows up, and grades answers against sources |
| Study plans | Turns a syllabus or a goal into a schedule with deliverables |
| Reading protocol | Before reading: a one-sentence question. After: claim, assumptions, setup, limits, and what was reproduced versus only stated |
| Multimodal intake | Lecture videos and slides into timestamped notes and cards (later phase) |

The Study harness looks different from Research by tone and shape, not by colour: warmer surface, round markers, pill radius, italic serif labels. See [02-ui-definition.md](02-ui-definition.md).

## 4. Shared core features

| Feature | Detail |
| --- | --- |
| BYOK providers | A list of provider rows with logos; `Add` a provider, paste a key, test, models are listed, then enable or disable it with a toggle and assign roles (planner, reader, writer, verifier, tutor). Keys stored in the OS keychain |
| Local models | Ollama, LM Studio, and any OpenAI-compatible endpoint |
| Decision-model slot | A scorer beside the reasoning model for fixed-choice decisions; remote or a local reranker as fallback |
| Stable IDs | Every query, file, source, chat, and claim has a short ID the agent passes around instead of full text |
| Memory | Context window, session summary, project memory, library retrieval, global profile; each memory tagged observed, user decision, inference, temporary, or sensitive |
| Goal and decision ledger | Goals, constraints verbatim, non-goals, decisions, and rejected options, kept outside the transcript so they survive compaction |
| Compaction | Model-specific thresholds; the log is persisted first; context is rebuilt from the ledger; the user is told when it happens |
| Cost and budget | Preflight cost range per run and per step; spend, tokens, provider quota, and local budget kept separate; unknown values shown as unknown |
| Safe pause | Near a budget limit, the run stops at a safe point, saves a checkpoint, and offers: resume later, cheaper model, smaller scope, or stop |
| Checkpoints | Scrubbable timeline; branch from any point; return to a paused line of work |
| Steering | Send a message mid-run; the agent picks it up at the next safe point (Enter = steer, Tab = queue, Esc = interrupt) |
| Watcher | Ask a second agent what the main one is doing, why, and how far along it is, without interrupting it |
| Loop and drift detection | Repeated calls, cycles, no-progress runs, and actions that drift from the goal are caught and paused |
| Permissions | Effect classes, scoped and expiring approvals, grouped previews to avoid approval fatigue |
| Self-diagnostics | `tern doctor` checks keys, models, crawler, disk, and index, and explains fixes in plain language |
| MCP and ACP | MCP client for tools; ACP server so Tern can run inside editors that speak it |
| Tabs | Each session has tabs (Chat, Report, Sources, Graph, Editor, Browser, Subagents, Explain in 3D) that can split right or down |
| Parallel tasks | Sub-researchers run in parallel under the watcher, each with its own budget; a Subagents tab shows each one as a single rollup line |
| Built-in browser | A Browser tab for reading sources with the evidence highlighter and signing in to library portals; shares the `tern-crawl` network guard and treats page text as untrusted |
| Explain in 3D | A separate mode that turns a concept into an interactive isometric explainer (solid machine plus line-figure overlay); saved to the Library |
| Git and GitHub | A project vault can be a git repository: commit (as a checkpoint), branch, push, open a pull request, and set up baseline files |
| Scheduled | Topic watches, weekly paper digests, and spaced-repetition reminders |

## 5. Chat and editing behaviour

| Behaviour | Detail |
| --- | --- |
| Edit | Editing a message forks the thread; the original stays as a version |
| Delete | Deleting a message also deletes its reply; a reply can be deleted alone |
| Regenerate, versions, pin | Standard, with versions kept |
| Reply to any message | Reply to an earlier message by its ID, with feedback on any message |
| Highlight actions | Select text to edit, ask, explain, quote, or open a throwaway side chat that writes nothing back |
| Clean a chat by range | Select from one query ID to another and remove it or move it into a new session that carries the context |
| Branch in the background | Send part of the context to a new session for side research; it reports back when done |
| Handoff | Condense a long session into a handoff packet (goal, decisions, open threads, key IDs) while the full session is preserved |
| Query rail | Tick marks along the transcript, one per user message and checkpoint; hover to preview, click to jump |
| Attachments | Any file type; parsed into the library |
| Grounding prompt file | Mark a file as the project's grounding prompt so it is treated as context, not just an attachment |

## 6. Vault editor (desktop)

Obsidian-compatible: plain `.md` files, `[[wikilinks]]`, backlinks, outline, local graph, callouts, LaTeX, Mermaid, frontmatter. Agent edits arrive as accept or reject diffs. Chat, Split, and Editor views. A project-level `TERN.md` holds project rules the agent reads.

## 7. Later and parked

| Idea | Status |
| --- | --- |
| Import whole chats from other assistants into a project with provenance | Later |
| Universal connector: make a typed tool from a service's API docs when it has no MCP server | Later |
| Learning from frustration signals (repeated corrections) as feedback, saved as reviewable preferences | Later |
| Find, install, or write skills for the current project; read a GitHub repo without cloning it | Later |
| Shared research and study rooms with per-person cursors and spectating | Parked until after v1 |
| Hardware research pack (datasheets, part comparison) | Parked |
| Interactive explainer export of a report or session | Later (builds on Explain in 3D) |
| Computer use | Later, scoped to research chores (library portals, downloading PDFs, citation tools), with per-app permission; off by default |

## 8. Open questions

| # | Question |
| --- | --- |
| 1 | Palette: Mono and Lichen (recommended), Graphite and Brass, or Ink and Paper |
| 2 | UI font: IBM Plex Sans or Inter, with Plex Mono |
| 3 | Which decision-model provider to integrate first, after verifying its API and pricing |
| 4 | Vector store: `sqlite-vec` or LanceDB |
