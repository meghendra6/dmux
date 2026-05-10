# Copy Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add line-range and search-based text selection for `save-buffer` so users can copy selected pane output into mux buffers before interactive copy-mode UI lands.

**Architecture:** Preserve the existing `SAVE_BUFFER` protocol as whole-capture save. Add explicit selection metadata for line ranges and search, then apply that selection to the captured active pane text before storing it in the server buffer store. This is a command-driven copy selection foundation; vi/emacs key movement and mouse-driven interactive selection remain attach-UI work.

**Tech Stack:** Rust standard library, existing CLI parser, existing Unix socket protocol, existing terminal capture and buffer store, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: CLI And Protocol Selection Options

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`

- [x] **Step 1: Write failing CLI tests**

Add tests:

```rust
#[test]
fn parses_save_buffer_line_range_selection() {
    let command = parse_args([
        "dmux",
        "save-buffer",
        "-t",
        "dev",
        "-b",
        "picked",
        "--screen",
        "--start-line",
        "2",
        "--end-line",
        "3",
    ])
    .unwrap();
    assert_eq!(
        command,
        Command::SaveBuffer {
            session: "dev".to_string(),
            buffer: Some("picked".to_string()),
            mode: CaptureMode::Screen,
            selection: BufferSelection::LineRange { start: 2, end: 3 },
        }
    );
}

#[test]
fn parses_save_buffer_search_selection() {
    let command = parse_args([
        "dmux",
        "save-buffer",
        "-t",
        "dev",
        "-b",
        "match",
        "--search",
        "needle",
    ])
    .unwrap();
    assert_eq!(
        command,
        Command::SaveBuffer {
            session: "dev".to_string(),
            buffer: Some("match".to_string()),
            mode: CaptureMode::All,
            selection: BufferSelection::Search("needle".to_string()),
        }
    );
}
```

- [x] **Step 2: Write failing protocol tests**

Add tests:

```rust
#[test]
fn round_trips_save_buffer_line_range_request() {
    let line = encode_save_buffer(
        "dev",
        Some("picked"),
        CaptureMode::Screen,
        BufferSelection::LineRange { start: 2, end: 3 },
    );
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::SaveBuffer {
            session: "dev".to_string(),
            buffer: Some("picked".to_string()),
            mode: CaptureMode::Screen,
            selection: BufferSelection::LineRange { start: 2, end: 3 },
        }
    );
}

#[test]
fn round_trips_save_buffer_search_request() {
    let line = encode_save_buffer(
        "dev",
        Some("match"),
        CaptureMode::All,
        BufferSelection::Search("needle".to_string()),
    );
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::SaveBuffer {
            session: "dev".to_string(),
            buffer: Some("match".to_string()),
            mode: CaptureMode::All,
            selection: BufferSelection::Search("needle".to_string()),
        }
    );
}
```

- [x] **Step 3: Verify RED**

Run:

```bash
cargo test --bin dmux save_buffer_line_range
cargo test --bin dmux save_buffer_search
```

Expected: FAIL because selection types and parser/protocol support do not exist.

- [x] **Step 4: Implement CLI and protocol selection**

Add `BufferSelection`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferSelection {
    All,
    LineRange { start: usize, end: usize },
    Search(String),
}
```

Update `Command::SaveBuffer` and `Request::SaveBuffer` with `selection: BufferSelection`. Parse:

```text
--start-line <n> --end-line <n>
--search <text>
```

Line range selection requires both endpoints, 1-based positive line numbers, and `start <= end`. `--search` requires a non-empty query. Range and search are mutually exclusive. Keep old `SAVE_BUFFER` decode as `BufferSelection::All`; add selected forms as:

```text
SAVE_BUFFER_LINES   <session> <mode> <buffer> <start> <end>
SAVE_BUFFER_SEARCH  <session> <mode> <buffer> <needle-hex>
```

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test --bin dmux save_buffer_line_range
cargo test --bin dmux save_buffer_search
```

Expected: PASS.

### Task 2: Server Selection Behavior

**Files:**
- Modify: `src/server.rs`
- Modify: `src/main.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Write failing selection unit tests**

