# Devmux Numbered Pane Selection Design

## Goal

Let an unzoomed multi-pane attached client select a pane by number from inside
attach. The user presses `C-b q` to show pane indexes briefly, then presses a
single digit to make that pane the server active pane. Subsequent typed input
routes through the existing active-pane input path.

This slice follows attach-time pane cycling and copy-mode. It does not add
mouse focus, pane-region dispatch, or composed-layout copy-mode.

## Current State

- Unzoomed multi-pane attach uses a polling snapshot redraw loop.
- Normal stdin in that attach mode is forwarded to the server active pane.
- `C-b o` already cycles the active pane by calling existing `list-panes` and
  `select-pane` protocol helpers.
- `list-panes -F "#{pane.index}\t#{pane.active}"` exposes pane indexes and the
  active flag.
- `select-pane -t <session> -p <index>` already changes the server active pane.
- `C-b [` already pauses polling redraw and enters active-pane copy-mode.

## Approaches Considered

1. **Client-side numbered selection using existing control requests.** The
   live snapshot input path treats `C-b q` as a local UI action, shows pane
   indexes from `list-panes`, and treats the next digit as a `select-pane`
   request. This keeps the server protocol unchanged and builds directly on
   the current active-pane routing.
2. **Add a server-side numbered selection protocol request.** The server could
   accept a requested pane number atomically. This is not needed because the
   client can already list and select panes.
3. **Add full pane targeting now, including mouse regions.** This would require
   mapping rendered layout regions back to pane indexes and deciding how mouse
   clicks interact with copy-mode. That is a larger slice than numbered
   keyboard selection.

## Chosen Design

Use approach 1. The live snapshot input translator gains a small state machine:

- `C-b q` emits `ShowPaneNumbers` and enters a pending numbered-selection state.
- While pending, the next ASCII digit emits `SelectPane(<digit>)`.
- Bytes after the selected digit in the same read are processed normally, so
  `C-b q0hello` selects pane `0` before forwarding `hello`.
- If the next byte is not a digit, numbered-selection state is cancelled and
  that byte is processed as normal input.
- Existing `C-b d`, `C-b o`, `C-b [`, and `C-b C-b` behavior remains unchanged.

The redraw loop handles `ShowPaneNumbers` by reading the current pane list and
rendering a transient message such as:

```text
panes: 0 [1] 2
```

The active pane is bracketed. The message is shown with the normal status line
and split snapshot for about one second, then normal polling redraw removes it.
Selecting a pane clears the message immediately after the selection redraw.

The selection helper validates the requested single-digit index against the
current pane listing before sending `SELECT_PANE`. Missing pane digits are
ignored and cause a redraw without exiting attach. Malformed pane listings or
control-request failures still return errors because they indicate server or
protocol problems, not ordinary user input.

## Data Flow

1. User presses `C-b q` during unzoomed multi-pane attach.
2. Input thread emits `LiveSnapshotInputEvent::ShowPaneNumbers`.
3. Redraw loop reads pane metadata with `LIST_PANES_FORMAT`.
4. Redraw loop writes a frame containing status line, transient pane message,
   and current split-layout snapshot.
5. User presses a digit.
6. Input thread emits `LiveSnapshotInputEvent::SelectPane(index)`.
7. Redraw loop confirms that index exists in the current pane listing.
8. Redraw loop sends `SELECT_PANE` for that index and writes a fresh frame.
9. Subsequent forwarded input goes to the selected active pane.

## Tests

Add tests before implementation:

- live snapshot input translates `C-b q` to `ShowPaneNumbers`
- live snapshot input keeps numbered-selection state across reads
- coalesced `C-b q0...` emits show, select, then forward actions in order
- non-digit after `C-b q` cancels selection and forwards the byte normally
- pane number message formats active pane with brackets
- unzoomed multi-pane attach can select pane `0` and then pane `1` with
  `C-b q0` / `C-b q1`, and input after each selection routes to that pane
- existing attach live input, pane cycling, copy-mode, snapshot handshake, and
  zoomed raw attach tests still pass

## Out Of Scope

- multi-digit pane indexes
- mouse focus or mouse-to-pane dispatch
- active-pane border highlighting
- pane index overlays placed inside each rendered pane
- timeout handling in the input thread
- composed-layout copy-mode
- new server protocol

## Acceptance Criteria

- In an unzoomed split attach, `C-b q` displays pane indexes briefly.
- Pressing a valid digit after `C-b q` selects that pane without writing the
  prefix or digit to any pane PTY.
- Normal typed input after selection goes to the selected pane.
- Invalid digits are ignored without terminating attach.
- `C-b d`, `C-b o`, `C-b [`, `C-b C-b`, and ordinary input keep their current
  behavior.
- Single-pane and zoomed-pane raw attach behavior is unchanged.
- README documents attach-time numbered pane selection and removes it from the
  current limits.
