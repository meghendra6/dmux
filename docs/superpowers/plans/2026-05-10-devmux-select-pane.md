# Select Pane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow a user or script to switch the active pane in a multi-pane session by pane index.

**Architecture:** Reuse the server-side active pane added in the split-pane slice. Add a small `SELECT_PANE` control request and a CLI command `select-pane -t <session> -p <index>`; existing active-pane operations continue to work without new target syntax.

**Tech Stack:** Rust standard library, existing Unix socket line protocol, existing in-memory server session model, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: CLI And Protocol Surface

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`

- [x] **Step 1: Write failing CLI and protocol tests**

Add this CLI test:

```rust
#[test]
fn parses_select_pane_target_and_index() {
    let command = parse_args(["dmux", "select-pane", "-t", "dev", "-p", "1"]).unwrap();
    assert_eq!(
        command,
        Command::SelectPane {
            session: "dev".to_string(),
            pane: 1,
        }
    );
}
```

Add this protocol test:

```rust
#[test]
fn round_trips_select_pane_request() {
    let line = encode_select_pane("dev", 1);
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::SelectPane {
            session: "dev".to_string(),
            pane: 1,
        }
    );
}
```

- [x] **Step 2: Verify RED**

Run: `cargo test select_pane`

Expected: FAIL because `Command::SelectPane`, `Request::SelectPane`, and `encode_select_pane` do not exist.

- [x] **Step 3: Implement minimal parsing and encoding**

Add `Command::SelectPane { session: String, pane: usize }`, parse `select-pane -t <session> -p <index>`, add `Request::SelectPane`, and encode/decode `SELECT_PANE\t<session>\t<index>\n`.

- [x] **Step 4: Verify GREEN**

Run: `cargo test select_pane`

Expected: PASS.

### Task 2: Server Active Pane Selection

**Files:**
- Modify: `src/main.rs`
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Add failing integration test**

Add this test:

```rust
#[test]
fn select_pane_switches_active_capture_target() {
    let socket = unique_socket("select-pane");
    let session = format!("select-pane-{}", std::process::id());

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

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    let selected = poll_capture(&socket, &session, "base-ready");
    assert!(selected.contains("base-ready"), "{selected:?}");
    assert!(!selected.contains("split-ready"), "{selected:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 2: Verify RED**

Run: `cargo test --test phase1_cli select_pane_switches_active_capture_target`

Expected: FAIL because the binary has no `select-pane` server behavior yet.

- [x] **Step 3: Wire command through main and server**

In `src/main.rs`, send `protocol::encode_select_pane(&session, pane)`. In `src/server.rs`, add `Session::select_pane(index)` that checks the index exists and updates `active_pane`; return `ERR missing pane` for out-of-range indexes.

- [x] **Step 4: Verify GREEN**

Run: `cargo test --test phase1_cli select_pane_switches_active_capture_target`

Expected: PASS.

### Task 3: Docs, Verification, Commit, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-select-pane.md`

- [x] **Step 1: Document the command**

Add `dmux select-pane -t <name> -p <index>` to the README command list and mention active pane selection in the Phase 3 groundwork bullets.

- [x] **Step 2: Run full verification**

Run:

```bash
cargo fmt --check
cargo test
forbidden-keyword scan
```

Expected: formatting and tests pass; the forbidden-keyword scan prints no matches.

- [x] **Step 3: Commit**

Run:

```bash
git add README.md src/cli.rs src/main.rs src/protocol.rs src/server.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-select-pane.md
git commit -m "feat: add active pane selection"
```

- [ ] **Step 4: Push and open draft PR**

Run:

```bash
git push -u origin devmux-select-pane
gh pr create --draft --base main --head devmux-select-pane --title "Add active pane selection" --body "<summary and validation>"
```
