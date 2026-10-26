# gpui-base 0.8.0, patched for Tern

A copy of the `gpui-base` 0.8.0 crate from crates.io (GPUI Component's
text-field engine, Apache-2.0, see LICENSE-APACHE), used through
`[patch.crates-io]` in the workspace `Cargo.toml`. Nothing else changed
except below. Search for "Tern patch".

- `src/input/base/state.rs`: `InputBaseState::set_right_to_left` (and
  `is_right_to_left`). The composer calls it when the first strong letter
  of its text is Urdu, Arabic, Persian or Hebrew, like `dir="auto"` on a
  web text field. Lines are right-aligned, multi-line text too. Upstream
  `set_text_align` works only on single-line fields. The Left and Right
  arrow keys (and Shift with them) swap, so Left moves towards the end of
  the text. Registered through `key_left`, `key_right`,
  `key_select_left` and `key_select_right` (`movement.rs`, `state.rs`).
- `src/input/base/state.rs`: `InputBaseState::set_text_highlights` gives
  colour (or other style) ranges for the current text in any mode. A
  textarea has no decoration collections, which only the code editor has.
  The ranges apply only while the text is the one they were set for. The
  composer colours `@` references with them. `element.rs` composes them in
  `highlight_lines` (`tern_text_highlights`).
- `src/input/editor/display_map/text_wrapper.rs` (`position_for_index`,
  `closest_index_for_x`, `wrapped_line_at`) and `src/input/base/element.rs`
  (`layout_range_corners`): with right-aligned text, each wrapped row is
  aligned by its own width, the same way it is painted. Upstream used the
  longest row for every row. A selection on right-to-left text is
  highlighted between its two caret positions, whichever is further left.

With `gpui-pre`'s caret patch (`third_party/gpui-pre/TERN_PATCH.md`), this
fixes the #68 composer. Typing `??` after Urdu puts the marks at the line's
left end, the logical end of the text.

Known gap: a selection that crosses from right-to-left into left-to-right
text on one row is drawn as one span between its carets, not as two
pieces.

Drop this copy when an upstream release handles right-to-left text fields.
