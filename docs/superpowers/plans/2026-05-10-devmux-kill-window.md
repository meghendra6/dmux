# Kill Window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `kill-window` so a multi-window session can remove one window without killing the whole session.

**Architecture:** Reuse the `WindowSet` introduced in the window selection slice. `kill-window` targets the active window by default or a window index with `-w`; the server refuses to remove the final window so session lifecycle remains explicit. Removed window panes are terminated before the window is untracked.

**Tech Stack:** Rust standard library, existing Unix socket protocol, existing process-group termination in `src/pty.rs`, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: CLI And Protocol Surface

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`

- [x] **Step 1: Write failing CLI and protocol tests**

Add CLI tests:

```rust
#[test]
fn parses_kill_window_target_and_index() {
    let command = parse_args(["dmux", "kill-window", "-t", "dev", "-w", "1"]).unwrap();
    assert_eq!(
        command,
        Command::KillWindow {
            session: "dev".to_string(),
            window: Some(1),
        }
    );
}

#[test]
fn parses_kill_window_target_without_index() {
    let command = parse_args(["dmux", "kill-window", "-t", "dev"]).unwrap();
    assert_eq!(
        command,
        Command::KillWindow {
            session: "dev".to_string(),
            window: None,
        }
    );
}
```

Add protocol tests:

```rust
#[test]
fn round_trips_kill_window_request_with_index() {
    let line = encode_kill_window("dev", Some(1));
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::KillWindow {
            session: "dev".to_string(),
            window: Some(1),
        }
    );
}

#[test]
fn round_trips_kill_window_request_for_active_window() {
    let line = encode_kill_window("dev", None);
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::KillWindow {
            session: "dev".to_string(),
            window: None,
        }
    );
}
```

- [x] **Step 2: Verify RED**

Run: `cargo test --bin dmux kill_window`

Expected: FAIL because kill-window command and protocol do not exist.

- [x] **Step 3: Implement minimal parsing and encoding**

Add `Command::KillWindow { session: String, window: Option<usize> }`, parse `kill-window -t <session> [-w <index>]`, add `Request::KillWindow`, and encode/decode `KILL_WINDOW\t<session>\tactive\n` or `KILL_WINDOW\t<session>\t<index>\n`.

- [x] **Step 4: Verify GREEN**

Run: `cargo test --bin dmux kill_window`

Expected: PASS.

### Task 2: Server Window Removal

**Files:**
- Modify: `src/main.rs`
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Add failing integration tests**

Add tests that:

1. Create a session and second window, run `kill-window -t <session>`, verify only window `0` remains and capture returns base window output.
2. Create a session and second window, run `kill-window -t <session> -w 0`, verify only window `0` remains and capture returns former second window output.

- [x] **Step 2: Verify RED**

Run: `cargo test --test phase1_cli kill_window`

Expected: FAIL because server-side window removal is not implemented.

- [x] **Step 3: Implement server handler**

Wire `Command::KillWindow` in `src/main.rs`. In `src/server.rs`, add `WindowSet::kill_window(target: Option<usize>) -> Result<Vec<Arc<Pane>>, &'static str>`, remove the window only after all panes in that window are terminated, update active window index, and return `ERR cannot kill last window; use kill-session` when only one window remains.

- [x] **Step 4: Verify GREEN**

Run: `cargo test --test phase1_cli kill_window`

Expected: PASS.

### Task 3: Docs, Verification, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-kill-window.md`

- [x] **Step 1: Document the command**

Add `dmux kill-window -t <name> [-w <index>]` to README and mention window removal in implemented groundwork.

- [x] **Step 2: Run full verification**

Run:

```bash
cargo fmt --check
cargo test
forbidden-keyword scan
```

Expected: formatting and tests pass; the forbidden-keyword scan prints no matches.

- [x] **Step 3: Request subagent review**

Dispatch a read-only review subagent over `git diff origin/main`. Apply technically valid blocking or important findings and rerun full verification.

- [ ] **Step 4: Commit and open draft PR**

Run:

```bash
git add README.md src/cli.rs src/main.rs src/protocol.rs src/server.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-kill-window.md
git commit -m "feat: add window removal"
git push -u origin devmux-kill-window
gh pr create --draft --base main --head devmux-kill-window --title "Add window removal" --body "<summary and validation>"
```
