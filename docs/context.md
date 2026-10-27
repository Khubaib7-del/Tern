# Context window and compaction

Every model reads a limited number of tokens at once: its context window. Tern shows how full it is and keeps long chats inside it.

## The context gauge

The ring beside the composer's send button is a small donut. Its arcs are coloured by how full the window is: green while there is room, amber from 70 %, red from 90 %. Hover it for the context popover:

| Part | What it holds | Colour |
| --- | --- | --- |
| System + instructions | Tern's system prompt, `TERN.md` rules, project instructions | slate |
| Tools | Tool descriptions and the tool protocol | teal |
| Attachments and sources | Files, pages, and MCP resources in the history | sand |
| Conversation | Messages and answers | lichen (the accent) |
| Summary | The pinned summary of compacted turns, after a compaction | violet-grey |
| Free | What is left of the window | the empty track |

The total is the provider's own count of the latest call when it gives one; the split by kind is always an estimate (about four characters a token). Before the first call, and right after a compaction, the whole figure is an estimate and starts with `~`.

The popover has two actions: **Compact now** and **New chat with summary** (a handoff packet that starts a fresh chat).

## Compaction

When the context passes 80 % of the window after an answer, Tern compacts the chat:

1. The chat's own model writes a summary of the older turns: facts, figures, decisions, open questions, links, and the user's goals.
2. The model's history becomes the pinned summary ("Earlier in this chat …"), then the turns that carry attachments, then the last four turns, word for word. The system prompt and tools are rebuilt on every call, so they always go in whole.
3. A `session.compacted` event is logged with the summary and the numbers of the turns it stands for. The event log is append-only: the folded turns stay in the database and in the transcript.
4. The transcript shows **Compacted earlier messages · View summary** after the answer.

A later compaction folds the earlier summary and the next older turns into a new summary. Reopening a chat rebuilds the same history from its last `session.compacted` event. **Compact now** in the context popover does the same at any time.

If the summary call fails, the history stays as it was and the next answer tries again.

## Settings

In `preferences.json`:

| Key | Default | Meaning |
| --- | --- | --- |
| `compact_off` | `false` | `true` turns automatic compaction off (**Compact now** still works) |
| `compact_at_percent` | `80` | The share of the window that starts a compaction (30 to 98) |
| `compact_keep_turns` | `4` | Turns kept word for word (1 to 20) |

The code is `tern_agent::compact` and `ChatSession::compact`.

## Long tool results

A tool result over 12,000 bytes (a long page, a PDF, a big file) is not cut off at the end any more (#72). The model sees its first 70 % and last 30 %, cut on line ends, with a line in between saying how many characters were left out. The whole text is saved for the chat as `<data>/runs/<session>/results/<id>.txt`, and the model can read any range of it, up to 12,000 characters a call, with the `read_result` tool. Errors are never cut.
