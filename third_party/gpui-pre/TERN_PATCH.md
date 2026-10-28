# gpui-pre 0.3.9, patched for Tern

A copy of the `gpui-pre` 0.3.9 crate from crates.io (Zed's `gpui`,
Apache-2.0, see LICENSE-APACHE), used through `[patch.crates-io]` in the
workspace `Cargo.toml`. The published crate's examples are left out (and
their `[[example]]` entries removed from `Cargo.toml`); nothing else
changed except below. Search for "Tern patch".

- `src/text_system/line_layout.rs`, `LineLayout::x_for_index`,
  `closest_index_for_x` and `index_for_x`: on a line with right-to-left
  text (a run whose glyphs, in screen order, fall in index: DirectWrite
  with Tern's `gpui-pre-windows` patch, cosmic-text), the caret before a
  right-to-left character is at its right edge, after the last one at its
  left edge, and a click picks the boundary whose caret is nearest
  (`rtl_clusters`, `caret_x`). GPUI assumed indices grow with x, so in the
  composer the caret after typed Urdu sat at the line's right end (its
  start) and a click at the left end landed inside the last word: `??`
  typed there went before the last word, and the message bubble and the
  sidebar title showed it so (#68). Left-to-right lines take GPUI's own
  code unchanged.

- `src/window.rs`, `Window::current_view` (#69): with no view on the
  rendered-entity stack (an element drawn outside any view: a tooltip, a
  drag preview, a prompt, or a frame asked for from an event handler) it
  returns the window's root view instead of `unwrap()`ing `None`, which
  aborted Tern on Windows (crash log 2026-10-25, `window.rs:5353`,
  "panic in a function that cannot unwind" inside the window procedure).
  It logs a warning; notifying the root view only redraws the window.

- `src/svg_renderer.rs`, the SVG fallback font (#73,
  `select_matching_fallback`): usvg 0.46's `default_fallback_selector`
  skips a face only when its style, weight *and* stretch all differ from
  the base face, so any face sharing one of them qualifies. A character
  IBM Plex Sans lacks (`→`, `∗`, `√` in a Mermaid label) was drawn in the
  first system face with it, often a bold italic one, and the label's
  text after it came out bold italic (gpt-oss's `Linear (d_model →
  4*d_model)`). A face matching the base in all three is tried first;
  usvg's own selector stays the last resort.

The composer's right alignment, swapped arrow keys and selection on
right-to-left text are in `third_party/gpui-base` (its TERN_PATCH.md).

Drop this copy when an upstream gpui-pre release handles right-to-left
carets.
