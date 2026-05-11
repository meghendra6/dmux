# Devmux Multi-Pane Copy Mode Design

## Goal

Let an unzoomed multi-pane attached client enter the existing basic copy-mode
with `C-b [`, operating on the current server active pane.

This completes the next attach usability slice after live redraw, live input,
and attach-time pane cycling. It does not add pane-region mouse dispatch,
numbered pane selection, or a full alternate-screen viewport.

## Current State

- Single-pane and zoomed attached clients use `C-b [` to enter local copy-mode.
- The copy-mode UI already supports `j`/`k`, `Ctrl-n`/`Ctrl-p`, `y` or Enter,
  `q` or Escape, and basic SGR mouse line selection.
- Server `COPY_MODE` and `SAVE_BUFFER` requests already operate on the session's
  active pane.
- Unzoomed multi-pane attach uses a client-side input thread that sends ordered
  events to a polling redraw loop.
- That live snapshot input path currently handles normal forwarding, `C-b d`,
  and `C-b o`, but not `C-b [`.

## Chosen Design

Extend the live snapshot input path to treat `C-b [` as a local copy-mode action.
The input translator emits `EnterCopyMode { initial_input }`, preserving any
forwarded bytes before the prefix and handing bytes after `[` to copy-mode as
initial input.

The live snapshot input thread will run copy-mode itself because it is the
component that owns stdin. Before it enters copy-mode, it sends a pause event to
the redraw loop so polling frames do not overwrite the copy-mode UI. After
copy-mode exits, it sends a redraw event and resumes normal live snapshot input
processing.

`run_copy_mode` will be split into a small wrapper plus a reader-based helper.
Single-pane attach keeps calling the wrapper, while live snapshot attach reuses
the helper with the input thread's existing stdin lock. This avoids competing
stdin readers.

## Data Flow

1. User presses `C-b [` during unzoomed multi-pane attach.
2. The input thread translates it to `EnterCopyMode`.
3. The input thread sends `PauseRedraw` to the redraw loop.
4. The input thread calls the existing copy-mode workflow against the current
   active pane.
5. Copy-mode reads further keys from the same stdin reader until copy or exit.
6. `SAVE_BUFFER` stores copied text from the active pane.
7. The input thread sends `RedrawNow` and resumes normal attach input handling.
8. The redraw loop renders the current statusline and split layout again.

## Error Handling

- If `COPY_MODE` or `SAVE_BUFFER` fails, the input thread sends an error event
  and the attach loop returns that error.
- EOF while in copy-mode exits copy-mode and then attach exits naturally on the
  next stdin EOF.
- Bytes typed before `C-b [` in the same read are forwarded before copy-mode
  starts.
- Bytes typed after `C-b [` in the same read are copy-mode initial input, not
  pane input.

## Tests

- Unit tests cover live snapshot translation for `C-b [` and coalesced initial
  copy-mode input.
- Integration test creates a split session, enters multi-pane attach, sends
  `C-b [` plus `y`, and verifies a buffer is saved from the active split pane.
- Existing tests for live input, pane cycling, snapshot handshake, and zoomed raw
  attach remain part of full verification.

## Out Of Scope

- `C-b [` choosing a pane by mouse region.
- Numbered pane selection from attach.
- Copy-mode over the composed multi-pane layout.
- Full tmux-compatible copy-mode viewport behavior.
- Event-driven redraw or compositor invalidation.

## Acceptance Criteria

- In unzoomed multi-pane attach, `C-b [` opens copy-mode instead of forwarding
  bytes to a pane.
- Copy-mode acts on the current server active pane.
- Coalesced input such as `C-b [` followed by `y` copies the current line.
- Polling redraw does not overwrite copy-mode while it is active.
- After copy-mode exits, multi-pane attach resumes and can detach with `C-b d`.
- README documents multi-pane `C-b [` support and removes it from current limits.
