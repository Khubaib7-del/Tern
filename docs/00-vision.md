# 00 · Vision

> Status: draft, 21 October 2026. Everything here describes intent. Nothing is built yet.

## The one-line version

Tern is a local-first agent harness for research and study: it turns a rough question into a verified, cited report, and turns what you read into something you can actually learn from.

## Why it exists

Tern starts as the research tool for its author's final-year project. The problem it answers is common to anyone who researches with AI today:

| Problem today | What Tern does instead |
| --- | --- |
| Chat answers that sound right but cite nothing, or cite the wrong thing | Every claim is linked to an exact source span and given a verdict before it reaches the report |
| Research lives in long chats that drift, loop, and lose early decisions when context is compacted | Goals, constraints, and decisions are kept in a ledger outside the transcript, so they survive compaction and model swaps |
| One vendor, one model, one bill you can't predict | Bring your own keys for many providers, or run local models; see the expected cost of a run and each step before it starts |
| Reading and studying are separate tools | The same project feeds a Study harness: tutor, flashcards, quizzes, and viva practice built from your own sources |
| Agents that hide what they are doing | One quiet summary line per turn, the full log one click away, and a watcher you can ask what the agent is doing right now |
| Cleaning up years of chats by hand | Stable IDs for every query, file, and source; move or remove a range of a chat; bulk-manage a library |

## Who it is for

| User | What they need from Tern |
| --- | --- |
| The author, first | A daily research and study tool for an FYP that will run for months |
| Students | Literature reviews, fact checks, study packs, and viva preparation grounded in real sources |
| Researchers | Reproducible runs: a methods log, a run report, exportable citations |

## The idea behind the harness

The model is the engine; the harness is the driver. A good harness should get far more verified, useful work out of a model, including smaller and local ones, than the model manages on its own. Tern's quality is measured per unit of verified work, not per token.

Two models work side by side:

| Slot | Job |
| --- | --- |
| Reasoning model | Plans, reads, writes. Any BYOK provider or a local model. |
| Decision model | Scores fixed choices quickly and cheaply: which source to read first, which tool next, does this claim pass, what to do now and what later. Hard checks always override its score. |

## Principles

| # | Principle | In practice |
| --- | --- | --- |
| 1 | Evidence before claims | A claim is a hypothesis until a source or check confirms it. Unverified stays unverified; it is never upgraded because a model sounds sure. |
| 2 | Local-first | Vaults, libraries, indexes, and run logs live on your machine as plain files and a local database. |
| 3 | Your keys, your choice | BYOK for remote providers; local models are first-class, not a fallback. |
| 4 | Show what leaves the machine | Before anything is sent to a provider, Tern says what and to whom. |
| 5 | The process folds away, the result stays open | Work collapses to one line; reports, sources, and verdicts stay in view. |
| 6 | Calm by default | Black and white, one restrained accent, motion only on real events, no decoration that moves when nothing is happening. |
| 7 | The user steers | Plans are shown before money is spent; runs can be steered, paused, checkpointed, and resumed. |
| 8 | One core, many surfaces | The core owns state and security; the TUI and desktop app only render its events. |

## What success looks like for the first usable version

- Ask a research question in the TUI on Windows, approve a plan, and get a short report in which every claim has a working citation and a verdict.
- See the expected cost before the run and the actual cost after it.
- Close the app mid-run and resume from a checkpoint without repeating paid work.
- Turn the same sources into a flashcard deck and a short quiz.

## Not goals, for now

- Distribution: no installers, website, or public releases until the core is proven.
- A hosted service or accounts: Tern runs on the user's machine.
- Building a web search index: Tern uses search APIs or a self-hosted search engine for discovery and its own crawler for reading.
- Coding-agent features beyond what research needs.
