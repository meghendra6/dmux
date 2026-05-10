# DevMux Phase 2 Resize Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add resize groundwork for Phase 2 by making PTY dimensions explicit, resizing session PTYs through the control socket, sending `SIGWINCH` through the kernel PTY path, and keeping capture state aligned with the current terminal size.

**Architecture:** Introduce a small `PtySize` type in `src/pty.rs`. Session creation uses an explicit size, the server owns current size per session, and a new `resize-pane` command sends `RESIZE` requests to the server. The server applies `ioctl(TIOCSWINSZ)` to the PTY master and resizes the derived `TerminalState`; attached clients still receive raw bytes unchanged.

**Tech Stack:** Rust standard library plus existing POSIX FFI. No external terminal parser or async runtime.

---

### Task 1: PTY Size Type And Terminal Resize Tests

**Files:**
- Modify: `src/pty.rs`
- Modify: `src/term.rs`

- [ ] **Step 1: Write failing unit tests**

Add `PtySize` tests in `src/pty.rs`:

```rust
#[test]
fn pty_size_rejects_zero_dimensions() {
    assert!(PtySize::new(0, 24).is_err());
    assert!(PtySize::new(80, 0).is_err());
    assert_eq!(PtySize::new(80, 24).unwrap(), PtySize { cols: 80, rows: 24 });
}
```

Add terminal resize tests in `src/term.rs`:

```rust
#[test]
fn resize_changes_wrap_width_for_future_output() {
    let mut state = TerminalState::new(5, 3, 100);
    state.apply_bytes(b"abcde");
    state.resize(3, 3);
    state.apply_bytes(b"XYZ");

    let captured = state.capture_text();
    assert!(captured.contains("abc"), "{captured:?}");
    assert!(captured.contains("XYZ"), "{captured:?}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test pty::tests::pty_size_rejects_zero_dimensions term::tests::resize_changes_wrap_width_for_future_output`

Expected: compilation fails because `PtySize` and `TerminalState::resize` do not exist.

- [ ] **Step 3: Implement size primitives**

Implement:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}
```

`PtySize::new(cols, rows)` returns `io::Result<Self>` and rejects zero dimensions.

Add `TerminalState::resize(width, height)` that:

- clamps width/height to at least 1
- creates a new screen grid
- copies the overlapping region from the old screen
- clamps cursor position
- does not mutate scrollback

- [ ] **Step 4: Run tests**

Run: `cargo test pty::tests term::tests`

Expected: all PTY and terminal tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/pty.rs src/term.rs
git commit -m "feat: add pty size and screen resize primitives"
```

---

### Task 2: Control Protocol And CLI Resize Command

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write failing parser/protocol tests**

Add CLI parser test:

```rust
#[test]
fn parses_resize_pane_target_and_size() {
    let command = parse_args(["dmux", "resize-pane", "-t", "dev", "-x", "100", "-y", "40"]).unwrap();
    assert_eq!(
        command,
        Command::ResizePane {
            session: "dev".to_string(),
            cols: 100,
            rows: 40,
        }
    );
}
```

Add protocol round-trip test:

```rust
#[test]
fn round_trips_resize_request() {
    let line = encode_resize("dev", 100, 40);
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::Resize {
            session: "dev".to_string(),
            cols: 100,
            rows: 40,
        }
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test cli::tests::parses_resize_pane_target_and_size protocol::tests::round_trips_resize_request`

Expected: compilation fails because resize variants/helpers are missing.

- [ ] **Step 3: Implement parser and protocol**

Add:

```rust
Command::ResizePane { session: String, cols: u16, rows: u16 }
Request::Resize { session: String, cols: u16, rows: u16 }
```

Support:

```bash
dmux resize-pane -t <session> -x <cols> -y <rows>
```

Protocol line:

```text
RESIZE\t<session>\t<cols>\t<rows>\n
```

Wire `main.rs` to send the resize request and print no output on success.

- [ ] **Step 4: Run tests**

Run: `cargo test cli::tests protocol::tests`

Expected: parser and protocol tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/cli.rs src/protocol.rs src/main.rs
git commit -m "feat: add resize-pane command protocol"
```

---

### Task 3: Server PTY Resize Integration

**Files:**
- Modify: `src/pty.rs`
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [ ] **Step 1: Write failing integration test**

Add:

```rust
#[test]
fn resize_pane_updates_child_pty_size() {
    let socket = unique_socket("resize");
    let session = format!("resize-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "stty size; trap 'stty size' WINCH; sleep 30"],
    ));

    let initial = poll_capture(&socket, &session, "24 80");
    assert!(initial.contains("24 80"), "{initial:?}");

    assert_success(&dmux(&socket, &["resize-pane", "-t", &session, "-x", "100", "-y", "40"]));

    let resized = poll_capture(&socket, &session, "40 100");
    assert!(resized.contains("40 100"), "{resized:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test phase1_cli resize_pane_updates_child_pty_size`

Expected: failure because server does not handle resize requests.

- [ ] **Step 3: Implement resize**

Implement `pty::resize(master: &File, size: PtySize) -> io::Result<()>` using `ioctl(TIOCSWINSZ)`.

Session stores:

```rust
size: Mutex<PtySize>
```

Server handles `Request::Resize`:

- validate dimensions with `PtySize::new`
- call `pty::resize` on the session writer/master
- update `size`
- call `terminal.resize(cols, rows)`
- return `OK`

Session creation uses `PtySize { cols: 80, rows: 24 }` instead of hardcoded local `WinSize`.

- [ ] **Step 4: Run tests**

Run: `cargo test`

Expected: all tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/pty.rs src/server.rs tests/phase1_cli.rs
git commit -m "feat: resize session ptys"
```

---

### Task 4: Documentation And Verification

**Files:**
- Modify: `README.md`
- Add: `docs/superpowers/plans/2026-05-10-devmux-phase-2-resize.md`

- [ ] **Step 1: Document resize command**

Add `dmux resize-pane -t <name> -x <cols> -y <rows>` to implemented commands and mention it sends PTY resize updates.

- [ ] **Step 2: Run verification**

Run:

```bash
cargo fmt --check
cargo test
```

Expected: both commands pass.

- [ ] **Step 3: Commit**

Run:

```bash
git add README.md docs/superpowers/plans/2026-05-10-devmux-phase-2-resize.md
git commit -m "docs: plan phase 2 resize slice"
```

---

## Self-Review

- This plan covers resize/SIGWINCH groundwork only.
- It does not implement layout, multiple panes, multi-client resize policy, raw terminal dimension detection, or automatic attach-time resize.
- The server remains PTY owner; clients only request size changes over the control protocol.
