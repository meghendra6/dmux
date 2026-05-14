# Basic Multiplexer Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `dmux` usable as a basic terminal multiplexer: open with no args, split/focus/close/zoom panes inside attach, detach/reattach, and shut down cleanly.

**Architecture:** Keep the existing text protocol and server layout model. Add a client-side open-default path, add attach-time pane command actions that reuse existing server requests, and harden attach lifecycle so server-side close events end attached clients.

**Tech Stack:** Rust stdlib, Unix domain sockets, POSIX PTY support already in the repo, existing unit tests and `tests/phase1_cli.rs` integration tests.

---

### Task 1: Open Path And Help Baseline

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/main.rs`
- Modify: `src/server.rs`
- Modify: `src/client.rs`
- Modify: `tests/phase1_cli.rs`
- Modify: `README.md`

- [ ] **Step 1: Add RED tests for no-arg open and no-empty-daemon commands**

Add integration tests for:

- `dmux` creates and attaches `default` when no server is running.
- `dmux` attaches existing `default` without creating a duplicate session.
- `ls` on a fresh socket fails clearly or reports no server without creating a
  socket.
- `attach -t missing` on a fresh socket fails without creating a socket.
- `kill-session -t missing` on a fresh socket fails without creating a socket.
- duplicate `new -s <name>` says `session already exists; use dmux attach -t <name>`.

Run:

```bash
cargo test --test phase1_cli dmux_without_args_ -- --test-threads=1
cargo test --test phase1_cli without_server_does_not_start_daemon -- --test-threads=1
cargo test --test phase1_cli new_existing_session_reports_attach_hint -- --test-threads=1
```

Expected: RED before implementation.

- [ ] **Step 2: Implement open-default and no-empty-daemon behavior**

Add `Command::OpenDefault` for no subcommand. In `src/main.rs`, ensure the
server, list sessions, create `default` only when missing, then attach.

For explicit `ls`, `attach`, and `kill-session`, check whether the server is
accepting connections before `ensure_server`. If not, return:

```text
no dmux server running; create a session with dmux new -s <name>
```

Change duplicate session creation in `src/server.rs` to include:

```text
use dmux attach -t <name>
```

- [ ] **Step 3: Add help/status RED tests and implementation**

Tests should assert:

- `attach --help` includes `Usage: dmux attach [-t <name>]`.
- Help says omitted `-t` targets `default`.
- Help lists `C-b %`, `C-b "`, `C-b h/j/k/l`, `C-b x`, `C-b z`, `C-b ?`.
- Normal attach status includes `C-b ? help`.
- `status-line` command output does not include the help hint unless requested
  through a format string.

Implement by updating static help text and default attach status formatting.

- [ ] **Step 4: Run Task 1 verification**

Run:

```bash
cargo test --test phase1_cli dmux_without_args_ -- --test-threads=1
cargo test --test phase1_cli without_server_does_not_start_daemon -- --test-threads=1
cargo test --test phase1_cli new_existing_session_reports_attach_hint -- --test-threads=1
cargo test attach_help_lists_prefix_bindings_and_split_command
cargo fmt --check
git diff --check -- src/cli.rs src/main.rs src/server.rs src/client.rs tests/phase1_cli.rs README.md
```

### Task 2: Attach-Time Pane Commands

**Files:**
- Modify: `src/client.rs`
- Modify: `tests/phase1_cli.rs`
- Modify: `README.md`

- [ ] **Step 1: Add RED unit tests for attach input translation**

Add pure tests for:

- raw attach translates `C-b %`, `C-b "`, `C-b x`, and `C-b z`.
- live snapshot attach translates `C-b %`, `C-b "`, `C-b h/j/k/l`,
  `C-b x`, and `C-b z`.
- forwarded bytes before a prefix command are preserved in order.

Run:

```bash
cargo test attach_input_ pane_command
cargo test live_snapshot_input_ pane_command
```

Expected: RED before implementation.

- [ ] **Step 2: Add RED integration tests for split from attach**

Add integration tests:

- Start `dmux attach` on one pane, send `C-b %`, verify a right split appears,
  active pane moves to the new pane, and the attach child remains live until
  `C-b d`.
- Start `dmux attach` on one pane, send `C-b "`, verify a vertical split
  appears and the attach child remains live until `C-b d`.

Run:

