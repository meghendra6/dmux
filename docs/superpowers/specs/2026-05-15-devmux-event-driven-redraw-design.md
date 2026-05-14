# Devmux Event-Driven Redraw Design

## Goal

Make unzoomed multi-pane attach redraw from server invalidation events instead
of relying on fixed 100ms polling as the primary update mechanism.

The slice covers layout and statusline updates that are already visible through
the existing multi-pane snapshot attach path. It keeps raw single-pane and
zoomed attach behavior separate because those paths already stream active-pane
PTY bytes directly.

## Current State

- Single visible pane attach is byte-stream based. The server writes active-pane
  history and then broadcasts PTY output bytes to attached clients.
- Zoomed attach uses the same raw attach path because the visible layout is a
  single pane.
- Unzoomed multi-pane attach uses snapshot mode. `ATTACH` returns
  `OK\tSNAPSHOT\n`, then the client redraw loop polls every 100ms by reading
  `STATUS_LINE` and `ATTACH_LAYOUT_SNAPSHOT`.
- Local attach actions such as `C-b o`, `C-b q`, mouse focus, help display, and
  copy-mode exit already trigger immediate redraws.
- Server-side changes that should affect the composed view do not notify
  attached snapshot clients:
  - pane PTY output;
  - split, kill-pane, select-pane;
  - new-window, select-window, kill-window;
  - zoom-pane;
  - resize.
- `display-message` is a stateless formatting endpoint. It does not create a
  persistent message with lifetime semantics, so it is not part of live
  event-driven UI in this slice.

## Chosen Design

Add a lightweight invalidation event stream and keep the existing client
compositor.

New clients that enter snapshot attach start an additional control connection:

```text
ATTACH_EVENTS\t<session>\n
```

The server response is:

```text
OK\n
REDRAW\n
REDRAW\n
...
```

Each `REDRAW` means "the composed attach view may have changed." The client does
not receive rendered frames from this stream. It responds by using the existing
redraw path: read `STATUS_LINE`, read `ATTACH_LAYOUT_SNAPSHOT`, clear the
screen, write the status row/message row, and write the snapshot bytes.

This preserves the current compositor and pane-region metadata boundary while
removing fixed polling from the normal update path.

## Server Model

Each session owns a shared attach-event client list. Each pane in that session
also holds an `Arc` to the same list so the PTY output pump can notify without
locking the session window tree.

On `ATTACH_EVENTS`:

1. Look up the session.
2. Return `ERR missing session` if absent.
3. Write `OK\n`.
4. Add a cloned stream to the session event client list.
5. Send an initial `REDRAW\n` to cover the race between initial attach snapshot
   rendering and event subscription.

The server sends `REDRAW\n` after:

- pane output is appended to history and applied to `TerminalState`;
- successful split, kill-pane, select-pane, new-window, select-window,
  kill-window, zoom-pane;
- successful resize after both the PTY and `TerminalState` size are updated.

`SEND` does not notify directly. The pane output pump notifies when the command
actually produces visible output.

Notification must not hold the session `windows` lock or pane `terminal` lock
while writing to event clients. Dead event clients are removed when writes fail.

## Client Model

Snapshot attach starts two background inputs:

- the existing stdin translator;
- an event-stream listener that sends `RedrawHint` events to the same redraw
  loop.

If the event stream returns an unknown-request error, fails to connect, or exits
before sending `OK\n`, the client silently keeps the existing 100ms polling path.
This is the new-client/old-daemon compatibility behavior.

When the event stream is active:

- `REDRAW` triggers an immediate redraw if redraw is not paused;
- copy-mode `PauseRedraw` suppresses both timeout and event-triggered redraws;
- copy-mode `RedrawNow` resumes redraw and renders exactly once;
- event stream close switches back to the 100ms polling fallback;
- transient help and pane-number message expiry still uses a timeout so the
  message row disappears without requiring server output;
- a slower safety redraw interval remains as a fallback for missed events.

Unknown event lines are ignored or terminate the event listener without crashing
the attach session.

## Compatibility

Old client + new daemon:

- Existing `ATTACH -> OK\tSNAPSHOT\n` behavior remains unchanged.
- Existing `ATTACH_LAYOUT_SNAPSHOT`, `ATTACH_SNAPSHOT`, and raw attach behavior
  remain unchanged.

New client + old daemon:

- `ATTACH_EVENTS` gets `ERR unknown request line: ...`.
- The client does not surface this as an attach failure.
- The redraw loop keeps the current polling behavior.

Mixed versions should not silently change copied content, input routing, or
single-pane attach behavior.

## Tests

Protocol and server:

- Round-trip protocol test for `ATTACH_EVENTS`.
- Server handler integration test proving `ATTACH_EVENTS` returns `OK` and an
  initial `REDRAW`.
- Server integration test proving pane output sends a `REDRAW` event.
- Server integration test proving status-affecting mutation such as
  `select-pane` sends a `REDRAW` event.
- Unit test proving dead event streams are removed from the registry, preferably
  with `UnixStream::pair` rather than filesystem-bound `UnixListener`.

Client:

- Unit test for event line parsing.
- Unit tests for redraw timeout selection:
  - old-daemon/no-event stream uses the existing 100ms polling interval;
  - active event stream uses the slower safety interval;
  - pending message expiry shortens the timeout;
  - expired messages can redraw immediately.
- Unit tests for redraw loop events where practical:
  - `RedrawHint` does not resume from paused copy-mode;
  - `RedrawNow` resumes and redraws.

Integration:

- Existing live redraw, mouse focus, numbered selection, composed copy-mode, and
  zoomed attach tests must continue to pass.
- Existing attach live redraw tests are positive behavior checks; they should
  not rely on fragile sub-100ms timing to prove polling disappeared.

## Out Of Scope

- Server-pushed composed frames.
- Removing the polling fallback entirely.
- Raw single-pane attach redesign.
- Persisted display-message state or message broadcast semantics.
- Attach-time split commands or new attach key bindings.
- Mouse wheel, drag resize, focus-follows-motion, or application mouse
  forwarding.
- Terminal protocol fidelity improvements unrelated to redraw invalidation.

## Acceptance Criteria

- New clients receive redraw hints from new daemons during unzoomed multi-pane
  attach.
- Pane output and active pane/window/zoom/layout mutations refresh the composed
  layout/status without waiting for the old fixed 100ms polling loop.
- New clients still attach successfully to old daemons and fall back to polling.
- Copy-mode continues to pause redraw while active and resumes cleanly.
- README describes event-driven multi-pane redraw with fallback instead of
  calling it purely polling-based.
