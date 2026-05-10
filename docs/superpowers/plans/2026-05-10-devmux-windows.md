# Windows Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a minimal window model with `new-window`, `list-windows`, and `select-window`.

**Architecture:** Wrap the existing pane set inside a `Window` object, then make each `Session` own a mutex-protected `WindowSet`. Existing pane commands operate on the active window's active pane. Layout rendering and named windows are deferred.

**Tech Stack:** Rust standard library, existing Unix socket protocol, POSIX PTY helpers, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: CLI And Protocol Surface

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`

- [x] **Step 1: Write failing CLI and protocol tests**

Add CLI tests:

```rust
#[test]
fn parses_new_window_target_and_command() {
    let command = parse_args([
        "dmux", "new-window", "-t", "dev", "--", "sh", "-c", "echo window",
    ])
    .unwrap();
    assert_eq!(
        command,
        Command::NewWindow {
            session: "dev".to_string(),
            command: vec!["sh".to_string(), "-c".to_string(), "echo window".to_string()],
        }
    );
}

#[test]
fn parses_list_windows_target() {
    let command = parse_args(["dmux", "list-windows", "-t", "dev"]).unwrap();
    assert_eq!(
        command,
        Command::ListWindows {
            session: "dev".to_string()
        }
    );
}

#[test]
fn parses_select_window_target_and_index() {
    let command = parse_args(["dmux", "select-window", "-t", "dev", "-w", "1"]).unwrap();
    assert_eq!(
        command,
        Command::SelectWindow {
            session: "dev".to_string(),
            window: 1,
        }
    );
}
```

Add protocol tests:

```rust
#[test]
fn round_trips_new_window_request() {
    let command = vec!["sh".to_string(), "-c".to_string(), "echo window".to_string()];
    let line = encode_new_window("dev", &command);
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::NewWindow {
            session: "dev".to_string(),
            command,
        }
    );
}

#[test]
fn round_trips_select_window_request() {
    let line = encode_select_window("dev", 1);
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::SelectWindow {
            session: "dev".to_string(),
            window: 1,
        }
    );
}
```

- [x] **Step 2: Verify RED**

Run: `cargo test window`

Expected: FAIL because window commands and protocol do not exist.

- [x] **Step 3: Implement minimal parsing and encoding**

Add `Command::{NewWindow, ListWindows, SelectWindow}` and `Request::{NewWindow, ListWindows, SelectWindow}`. Encode `NEW_WINDOW`, `LIST_WINDOWS`, and `SELECT_WINDOW` with the existing argument separator pattern.

- [x] **Step 4: Verify GREEN**

Run: `cargo test window`

Expected: PASS.

### Task 2: Server Window Model

**Files:**
- Modify: `src/main.rs`
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Add failing integration test**

Add this test:

```rust
#[test]
fn new_window_creates_second_active_window() {
    let socket = unique_socket("new-window");
    let session = format!("new-window-{}", std::process::id());

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
            "printf base-window; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-window");
    assert!(base.contains("base-window"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-window",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf second-window; sleep 30",
        ],
    ));

    let windows = dmux(&socket, &["list-windows", "-t", &session]);
    assert_success(&windows);
    let windows = String::from_utf8_lossy(&windows.stdout);
    assert_eq!(windows.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    let second = poll_capture(&socket, &session, "second-window");
    assert!(second.contains("second-window"), "{second:?}");
    assert!(!second.contains("base-window"), "{second:?}");

    assert_success(&dmux(&socket, &["select-window", "-t", &session, "-w", "0"]));
    let selected = poll_capture(&socket, &session, "base-window");
    assert!(selected.contains("base-window"), "{selected:?}");
    assert!(!selected.contains("second-window"), "{selected:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 2: Verify RED**

Run: `cargo test --test phase1_cli new_window_creates_second_active_window`

Expected: FAIL because server-side window commands are not implemented.

- [x] **Step 3: Refactor `Session` to own `WindowSet`**

Add:

```rust
struct Window {
    panes: PaneSet<Arc<Pane>>,
}

struct WindowSet {
    windows: Vec<Window>,
    active: usize,
}
```

`Session` owns `Mutex<WindowSet>`. Existing `active_pane`, `add_pane`, `select_pane`, `kill_pane`, `pane_count`, and `panes` operate through the active window.

- [x] **Step 4: Implement window handlers**

`handle_new_window` creates a new `Window` with one pane, appends it, and selects it. `handle_list_windows` prints window indexes. `handle_select_window` selects by index or returns `ERR missing window`.

- [x] **Step 5: Verify GREEN**

Run: `cargo test --test phase1_cli new_window_creates_second_active_window`

Expected: PASS.

### Task 3: Docs, Verification, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-windows.md`

- [x] **Step 1: Document the command**

Add `dmux new-window -t <name> [-- command...]`, `dmux list-windows -t <name>`, and `dmux select-window -t <name> -w <index>` to README. Mention minimal window tracking in implemented groundwork.

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

- [x] **Step 4: Commit and open draft PR**

Run:

```bash
git add README.md src/cli.rs src/main.rs src/protocol.rs src/server.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-windows.md
git commit -m "feat: add window selection"
git push -u origin devmux-windows
gh pr create --draft --base main --head devmux-windows --title "Add window selection" --body "<summary and validation>"
```
