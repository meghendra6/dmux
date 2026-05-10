# Copy Mode Keys Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the first attach-time interactive copy-mode input handling so users can enter copy mode, move through inspected lines with vi/emacs keys, and copy the current line into a mux buffer.

**Architecture:** Reuse the command-driven `copy-mode` and `save-buffer` server endpoints. The client intercepts prefix `C-b [` during attach, fetches numbered copy-mode lines through the control socket, renders a simple copy-mode view, and consumes copy-mode keys locally until exit. This slice deliberately avoids full alternate-screen UI and mouse reporting; it creates the state machine and attach hook needed for richer rendering later.

**Tech Stack:** Rust standard library, existing client raw-mode attach loop, existing Unix socket protocol, unit tests in `src/client.rs`.

---

### Task 1: Copy Mode View State Machine

**Files:**
- Modify: `src/client.rs`

- [x] **Step 1: Write failing unit tests**

Add tests:

```rust
#[test]
fn copy_mode_view_moves_with_vi_and_emacs_keys() {
    let mut view = CopyModeView::from_numbered_output("1\tfirst\n2\tsecond\n").unwrap();

    assert_eq!(view.cursor_line_number(), Some(1));
    assert_eq!(view.apply_key(b'j'), CopyModeAction::Redraw);
    assert_eq!(view.cursor_line_number(), Some(2));
    assert_eq!(view.apply_key(0x10), CopyModeAction::Redraw);
    assert_eq!(view.cursor_line_number(), Some(1));
}

#[test]
fn copy_mode_view_copies_current_line() {
    let mut view = CopyModeView::from_numbered_output("7\tselected\n").unwrap();

    assert_eq!(view.apply_key(b'y'), CopyModeAction::CopyLine(7));
}

#[test]
fn copy_mode_view_exits_on_q_or_escape() {
    let mut view = CopyModeView::from_numbered_output("1\tfirst\n").unwrap();

    assert_eq!(view.apply_key(b'q'), CopyModeAction::Exit);
    assert_eq!(view.apply_key(0x1b), CopyModeAction::Exit);
}
```

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test --bin dmux copy_mode_view
```

Expected: FAIL because copy-mode client state types do not exist.

- [x] **Step 3: Implement state machine**

Add private types in `src/client.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum CopyModeAction {
    Redraw,
    CopyLine(usize),
    Exit,
    Ignore,
}

struct CopyModeLine {
    number: usize,
    text: String,
}

struct CopyModeView {
    lines: Vec<CopyModeLine>,
    cursor: usize,
}
```

Parse numbered lines from `copy-mode` output, move down with `j`/`Ctrl-n`, move up with `k`/`Ctrl-p`, copy current line with `y` or Enter, exit with `q` or Escape, and render with a `>` cursor marker.

- [x] **Step 4: Verify GREEN**

Run:

```bash
cargo test --bin dmux copy_mode_view
```

Expected: PASS.

### Task 2: Attach Prefix Hook

**Files:**
- Modify: `src/client.rs`
- Modify: `src/main.rs`
- Modify: `README.md`

- [x] **Step 1: Write failing unit tests**

Add a test around a pure stdin translation helper:

```rust
#[test]
fn attach_input_dispatches_copy_mode_prefix_without_forwarding_bytes() {
    let mut actions = Vec::new();
    let forwarded = translate_attach_input(b"\x02[", &mut false, || {
        actions.push("copy-mode");
        Ok(())
    })
    .unwrap();

    assert_eq!(forwarded, Vec::<u8>::new());
    assert_eq!(actions, vec!["copy-mode"]);
}
```

Implementation note: the helper returns `AttachInputAction::Forward(bytes)`,
`AttachInputAction::EnterCopyMode { forward, initial_input }`, or
`AttachInputAction::Detach` so `C-b d` remains explicit while `C-b [` forwards
no bytes and coalesced copy-mode keys stay out of the child PTY.

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test --bin dmux attach_input_dispatches_copy_mode_prefix
```

Expected: FAIL because the helper does not exist.

- [x] **Step 3: Implement attach hook**

Refactor `forward_stdin_until_detach` to use `translate_attach_input`. Prefix `C-b d` still detaches; prefix `C-b [` enters copy-mode and forwards no bytes to the child PTY.

Implement `run_copy_mode(socket, session)`:

1. Send `COPY_MODE` request to get numbered lines.
2. Render `CopyModeView` to stdout.
3. Read stdin bytes until `CopyModeAction::Exit` or `CopyLine`.
4. On `CopyLine(n)`, send `SAVE_BUFFER` with `BufferSelection::LineRange { start: n, end: n }`.

- [x] **Step 4: Verify GREEN**

Run:

```bash
cargo test --bin dmux attach_input_dispatches_copy_mode_prefix
cargo test --bin dmux copy_mode_view
```

Expected: PASS.

### Task 3: Verification, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-copy-mode-keys.md`

- [x] **Step 1: Document attach key**

Document that attached clients can enter the current basic copy-mode with `C-b [` and use `j/k`, `Ctrl-n/Ctrl-p`, `y`, and `q`/Escape. Mention mouse selection still remains pending.

- [x] **Step 2: Run full verification**

Run:

```bash
cargo fmt --check
cargo test
keyword scan
git diff --check
```

Expected: formatting and tests pass; keyword scan prints no matches; whitespace check passes.

- [x] **Step 3: Request subagent review**

Dispatch a read-only review subagent over `git diff origin/main`. Apply technically valid blocking or important findings and rerun full verification.

- [ ] **Step 4: Commit and open draft PR**

Run:

```bash
git add README.md src/client.rs src/main.rs docs/superpowers/plans/2026-05-10-devmux-copy-mode-keys.md
git commit -m "feat: add copy mode keys"
git push -u origin devmux-copy-mode-keys
gh pr create --draft --base main --head devmux-copy-mode-keys --title "Add copy mode keys" --body "<summary and validation>"
```