Add server unit tests:

```rust
#[test]
fn selected_buffer_text_returns_line_range() {
    let text = select_buffer_text(
        "first\nkeep-one\nkeep-two\nlast\n",
        &BufferSelection::LineRange { start: 2, end: 3 },
    )
    .unwrap();
    assert_eq!(text, "keep-one\nkeep-two\n");
}

#[test]
fn selected_buffer_text_returns_first_search_match() {
    let text = select_buffer_text(
        "first\nneedle-one\nneedle-two\n",
        &BufferSelection::Search("needle".to_string()),
    )
    .unwrap();
    assert_eq!(text, "needle-one\n");
}
```

- [x] **Step 2: Write failing integration test**

Add:

```rust
#[test]
fn save_buffer_can_copy_line_range_and_search_match() {
    let socket = unique_socket("copy-selection");
    let source = format!("copy-source-{}", std::process::id());
    let sink = format!("copy-sink-{}", std::process::id());
    let file = unique_temp_file("copy-selection");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &source,
            "--",
            "sh",
            "-c",
            "printf first; printf '\\n'; printf keep-one; printf '\\n'; printf keep-two; printf '\\n'; printf last; printf '\\n'; sleep 30",
        ],
    ));
    let captured = poll_capture(&socket, &source, "last");
    assert!(captured.contains("keep-one"), "{captured:?}");

    assert_success(&dmux(
        &socket,
        &[
            "save-buffer",
            "-t",
            &source,
            "-b",
            "picked",
            "--screen",
            "--start-line",
            "2",
            "--end-line",
            "3",
        ],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "save-buffer",
            "-t",
            &source,
            "-b",
            "match",
            "--screen",
            "--search",
            "last",
        ],
    ));

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &sink,
            "--",
            "sh",
            "-c",
            &format!("cat > {}; sleep 30", file.display()),
        ],
    ));

    assert_success(&dmux(&socket, &["paste-buffer", "-t", &sink, "-b", "picked"]));
    assert_success(&dmux(&socket, &["paste-buffer", "-t", &sink, "-b", "match"]));
    assert!(poll_file_contains(&file, "keep-one\nkeep-two\nlast\n"));

    assert_success(&dmux(&socket, &["kill-session", "-t", &source]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &sink]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 3: Verify RED**

Run:

```bash
cargo test --bin dmux selected_buffer_text
cargo test --test phase1_cli save_buffer_can_copy
```

Expected: FAIL because server selection is not implemented.

- [x] **Step 4: Implement selection before buffer save**

In `handle_save_buffer`, capture pane text, run `select_buffer_text(&captured, &selection)`, then save the selected text. `BufferSelection::All` returns the captured text unchanged. Line range selection is 1-based inclusive and preserves a trailing newline for non-empty selections. Search returns the first line containing the query plus trailing newline, or reports `missing match`.

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test --bin dmux selected_buffer_text
cargo test --test phase1_cli save_buffer_can_copy
```

Expected: PASS.

### Task 3: Docs, Verification, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-copy-selection.md`

- [x] **Step 1: Document selection options**

Update `save-buffer` syntax:

```text
dmux save-buffer -t <name> [-b <buffer>] [--screen|--history|--all] [--start-line <n> --end-line <n>|--search <text>]
```

Mention that selection is currently command-driven by line range or first search match; interactive vi/emacs and mouse selection are still pending attach-UI work.

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

- [x] **Step 4: Commit and open draft PR**

Run:

```bash
git add README.md src/cli.rs src/main.rs src/protocol.rs src/server.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-copy-selection.md
git commit -m "feat: add copy selection"
git push -u origin devmux-copy-selection
gh pr create --draft --base main --head devmux-copy-selection --title "Add copy selection" --body "<summary and validation>"
```
