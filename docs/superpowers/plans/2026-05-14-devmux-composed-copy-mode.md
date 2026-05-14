# Devmux Composed Layout Copy Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make unzoomed multi-pane attach `C-b [` copy from the rendered composed layout instead of the active pane.

**Architecture:** Keep existing active-pane copy-mode for raw single-pane and zoomed attach. Add `SAVE_BUFFER_TEXT` as a narrow server request for saving client-selected text, then add a composed copy-mode source that builds `CopyModeView` from `ATTACH_LAYOUT_SNAPSHOT` bytes and saves selected visual rows with the new request.

**Tech Stack:** Rust standard library, existing Unix socket control protocol, existing client copy-mode UI, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Protocol And Server Text Buffer Save

**Files:**
- Modify: `src/protocol.rs`
- Modify: `src/server.rs`
- Test: `src/protocol.rs`
- Test: `tests/phase1_cli.rs`

- [ ] **Step 1: Write failing protocol test**

Add a test near existing save-buffer request tests:

```rust
#[test]
fn round_trips_save_buffer_text_request() {
    let line = encode_save_buffer_text("dev", Some("layout"), "left\t|\tright\n");
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::SaveBufferText {
            session: "dev".to_string(),
            buffer: Some("layout".to_string()),
            text: "left\t|\tright\n".to_string(),
        }
    );
}
```

- [ ] **Step 2: Verify protocol RED**

Run:

```bash
cargo test round_trips_save_buffer_text_request
```

Expected: FAIL because `encode_save_buffer_text` and `Request::SaveBufferText`
do not exist.

- [ ] **Step 3: Add protocol support**

Add `Request::SaveBufferText { session, buffer, text }`, implement:

```rust
pub fn encode_save_buffer_text(session: &str, buffer: Option<&str>, text: &str) -> String {
    format!(
        "SAVE_BUFFER_TEXT\t{session}\t{}\t{}\n",
        encode_optional_text(buffer),
        encode_hex(text.as_bytes())
    )
}
```

Decode:

```rust
["SAVE_BUFFER_TEXT", session, buffer, text] => Ok(Request::SaveBufferText {
    session: (*session).to_string(),
    buffer: decode_optional_text(buffer, "SAVE_BUFFER_TEXT")?,
    text: decode_utf8_hex(text, "SAVE_BUFFER_TEXT")?,
}),
```

- [ ] **Step 4: Verify protocol GREEN**

Run:

```bash
cargo test round_trips_save_buffer_text_request
```

Expected: PASS.

- [ ] **Step 5: Write failing server integration tests**

Add tests in `tests/phase1_cli.rs` using the existing raw socket helpers:

```rust
#[test]
fn raw_save_buffer_text_stores_arbitrary_text() {
    let socket = unique_socket("save-buffer-text");
    let session = format!("save-buffer-text-{}", std::process::id());
    assert_success(&dmux(&socket, &["new", "-d", "-s", &session, "--", "sleep", "30"]));

    let response = raw_control_request(
        &socket,
        &dmux::protocol::encode_save_buffer_text(
            &session,
            Some("layout"),
            "base-copy | split-copy\n",
        ),
    );
    assert!(response.starts_with("OK\nlayout\n"), "{response:?}");

    let listed = dmux(&socket, &["list-buffers"]);
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(listed.lines().any(|line| line.ends_with("\t23\tbase-copy | split-copy")), "{listed:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn raw_save_buffer_text_reports_missing_session() {
    let socket = unique_socket("save-buffer-text-missing");
    let _server = start_server(&socket);

    let response = raw_control_request(
        &socket,
        &dmux::protocol::encode_save_buffer_text("missing", None, "layout\n"),
    );

    assert_eq!(response, "ERR missing session\n");
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

If helper names differ, reuse the existing raw socket helpers in
`tests/phase1_cli.rs` and keep the assertions equivalent.

- [ ] **Step 6: Verify server RED**

Run:

```bash
cargo test --test phase1_cli raw_save_buffer_text_stores_arbitrary_text
cargo test --test phase1_cli raw_save_buffer_text_reports_missing_session
```

Expected: FAIL because the server has no `SAVE_BUFFER_TEXT` handler.

- [ ] **Step 7: Implement server handler**

In `src/server.rs`, match `Request::SaveBufferText` in `handle_client` and add a
handler that:

1. Looks up the session and returns `ERR missing session` if absent.
2. Saves `text` through `state.buffers.lock().unwrap().save(buffer, text)`.
3. Writes `OK\n<saved-name>\n`.

- [ ] **Step 8: Verify server GREEN**

Run:

```bash
cargo test round_trips_save_buffer_text_request
cargo test --test phase1_cli raw_save_buffer_text_stores_arbitrary_text
cargo test --test phase1_cli raw_save_buffer_text_reports_missing_session
cargo fmt --check
```

Expected: PASS. If sandboxed integration tests fail with server readiness or
socket errors, rerun the same focused command with escalation.

- [ ] **Step 9: Commit**

Run:

```bash
git add src/protocol.rs src/server.rs tests/phase1_cli.rs
git commit -m "feat: save composed copy text"
```

### Task 2: Client Composed Copy-Mode Source

**Files:**
- Modify: `src/client.rs`
- Test: `src/client.rs`

- [ ] **Step 1: Write failing view tests**

Add tests near existing `copy_mode_view` tests:

```rust
#[test]
fn copy_mode_view_numbers_plain_rendered_lines() {
    let view = CopyModeView::from_plain_text("base | split\r\n-----+------\r\n").unwrap();

    assert_eq!(view.cursor_line_number(), Some(1));
    assert_eq!(view.selected_text_for_line_range(1, 2).unwrap(), "base | split\n-----+------\n");
}

