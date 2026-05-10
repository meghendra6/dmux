# Copy Mode Mouse Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add basic mouse-driven line selection inside attach-time copy-mode so users can click or drag rendered lines into a mux buffer.

**Architecture:** Keep the existing command-driven `copy-mode` and `save-buffer` server endpoints. The attach client enables SGR mouse reporting while local copy-mode is active, parses SGR mouse press/drag/release sequences, maps rendered rows to numbered copy-mode lines, and saves the selected line range through `SAVE_BUFFER_LINES`. This slice stays intentionally basic: no alternate-screen history viewport, no horizontal selection, and no clipboard integration.

**Tech Stack:** Rust standard library, existing client raw-mode attach loop, existing Unix socket protocol, unit tests in `src/client.rs`.

---

### Task 1: Mouse Selection State Machine

**Files:**
- Modify: `src/client.rs`

- [x] **Step 1: Write failing unit tests**

Add tests:

```rust
#[test]
fn copy_mode_mouse_click_copies_clicked_line() {
    let mut view = CopyModeView::from_numbered_output("3\talpha\n4\tbeta\n").unwrap();

    assert_eq!(
        view.apply_mouse_event(SgrMouseEvent {
            code: 0,
            col: 1,
            row: 3,
            release: false,
        }),
        CopyModeAction::Redraw
    );
    assert_eq!(
        view.apply_mouse_event(SgrMouseEvent {
            code: 0,
            col: 1,
            row: 3,
            release: true,
        }),
        CopyModeAction::CopyLineRange { start: 4, end: 4 }
    );
}

#[test]
fn copy_mode_mouse_drag_copies_inclusive_line_range() {
    let mut view = CopyModeView::from_numbered_output("10\tone\n11\ttwo\n12\tthree\n").unwrap();

    assert_eq!(
        view.apply_mouse_event(SgrMouseEvent {
            code: 0,
            col: 1,
            row: 2,
            release: false,
        }),
        CopyModeAction::Redraw
    );
    assert_eq!(
        view.apply_mouse_event(SgrMouseEvent {
            code: 32,
            col: 1,
            row: 4,
            release: false,
        }),
        CopyModeAction::Redraw
    );
    assert_eq!(
        view.apply_mouse_event(SgrMouseEvent {
            code: 0,
            col: 1,
            row: 4,
            release: true,
        }),
        CopyModeAction::CopyLineRange { start: 10, end: 12 }
    );
}
```

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test --bin dmux copy_mode_mouse
```

Expected: FAIL because `SgrMouseEvent`, `apply_mouse_event`, and `CopyLineRange` do not exist.

- [x] **Step 3: Implement state machine**

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SgrMouseEvent {
    code: u16,
    col: u16,
    row: u16,
    release: bool,
}
```

Extend `CopyModeAction` with:

```rust
CopyLineRange { start: usize, end: usize },
```

Add `selection_anchor: Option<usize>` to `CopyModeView`. Map rendered rows to line indexes with row 2 as the first copy-mode line because row 1 is the `-- copy mode --` header. On mouse press, set the anchor and cursor. On drag, update the cursor while preserving the anchor. On release, return an inclusive normalized line-number range.

- [x] **Step 4: Verify GREEN**

Run:

```bash
cargo test --bin dmux copy_mode_mouse
cargo test --bin dmux copy_mode_view
```

Expected: PASS.

### Task 2: SGR Mouse Input Parsing

**Files:**
- Modify: `src/client.rs`

- [x] **Step 1: Write failing parser tests**

Add tests:

```rust
#[test]
fn parses_sgr_mouse_press_and_release() {
    assert_eq!(
        parse_sgr_mouse_event(b"\x1b[<0;12;3M"),
        Some((
            SgrMouseEvent {
                code: 0,
                col: 12,
                row: 3,
                release: false,
            },
            10,
        ))
    );
    assert_eq!(
        parse_sgr_mouse_event(b"\x1b[<0;12;3m"),
        Some((
            SgrMouseEvent {
                code: 0,
                col: 12,
                row: 3,
                release: true,
            },
            10,
        ))
    );
}
```

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test --bin dmux parses_sgr_mouse
```

Expected: FAIL because parser does not exist.

- [x] **Step 3: Implement parser and input dispatch**

Implement `parse_sgr_mouse_event(input: &[u8]) -> Option<(SgrMouseEvent, usize)>` for SGR sequences shaped as `ESC [ < code ; col ; row M` or `ESC [ < code ; col ; row m`.

Refactor copy-mode byte handling so `handle_copy_mode_input(socket, session, view, input)` loops over a byte slice. When it sees a valid SGR mouse event, call `view.apply_mouse_event(event)`. Otherwise keep existing key behavior, including Escape exit.

- [x] **Step 4: Verify GREEN**

Run:

```bash
cargo test --bin dmux parses_sgr_mouse
cargo test --bin dmux attach_input
cargo test --bin dmux copy_mode_view
```

Expected: PASS.

### Task 3: Attach Mouse Reporting and Docs

**Files:**
- Modify: `src/client.rs`
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-copy-mode-mouse.md`

- [x] **Step 1: Enable mouse reporting in copy-mode**

Add a `MouseModeGuard` that writes these sequences when copy-mode starts:

```rust
const ENABLE_MOUSE_MODE: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h";
const DISABLE_MOUSE_MODE: &[u8] = b"\x1b[?1006l\x1b[?1002l\x1b[?1000l";
```

Create the guard inside `run_copy_mode` after the initial `COPY_MODE` request succeeds and before rendering the view. On drop, it writes the disable sequence.

- [x] **Step 2: Save mouse-selected ranges**

Update copy-mode action handling so both `CopyLine(n)` and `CopyLineRange { start, end }` call `SAVE_BUFFER_LINES` through `protocol::encode_save_buffer`.

- [x] **Step 3: Document mouse selection**

Update README copy-mode text to say mouse click saves one line, mouse drag saves an inclusive line range, and mouse selection is basic line-level selection.

- [x] **Step 4: Run full verification**

Run:

```bash
cargo fmt --check
git diff --check origin/main
keyword scan
cargo test
```

Expected: formatting and tests pass; keyword scan prints no matches; whitespace check passes.

- [x] **Step 5: Request subagent review**

Dispatch a read-only review subagent over `git diff origin/main`. Apply technically valid blocking or important findings and rerun full verification.

- [x] **Step 6: Commit and open draft PR**

Run:

```bash
git add README.md src/client.rs docs/superpowers/plans/2026-05-10-devmux-copy-mode-mouse.md
git commit -m "feat: add copy mode mouse selection"
git push -u origin devmux-copy-mode-mouse
gh pr create --draft --base main --head devmux-copy-mode-mouse --title "Add copy mode mouse selection" --body "<summary and validation>"
```
