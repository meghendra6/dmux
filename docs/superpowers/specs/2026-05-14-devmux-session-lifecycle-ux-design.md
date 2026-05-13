# Session Lifecycle And Attach Help Design

## Goal

Make the first manual `dmux` workflow reliable and discoverable:

- `dmux new -s <name>` creates a session and immediately attaches instead of
  failing after session creation.
- `dmux kill-server` is idempotent when a stale socket path remains.
- Users can discover attach bindings and pane commands from CLI help and from an
  attach-time `C-b ?` help command.
- Add smoke coverage for create, attach, detach, kill, server shutdown, and
  recreate.

## Current Problem

Manual use on 2026-05-14 exposed two basic issues:

- Sessions did not reliably open and did not always feel cleanly shut down.
- A new user could not tell how to split panes or move/select panes from
  shortcuts.

The most concrete code mismatch is that `README.md` lists `dmux new -s <name>`,
but `src/main.rs` currently creates the session and then returns
`interactive attach is not implemented yet; use -d`. That means the command
both mutates server state and exits as a failure, which is confusing and makes
the basic happy path look broken.

## Design

### Interactive `new`

For `Command::New { detach: false, .. }`, keep the existing server start and
`NEW` request, then attach to the newly created session using the same attach
path as `dmux attach -t <name>`.

This is intentionally a client-side orchestration change. It does not add a new
server request and does not change detached `new -d`.

If attach fails after `NEW` succeeds, return the attach error. Do not silently
kill the session, because the server-side session was successfully created and
the user may want to recover it with `dmux attach -t <name>` or
`dmux kill-session -t <name>`.

### Stale Socket Shutdown

For `kill-server`, if the socket path exists but connecting fails, remove that
stale socket path and return success. This keeps shutdown idempotent after a
server crash or forced process termination.

Do not hide protocol errors from a live server. Only treat connection failure to
an existing socket path as stale cleanup.

### Help Surfaces

Add static help text in the CLI for:

- `dmux --help`, `dmux help`
- `dmux attach --help`, `dmux help attach`

The attach help must list:

- `C-b d`: detach
- `C-b o`: cycle panes
- `C-b q`: show/select pane numbers
- `C-b [`: copy-mode
- mouse click: focus a pane in unzoomed multi-pane attach
- pane splitting is currently done with `dmux split-window -t <name> -h|-v`

Add attach-time `C-b ?` handling. For multi-pane snapshot attach, render the
same help as the transient message row. For raw single-pane attach, print the
help to stdout and keep the attach session alive.

Do not add attach-time split shortcuts in this PR. Single-pane raw attach cannot
switch cleanly into multi-pane snapshot mode without a larger mode-transition
design, so help should explicitly document the CLI split command.

## Verification

Add focused tests before implementation:

- Unit tests for top-level and attach help parsing.
- Unit tests for attach input translation recognizing `C-b ?`.
- Integration test for `dmux new -s <name>` attaching immediately and detaching
  with `C-b d`.
- Integration smoke test for `new -d`, `attach`, detach, `kill-session`, `ls`
  absence, `kill-server`, and recreate on the same socket path.
- Integration test for stale socket cleanup by `kill-server`.
- Integration test for attach-time `C-b ?` displaying help and leaving attach
  alive until `C-b d`.

Run final verification:

```bash
cargo fmt --check
git diff --check origin/main
rg -ni "co""dex" .
cargo test
```

In this environment, PTY/server integration tests may require escalated
execution.

## Out Of Scope

- Composed-layout copy-mode.
- Event-driven redraw/status updates.
- Attach-time split shortcuts.
- App mouse forwarding, wheel scrolling, drag resize, focus-follows-motion.
- Persistent buffers or screen history.