#[test]
fn copy_mode_view_preserves_blank_plain_lines() {
    let view = CopyModeView::from_plain_text("top\r\n\r\nbottom\r\n").unwrap();

    assert_eq!(view.selected_text_for_line_range(2, 2).unwrap(), "\n");
}
```

- [ ] **Step 2: Verify view RED**

Run:

```bash
cargo test copy_mode_view_numbers_plain_rendered_lines
cargo test copy_mode_view_preserves_blank_plain_lines
```

Expected: FAIL because `from_plain_text` and `selected_text_for_line_range` do
not exist.

- [ ] **Step 3: Implement plain text view helpers**

Add `CopyModeView::from_plain_text(text: &str)` that treats `\r\n` and `\n`
line endings as rendered lines, strips the trailing line terminator from each
stored line, and assigns 1-based visual line numbers.

Add `selected_text_for_line_range(start, end)` that finds the inclusive line
number range and returns selected line text joined with `\n`, with a trailing
`\n`.

- [ ] **Step 4: Verify view GREEN**

Run:

```bash
cargo test copy_mode_view_numbers_plain_rendered_lines
cargo test copy_mode_view_preserves_blank_plain_lines
```

Expected: PASS.

- [ ] **Step 5: Add source-aware save path**

Refactor only the action/save boundary:

```rust
enum CopyModeSaveSource {
    ActivePane,
    ComposedText,
}
```

Active-pane copy actions keep using `SAVE_BUFFER_LINES`. Composed copy actions
call `selected_text_for_line_range` and send `SAVE_BUFFER_TEXT`.

If `SAVE_BUFFER_TEXT` returns an unknown-request error, convert it into a clear
`composed copy-mode requires an updated dmux server` error. Do not fall back to
`SAVE_BUFFER_LINES`, because that silently copies the active pane.

- [ ] **Step 6: Add composed copy-mode runner**

Keep `run_copy_mode` using `ActivePane`. Add
`run_composed_copy_mode_with_reader` that:

1. Calls `read_attach_layout_snapshot`.
2. Builds `CopyModeView::from_plain_text` from `snapshot`.
3. Runs the existing input loop.
4. Saves selected lines with `SAVE_BUFFER_TEXT`.

Do not change raw single-pane attach behavior.

- [ ] **Step 7: Verify client GREEN**

Run:

```bash
cargo test copy_mode_view_numbers_plain_rendered_lines
cargo test copy_mode_view_preserves_blank_plain_lines
cargo test copy_mode_mouse
cargo test attach_input_dispatches_copy_mode_prefix_without_forwarding_bytes
cargo fmt --check
```

Expected: PASS.

- [ ] **Step 8: Commit**

Run:

```bash
git add src/client.rs
git commit -m "feat: compose attach copy mode"
```

### Task 3: Multi-Pane Attach Integration And Docs

**Files:**
- Modify: `tests/phase1_cli.rs`
- Modify: `README.md`
- Modify: `HANDOFF.md` locally only

- [ ] **Step 1: Write failing integration expectation**

Rename or update the existing multi-pane copy-mode integration test to assert
composed layout behavior:

```rust
#[test]
fn attach_prefix_bracket_copies_composed_layout_line_in_multi_pane_attach() {
    // Reuse the existing split setup with base-copy and split-copy.
    // Send C-b [ y.
    // Poll list-buffers until a buffer preview contains both pane texts and
    // the rendered separator, for example "base-copy | split-copy".
}
```

- [ ] **Step 2: Verify integration RED**

Run:

```bash
cargo test --test phase1_cli attach_prefix_bracket_copies_composed_layout_line_in_multi_pane_attach
```

Expected: FAIL before the live snapshot attach branch is wired to composed
copy-mode because the saved buffer contains only active-pane text.

- [ ] **Step 3: Wire live snapshot attach to composed runner**

In `spawn_live_snapshot_input_thread`, change the `EnterCopyMode` branch to call
`run_composed_copy_mode_with_reader` instead of the active-pane runner.

- [ ] **Step 4: Verify integration GREEN**

Run:

```bash
cargo test --test phase1_cli attach_prefix_bracket_copies_composed_layout_line_in_multi_pane_attach
```

Expected: PASS. If sandboxed execution fails with readiness or Unix socket
errors, rerun with escalation.

- [ ] **Step 5: Update README**

Update the attach copy-mode paragraph and Phase 2 list to say unzoomed
multi-pane attach copy-mode operates over the rendered composed layout. Remove
the current limit that says composed-layout copy-mode is not implemented.

- [ ] **Step 6: Verification before PR**

Run:

```bash
cargo fmt --check
git diff --check origin/main
cargo test round_trips_save_buffer_text_request
cargo test copy_mode_view
cargo test --test phase1_cli raw_save_buffer_text_stores_arbitrary_text
cargo test --test phase1_cli raw_save_buffer_text_reports_missing_session
cargo test --test phase1_cli attach_prefix_bracket_copies_composed_layout_line_in_multi_pane_attach
cargo test
```

Expected: PASS. In this environment, full `cargo test` and server-backed focused
integration tests may need escalated execution because sandboxed Unix socket or
PTY readiness can fail before behavior is exercised.

- [ ] **Step 7: Commit**

Run:

```bash
git add tests/phase1_cli.rs README.md
git commit -m "test: cover composed copy mode"
```
