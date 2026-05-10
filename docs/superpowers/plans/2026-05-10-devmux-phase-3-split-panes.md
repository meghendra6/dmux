# Split Panes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the first Phase 3 pane model slice: `split-window` creates an additional PTY pane inside an existing session.

**Architecture:** Keep the existing single attached view and introduce a `Pane` object inside each `Session`. Existing capture, resize, send, and attach operations target the active pane; `split-window` appends a new pane and makes it active. Layout rendering is intentionally deferred.

**Tech Stack:** Rust standard library, Unix socket control protocol, POSIX PTY helpers already present in `src/pty.rs`, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: CLI And Protocol Surface

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`

- [x] **Step 1: Add failing CLI and protocol tests**

Add these tests:

```rust
#[test]
fn parses_split_window_target_direction_and_command() {
    let command = parse_args([
        "dmux", "split-window", "-t", "dev", "-h", "--", "sh", "-c", "echo split",
    ])
    .unwrap();

    assert_eq!(
        command,
        Command::SplitWindow {
            session: "dev".to_string(),
            direction: SplitDirection::Horizontal,
            command: vec!["sh".to_string(), "-c".to_string(), "echo split".to_string()],
        }
    );
}

#[test]
fn parses_list_panes_target() {
    let command = parse_args(["dmux", "list-panes", "-t", "dev"]).unwrap();
    assert_eq!(
        command,
        Command::ListPanes {
            session: "dev".to_string()
        }
    );
}
```

```rust
#[test]
fn round_trips_split_request() {
    let command = vec!["sh".to_string(), "-c".to_string(), "echo split".to_string()];
    let line = encode_split("dev", SplitDirection::Horizontal, &command);

    assert_eq!(
        decode_request(&line).unwrap(),
        Request::Split {
            session: "dev".to_string(),
            direction: SplitDirection::Horizontal,
            command
        }
    );
}
```

- [x] **Step 2: Verify RED**

Run: `cargo test cli::tests::parses_split_window_target_direction_and_command protocol::tests::round_trips_split_request`

Expected: FAIL because `SplitWindow`, `ListPanes`, and split protocol support do not exist yet.

- [x] **Step 3: Implement minimal CLI and protocol parsing**

Add a shared `SplitDirection` enum in `src/protocol.rs` and import it in `src/cli.rs`. Add `split-window` and `split` parsing with required `-t <session>` plus one of `-h` or `-v`. Add `list-panes -t <session>`.

- [x] **Step 4: Verify GREEN**

Run: `cargo test cli::tests::parses_split_window_target_direction_and_command protocol::tests::round_trips_split_request`

Expected: PASS.

### Task 2: Server Pane Model

**Files:**
- Modify: `src/server.rs`
- Modify: `src/main.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Add failing integration test**

Add this test:

```rust
#[test]
fn split_window_creates_second_active_pane() {
    let socket = unique_socket("split-window");
    let session = format!("split-window-{}", std::process::id());

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

    let panes = dmux(&socket, &["list-panes", "-t", &session]);
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    let active = poll_capture(&socket, &session, "split-ready");
    assert!(active.contains("split-ready"), "{active:?}");
    assert!(!active.contains("base-ready"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 2: Verify RED**

Run: `cargo test --test phase1_cli split_window_creates_second_active_pane`

Expected: FAIL because the binary has no server implementation for split/list panes.

- [x] **Step 3: Introduce `Pane` and active-pane helpers**

Refactor `src/server.rs` so `Session` owns:

```rust
struct Session {
    panes: Mutex<Vec<Arc<Pane>>>,
    active_pane: Mutex<usize>,
}
```

Move existing PTY state into:

```rust
struct Pane {
    child_pid: i32,
    writer: Arc<Mutex<File>>,
    size: Mutex<PtySize>,
    raw_history: Arc<Mutex<Vec<u8>>>,
    terminal: Arc<Mutex<TerminalState>>,
    clients: Arc<Mutex<Vec<UnixStream>>>,
}
```

Add `Session::active_pane()` to return the active `Arc<Pane>`, and update capture, resize, send, and attach to use it.

- [x] **Step 4: Add split/list pane handlers**

Add `Request::Split` and `Request::ListPanes` handling in `src/main.rs` and `src/server.rs`. `handle_split` spawns a PTY with the active pane size, appends it to the session, marks it active, starts an output pump, and returns `OK`. `handle_list_panes` returns `OK` followed by pane indexes, one per line.

- [x] **Step 5: Verify GREEN**

Run: `cargo test --test phase1_cli split_window_creates_second_active_pane`

Expected: PASS.

### Task 3: Docs And Full Verification

**Files:**
- Modify: `README.md`

- [x] **Step 1: Document the new commands and current limits**

Add `dmux split-window -t <name> -h|-v [-- command...]` and `dmux list-panes -t <name>` to the command list. Update current limits to say layout rendering is not implemented yet rather than saying only one pane exists.

- [x] **Step 2: Run formatting and full tests**

Run:

```bash
cargo fmt --check
cargo test
```

Expected: all tests pass.

- [x] **Step 3: Commit**

Run:

```bash
git add src/cli.rs src/protocol.rs src/main.rs src/server.rs tests/phase1_cli.rs README.md docs/superpowers/plans/2026-05-10-devmux-phase-3-split-panes.md
git commit -m "feat: add split pane sessions"
```
