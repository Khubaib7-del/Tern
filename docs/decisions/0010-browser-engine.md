# ADR 0010 · The built-in browser's page engine: the system web view through wry

| Field | Value |
| --- | --- |
| Status | Accepted; the in-window engine is replaced by a helper process in ADR 0011 |
| Date | 2026-10-24 |
| Decider | Khubaib Nazeer |

## Context

The desktop app's browser pane (ADR 0005 gave it Tern's reader: a page fetched by Tern Crawl and shown as clean Markdown) could not show a page as the site draws it: no layout, no images in place, no scripts, no sign-in. Feedback asked for a real browser beside the chat (tabs, an address bar that takes words or an address, back, forward, reload), with Tern's explanation of the page next to it.

GPUI draws everything itself on the GPU; it has no HTML engine, and writing one is out of scope. The options were:

| Option | Cost | Verdict |
| --- | --- | --- |
| Bundle Chromium (CEF) | ~150 MB per platform, its own process model and build | Rejected: size and build time for a pane |
| A Rust engine (Servo, Blitz) | Incomplete on real sites; large dependency tree | Rejected for now |
| **The system web view through `wry`** | WebView2 (Windows, Evergreen runtime ships with Windows 10/11), WKWebView (macOS, part of the OS); one small crate | **Chosen** |

## Decision

- `tern-desktop` depends on `wry` (0.57, default features off) **only on Windows and macOS** (`[target.'cfg(any(target_os = "windows", target_os = "macos"))'.dependencies]`). No `tern-*` dependency changes, so the dependency rules (`docs/03-architecture.md` §3) still hold.
- One web view per Tern window, created as a **native child** of the GPUI window (`WebViewBuilder::build_as_child`, through the window's `raw-window-handle`) on first use, and reused across tabs: a tab switch loads that tab's address. Its profile (cookies, cache) lives in Tern's cache directory (`<cache>/webview`).
- The browser pane draws an empty canvas where the page goes; every frame its prepaint hands the canvas bounds to `crate::webview::Host::place`, which moves the native view onto them. A native child sits above everything GPUI draws in its rectangle, so the host **hides the view** whenever the page is not what shows: the pane is closed, the toggle is on Explanation, Settings or a modal (onboarding, the harness switcher) is open, or the page element was not drawn last frame.
- Navigation from the live page leads: when a link clicked inside the page moves it, the address bar, the tab title and the explanation side follow (the crawler reads the new address).
- The pane toggles **Page | Explanation | Side by side** (side by side by default). The explanation side is the page as Tern Crawl reads it (or, for words, the search service's results, each opening in the pane) and Tern's cited answer about it, asked for with one button.
- Words typed in the bar load a DuckDuckGo results page on the page side (`engine_address`), and Tern's own search on the explanation side.
- **Linux** has no engine in this build: wry needs WebKitGTK and a GTK main loop running beside GPUI's, which GPUI does not provide. The Page side shows Tern's reader there, with the same tabs, bar and explanation. If the engine fails to start on Windows or macOS (for example, no WebView2 runtime), the pane says so once and falls back to the reader too.

## Consequences

- Real pages, sign-ins and scripts work on Windows and macOS at the cost of one small dependency and no bundled engine; the installer size barely changes.
- The page cannot be drawn under GPUI overlays (menus, toasts) while it shows; the hide rules above keep Tern's own UI usable, and anything new that floats over the pane has to join them.
- The engine is touched only on the GPUI main thread, as every system web view requires.
- Linux users get the reader view until GPUI and WebKitGTK can share a loop (or an offscreen engine is worth its cost); this ADR is revisited then.
- Tests cover the browser state (tabs, history, engine-led navigation, the search address) on every OS; the engine itself is exercised by the Windows desktop build and by hand.
