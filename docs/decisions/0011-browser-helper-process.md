# ADR 0011 · The browser's page engine runs in its own process

| Field | Value |
| --- | --- |
| Status | Accepted (replaces the "native child of the GPUI window" part of ADR 0010) |
| Date | 2026-10-24 |
| Decider | Khubaib Nazeer |

## Context

ADR 0010 put the system web view (through `wry`) inside Tern's window as a native child, created on first use from the browser pane's prepaint. On Windows this froze the whole app when the Browser button was clicked: a busy cursor, then no clicks and no way to close, then the process exited by itself after 30 to 40 seconds. It reproduced every time.

**Root cause** (from the wry 0.57.1, webview2-com 0.38 and gpui-pre-windows 0.3.9 sources):

1. `WebViewBuilder::build_as_child` on Windows creates the WebView2 environment and controller synchronously. Both are asynchronous COM operations, so wry waits for them with `webview2_com::wait_with_pump`, a nested `GetMessage` / `DispatchMessage` loop that runs until the completion handler fires.
2. Tern called it from inside GPUI's frame (the page canvas's prepaint), so that nested loop ran on GPUI's UI thread while GPUI was in the middle of drawing, with the app state borrowed for the frame.
3. Every message the nested loop dispatched went back into GPUI's window procedure. GPUI defers re-entrant `WM_PAINT` (`draw_coordinator.try_begin_draw` fails, so it validates the region and returns), so the window never repaints: it looks frozen. Input and close messages reach callbacks that are already taken for the frame being drawn, so clicks and the close button do nothing. GPUI's own foreground tasks (`WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD`) also run inside the nested loop. When one of them finished (opening the pane also started a crawler read, which comes back as one of these tasks, typically tens of seconds later), it tried to borrow the app state the outer frame still held. The borrow panicked inside a window procedure, which cannot unwind, so the process aborted: the "crash after 30 to 40 s".
4. Any WebView2 creation that stalls (first-run runtime setup, a locked profile folder, a slow machine) keeps GPUI inside that loop just as long.

The same design could never work on Linux (WebKitGTK needs a GTK main loop that GPUI does not run), and on macOS it relied on a native child view that GPUI's overlays cannot draw over.

## Decision

- The engine runs in a separate helper process, **`tern-browser`** (crate `crates/tern-browser`). It uses `wry` with `tao` and its own event loop: WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux. No front end links an engine crate. `dependency_rules.rs` checks that `tern-desktop` and `tern-tui` do not depend on `wry`, `tao`, `webview2-com` or `webkit2gtk`, and that `tern-browser` depends on `tern-core` alone.
- **Protocol** (`tern_core::browser_ipc`): one JSON object per line. Commands go to the helper's stdin: `navigate`, `back`, `forward`, `reload`, `bounds` (a screen rectangle in logical px), `show`, `hide`, `detach`, `on_top` and `quit`. Events come back on its stdout: `ready` (engine name, and whether it can place its own window), `url`, `title`, `loading`, `focus`, `closed` and `error`. Lines that do not parse are ignored on both sides.
- **The desktop never waits on it** (`crate::webview::Host`). The helper is spawned, then written to and read from on two background threads, and each frame only drains a channel. A 250 ms timer redraws while the helper is starting or up. If no `ready` arrives within **5 s**, or the helper exits or crashes, the pane says "Browser stopped (why)" and offers **Restart**, and Tern's reader view (ADR 0005) shows the page as text in the meantime. Tern keeps running whatever the helper does. When Tern exits, the helper's stdin closes and the helper exits too.
- **Placement.** The helper's window is borderless and docked over the browser pane's page area: the pane sends its rectangle on the screen whenever it moves or resizes. On Windows the window is *owned* by Tern's (`--owner <HWND>`), so it stays above Tern, minimises with it and has no taskbar button. On macOS and Linux X11 it is kept on top only while Tern or the page is in front, and hidden otherwise. The existing hide rules from ADR 0010 still apply: Explanation only, Settings, a modal, or the pane closed. Where a client cannot place its own window (Wayland), or when the user picks **Open the page in its own window**, the page opens as an ordinary attached window and the pane shows the text view.
- **CLI.** `tern browse <address or words>` prints the reader view of a page and Tern's explanation with numbered citations, or the search results for words. `--open` launches the same helper as a standalone window (`tern-browser --standalone --url …`).
- **Packaging.** `tern-browser` ships next to `tern-desktop` and `tern` in the Windows zip, inside `Tern.app/Contents/MacOS` and next to the CLI in the macOS zip, and in the Linux tarball. Linux needs WebKitGTK 4.1 at run time, and CI installs `libwebkit2gtk-4.1-dev` and `libgtk-3-dev` to build it. `TERN_BROWSER_HELPER` overrides the path.

## Consequences

- A hung or crashing page, or a WebView2 runtime that is missing or slow, can no longer freeze or kill Tern. The worst case is the text view plus a Restart button.
- The browser now works on Linux (X11 docked, Wayland as an attached window), which was reader-only before.
- A docked window is not a child view. While Tern is dragged, the page catches up a frame or so late, and GPUI menus drawn over the pane still need the hide rules.
- One more binary in each artifact, about the size of a small tao app. The engines themselves remain the system's own.
- The protocol is versioned (`VERSION`) and tolerant of unknown lines, so the app and the helper can drift by a release without breaking.
