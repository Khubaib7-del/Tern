# ADR 0012 · Tool groups and lazy schemas

| Field | Value |
| --- | --- |
| Status | Proposed (for #73) |
| Date | 2026-10-26 |
| Decider | Khubaib Nazeer |

## Context

Every model call in a turn carries the full tool list twice over: the text protocol's list in the system prompt (`ToolRegistry::describe`, each tool's summary plus its whole input schema) and, since #65, the provider's native tool format (`ToolRegistry::specs`). Today Tern's own tools are few (`file_read`, `web_search`, `read_url`, `vault_search`, `vault_read`, `vault_write`, `load_skill`), but every MCP server the user adds puts all of its tools, with their schemas, into every request. A GitHub or filesystem server alone adds tens of tools and several thousand tokens to each call, whether the turn needs them or not.

That costs in three ways:

1. **Tokens and latency.** The schema block is paid on every model call of every turn, including the second call after a tool result.
2. **Choice.** Small and mid-size models (Codestral, the local ones) pick the wrong tool, or none, more often as the list grows, and some refuse tools outright when the list is long.
3. **Prompt caching.** Providers cache a request's longest unchanged prefix (Anthropic with explicit breakpoints, OpenAI, Mistral and Gemini implicitly). Anything that reorders or rewrites the tool list or the system prompt before the conversation invalidates the cache for the whole request.

PR #66 made one part cheaper already: every call in a reply (a provider's parallel tool calls, or several text-protocol blocks) now runs in the same round and all the results go back together, so a model that asks for three searches costs one extra model call, not three. Results are cut at `RESULT_LIMIT` (12 000 characters) each.

## Decision (proposed)

### Tool groups

Tools belong to a **group**: `files` (file_read and the workspace tools), `web` (web_search, read_url), `vault` (vault_search/read/write), `skills` (load_skill), and one group per MCP server (`mcp:<server>`). A group is offered **whole** or **not at all**, so the model never sees half a feature.

- Groups are switched on per turn from what Tern already knows before the first call: the mode (`Mode::allows`), the chat's workspace, `webintent::asks_for_web`, an `@` mention of a file or a server, the skills that match, and the groups the chat used in its last turns (sticky for the chat, so a follow-up keeps its tools).
- The user can pin groups on or off per chat and per project; the policy gate still decides each call, so a group being offered never grants anything.

### Lazy schemas

Inside the offered groups, the request carries a **compact index** rather than every schema: one line per tool, `name — summary (required args)`, at most about 120 characters each. Full input schemas are sent:

- for Tern's own tools in an offered group (small and stable), and
- for any tool on demand: a built-in `tool_schema { name }` tool returns the full schema, and the next call in the same turn includes that tool's native spec. A call whose arguments do not validate against the full schema gets the schema back in the error, so a model that guessed from the index fixes itself in one round.

MCP servers with more than a handful of tools are index-only by default.

### Prompt-cache-stable ordering

The request is laid out from most to least stable, so the cached prefix survives as long as possible:

1. Tern's system prompt (fixed per release),
2. tool specs and the index, **sorted by group then name** (the registry is already a `BTreeMap`; MCP tools stay after Tern's own), with no timestamps, counts or per-turn text inside,
3. the chat's pinned context (project rules, skills), then the history, then the new message.

A group switched on mid-chat is appended after the groups already offered in that chat rather than re-sorted in, so a later turn's prefix still matches. On Anthropic, cache breakpoints go after (2) and after the last history message.

### Result trimming

Tool results are trimmed by kind before they enter the history, not only cut at 12 000 characters: search results keep title, URL and snippet; a page read keeps the reader text up to a budget and says how much was left out; a file read beyond the budget keeps head and tail with the line numbers skipped. Results older than the last two rounds are replaced in the history by a one-line digest (`web_search "langchain creator" → 5 results, cited [1][3]`) once the conversation nears the model's context, with the full text kept in the ledger.

## Consequences

- Fewer tokens per call, most of all with MCP servers; a turn that needs no tools sends an index of a few lines.
- Small models see a short, relevant list, which should cut wrong-tool and no-tool replies. The eval suite gets cases for "right group offered" and "schema fetched then called correctly".
- One more round when a model needs a schema it only saw in the index. Groups that are likely for the message are sent with full schemas to keep this rare.
- The request layout becomes a contract: tests snapshot the tool block's order (like the Mistral Quick-turn body snapshot in #66) so a change that breaks caching is visible in review.
- Digested old results mean a model cannot re-quote a page it read many turns ago without reading it again; the ledger keeps it for the user.

## Not decided here

- Whether groups are chosen by a cheap classifier call instead of the rules above.
- Per-provider limits on parallel calls per round.
