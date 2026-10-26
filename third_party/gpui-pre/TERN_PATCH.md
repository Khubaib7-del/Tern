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

The composer's right alignment, swapped arrow keys and selection on
right-to-left text are in `third_party/gpui-base` (its TERN_PATCH.md).

Drop this copy when an upstream gpui-pre release handles right-to-left
carets.
