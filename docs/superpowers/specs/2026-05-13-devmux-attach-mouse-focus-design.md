# Devmux Attach Mouse Focus Design

## Goal

Let an unzoomed multi-pane attached client select the server active pane with a
mouse click inside the rendered pane layout. Subsequent typed input should route
to the clicked pane without running a separate `select-pane` command.

This slice builds on the internal pane-region map from PR #28. It does not add
event-driven redraw, drag resizing, wheel forwarding, composed-layout copy-mode,
or full terminal mouse protocol fidelity.

## Current State

- Unzoomed multi-pane attach uses the snapshot/live polling path.
- The attach client already supports `C-b o`, `C-b q`, `C-b [`, normal input
  forwarding, and detach.
- Forwarded input uses synchronous `SEND` control requests, so bytes before a
  later control action are acknowledged before that action runs.
- Copy-mode has SGR mouse parsing and a `MouseModeGuard`, but live attach does
  not enable mouse reporting or parse mouse events.
- The server renderer now computes `PaneRegion` metadata from the exact
  rendered attach layout, but `ATTACH_SNAPSHOT` still returns only display text.
- `ATTACH_SNAPSHOT` compatibility matters because current clients treat the
  response body as opaque display bytes.

## Approaches Considered

1. **Add a new combined snapshot-with-regions request.** The server renders the
   layout once and returns both region metadata and the exact snapshot bytes.
   Existing `ATTACH_SNAPSHOT` output remains unchanged.
2. **Add a sidecar region-only request.** The client would request text and
   regions separately. This is smaller at the protocol level, but the metadata
   can become stale because polling output and layout can change between calls.
3. **Add a server-side `SELECT_PANE_AT` request.** The client would send a row
   and column and let the server choose the pane. This hides region metadata,
   but it couples the server protocol directly to mouse behavior and still
   requires client-side status/message row adjustment.
4. **Infer regions from snapshot text in the client.** This duplicates renderer
   logic and is brittle for nested layouts and padding.

## Chosen Design

Use approach 1.

Add a new protocol request:

```text
ATTACH_LAYOUT_SNAPSHOT\t<session>\n
```

The response keeps the normal `OK\n` status line and then returns a structured
body:

```text
REGIONS\t<count>\n
REGION\t<pane>\t<row_start>\t<row_end>\t<col_start>\t<col_end>\n
...
SNAPSHOT\t<byte_len>\n
<exact existing snapshot bytes>
```

`row_start`, `row_end`, `col_start`, and `col_end` use the same zero-based,
half-open snapshot-body coordinates as `PaneRegion`. The snapshot bytes after
the `SNAPSHOT` header are byte-for-byte the text that `ATTACH_SNAPSHOT` would
return for the same render pass.

The existing `ATTACH`, `OK\tSNAPSHOT`, and `ATTACH_SNAPSHOT` behavior remains
unchanged.

## Server Flow

1. `ATTACH_LAYOUT_SNAPSHOT` looks up the session.
2. The server captures the current `AttachLayoutSnapshot`.
3. It renders the layout once into text plus pane regions.
4. If the layout and visible panes disagree, the response contains the existing
   ordered-section fallback text and zero regions.
5. The server writes `OK\n`, region rows, a `SNAPSHOT` length header, and the
   snapshot bytes.

## Client Flow

Live snapshot attach switches from `ATTACH_SNAPSHOT` to
`ATTACH_LAYOUT_SNAPSHOT`. Each redraw stores the latest region map and uses the
snapshot bytes for display.

The client enables SGR mouse reporting for unzoomed multi-pane live attach only.
Raw single-pane and zoomed attach stay raw/live and do not enable live mouse
focus.

`translate_live_snapshot_input` consumes SGR mouse sequences in order:

- A plain left-button press emits `LiveSnapshotInputAction::MousePress { col,
  row }`.
- Release, drag, wheel, malformed complete mouse sequences, and clicks with
  row/column zero are consumed or ignored, not forwarded to panes.
- Incomplete SGR mouse sequences are buffered across reads.
- Forwarded bytes before a mouse action are emitted before the mouse action.
- Forwarded bytes after a mouse action are emitted after the mouse action.

The live attach loop maps terminal mouse coordinates into snapshot coordinates:

- Terminal coordinates are 1-based.
- The status line consumes one terminal row when present.
- A pane-number message consumes one extra terminal row while it is visible.
- Snapshot row = `event.row - 1 - header_rows`.
- Snapshot col = `event.col - 1`.

If the point falls inside a latest-frame region, the client sends existing
`SELECT_PANE` for that pane, clears pane-number UI, and redraws immediately. If
the click hits a separator, status/message row, or no-region fallback snapshot,
attach stays alive and active pane remains unchanged.

## Mouse Mode Ownership

Live attach and copy-mode can both need mouse reporting. A naive nested
`MouseModeGuard` would disable mouse reporting when copy-mode exits even though
live attach continues.

`MouseModeGuard` should therefore be process-local reference counted: the first
guard writes the enable sequence, nested guards only increment depth, and the
last dropped guard writes the disable sequence.

## Tests

Add tests before implementation:

- Protocol round-trip for `ATTACH_LAYOUT_SNAPSHOT`.
- Server response formatting includes regions and exact snapshot bytes.
- Existing `ATTACH_SNAPSHOT` output remains plain snapshot text.
- Client parser accepts the new body format and rejects malformed region or
  snapshot headers.
- Live input translation emits ordered `Forward`, `MousePress`, `Forward` for
  coalesced input.
- Live input buffers split SGR mouse sequences across reads.
- Release, drag, and wheel mouse events are consumed or ignored.
- Click-to-region mapping subtracts status/message rows.
- Integration: click base pane, then typed input reaches the base pane.
- Integration: coalesced `split input + mouse-to-base + base input` preserves
  ordering.
- Integration: separator click leaves the active pane unchanged and attach
  stays alive.
- Existing copy-mode mouse integration still passes after live mouse mode is
  enabled.

## Out Of Scope

- App mouse forwarding to pane PTYs.
- Drag-to-resize, wheel scrolling, or focus-follows-motion.
- Mouse focus for raw single-pane or zoomed attach.
- Event-driven redraw/status updates.
- Composed-layout copy-mode.
- Terminal display-width fixes for wide or combining Unicode.
- Terminal-size-aware blank area allocation beyond the current rendered text.

## Acceptance Criteria

- In unzoomed multi-pane attach, a mouse click inside a pane selects that pane.
- Input typed after the click routes to the clicked pane.
- Mouse click events are not written to pane PTYs.
- Separator or out-of-region clicks do not change active pane and do not detach.
- Coalesced forwarded bytes before a click are delivered before pane selection.
- Existing `C-b d`, `C-b o`, `C-b q`, `C-b [`, regular input, snapshot
  handshake, and zoomed/raw attach behavior are unchanged.
- `ATTACH_SNAPSHOT` remains compatible and contains no region metadata.
- README documents attach-time mouse focus and keeps composed-layout copy-mode
  pending.
