# Internals handbook (draft)

> Status: draft, started in PR #85 (31 October 2026). It grows pull request by pull request and is finished in #116, the handbook bundle. It explains how a turn runs inside Tern, for someone changing the engine. The architecture overview is [03-architecture.md](../03-architecture.md).

## 1. A turn, end to end

A chat message becomes one **turn** with a stable turn id. `ChatSession::send` (in `tern-agent/src/chat.rs`) builds the tools, the instructions and the run hooks, and hands the message to `TurnLoop::run` (`tern-agent/src/engine.rs`). Inside, a `Run` drives the turn:

1. **Prepare.** The turn fingerprint is recorded on `turn.started` (prompt version and hash, tools hash, model, provider, native tools, sampling), so two chats can be compared later.
2. **Prefetch.** Before the first model call Tern reads what the message plainly asks for (`prefetch_calls`): a file named by its full path, a web search for "use the web", and `scholar_search` for an academic question. A follow-up that only points back at the previous answer ("read the top 5 papers from that list", `followup::points_back`) runs no prefetch; the model is handed the previous answer's paper ids instead (`followup::paper_ids`).
3. **Calls.** The loop asks the model, runs the tools it calls (`Run::dispatch` → `Run::tool_step`), and feeds the results back, until the model answers or a budget or a stop ends the turn.
4. **End.** `turn.finished` (or `budget.exceeded` / `run.cancelled`) carries the answer, the cost and the partial text; a turn that stopped before its answer also writes a `turn.receipt`.

## 2. Never losing a turn

- **History.** `PastTurn::exchange()` admits failed, cancelled and budget-ended turns with their question, partial text, sources and a one-line tool digest, so the next message sees them.
- **Checkpoints.** A quick turn is saved at its start and after every tool result (`Run::save`): the messages so far, the write marks, and the **step journal**. Streamed text is checkpointed in the event log every second or 200 characters.
- **Journal.** Each tool call that ran is a `JournalStep { step, key, tool, result }`, keyed by the call's cache key under the turn id. On Continue, `tool_step` looks the call up first and replays it (`tool.replayed`) instead of running it.
- **Resume.** A resumed turn keeps its turn id; its saved partial answer goes back as an assistant message followed by `RESUME_PROMPT`, so the model goes on from where the text stopped.
- **Receipts.** `TurnReceipt` says how the turn stopped (`PausedBy`: user, budget, daily quota, network, closed) and what it kept (`KeptSummary`). A turn found on launch with no end gets `RunCheckpoint::closed_receipt()`.

## 3. Rate limits, windows and budgets

- **Governor** (`tern-providers/src/governor.rs`): per key and model, request and token windows seeded from `limits.json` and corrected from `x-ratelimit-*` headers and 429 bodies. Before each call the engine asks it to wait, to shrink the request to fit (pre-fit), or to pause for the day. A daily limit is never retried.
- **Envelope.** The largest request one call may carry. `read_batch_for(envelope)` sets how many `paper_read` results one step returns (3, 2 under 24k tokens, 1 under 12k); `paper_share` trims each to its share, with the full text in the result store.
- **Estimator** (`tern-providers/src/estimate.rs`): characters × a per-model ratio learned from reported prompt tokens.
- **Budgets.** Model calls, tool calls, tokens, and dollars (`RunHooks.max_cost_micros`, priced by `tern-providers/src/prices.rs` from the models.dev snapshot). Failed calls are not counted. With `wrap_up` on, one writer call is kept in reserve so a spent budget still yields an answer listing its gaps (`CompletedWithGaps`).
- **Fallback.** `RunHooks.fallback_model` (from `within_provider_fallback`) takes the rest of one turn after a daily limit, with a `model.switched` event; off when `Preferences.fallback_off` is set.

## 4. Tool-call hygiene

- `protocol::normalize_args` unwraps JSON-string arguments and `{tool, args}` / `arguments` / `parameters` envelopes before validation.
- `Run::skip_fetch` answers without running: an address that already failed this turn (`fetch_key`), a paper already read for the same question, or a `paper_read` past the step's batch. Skips are `tool.skipped` events and reach the Ledger with their reason.
- `redo.rs` (#82) stops write loops: a `file_write` repeating the text a path already holds does not run, and after two writes to one path more are refused.
- `toolcache.rs` keeps search and read results; searches are kept **per chat** (`ToolCache::set_chat`), reads are shared.
- `quickread.rs`: after the first `web_search` of a general question (`RunHooks.quick_reads`, on in the app), the top three results are read and each page's opening and best passage go back with the search.

## 5. Signals

`signals.rs` taps every event a chat records (through the chat's event tee) and turns provider, tool, verifier and loop events and user corrections into deterministic signatures (`tool:read_url:http #`, `loop:model.repeated`, `correction:asked`). `IncidentStore` counts them across chats in `signals/incidents.json`. `learned_context` is the only way learned entries will reach a prompt: entries for the current `HARNESS_VERSION` only, switched off by `TERN_NO_LEARNED=1`, with a snapshot per version. No entries are learned yet.

## 6. Developer switches

`TERN_DEV=1` with `TERN_CHAOS` (`chaos.rs`) wraps the model: `429-short`, `429-daily`, `drop:N` (stream cut after N characters), `bad-args`, `kill`, comma-separated per call. `TERN_NO_TOOL_CACHE=1` turns the tool cache off.

## 7. Fixes from the #85 Windows testing (#90)

- **Scholarly queries** (`tern-agent/src/query.rs`, `scholar.rs`): `scholar_keywords` turns a message into at most eight keywords and drops instruction words; one `scholar_search` call runs up to three queries at once; when the ranked evidence is not enough, `broader` gives one or two wider wordings (acronym expansions, fewer terms) and the search runs again before "not enough evidence". The result line logs `queries`, `widened`, `found` and `kept`.
- **Citation requests** (`intent::snowball_request`): "papers citing X" and "follow the citations of arXiv …" name a seed and a direction, and the prefetch runs `paper_snowball` instead of a keyword search. Citers come from OpenAlex (`openalex_citers`) sorted by citation count.
- **Resume** (`engine.rs`): `resume_prompt` hands the model the exact kept tail; `stitch(kept, lead, more)` joins the continuation, dropping text the model repeated and a sentence it restarted.
- **Chaos kill** (`chaos.rs`): `kill_point` is awaited before `turn.finished` is written, so a `kill` queued for a call fires mid-turn, never after the end is saved.
- **Fallback** (`chat.rs`): a known smaller same-provider model for every provider; the desktop passes the setting to the open chat; one `model.switched` row per turn; a paused turn's reason appears in its receipt only.
- **Small talk**: `small_talk_note` adds `SMALL_TALK_NOTE` to a greeting, and the base prompt says a greeting never restarts an earlier task.
- **Author-year markers** (`verify.rs`): an author and year in prose that matches a paper the turn listed or read gets that paper's `[n]`, so the claim checker sees it.
- **Desktop** (`tern-desktop/src/follow.rs`, `cull.rs`, `model.rs`): `Follow` reads the scroll offset each frame; a move up lets go of the end, coming back within 48 px or **Jump to latest** takes it again, and text arriving calls `follow_end` (scrolls only while following). The chat culls with `CHAT_MARGIN` (1600 px) and always draws its last three entries. `Model::panel_follows_chat` runs from render and only changes model state: it saves the panel (open, tab) of the chat left in `chat_panels` and restores the opened chat's, or a plain panel; `new_chat` saves the outgoing chat's panel first and resets `last_run`.

