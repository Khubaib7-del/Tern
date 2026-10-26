# The built-in browser

Tern's browser sits beside the chat. The page is the real site, shown by the system web view (WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux). It runs in a helper process, `tern-browser`, docked over the pane, so a slow or crashing page cannot freeze Tern. [ADR 0010](decisions/0010-browser-engine.md) explains the engine choice and [ADR 0011](decisions/0011-browser-helper-process.md) the helper process.

## Opening a page (#68)

- A link (a citation, a source row, a search result) opens the pane with chat and page **half and half**. Drag the divider anywhere between 30 % and 70 %.
- The sidebar folds away while the page is open and comes back when the page closes.
- The pane opens on the **Page** tab. **Explanation** shows the page as Tern Crawl reads it, and **Side by side** shows both.
- Scout keeps to the left of the pane, so it never covers the page.

## No flash on open or resize

The native page is a separate window. It is hidden straight away only when the pane is not on screen. A frame that misses its placement, or a moment when focus passes between Tern and the page, gets a 250 ms grace before the page is hidden. Before #68, a resize or opening the pane showed a blank frame. Run with `TERN_PERF=1` and `tern.log` gets a `PERF web show` or `PERF web hide` line each time, with the running counts of shows and hides.

## The run panel

The run panel (Plan, Sources, Report, Graph, Context) opens by itself only for deep research, and only when **Auto-open** is on. Turn it on or off with the switch in the panel's header, or in Settings › Appearance › Open run panel automatically. The choice is remembered. You can always open the panel yourself.
