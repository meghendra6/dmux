# Basic Multiplexer Skeleton Design

## Goal

Bring `dmux` to a basic terminal multiplexer skeleton: a user can start it with
no arguments, get one interactive terminal, split panes from inside attach,
move focus, close/zoom panes, detach, reattach, and shut down cleanly.

The target is not feature parity with mature multiplexers. The target is that
the core open, pane, attach, and close loop works without relying on separate
CLI commands for ordinary pane operations.

## Required User-Visible Behavior

### Open And Reattach

- `dmux` with no subcommand opens the `default` session.
- If `default` does not exist, `dmux` creates it and attaches.
- If `default` exists, `dmux` attaches without creating a duplicate session.
- `dmux new -s <name>` keeps creating and attaching; `-d` keeps creating in the
  background.
- Explicit `attach -t <name>`, `kill-session -t <name>`, and `ls` must not start
  an empty daemon when no server is running.

### Attach-Time Pane Basics

Use the existing `C-b` prefix model and add the minimum pane operations a user
expects inside attach:

- `C-b d`: detach.
- `C-b %`: split right.
- `C-b "`: split down.
- `C-b o`: focus next pane.
- `C-b h/j/k/l`: focus left/down/up/right when a pane exists in that direction.
- `C-b q` then digit: select pane by number.
- `C-b x`: close the active pane, but do not close the last pane.
- `C-b z`: toggle zoom for the active pane.
- `C-b [`: copy-mode.
- `C-b ?`: show help.
- Mouse click keeps focusing panes in unzoomed multi-pane attach.

These commands should work from a fresh single-pane attach. The first split may
transition the client from raw single-pane attach to live multi-pane attach, but
this transition must be automatic.

### Simple UI

- Keep the UI plain: no pane title bars, frames, decorative padding, or extra
  margins.
- Single-pane attach should show the pane content and a compact status/help hint.
- Multi-pane attach should keep the existing compact separators (` | ` for
  side-by-side and a dashed line for vertical splits).
- The normal status line should include a terse `C-b ? help` hint.
- `attach --help` should list the attach-time pane keys and explain that omitted
  `-t` targets `default`.

### Close And Shutdown

- `C-b x` on the last pane reports a message and keeps the session alive.
- `kill-session` closes active attached clients promptly.
- `kill-server` closes active attached clients promptly.
- If the only attached pane process exits, the attached client exits instead of
  waiting forever for stdin.
- Detach stays client-local and leaves the session running.

## Implementation Shape

- Keep the server protocol text-based.
- Add a no-subcommand command path for "open default".
- Reuse existing `SPLIT`, `SELECT_PANE`, `KILL_PANE`, and `ZOOM_PANE` requests
  for attach-time commands.
- Extend the live snapshot attach input state with pane command actions.
- For directional focus, use the current rendered pane regions and active pane
  listing to pick the nearest pane in the requested direction.
- Track attach lifetime streams in the server so session/server close can shut
  them down.
- Make raw single-pane attach return on server output EOF, not only on stdin
  detach.

## Verification

Add regression tests before implementation:

- Bare `dmux` creates `default`, attaches, and detaches.
- Bare `dmux` attaches to existing `default` without duplicate creation.
- `ls`, `attach -t missing`, and `kill-session -t missing` on a fresh socket do
  not start an empty daemon.
- Duplicate `new -s <name>` includes an attach recovery hint.
- Raw single-pane attach starts cleanly, has no split separators or pane header
  padding, and detaches.
- From single-pane attach, `C-b %` creates a right split and moves to live
  multi-pane attach.
- From single-pane attach, `C-b "` creates a down split and moves to live
  multi-pane attach.
- In multi-pane attach, `C-b h/j/k/l`, `C-b o`, `C-b x`, and `C-b z` operate on
  panes and redraw.
- Detach/reattach preserves pane layout and active-pane input.
- Active attach exits after `kill-session`, `kill-server`, and pane process EOF.
- Attach help/status tests cover `C-b ? help` and the new key list.

Final verification:

```bash
cargo fmt --check
git diff --check origin/main
cargo test
```

Process/socket integration tests may require escalated execution in this
environment.

## Out Of Scope

- Configurable keybindings.
- Modal pane/tab/session bars.
- Persistent session resurrection.
- Floating panes.
- Full terminal protocol fidelity.
- Rich copy-mode selection and persistent buffers.
