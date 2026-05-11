# Devmux Attach Pane Focus Design

## Goal

Let an unzoomed multi-pane attached client switch the server active pane from
inside attach, so subsequent typed input routes to a different pane without
running a separate `select-pane` command.

This slice adds one familiar keybinding: `C-b o` cycles to the next pane in the
active window. It does not add mouse focus, numbered pane selection, or
copy-mode for multi-pane attach.

## Current State

- Multi-pane attach redraws polling snapshots and forwards stdin to the server
  active pane.
- `select-pane -t <session> -p <index>` already changes the server active pane.
- `list-panes -t <session> -F <format>` already exposes pane indexes and the
  active flag.
- The live snapshot input translator treats `C-b d` as detach, `C-b C-b` as a
  literal prefix byte, and other prefix combinations as literal input.

## Approaches Considered

1. **Client-side cycle using existing control commands:** on `C-b o`, the client
   calls `LIST_PANES_FORMAT` to find pane order and active pane, then calls
   `SELECT_PANE` for the next index. This reuses existing server behavior and
   avoids new protocol surface.
2. **Add a new `SELECT_NEXT_PANE` protocol request:** the server would perform
   the cycle atomically. This is cleaner long-term but adds a command solely for
   attach UI convenience.
3. **Add full pane targeting keybindings now:** support `C-b q`, numbers, and
   mouse regions. This jumps beyond the current phase and needs UI feedback and
   region mapping decisions.

## Chosen Design

Use approach 1. The client handles `C-b o` in live snapshot attach as a local
control action:

- input translator returns `SelectNextPane`
- redraw loop calls `select_next_pane(socket, session)`
- `select_next_pane` requests `list-panes -F "#{pane.index}\t#{pane.active}"`
- it chooses the next listed pane after the active pane, wrapping to the first
- it sends `SELECT_PANE` with that pane index
- it immediately redraws a frame so the statusline and layout reflect the new
  active pane

If pane listing has zero panes, no active pane, malformed lines, or the select
request fails, attach returns that error and exits. This matches current
control-request behavior and keeps failures visible instead of silently dropping
the keybinding.

## Data Flow

1. User presses `C-b o` during unzoomed multi-pane attach.
2. Client input thread emits `LiveSnapshotInputEvent::SelectNextPane`.
3. Redraw loop receives the event and calls `select_next_pane`.
4. Client opens a control connection and sends `LIST_PANES_FORMAT`.
5. Client parses pane index plus active flag.
6. Client opens a control connection and sends `SELECT_PANE` for the next pane.
7. Redraw loop renders a fresh statusline and snapshot.
8. Subsequent forwarded input goes to the newly active pane through the existing
   live input path.

## Tests

Add tests before implementation:

- live snapshot input translates `C-b o` to select-next-pane and forwards no
  bytes
- `C-b o` in attach switches from pane 1 to pane 0 or from pane 0 to pane 1
  according to current pane order
- input after `C-b o` goes to the newly active pane
- existing detach, literal prefix, live input, snapshot handshake, and zoomed
  raw attach tests still pass

## Out Of Scope

- numbered pane selection from attach
- previous-pane cycling
- mouse-to-pane focus
- visual pane borders or active-pane highlighting
- multi-pane copy-mode entry
- server-side atomic next-pane protocol

## Acceptance Criteria

- In an unzoomed split attach, `C-b o` changes the active pane.
- After `C-b o`, normal typed input goes to the newly active pane.
- `C-b o` is not written to any pane PTY.
- `C-b d`, `C-b C-b`, and regular input keep their current behavior.
- Single-pane and zoomed-pane raw attach behavior is unchanged.
- README documents attach-time pane cycling and keeps richer focus controls as
  pending.
