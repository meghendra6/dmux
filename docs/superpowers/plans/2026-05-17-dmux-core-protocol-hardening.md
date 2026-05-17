# dmux core protocol hardening

**Goal:** Close the next finite core-mux milestone before agent-specific work by tracking terminal protocol metadata that affects shell/editor/TUI behavior and by making that state visible to command/status surfaces.

**Scope:** This plan intentionally stays in the terminal multiplexer core. It covers pane cwd, pane title, bell/activity, and synchronized-output redraw behavior. Larger product work such as agent hooks, worktree orchestration, named layouts, persistent storage, and desktop notification routing remains outside this milestone.

**Architecture:** Keep PTY lifetime and terminal parsing server-owned. Let `TerminalState::apply_bytes` continue to parse child output, but extend `TerminalChanges` so the server can update pane metadata without scraping rendered text. Pane metadata then flows through `list-panes`, `status-line`, `display-message`, and attach render scheduling. UI overlays still must not be written into child PTYs.

## Planned Work

1. **OSC 7 cwd tracking**
   - Parse OSC 7 `file://...` payloads from child output.
   - Update the emitting pane's server-side cwd when the path is valid.
   - Expose `#{pane.cwd}` in pane and status formats.
   - Use the updated pane cwd for server-side split/new-window/respawn defaults when the request does not include an invoking client cwd.

2. **OSC 0/2 pane title tracking**
   - Parse OSC 0 and OSC 2 title payloads from child output.
   - Store the latest pane title in server metadata.
   - Expose `#{pane.title}` in pane and status formats.

3. **Bell and activity state**
   - Treat BEL from a pane as a pane bell signal.
   - Mark non-active pane output as activity.
   - Expose `#{pane.bell}` and `#{pane.activity}` in pane and status formats.
   - Clear bell/activity when selecting that pane.

4. **Synchronized output redraw gating**
   - Track DEC private mode 2026 begin/end in `TerminalState`.
   - Keep applying bytes to terminal state and raw clients immediately.
   - Defer attach-render redraw notifications while synchronized output is active.
   - Flush one redraw when synchronized output ends, with a bounded timeout guard for unterminated sync blocks.

## Verification

- Add targeted unit tests for terminal parser state changes.
- Add integration tests in `tests/phase1_cli.rs` for exposed formats and split cwd behavior.
- Run focused red/green tests for each slice.
- Run final verification:
  - `cargo fmt --check`
  - `git diff --check origin/main`
  - reserved keyword scan for upload-facing artifacts
  - `cargo test`
  - `cargo install --path .`
  - `/Users/meghendra/.cargo/bin/dmux --help`

## Completion

This milestone is complete only when all four planned work items are implemented, final verification passes, the PR is merged, `main` is updated, and the local installed `dmux` binary is refreshed from the merged code.
