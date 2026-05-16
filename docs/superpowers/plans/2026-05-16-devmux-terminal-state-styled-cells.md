# Devmux Terminal State Styled Cells Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert `TerminalState` from plain characters to styled cells while preserving existing plain capture behavior.

**Architecture:** Keep the existing parser and screen model, but store `Cell { ch, style }` instead of `char`. Add a minimal ANSI render projection for future frame composition, and add alternate-screen support so pane-local TUI state is not flattened into primary screen text.

**Tech Stack:** Rust stdlib only, existing `cargo test` and `phase1_cli` integration tests.

---

### Task 1: Styled SGR Cells

**Files:**
- Modify: `src/term.rs`

- [ ] **Step 1: Write failing unit tests**

Add tests in `src/term.rs`:

```rust
#[test]
fn styled_render_preserves_sgr_foreground_and_reset() {
    let mut state = TerminalState::new(20, 3, 100);
    state.apply_bytes(b"\x1b[31mred\x1b[0m plain");

    assert_eq!(state.capture_screen_text(), "red plain\n");
    assert_eq!(
        state.render_screen_ansi_text(),
        "\x1b[31mred\x1b[0m plain\n"
    );
}

#[test]
fn styled_render_preserves_truecolor_and_bold() {
    let mut state = TerminalState::new(20, 3, 100);
    state.apply_bytes(b"\x1b[1;38;2;1;2;3mhi\x1b[0m");

    assert_eq!(
        state.render_screen_ansi_text(),
        "\x1b[1;38;2;1;2;3mhi\x1b[0m\n"
    );
}
```

- [ ] **Step 2: Run tests to verify RED**

Run:

```bash
cargo test term::tests::styled_render -- --test-threads=1
```

Expected: tests fail because `render_screen_ansi_text` does not exist or does not preserve style.

- [ ] **Step 3: Implement styled cells**

Replace plain row characters with cells carrying `CellStyle`. Parse common SGR codes into the current style. Add `render_screen_ansi_text()`.

- [ ] **Step 4: Run tests to verify GREEN**

Run:

```bash
cargo test term::tests::styled_render -- --test-threads=1
```

Expected: both styled render tests pass.

### Task 2: Alternate Screen In TerminalState

**Files:**
- Modify: `src/term.rs`

- [ ] **Step 1: Write failing unit test**

Add test in `src/term.rs`:

```rust
#[test]
fn alternate_screen_preserves_primary_screen() {
    let mut state = TerminalState::new(20, 3, 100);
    state.apply_bytes(b"primary");
    state.apply_bytes(b"\x1b[?1049halternate");

    assert_eq!(state.capture_screen_text(), "alternate\n");

    state.apply_bytes(b"\x1b[?1049l");
    assert_eq!(state.capture_screen_text(), "primary\n");
}
```

- [ ] **Step 2: Run test to verify RED**

Run:

```bash
cargo test term::tests::alternate_screen_preserves_primary_screen -- --test-threads=1
```

Expected: fails because alternate screen is not implemented.

- [ ] **Step 3: Implement alternate screen**

Track primary and alternate `TerminalScreen` values. `?1049h` activates a fresh alternate screen; `?1049l` restores the primary screen. Alternate screen output does not enter primary scrollback.

- [ ] **Step 4: Run focused and full checks**

Run:

```bash
cargo test term::tests::alternate_screen_preserves_primary_screen -- --test-threads=1
cargo test term::tests -- --test-threads=1
cargo test --test phase1_cli interactive_new_default_shell_splits_and_remains_usable_in_real_pty -- --test-threads=1 --nocapture
cargo fmt --check
```

Expected: all pass.
