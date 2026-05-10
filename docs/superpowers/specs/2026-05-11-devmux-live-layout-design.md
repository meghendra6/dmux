# Devmux Live Layout Foundation Design

## Goal

Build the next attach-layout foundation by preserving split direction in the
server's active window and using it to render deterministic multi-pane attach
snapshots. This is the prerequisite for live multi-pane attach because the
server currently accepts `split-window -h|-v` but discards the direction before
rendering.

This slice does not implement live multi-pane redraw. It turns the current
labeled pane list snapshot into a real split layout snapshot and keeps the
existing single-visible-pane live attach behavior unchanged.

## Current State

- `split-window -h|-v` exists and creates a new pane.
- The server tracks panes as a flat `PaneSet`.
- `handle_split` ignores the requested split direction.
- `attach` stays live when one pane is visible, including zoomed split windows.
- `attach` returns snapshot mode when multiple panes are visible.
- The multi-pane snapshot currently renders pane screens as labeled sections,
  not as a layout.

## Chosen Approach

Add a small window-local layout tree that mirrors pane membership and split
direction:

```text
LayoutNode::Pane(index)
LayoutNode::Split {
    direction: SplitDirection,
    first: Box<LayoutNode>,
    second: Box<LayoutNode>,
}
```

When a pane is split, the leaf for the active pane becomes a split node. The
existing pane remains the first child and the new pane becomes the second child.
The split direction is the user's `-h` or `-v` argument. The new pane remains
active, matching current behavior.

When a pane is killed, its leaf is removed and its sibling is promoted. Pane
indexes continue to follow the existing `PaneSet` indexes, so the layout tree
must adjust indexes above the removed pane.

## Attach Rendering

For multiple visible panes, `ATTACH_SNAPSHOT` renders the active window layout
tree using each pane's current terminal screen text.

Initial rendering is static and text-based:

- horizontal splits place panes side by side
- vertical splits stack panes
- each pane region is clipped to its allocated rows and columns
- horizontal joins pad the left region and insert ` | ` before the right region
- vertical joins insert a separator row of `-` characters between regions
- rendered layout bytes are written only to the attach client, never to PTYs or
  pane capture history

Zoomed panes continue to expose only one visible pane. In that case attach stays
on the existing live raw stream path and no layout snapshot is printed.

## Data Flow

1. CLI parses `split-window -h|-v` as it does today.
2. Protocol carries `SplitDirection`.
3. Server `handle_split` passes `SplitDirection` to the session.
4. The active window spawns the pane, updates `PaneSet`, and updates the layout
   tree in the same session lock.
5. `ATTACH` checks visible pane count as today.
6. `ATTACH_SNAPSHOT` asks the session for a layout snapshot.
7. The renderer combines pane screen text according to the layout tree and
   returns client-only output.

## Error Handling

- Splitting a missing session or missing active pane keeps the current error
  behavior.
- If the layout tree and pane set ever disagree, snapshot rendering falls back
  to the existing ordered pane-section output instead of failing attach. This
  preserves usability while making the invariant violation testable.
- Killing the last pane remains rejected by the existing pane-removal rule.

## Tests

Add focused tests before implementation:

- a horizontal split attach snapshot has a non-empty rendered row containing
  base content, ` | `, and split content in that order
- a vertical split attach snapshot places split pane content below the base pane
  with a separator row between them
- a killed pane is removed from the layout snapshot and remaining pane indexes
  are still valid
- zoomed split-pane attach remains live and does not print layout separators
- existing `capture-pane` output does not contain attach-only layout separators

Unit tests cover layout tree split/removal/index-adjust behavior without
spawning PTYs. Integration tests cover the user-visible attach output.

## Out Of Scope

- live multi-pane redraw loop
- input routing for unzoomed multi-pane live attach
- live statusline redraw
- persistent layout serialization across server restarts
- named layouts or tmux-compatible layout strings
- full terminal escape/layout fidelity beyond current `TerminalState` capture

## Acceptance Criteria

- `split-window -h|-v` direction affects multi-pane attach snapshot layout.
- Existing single-pane and zoomed-pane live attach behavior still passes.
- Existing pane selection, pane killing, and capture behavior still pass.
- README current limits distinguish implemented split-direction snapshot layout
  from still-pending live multi-pane redraw.
- Full verification passes before opening the PR.
