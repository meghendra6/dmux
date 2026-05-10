# DevMux Phase 2 Screen Capture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace raw-byte `capture-pane` behavior with a minimal terminal screen model and bounded scrollback so captured output reflects terminal semantics for basic shell output.

**Architecture:** Keep PTY byte ownership in `dmuxd`. The server still broadcasts raw PTY bytes to attached clients, but each session also feeds those bytes into a `TerminalState` that maintains a text grid plus scrollback. `capture-pane -p` reads from the terminal state, not the raw replay buffer.

**Tech Stack:** Rust standard library only. This first Phase 2 slice implements a small parser for printable ASCII, CR/LF, tab, backspace, SGR stripping, simple cursor movement, clear screen, and OSC skipping. A full parser crate can replace this later without changing the server boundary.

---

### Task 1: Terminal State Unit Tests

**Files:**
- Create: `src/term.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write failing unit tests**

Add `mod term;` in `src/main.rs`.

Create `src/term.rs` with these tests and no implementation:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carriage_return_rewrites_current_line() {
        let mut state = TerminalState::new(10, 3, 100);
        state.apply_bytes(b"hello\rworld");
        assert_eq!(state.capture_text(), "world\n");
    }

    #[test]
    fn sgr_sequences_do_not_appear_in_capture() {
        let mut state = TerminalState::new(10, 3, 100);
        state.apply_bytes(b"\x1b[31mred\x1b[0m\n");
        let captured = state.capture_text();
        assert!(captured.contains("red"), "{captured:?}");
        assert!(!captured.contains('\x1b'), "{captured:?}");
    }

    #[test]
    fn scrollback_keeps_lines_that_leave_the_screen() {
        let mut state = TerminalState::new(10, 3, 100);
        state.apply_bytes(b"1\n2\n3\n4\n");
        let captured = state.capture_text();
        assert!(captured.contains("1"), "{captured:?}");
        assert!(captured.contains("4"), "{captured:?}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test term::tests`

Expected: compilation fails because `TerminalState` is not defined.

- [ ] **Step 3: Implement minimal terminal state**

Implement:

```rust
pub struct TerminalState {
    screen: TerminalScreen,
    scrollback: Scrollback,
    parser: ParserState,
}
```

Required behavior:

- printable ASCII writes at the cursor and advances it
- `\r` moves cursor column to 0
- `\n` moves down and scrolls when at the bottom
- `\t` inserts spaces to the next tab stop
- `\x08` moves cursor left one column
- CSI `m` sequences are consumed and ignored
- CSI `2J` clears the screen and moves cursor to 0,0
- CSI `K` clears from cursor to end of line
- OSC sequences are skipped until BEL or `ESC \`
- `capture_text()` returns scrollback plus non-empty current screen rows, with trailing spaces removed

- [ ] **Step 4: Run tests**

Run: `cargo test term::tests`

Expected: 3 tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/main.rs src/term.rs
git commit -m "feat: add basic terminal screen state"
```

---

### Task 2: Server Capture Uses Terminal State

**Files:**
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [ ] **Step 1: Write failing integration tests**

Add two integration tests:

```rust
#[test]
fn capture_pane_strips_sgr_sequences() {
    let socket = unique_socket("sgr-capture");
    let session = format!("sgr-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "printf '\\033[31mred\\033[0m\\n'; sleep 30"],
    ));

    let captured = poll_capture(&socket, &session, "red");
    assert!(captured.contains("red"), "{captured:?}");
    assert!(!captured.contains('\x1b'), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn capture_pane_applies_carriage_return_overwrite() {
    let socket = unique_socket("cr-capture");
    let session = format!("cr-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "printf 'hello\\rworld'; sleep 30"],
    ));

    let captured = poll_capture(&socket, &session, "world");
    assert!(captured.contains("world"), "{captured:?}");
    assert!(!captured.contains("hello"), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test phase1_cli capture_pane`

Expected: at least the SGR test fails because current capture returns raw escape bytes.

- [ ] **Step 3: Feed PTY output into terminal state**

Change `Session` to keep both:

```rust
raw_history: Arc<Mutex<Vec<u8>>>,
terminal: Arc<Mutex<TerminalState>>,
```

Update the output pump:

- append raw bytes to `raw_history` for attach replay
- call `terminal.apply_bytes(bytes)` for screen-aware capture
- broadcast raw bytes to attached clients unchanged

Update `handle_capture` to return `terminal.capture_text()`.

- [ ] **Step 4: Run tests**

Run: `cargo test`

Expected: all existing tests plus new capture tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/server.rs tests/phase1_cli.rs
git commit -m "feat: capture panes from terminal screen state"
```

---

### Task 3: Documentation And Verification

**Files:**
- Modify: `README.md`
- Add: `docs/superpowers/plans/2026-05-10-devmux-phase-2-screen-capture.md`

- [ ] **Step 1: Document Phase 2 slice**

Update README current limits to say capture is now screen-aware for basic shell output, but full terminal protocol support is still incomplete.

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
git add README.md docs/superpowers/plans/2026-05-10-devmux-phase-2-screen-capture.md
git commit -m "docs: plan phase 2 screen capture slice"
```

---

## Self-Review

- This plan covers the Phase 2 screen/capture groundwork only.
- It does not implement a full terminal parser, copy mode, panes/windows, alternate screen, mouse, extended keyboard, OSC policy UI, or agent features.
- Server/client separation remains intact: raw PTY bytes are broadcast unchanged, while capture reads derived terminal state.
