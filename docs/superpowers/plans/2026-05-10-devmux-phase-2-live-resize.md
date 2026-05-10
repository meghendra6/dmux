# DevMux Phase 2 Live Resize Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** While attached, react to terminal `SIGWINCH` and resize the server-owned PTY when the current terminal size changes.

**Architecture:** Keep resize state in the client attach path. A tiny signal handler flips an atomic flag on `SIGWINCH`; the attach input loop checks that flag and calls a resize callback when `detect_attach_size()` returns a size different from the last sent size. The server protocol is already implemented through `RESIZE`, so this slice only adds client-side live resize dispatch.

**Tech Stack:** Rust standard library plus POSIX `signal`. No event loop or async runtime.

---

### Task 1: Resize Event Coalescing Helper

**Files:**
- Modify: `src/client.rs`

- [ ] **Step 1: Write failing unit tests**

Add a small helper test:

```rust
#[test]
fn maybe_emit_resize_only_emits_changed_sizes() {
    let mut last = Some(crate::pty::PtySize { cols: 80, rows: 24 });
    let mut emitted = Vec::new();

    maybe_emit_resize(Some(crate::pty::PtySize { cols: 80, rows: 24 }), &mut last, |size| {
        emitted.push(size);
        Ok(())
    })
    .unwrap();
    maybe_emit_resize(Some(crate::pty::PtySize { cols: 100, rows: 40 }), &mut last, |size| {
        emitted.push(size);
        Ok(())
    })
    .unwrap();
    maybe_emit_resize(None, &mut last, |size| {
        emitted.push(size);
        Ok(())
    })
    .unwrap();

    assert_eq!(emitted, vec![crate::pty::PtySize { cols: 100, rows: 40 }]);
    assert_eq!(last, Some(crate::pty::PtySize { cols: 100, rows: 40 }));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test client::tests::maybe_emit_resize_only_emits_changed_sizes`

Expected: compilation fails because `maybe_emit_resize` does not exist.

- [ ] **Step 3: Implement helper**

Implement:

```rust
fn maybe_emit_resize<F>(
    current: Option<PtySize>,
    last: &mut Option<PtySize>,
    emit: F,
) -> io::Result<()>
where
    F: FnOnce(PtySize) -> io::Result<()>,
```

Behavior:

- if `current` is `None`, do nothing
- if `current == *last`, do nothing
- otherwise call `emit(size)` and update `last`

- [ ] **Step 4: Run tests**

Run: `cargo test client::tests`

Expected: all client tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/client.rs
git commit -m "feat: coalesce attach resize events"
```

---

### Task 2: Live Resize During Attach

**Files:**
- Modify: `src/client.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write failing unit test for signal flag**

Add:

```rust
#[test]
fn winch_flag_can_be_taken_once() {
    WINCH_PENDING.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(take_winch_pending());
    assert!(!take_winch_pending());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test client::tests::winch_flag_can_be_taken_once`

Expected: compilation fails because `WINCH_PENDING`/`take_winch_pending` do not exist.

- [ ] **Step 3: Implement live resize plumbing**

Add in `client.rs`:

- static `WINCH_PENDING: AtomicBool`
- `install_winch_handler()`
- `take_winch_pending()`
- POSIX signal handler for `SIGWINCH`

Change attach API to:

```rust
pub fn attach<F>(
    socket: &Path,
    session: &str,
    initial_size: Option<PtySize>,
    on_resize: F,
) -> io::Result<()>
where
    F: FnMut(PtySize) -> io::Result<()>,
```

Inside attach:

- install the WINCH handler
- keep `last_size = initial_size`
- in the stdin loop, check `take_winch_pending()`
- when pending, call `detect_attach_size()` and `maybe_emit_resize`

In `main.rs`, the attach branch should:

- compute `initial_size`
- send initial `RESIZE` if available
- pass a closure to `client::attach` that sends future `RESIZE` requests

This task does not add an integration test for real terminal resizing because the current test harness is non-interactive; the signal flag and size coalescing are unit-tested, and existing integration tests cover resize protocol behavior.

- [ ] **Step 4: Run tests**

Run: `cargo test`

Expected: all tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/client.rs src/main.rs
git commit -m "feat: resize attached session on sigwinch"
```

---

### Task 3: Documentation And Verification

**Files:**
- Modify: `README.md`
- Add: `docs/superpowers/plans/2026-05-10-devmux-phase-2-live-resize.md`

- [ ] **Step 1: Document live resize**

Add a README bullet that attached clients react to `SIGWINCH` and request PTY resize when terminal size changes.

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
git add README.md docs/superpowers/plans/2026-05-10-devmux-phase-2-live-resize.md
git commit -m "docs: plan phase 2 live resize slice"
```

---

## Self-Review

- This plan covers client-side `SIGWINCH`-driven resize only.
- It does not implement multi-client resize policy, debounce configuration, layout recalculation, or automatic server-side client-size ownership.
- Existing resize integration tests remain the behavioral proof that `RESIZE` updates child PTY size.
