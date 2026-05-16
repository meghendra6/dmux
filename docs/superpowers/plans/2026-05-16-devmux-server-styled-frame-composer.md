# Devmux Server Styled Frame Composer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a server-side styled frame composer that renders layout regions from pane terminal grids without switching the live attach protocol yet.

**Architecture:** Keep `ATTACH_LAYOUT_SNAPSHOT` untouched for compatibility. Add a separate frame-composition path that accepts pane `TerminalState` values, asks each terminal for ANSI-rendered rows clipped to its allocated region, then joins those rows with the existing minimal separators and region metadata.

**Tech Stack:** Rust stdlib only, existing `cargo test` unit tests in `src/server.rs` and `src/term.rs`.

---

### Task 1: Terminal ANSI Row Projection

**Files:**
- Modify: `src/term.rs`

- [ ] **Step 1: Write failing unit test**

Add this test in `src/term.rs`:

```rust
#[test]
fn styled_render_lines_clip_and_pad_visible_cells() {
    let mut state = TerminalState::new(10, 2, 100);
    state.apply_bytes(b"\x1b[31mabcdef\x1b[0m");

    assert_eq!(
        state.render_screen_ansi_lines(4, 2),
        vec!["\x1b[31mabcd\x1b[0m".to_string(), "    ".to_string()]
    );
}
```

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test term::tests::styled_render_lines_clip_and_pad_visible_cells -- --test-threads=1
```

Expected: fail because `render_screen_ansi_lines` does not exist.

- [ ] **Step 3: Implement minimal projection**

Add `TerminalState::render_screen_ansi_lines(width, height) -> Vec<String>`. It should render exactly `height` rows and exactly `width` visible cells per row. SGR escapes do not count toward visible width, and each row resets style before ending if it emitted styled cells.

- [ ] **Step 4: Verify GREEN**

Run:

```bash
cargo test term::tests::styled_render_lines_clip_and_pad_visible_cells -- --test-threads=1
```

Expected: pass.

### Task 2: Server Styled Frame Composer

**Files:**
- Modify: `src/server.rs`

- [ ] **Step 1: Write failing unit tests**

Add tests in `src/server.rs`:

```rust
#[test]
fn render_attach_frame_for_size_preserves_styled_pane_output() {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        first: Box::new(LayoutNode::Pane(0)),
        second: Box::new(LayoutNode::Pane(1)),
    };
    let mut left = TerminalState::new(20, 1, 100);
    left.apply_bytes(b"\x1b[31mleft\x1b[0m");
    let mut right = TerminalState::new(20, 1, 100);
    right.apply_bytes(b"\x1b[1;38;2;1;2;3mright\x1b[0m");
    let panes = vec![
        PaneRenderSnapshot { index: 0, terminal: &left },
        PaneRenderSnapshot { index: 1, terminal: &right },
    ];

    let rendered =
        render_attach_frame_for_size(&layout, &panes, PtySize { cols: 20, rows: 1 }).unwrap();

    assert_eq!(
        rendered.text,
        "\x1b[31mleft\x1b[0m    | \x1b[1;38;2;1;2;3mright\x1b[0m    \r\n"
    );
}

#[test]
fn render_attach_frame_for_size_clips_styled_content_to_region_width() {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        first: Box::new(LayoutNode::Pane(0)),
        second: Box::new(LayoutNode::Pane(1)),
    };
    let mut left = TerminalState::new(20, 1, 100);
    left.apply_bytes(b"\x1b[31mabcdef\x1b[0m");
    let mut right = TerminalState::new(20, 1, 100);
    right.apply_bytes(b"right");
    let panes = vec![
        PaneRenderSnapshot { index: 0, terminal: &left },
        PaneRenderSnapshot { index: 1, terminal: &right },
    ];

    let rendered =
        render_attach_frame_for_size(&layout, &panes, PtySize { cols: 10, rows: 1 }).unwrap();

    assert_eq!(rendered.text, "\x1b[31mabc\x1b[0m | right\r\n");
}
```

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test server::tests::render_attach_frame_for_size -- --test-threads=1
```

Expected: fail because `PaneRenderSnapshot` and `render_attach_frame_for_size` do not exist.

- [ ] **Step 3: Implement minimal composer**

Add a private `PaneRenderSnapshot<'a> { index, terminal: &'a TerminalState }`, `RenderedAttachFrame`, and `render_attach_frame_for_size`. Reuse existing `LayoutNode`, `PaneRegion`, `split_extent`, separator helpers, and `render_client_lines`. Do not modify the snapshot protocol.

- [ ] **Step 4: Verify GREEN and regression suite**

Run:

```bash
cargo test server::tests::render_attach_frame_for_size -- --test-threads=1
cargo test term::tests -- --test-threads=1
cargo test --test phase1_cli live_snapshot -- --test-threads=1 --nocapture
cargo fmt --check
```

Expected: all pass.
