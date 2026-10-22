# ADR 0006 · Desktop shell on GPUI Kit, brought forward

| Field | Value |
| --- | --- |
| Status | Accepted |
| Date | 2026-10-22 |
| Decider | Khubaib Nazeer |

## Context

The owner tests Tern in the desktop app rather than the terminal. Phase 3 ended with a working chat engine (`ChatSession`) and `tern ask`, so the desktop shell is brought forward from phase 6 to run on that engine now. Phases 4 and 5 still come next for the engine; the desktop grows alongside them.

GPUI has no stable release and ships no text input, buttons, or Markdown view. Writing those by hand would cost weeks before a single chat could be typed.

## Decision

| Part | Decision |
| --- | --- |
| Toolkit | `gpui-kit` 0.8 (Apache-2.0, Longbridge), which pins a matching GPUI build (`gpui-pre` 0.3.9) and brings styled components: multi-line input, buttons, title bar with window controls, Markdown view, spinner, Lucide icons. Pinned to 0.8; upgrades are their own PR |
| Crate rule | `tern-desktop` depends on `tern-agent` and `tern-core` only, like the TUI. It never calls a provider, the crawler, the keychain, or the store |
| Threads | GPUI owns the main thread. Turns run on a separate tokio runtime (`bridge.rs`); events, approvals, and the outcome come back over one channel. The session is handed to the runtime for a turn and handed back with the outcome, so two turns can never overlap |
| State | `model.rs` holds everything the window shows, with no GPUI types, so it is unit tested without a display. `view.rs` only renders it and forwards clicks |
| First slice | Title bar (Tern, sidebar toggle, Research or Study switch at top centre, theme toggle, run panel toggle), icon rail (new chat, chats; library, editor, settings shown as coming), chat list from the store, transcript (user bubbles, Markdown answers with time and step count), working line with spinner and timer, inline approval card (allow once, allow for this chat, deny), composer (Enter sends, Shift+Enter new line, stop while running, provider and model, tokens used of the context window), run panel with the turn's steps |
| Look | Neutral greys only, dark by default, light on the toggle. The palette and type land with ADR 0002 in one file (`theme.rs`) |
| Testing on Windows | CI builds release `tern-desktop.exe` and `tern.exe` on every PR and uploads them as the `tern-windows-x64` artifact |

## Alternatives

| Option | Why not now |
| --- | --- |
| Plain `gpui` from crates.io (0.2) | Older than the Zed build the kit pins, and no input or Markdown components |
| Tauri or a web view | Rejected in ADR 0001: two languages, and the web view would hold page text next to instructions |
| Wait for phase 6 | The owner wants to test in the desktop app now |

## Consequences

- First builds are heavy (about 600 crates); CI caches them.
- Study shows a note that it arrives in phase 7 and keeps the chat in Research.
- Not in this slice: streaming tokens, reopening past chats, providers screen, context ring drawing, source rows with logos, tabs, Scout, vault editor. Each is a later PR against the phase 6 table.
- If the kit stalls or breaks with GPUI, the shell's own code is small (the model is toolkit free), so moving to raw GPUI components is contained to `view.rs`.

## Revisit when

GPUI publishes a stable release, or `gpui-kit` stops tracking it.

## Addendum: Providers screen (PR #14)

- The rail's Settings button, the model label under the composer, and the
  "Set up a provider" button on the empty state open the Providers screen.
- Two sections, Models and Web search, one row per catalog entry. Each row
  starts with a logo tile. Until the service glyph set is approved, the tile
  shows the service's initial on a neutral square.
- Add opens a masked key field. The bridge tests the key against the
  service and saves it to the OS keychain only when it works; the field is
  cleared as soon as the test starts. "Get a key" opens the provider's key page.
- Saved and local rows offer Use (make default), Test, and Remove. The first
  working key becomes the default on its own so the chat can open.
- The default provider's row lists its chat models; picking one saves it.
- Choices (provider, model, search service) live in `preferences.json` in
  the app data directory (`tern_agent::Preferences`). Keys never go there.
  Changing a choice opens a new chat with it.
