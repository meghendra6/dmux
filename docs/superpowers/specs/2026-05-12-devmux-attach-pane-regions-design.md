# Devmux Attach Pane Regions Design

## Goal

Add an internal pane-region map for the server's rendered multi-pane attach
layout. The map records which rendered rows and columns belong to each pane, so
a later mouse-focus PR can translate a terminal mouse coordinate into a pane
index.

This slice does not enable mouse focus, parse attach-time mouse events, or
change protocol output. It only makes the renderer produce tested region
metadata alongside the current text snapshot.

## Current State

- Unzoomed multi-pane attach renders polling `ATTACH_SNAPSHOT` frames.
- Snapshot rendering is server-side in `src/server.rs`.
- `render_attach_pane_snapshot` turns a `LayoutNode` plus pane screen text into
  client-only text.
- Horizontal splits insert ` | ` between left and right rendered panes.
- Vertical splits insert one separator row of `-` characters.
- The renderer currently returns only text, so pane indexes are lost after
  layout composition.
- Mouse focus remains a README limit.

## Approaches Considered

1. **Compute regions in the server renderer.** The renderer already knows the
   `LayoutNode`, pane indexes, separator rows, separator columns, and padding
   widths. Returning region metadata from this layer keeps coordinates aligned
   with the exact text that `ATTACH_SNAPSHOT` emits.
2. **Reconstruct regions in the client from rendered text.** This avoids new
   server structs, but it would require parsing visual separators and guessing
   nested split ownership from plain text. That is brittle and duplicates
   renderer logic.
3. **Expose a new protocol response now.** A future mouse-focus client will need
   regions, but this PR can keep the metadata internal and tested first. Adding
   protocol surface before there is a caller would expand the slice.

## Chosen Design

Use approach 1. Refactor the server renderer to produce:

```rust
struct RenderedAttachLayout {
    lines: Vec<String>,
    regions: Vec<PaneRegion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PaneRegion {
    pane: usize,
    row_start: usize,
    row_end: usize,
    col_start: usize,
    col_end: usize,
}
```

Coordinates are zero-based. `row_start` and `col_start` are inclusive;
`row_end` and `col_end` are exclusive. A point is inside a region when
`row_start <= row < row_end` and `col_start <= col < col_end`.

Regions are rectangular pane allocations, not just visible text cells. If a
pane is padded because its sibling is taller or wider, the blank cells still
belong to that pane. Separator columns in horizontal splits and separator rows
in vertical splits do not belong to any pane.

The existing `render_attach_pane_snapshot` remains the public text-returning
helper. It calls the new renderer and converts `lines` back to the existing
CRLF text. No protocol changes are made.

## Coordinate Rules

For a leaf pane:

- lines are `screen_lines(screen)`
- width is the maximum line width using the renderer's current
  `chars().count()` semantics
- height is the number of rendered lines, at least 1
- region is `{ row: 0..height, col: 0..width }`

A pane whose current screen renders as an empty line has a zero-width region in
this text snapshot. This preserves the existing snapshot text exactly; a later
terminal-size-aware layout can expand pane allocations beyond visible text.

For a horizontal split:

- left child starts at row `0`, col `0`
- right child starts at row `0`, col `left_width + 3`
- the three separator columns from `left_width` through `left_width + 2` are
  excluded from all regions
- if a child is shorter than the joined row count, only descendant regions that
  touch that child's bottom boundary have `row_end` expanded to the joined row
  count. This maps bottom padding without making upper nested panes overlap
  lower nested panes.

For a vertical split:

- first child starts at row `0`, col `0`
- second child starts at row `first_height + 1`, col `0`
- the separator row at `first_height` is excluded from all regions
- if a child is narrower than the joined width, only descendant regions that
  touch that child's right boundary have `col_end` expanded to the joined width.
  This maps right padding without making left nested panes overlap right nested
  panes.

For nested layouts, offsets accumulate recursively.

If the layout tree and visible panes disagree, snapshot text keeps the existing
ordered-section fallback and no region metadata is returned. Future mouse focus can
choose to ignore clicks when no regions are available.

## Data Flow

1. `ATTACH_SNAPSHOT` asks the active session for `AttachLayoutSnapshot`.
2. The server builds `PaneSnapshot { index, screen }` values as it does today.
3. `render_attach_pane_snapshot` calls `render_attach_layout`.
4. `render_attach_layout` recursively composes text lines and pane regions.
5. `render_attach_pane_snapshot` returns the same text format as before.
6. Region metadata remains internal until a later PR introduces a caller.

## Tests

Add tests before implementation:

- horizontal split regions exclude the ` | ` separator columns
- vertical split regions exclude the separator row
- nested split regions accumulate row and column offsets correctly
- fallback rendering returns no regions when layout and pane list disagree
- existing attach snapshot integration tests still pass

## Out Of Scope

- mouse event parsing in multi-pane attach
- selecting panes from mouse clicks
- protocol changes for exposing regions to the client
- terminal-size-aware layout allocation
- resizing panes based on region dimensions
- composed-layout copy-mode
- event-driven redraw

## Acceptance Criteria

- Existing attach snapshot text output is unchanged.
- The renderer can return pane regions for horizontal, vertical, and nested
  layouts.
- Separator rows and columns are not mapped to panes.
- Fallback rendering produces no regions.
- README still lists mouse focus as pending, while docs record the internal
  region mapping foundation.
- Full verification passes before opening the PR.
