# Attach Pane Snapshot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show all visible panes from the active window when a client attaches while multiple panes are visible.

**Architecture:** Keep live attach streaming unchanged whenever only one pane is visible, including zoomed split-pane windows. When multiple panes are visible, `ATTACH` returns a snapshot-only mode, the client prints the statusline plus a one-time `ATTACH_SNAPSHOT` control response, then exits instead of registering for live pane broadcasts. The snapshot is generated from `TerminalState` screen text, never written into child PTYs, and leaves live multi-pane attach, split-direction persistence, and live layout redraw for later.

**Tech Stack:** Rust standard library, existing Unix socket protocol in `src/protocol.rs`, existing attach client in `src/client.rs`, existing `Session`/`Window`/`PaneSet` state in `src/server.rs`, existing terminal screen capture in `src/term.rs`, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Attach Split-Pane Snapshot

**Files:**
- Modify: `tests/phase1_cli.rs`
- Modify: `src/protocol.rs`
- Modify: `src/client.rs`
- Modify: `src/server.rs`

- [x] **Step 1: Write failing integration test**

Add this test after `attach_renders_status_line_snapshot` in `tests/phase1_cli.rs`:

```rust
#[test]
fn attach_renders_split_pane_snapshot() {
    let socket = unique_socket("attach-pane-snapshot");
    let session = format!("attach-pane-snapshot-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .args(["attach", "-t", &session])
        .stdin(Stdio::null())
        .output()
        .expect("run attach");
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("-- pane 0 --"), "{stdout:?}");
    assert!(stdout.contains("base-ready"), "{stdout:?}");
    assert!(stdout.contains("-- pane 1 --"), "{stdout:?}");
    assert!(stdout.contains("split-ready"), "{stdout:?}");

    let captured = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
    assert_success(&captured);
    let captured = String::from_utf8_lossy(&captured.stdout);
    assert!(!captured.contains("-- pane 0 --"), "{captured:?}");
    assert!(!captured.contains("-- pane 1 --"), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test --test phase1_cli attach_renders_split_pane_snapshot
```

Expected: FAIL because attach output only replays the active pane and does not include pane labels or the inactive pane screen.

- [x] **Step 3: Add internal snapshot request and client writer**

Add `Request::AttachSnapshot { session }`, encode it as
`ATTACH_SNAPSHOT\t<session>\n`, decode it, and cover it with a protocol
round-trip test.

In `src/client.rs`, parse attach `OK\n` as live mode and
`OK\tSNAPSHOT\n` as snapshot-only mode. After the statusline is printed, request
`ATTACH_SNAPSHOT` only in snapshot mode, write the returned body to stdout, shut
down the attach stream, and return without entering raw-mode stdin forwarding.

- [x] **Step 4: Render split-pane attach snapshot**

In `src/server.rs`, expose the active window's visible panes. If more than one
pane is visible, `handle_attach` sends `OK\tSNAPSHOT\n` and returns without raw
history replay or live client registration. Single-pane attach keeps the
existing `OK\n`, raw-history replay, `pane.clients` registration, and stdin
forwarding behavior.

Handle `ATTACH_SNAPSHOT` by rendering visible pane screens as labeled sections:

```text
\r\n-- pane 0 --\r\n
...
\r\n-- pane 1 --\r\n
...
```

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test --test phase1_cli attach_renders_split_pane_snapshot
```

Expected: PASS.

- [x] **Step 6: Verify zoomed split panes stay live**

Add `attach_keeps_zoomed_split_pane_live` in `tests/phase1_cli.rs`. It creates
a split-pane session, zooms the active pane, attaches with piped stdin, sends
`hello\n` followed by `C-b d`, and asserts:

- attach stdout does not contain `-- pane 0 --` or `-- pane 1 --`
- active pane capture eventually contains `live:hello`

Run:

```bash
cargo test --test phase1_cli attach_keeps_zoomed_split_pane_live
```

Expected: PASS because zoom leaves only one pane visible and should keep the
existing live attach path.

### Task 2: Documentation, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-attach-pane-snapshot.md`

- [x] **Step 1: Document snapshot scope**

Update README implemented bullets to include attach-time split-pane snapshot rendering. Update current limits so multi-pane attach rendering is described as snapshot-only when multiple panes are visible, while zoomed single-visible-pane attach remains live.

- [x] **Step 2: Run full verification**

Run:

```bash
cargo fmt --check
git diff --check origin/main
rg -ni "co""dex" .
cargo test
```

Expected: formatting and tests pass; keyword scan prints no matches; whitespace check passes.

- [x] **Step 3: Request subagent review**

Dispatch a read-only review subagent over `git diff origin/main`. Apply technically valid blocking or important findings and rerun full verification.

- [x] **Step 4: Commit and open draft PR**

Run:

```bash
git add README.md src/client.rs src/protocol.rs src/server.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-attach-pane-snapshot.md
git commit -m "feat: render attach pane snapshot"
git push -u origin devmux-attach-layout
gh pr create --draft --base main --head devmux-attach-layout --title "Render attach pane snapshot" --body $'## Summary\n- Render a one-time attach snapshot for split-pane sessions.\n- Keep pane labels out of PTY and capture output.\n- Document snapshot-only layout behavior.\n\n## Validation\n- cargo fmt --check\n- git diff --check origin/main\n- rg -ni "co""dex" .\n- cargo test\n\n## Review\n- Read-only subagent review completed before merge.'
```
