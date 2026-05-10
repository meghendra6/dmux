# DevMux Phase 2 Attach Size Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `dmux attach` resize the target session to the attaching terminal size before starting interactive passthrough.

**Architecture:** Keep resize ownership in the server. The CLI detects terminal dimensions, sends the existing `RESIZE` control request, then calls the attach client. Detection lives in `client.rs` and is testable without a TTY through a narrow `DEVMUX_ATTACH_SIZE=<cols>x<rows>` override used only by tests and automation.

**Tech Stack:** Rust standard library plus existing `stty size` shell-out for terminal size detection. No async runtime or external terminal crate.

---

### Task 1: Terminal Size Detection

**Files:**
- Modify: `src/client.rs`

- [ ] **Step 1: Write failing unit tests**

Add tests:

```rust
#[test]
fn parses_attach_size_override() {
    assert_eq!(
        parse_attach_size("120x40").unwrap(),
        crate::pty::PtySize { cols: 120, rows: 40 }
    );
    assert!(parse_attach_size("0x40").is_err());
    assert!(parse_attach_size("120").is_err());
}

#[test]
fn parses_stty_size_output_as_rows_then_cols() {
    assert_eq!(
        parse_stty_size("40 120\n").unwrap(),
        crate::pty::PtySize { cols: 120, rows: 40 }
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test client::tests`

Expected: compilation fails because parser helpers do not exist.

- [ ] **Step 3: Implement detection helpers**

Implement:

```rust
pub fn detect_attach_size() -> Option<crate::pty::PtySize>
fn parse_attach_size(value: &str) -> io::Result<crate::pty::PtySize>
fn parse_stty_size(value: &str) -> io::Result<crate::pty::PtySize>
```

Rules:

- `DEVMUX_ATTACH_SIZE=<cols>x<rows>` wins when set.
- Otherwise, if stdin is a TTY, run `stty size` and parse `<rows> <cols>`.
- Return `None` if no usable size is available.

- [ ] **Step 4: Run tests**

Run: `cargo test client::tests`

Expected: tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/client.rs
git commit -m "feat: detect attach terminal size"
```

---

### Task 2: Attach Sends Resize Before Passthrough

**Files:**
- Modify: `src/main.rs`
- Modify: `tests/phase1_cli.rs`

- [ ] **Step 1: Write failing integration test**

Add:

```rust
#[test]
fn attach_resizes_session_before_passthrough() {
    let socket = unique_socket("attach-size");
    let session = format!("attach-size-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "stty size; while true; do sleep 0.2; stty size; done"],
    ));

    let initial = poll_capture(&socket, &session, "24 80");
    assert!(initial.contains("24 80"), "{initial:?}");

    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .env("DEVMUX_ATTACH_SIZE", "132x43")
        .args(["attach", "-t", &session])
        .output()
        .expect("run attach");
    assert_success(&output);

    let resized = poll_capture(&socket, &session, "43 132");
    assert!(resized.contains("43 132"), "{resized:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test phase1_cli attach_resizes_session_before_passthrough`

Expected: failure because attach does not send resize.

- [ ] **Step 3: Implement attach-time resize**

In `main.rs` attach branch:

1. call `client::detect_attach_size()`
2. if `Some(size)`, send `protocol::encode_resize(&session, size.cols, size.rows)`
3. call `client::attach(...)`

The attach client already exits immediately in non-interactive tests because stdin is not a TTY/has EOF, so this test does not need interactive input.

- [ ] **Step 4: Run tests**

Run: `cargo test`

Expected: all tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/main.rs tests/phase1_cli.rs
git commit -m "feat: resize session on attach"
```

---

### Task 3: Documentation And Verification

**Files:**
- Modify: `README.md`
- Add: `docs/superpowers/plans/2026-05-10-devmux-phase-2-attach-size.md`

- [ ] **Step 1: Document attach-size behavior**

Add note that attach resizes the session PTY to the current terminal size when available. Mention `DEVMUX_ATTACH_SIZE=<cols>x<rows>` as a test/automation override.

- [ ] **Step 2: Run verification**

Run:

```bash
cargo fmt --check
cargo test
```

Expected: both pass.

- [ ] **Step 3: Commit**

Run:

```bash
git add README.md docs/superpowers/plans/2026-05-10-devmux-phase-2-attach-size.md
git commit -m "docs: plan phase 2 attach size slice"
```

---

## Self-Review

- This plan implements attach-time size propagation only.
- It does not implement multi-client resize policy, resize debouncing, automatic resize-on-SIGWINCH while attached, or pane/window layout.
- `DEVMUX_ATTACH_SIZE` is an automation/testing seam, not a user-facing config surface.
