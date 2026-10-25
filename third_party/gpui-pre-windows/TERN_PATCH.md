# gpui-pre-windows 0.3.9, patched for Tern

A copy of the `gpui-pre-windows` 0.3.9 crate from crates.io (Zed's
`gpui_windows`, Apache-2.0, see LICENSE-APACHE), used through
`[patch.crates-io]` in the workspace `Cargo.toml`.

Two changes, in `src/direct_write.rs`. Search for "Tern patch".

- `DrawGlyphRun`: DirectWrite hands a right-to-left glyph run (odd
  `bidiLevel`) in logical order and expects it drawn from the run's right
  edge leftwards. The original placed every run's glyphs left to right, so
  Urdu and Arabic showed reversed with unjoined-looking letters. The patch
  places RTL glyphs right to left and stores them in left-to-right screen
  order.
- Each run is placed at the `baselineOriginX` DirectWrite passes (its left
  edge, or its right edge for a right-to-left run) instead of after the
  previous run, and `layout_line` sorts the runs into screen order.
  DirectWrite draws runs in logical order, so an Urdu word followed by a
  `:` from a fallback font showed the colon on the wrong side (#66).

Drop this copy when an upstream gpui-pre release fixes RTL runs.