```bash
cargo test --test phase1_cli attach_prefix_percent_splits_right_from_single_pane -- --test-threads=1
cargo test --test phase1_cli attach_prefix_quote_splits_down_from_single_pane -- --test-threads=1
```

Expected: RED before implementation.

- [ ] **Step 3: Implement split commands and raw-to-live transition**

Add attach input actions for split right/down. For raw single-pane attach, run
the split control request and reconnect into snapshot attach automatically. For
live snapshot attach, run the split request and redraw.

Use the default shell command for attach-time splits.

- [ ] **Step 4: Add RED integration tests for focus/close/zoom**

Add integration tests:

- `C-b h/j/k/l` selects directional panes in a two- or three-pane layout.
- `C-b x` closes the active pane, redraws the remaining pane, and preserves the
  session.
- `C-b x` on the last pane shows a message and keeps the attach running.
- `C-b z` toggles zoom and redraws between focused pane and full layout.
- Detach, reattach, and active-pane input still work after a split.

Run each focused test with `--test-threads=1`.

- [ ] **Step 5: Implement focus/close/zoom commands**

Add live snapshot actions for focus left/down/up/right, close pane, and zoom.
Use existing `LIST_PANES`, `ATTACH_LAYOUT_SNAPSHOT`, `SELECT_PANE`,
`KILL_PANE`, and `ZOOM_PANE` requests.

For directional focus, compare the active pane's rendered region with candidate
regions and select the nearest pane in the requested direction. If no pane
exists in that direction, do nothing.

For close-last-pane errors, show the server error as a transient message instead
of exiting attach.

- [ ] **Step 6: Run Task 2 verification**

Run all Task 2 focused tests plus:

```bash
cargo test live_snapshot_input_
cargo test pane_at_mouse_position_subtracts_header_rows
cargo fmt --check
git diff --check -- src/client.rs tests/phase1_cli.rs README.md
```

### Task 3: Attach Close And Shutdown Hardening

**Files:**
- Modify: `src/client.rs`
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [ ] **Step 1: Add RED active attach shutdown tests**

Add integration tests:

- Active raw attach exits when another process runs `kill-session`.
- Active raw attach exits when another process runs `kill-server`.
- Active raw attach exits when the pane process exits.
- Active multi-pane attach exits when another process runs `kill-session`.

Keep attach stdin piped and open in these tests so the old stdin-blocking bug is
observable.

- [ ] **Step 2: Implement server-side attach stream closing**

Track attach lifetime streams per session. Register raw and snapshot `ATTACH`
streams. On `kill-session` and `kill-server`, shut down attach streams, pane
output clients, and event streams before terminating pane processes. On PTY EOF,
close that pane's raw output clients.

- [ ] **Step 3: Implement client-side EOF exits**

For raw attach, run stdin forwarding in a worker thread and let the main thread
return when the server output stream reaches EOF.

For live snapshot attach, monitor the original `ATTACH` stream and send an EOF
event to the live loop when it closes.

- [ ] **Step 4: Run Task 3 verification**

Run:

```bash
cargo test --test phase1_cli active_attach_exits_when_ -- --test-threads=1
cargo test --test phase1_cli active_multi_pane_attach_exits_when_kill_session_runs_from_another_process -- --test-threads=1
cargo test live_snapshot_redraw_event
cargo test live_snapshot_input_
cargo fmt --check
git diff --check -- src/client.rs src/server.rs tests/phase1_cli.rs
```

### Task 4: Final Verification, PR, Review, Merge

**Files:**
- Modify: `HANDOFF.md`

- [ ] **Step 1: Run full verification**

Run:

```bash
cargo fmt --check
git diff --check origin/main
cargo test
```

If sandboxed process integration fails because the daemon cannot bind/listen,
rerun the same command with escalation and record both outcomes.

- [ ] **Step 2: Check the final diff scope**

Before every commit and before PR creation, run:

```bash
git diff --name-only origin/main
```

The final diff should include only product code, tests, user-facing docs, and
the committed design/plan for this PR.

- [ ] **Step 3: PR and critical review**

Create the PR. Then dispatch a critical review subagent for the PR diff. Assess
each finding technically, fix valid Critical/Important findings, rerun focused
verification, push, and update the PR body.

- [ ] **Step 4: Merge and retrospective**

After review and verification, merge the PR, delete the remote branch if needed,
fetch/prune, and update `HANDOFF.md` with the merge commit, review outcome,
verification evidence, and retrospective.
