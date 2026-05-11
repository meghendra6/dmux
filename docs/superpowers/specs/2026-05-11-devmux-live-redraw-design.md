# Devmux Live Redraw Design

## Goal

Make unzoomed multi-pane `attach` stay open and redraw the split-layout snapshot
repeatedly, so users can see pane output change without rerunning `attach`.

This slice is read-only for unzoomed multi-pane attach. It does not route user
input to panes and does not implement an event-driven compositor. Single visible
pane attach, including zoomed split windows, keeps the existing live raw PTY
stream behavior.

## Current State

- Single visible pane attach streams raw PTY bytes and forwards stdin to the
  active pane.
- Multiple visible panes make `ATTACH` return snapshot mode.
- Snapshot mode prints the statusline, requests `ATTACH_SNAPSHOT`, writes the
  split-layout snapshot once, and exits.
- `ATTACH_SNAPSHOT` is already safe to call repeatedly because it renders from
  server-owned `TerminalState` and writes only to the requesting client.

## Approaches Considered

1. **Direct pane byte compositor:** subscribe the attach client to every pane's
   raw byte stream and compose updates locally. This reaches true live behavior
   faster, but it repeats the race and terminal-state risks avoided in the
   snapshot work.
2. **Server-side event compositor:** introduce server-managed attach clients
   that receive layout redraw frames when pane output changes. This is the right
   long-term architecture, but it needs a new client registry, invalidation
   policy, resize handling, and input routing decisions.
3. **Polling live redraw:** keep the current control protocol and have the
   client repeatedly request `status-line` plus `ATTACH_SNAPSHOT`. This is the
   recommended slice because it is small, testable, and builds a user-visible
   live view without changing pane PTY ownership.

## Chosen Design

When multiple panes are visible, `ATTACH` keeps returning the existing snapshot
mode:

```text
OK\tSNAPSHOT
```

The client handles this mode by entering a read-only redraw loop. This keeps
older clients compatible with newer servers: an older client still renders the
snapshot once and exits, while the newer client polls the existing snapshot
endpoint.

- render the statusline and current split-layout snapshot immediately
- clear the terminal viewport before each redraw using ANSI home + clear-screen
- request fresh `status-line` and `ATTACH_SNAPSHOT` on a fixed interval
- exit on `C-b d`
- exit on stdin EOF, which keeps non-interactive tests from hanging
- do not forward arbitrary stdin bytes to pane PTYs in this mode

Zoomed panes still expose one visible pane, so `ATTACH` returns `OK` and uses
the existing live raw path.

## Timing

Use a small fixed interval in the client, such as 100 ms. This is intentionally
not configurable in this slice. Tests do not assert the interval directly; they
poll captured stdout until newly emitted pane content appears.

## Data Flow

1. Client sends `ATTACH`.
2. Server detects multiple visible panes and writes `OK\tSNAPSHOT\n`.
3. Client enters raw mode and starts a read-only input thread that watches for
   `C-b d` or EOF.
4. Client redraw loop requests `status-line` and `ATTACH_SNAPSHOT`.
5. Client writes clear-screen, statusline, and layout snapshot to stdout.
6. On detach or EOF, client shuts down the attach stream and exits.

## Error Handling

- A missing session still returns the existing attach error.
- If a redraw control request fails, attach returns that error and exits.
- If stdin EOF happens before the first tick, the client renders one frame first
  and then exits.
- If a pane disappears between redraw requests, the next `ATTACH_SNAPSHOT`
  reflects the server's current active window state, including a single
  remaining visible pane.

## Tests

Add tests before implementation:

- multi-pane attach keeps the existing `OK\tSNAPSHOT\n` handshake
- live snapshot input exits on `C-b d` without forwarding bytes
- unzoomed split-pane attach stays open long enough to render output emitted
  after attach starts
- stdin EOF for unzoomed split-pane attach renders one frame and exits
- zoomed split-pane attach still forwards stdin to the active pane

## Out Of Scope

- forwarding user input to unzoomed multi-pane sessions
- pane focus switching from attach UI
- event-driven redraw instead of polling
- live statusline invalidation separate from the polling loop
- resize-aware pane region allocation
- copy-mode entry from read-only multi-pane live redraw

## Acceptance Criteria

- Multi-pane attach no longer exits immediately when stdin remains open.
- A process writing to any visible pane after attach starts becomes visible in
  attach stdout without rerunning `attach`.
- `C-b d` exits read-only multi-pane attach.
- Arbitrary read-only attach input is not written to pane PTYs.
- Single-pane and zoomed-pane attach behavior is unchanged.
- README documents polling-based read-only multi-pane live redraw and keeps input
  routing listed as pending.
