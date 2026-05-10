# Kill Pane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `kill-pane` so a multi-pane session can remove one pane without killing the whole session.

**Architecture:** Reuse the existing server-side pane vector and active pane index. `kill-pane` targets the active pane by default or a pane index with `-p`; the server refuses to remove the final pane so session-level lifecycle remains explicit.

**Tech Stack:** Rust standard library, existing Unix socket protocol, POSIX process termination helper in `src/pty.rs`, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: CLI And Protocol Surface

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`

- [x] **Step 1: Write failing CLI and protocol tests**

Add CLI tests:

```rust
#[test]
fn parses_kill_pane_target_and_index() {
    let command = parse_args(["dmux", "kill-pane", "-t", "dev", "-p", "1"]).unwrap();
    assert_eq!(
        command,
        Command::KillPane {
            session: "dev".to_string(),
            pane: Some(1),
        }
    );
}

#[test]
fn parses_kill_pane_target_without_index() {
    let command = parse_args(["dmux", "kill-pane", "-t", "dev"]).unwrap();
    assert_eq!(
        command,
        Command::KillPane {
            session: "dev".to_string(),
            pane: None,
        }
    );
}
```

Add protocol tests:

```rust
#[test]
fn round_trips_kill_pane_request_with_index() {
    let line = encode_kill_pane("dev", Some(1));
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::KillPane {
            session: "dev".to_string(),
            pane: Some(1),
        }
    );
}

#[test]
fn round_trips_kill_pane_request_for_active_pane() {
    let line = encode_kill_pane("dev", None);
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::KillPane {
            session: "dev".to_string(),
            pane: None,
        }
    );
}
```

- [x] **Step 2: Verify RED**

Run: `cargo test kill_pane`

Expected: FAIL because `Command::KillPane`, `Request::KillPane`, and `encode_kill_pane` do not exist.

- [x] **Step 3: Implement minimal parsing and encoding**

Add `Command::KillPane { session: String, pane: Option<usize> }`, parse `kill-pane -t <session> [-p <index>]`, add `Request::KillPane`, and encode/decode `KILL_PANE\t<session>\tactive\n` or `KILL_PANE\t<session>\t<index>\n`.

- [x] **Step 4: Verify GREEN**

Run: `cargo test kill_pane`

Expected: PASS.

### Task 2: Server Pane Removal

**Files:**
- Modify: `src/main.rs`
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Add failing integration test**

Add this test:

```rust
#[test]
fn kill_pane_removes_active_pane_and_keeps_session() {
    let socket = unique_socket("kill-pane");
    let session = format!("kill-pane-{}", std::process::id());

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

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session]));

    let panes = dmux(&socket, &["list-panes", "-t", &session]);
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0"]);

    let active = poll_capture(&socket, &session, "base-ready");
    assert!(active.contains("base-ready"), "{active:?}");
    assert!(!active.contains("split-ready"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 2: Verify RED**

Run: `cargo test --test phase1_cli kill_pane_removes_active_pane_and_keeps_session`

Expected: FAIL because server-side pane removal is not implemented.

- [x] **Step 3: Implement server handler**

Wire `Command::KillPane` in `src/main.rs`. In `src/server.rs`, add `Session::kill_pane(target: Option<usize>) -> Result<Arc<Pane>, &'static str>`, remove the pane, update active index, terminate the removed pane process, and return `ERR cannot kill last pane` when only one pane remains.

- [x] **Step 4: Verify GREEN**

Run: `cargo test --test phase1_cli kill_pane_removes_active_pane_and_keeps_session`

Expected: PASS.

### Task 3: Docs, Verification, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-kill-pane.md`

- [x] **Step 1: Document the command**

Add `dmux kill-pane -t <name> [-p <index>]` to the README command list and mention pane removal in Phase 3 groundwork.

- [x] **Step 2: Run full verification**

Run:

```bash
cargo fmt --check
cargo test
forbidden-keyword scan
```

Expected: formatting and tests pass; the forbidden-keyword scan prints no matches.

- [x] **Step 3: Request subagent review**

Dispatch a read-only review subagent over the diff against `origin/main`. Apply any technically valid blocking or important findings, then rerun full verification.

- [x] **Step 4: Commit and open draft PR**

Run:

```bash
git add README.md src/cli.rs src/main.rs src/protocol.rs src/server.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-kill-pane.md
git commit -m "feat: add pane removal"
git push -u origin devmux-kill-pane
gh pr create --draft --base main --head devmux-kill-pane --title "Add pane removal" --body "<summary and validation>"
```
