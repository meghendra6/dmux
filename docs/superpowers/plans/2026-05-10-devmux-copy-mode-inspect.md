# Copy Mode Inspect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `copy-mode` command that prints line-numbered pane capture output, with optional search filtering, so users can inspect history and choose line ranges for buffer saves.

**Architecture:** Keep attach-time interactive copy UI out of this slice. Implement `copy-mode` as a command-driven inspection endpoint over the existing server capture state; it formats captured lines as `<line-number><TAB><text>`, and `--search` filters to matching lines while preserving original line numbers. This complements the existing `save-buffer --start-line/--end-line` selection.

**Tech Stack:** Rust standard library, existing CLI parser, existing Unix socket protocol, existing terminal capture and buffer code, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: CLI And Protocol Copy Mode Command

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`

- [x] **Step 1: Write failing CLI tests**

Add:

```rust
#[test]
fn parses_copy_mode_history_search() {
    let command = parse_args([
        "dmux",
        "copy-mode",
        "-t",
        "dev",
        "--history",
        "--search",
        "needle",
    ])
    .unwrap();
    assert_eq!(
        command,
        Command::CopyMode {
            session: "dev".to_string(),
            mode: CaptureMode::History,
            search: Some("needle".to_string()),
        }
    );
}
```

- [x] **Step 2: Write failing protocol tests**

Add:

```rust
#[test]
fn round_trips_copy_mode_request() {
    let line = encode_copy_mode("dev", CaptureMode::History, Some("needle"));
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::CopyMode {
            session: "dev".to_string(),
            mode: CaptureMode::History,
            search: Some("needle".to_string()),
        }
    );
}
```

- [x] **Step 3: Verify RED**

Run:

```bash
cargo test --bin dmux copy_mode
```

Expected: FAIL because the command/protocol variants do not exist.

- [x] **Step 4: Implement CLI and protocol**

Add `Command::CopyMode { session, mode, search }` and `Request::CopyMode { session, mode, search }`. Parse:

```text
copy-mode -t <session> [--screen|--history|--all] [--search <text>]
```

Default mode is `All`. Search text must be non-empty. Encode as:

```text
COPY_MODE <session> <screen|history|all> <search-hex-or-empty>
```

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test --bin dmux copy_mode
```

Expected: PASS.

### Task 2: Server Inspection Output

**Files:**
- Modify: `src/server.rs`
- Modify: `src/main.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Write failing formatter tests**

Add:

```rust
#[test]
fn format_copy_mode_lines_numbers_all_lines() {
    let text = format_copy_mode_lines("first\nsecond\n", None);
    assert_eq!(text, "1\tfirst\n2\tsecond\n");
}

#[test]
fn format_copy_mode_lines_filters_search_matches() {
    let text = format_copy_mode_lines("first\nneedle-one\nlast\nneedle-two\n", Some("needle"));
    assert_eq!(text, "2\tneedle-one\n4\tneedle-two\n");
}
```

- [x] **Step 2: Write failing integration test**

Add:

```rust
#[test]
fn copy_mode_prints_numbered_lines_and_search_matches() {
    let socket = unique_socket("copy-mode");
    let session = format!("copy-mode-{}", std::process::id());

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
            "printf first; printf '\\n'; printf needle-one; printf '\\n'; printf last; printf '\\n'; printf needle-two; printf '\\n'; sleep 30",
        ],
    ));
    let captured = poll_capture(&socket, &session, "needle-two");
    assert!(captured.contains("needle-one"), "{captured:?}");

    let output = dmux(&socket, &["copy-mode", "-t", &session, "--screen"]);
    assert_success(&output);
    let output = String::from_utf8_lossy(&output.stdout);
    assert!(output.contains("1\tfirst\n"), "{output:?}");
    assert!(output.contains("4\tneedle-two\n"), "{output:?}");

    let output = dmux(&socket, &["copy-mode", "-t", &session, "--screen", "--search", "needle"]);
    assert_success(&output);
    let output = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output, "2\tneedle-one\n4\tneedle-two\n");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 3: Verify RED**

Run:

```bash
cargo test --bin dmux format_copy_mode_lines
cargo test --test phase1_cli copy_mode_prints
```

Expected: FAIL because server handling is not implemented.

- [x] **Step 4: Implement server handling**

Add `handle_copy_mode` that gets the active pane, captures text with the requested mode, formats it with `format_copy_mode_lines`, and writes the result. `format_copy_mode_lines` should enumerate original 1-based line numbers and filter lines only when `search` is present.

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test --bin dmux format_copy_mode_lines
cargo test --test phase1_cli copy_mode_prints
```

Expected: PASS.

### Task 3: Docs, Verification, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-copy-mode-inspect.md`

- [x] **Step 1: Document copy-mode inspect**

Add command:

```text
dmux copy-mode -t <name> [--screen|--history|--all] [--search <text>]
```

Mention that current copy-mode is command-driven line inspection; attach-time vi/emacs movement and mouse selection remain pending.

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
git add README.md src/cli.rs src/main.rs src/protocol.rs src/server.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-copy-mode-inspect.md
git commit -m "feat: add copy mode inspection"
git push -u origin devmux-copy-mode-inspect
gh pr create --draft --base main --head devmux-copy-mode-inspect --title "Add copy mode inspection" --body "<summary and validation>"
```
