# Buffers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the first Phase 4 mux buffer commands so captured pane text can be stored, listed, pasted into another pane, and deleted.

**Architecture:** Store buffers in the server process as a bounded in-memory buffer store. Until interactive selection exists, `save-buffer` saves active pane capture text using the existing screen/history/all capture modes; later copy mode can write selected text into the same store. `paste-buffer` writes stored bytes to the active pane PTY, matching existing `send-keys` routing.

**Tech Stack:** Rust standard library, existing CLI parser, existing Unix socket protocol, existing server session/pane model, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: CLI And Protocol Buffer Commands

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`

- [x] **Step 1: Write failing CLI tests**

Add parser tests for:

```rust
#[test]
fn parses_save_buffer_named_screen_capture() {
    let command = parse_args(["dmux", "save-buffer", "-t", "dev", "-b", "saved", "--screen"]).unwrap();
    assert_eq!(
        command,
        Command::SaveBuffer {
            session: "dev".to_string(),
            buffer: Some("saved".to_string()),
            mode: CaptureMode::Screen,
        }
    );
}

#[test]
fn parses_list_buffers() {
    assert_eq!(parse_args(["dmux", "list-buffers"]).unwrap(), Command::ListBuffers);
}

#[test]
fn parses_paste_buffer_named_target() {
    let command = parse_args(["dmux", "paste-buffer", "-t", "dev", "-b", "saved"]).unwrap();
    assert_eq!(
        command,
        Command::PasteBuffer {
            session: "dev".to_string(),
            buffer: Some("saved".to_string()),
        }
    );
}

#[test]
fn parses_delete_buffer_named() {
    assert_eq!(
        parse_args(["dmux", "delete-buffer", "-b", "saved"]).unwrap(),
        Command::DeleteBuffer {
            buffer: "saved".to_string(),
        }
    );
}
```

- [x] **Step 2: Write failing protocol tests**

Add protocol round-trip tests for:

```rust
#[test]
fn round_trips_save_buffer_request() {
    let line = encode_save_buffer("dev", Some("saved"), CaptureMode::Screen);
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::SaveBuffer {
            session: "dev".to_string(),
            buffer: Some("saved".to_string()),
            mode: CaptureMode::Screen,
        }
    );
}

#[test]
fn round_trips_paste_buffer_request() {
    let line = encode_paste_buffer("dev", Some("saved"));
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::PasteBuffer {
            session: "dev".to_string(),
            buffer: Some("saved".to_string()),
        }
    );
}
```

- [x] **Step 3: Verify RED**

Run:

```bash
cargo test --bin dmux save_buffer
cargo test --bin dmux paste_buffer
```

Expected: FAIL because the command and protocol variants do not exist yet.

- [x] **Step 4: Implement CLI and protocol**

Add command variants:

```rust
SaveBuffer {
    session: String,
    buffer: Option<String>,
    mode: CaptureMode,
},
ListBuffers,
PasteBuffer {
    session: String,
    buffer: Option<String>,
},
DeleteBuffer {
    buffer: String,
},
```

Encode requests as:

```text
SAVE_BUFFER    <session>    <screen|history|all>    <buffer-name-hex-or-empty>
LIST_BUFFERS
PASTE_BUFFER   <session>    <buffer-name-hex-or-empty>
DELETE_BUFFER  <buffer-name-hex>
```

Parse:

```text
save-buffer -t <session> [-b <name>] [--screen|--history|--all]
list-buffers
paste-buffer -t <session> [-b <name>]
delete-buffer -b <name>
```

`save-buffer` defaults to combined capture when no mode is supplied. Capture mode flags are mutually exclusive.

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test --bin dmux save_buffer
cargo test --bin dmux paste_buffer
```

Expected: PASS.

### Task 2: Server Buffer Store And Pane Paste

**Files:**
- Modify: `src/server.rs`
- Modify: `src/main.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Write failing integration test**

Add:

```rust
#[test]
fn buffers_save_capture_list_paste_and_delete() {
    let socket = unique_socket("buffers");
    let source = format!("buffer-source-{}", std::process::id());
    let sink = format!("buffer-sink-{}", std::process::id());
    let file = unique_temp_file("buffer-paste");

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
            "printf buffer-alpha; printf '\\n'; sleep 30",
        ],
    ));
    let captured = poll_capture(&socket, &source, "buffer-alpha");
    assert!(captured.contains("buffer-alpha"), "{captured:?}");

    let saved = dmux(&socket, &["save-buffer", "-t", &source, "-b", "saved", "--screen"]);
    assert_success(&saved);
    assert_eq!(String::from_utf8_lossy(&saved.stdout), "saved\n");

    let listed = dmux(&socket, &["list-buffers"]);
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(listed.lines().any(|line| line.starts_with("saved\t")), "{listed:?}");

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

    assert_success(&dmux(&socket, &["paste-buffer", "-t", &sink, "-b", "saved"]));
    assert!(poll_file_contains(&file, "buffer-alpha"));

    assert_success(&dmux(&socket, &["delete-buffer", "-b", "saved"]));
    let listed = dmux(&socket, &["list-buffers"]);
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(!listed.lines().any(|line| line.starts_with("saved\t")), "{listed:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &source]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &sink]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

Add helper:

```rust
fn poll_file_contains(path: &std::path::Path, needle: &str) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if std::fs::read_to_string(path).is_ok_and(|text| text.contains(needle)) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}
```

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test --test phase1_cli buffers_save_capture
```

Expected: FAIL because server request handling is not implemented yet.

- [x] **Step 3: Implement server buffer store**

Add `buffers: Mutex<BufferStore>` to `ServerState`, plus:

```rust
const MAX_BUFFER_BYTES: usize = 1024 * 1024;

struct Buffer {
    name: String,
    text: String,
}

struct BufferDescription {
    name: String,
    bytes: usize,
    preview: String,
}

struct BufferStore {
    buffers: Vec<Buffer>,
    next_auto: usize,
}
```

Implement save, list, resolve latest-or-named, and delete. Replacing an existing named buffer should move it to the latest position so unnamed paste uses the most recently saved buffer.

- [x] **Step 4: Implement request handlers and main dispatch**

Add server handlers:

```rust
handle_save_buffer
handle_list_buffers
handle_paste_buffer
handle_delete_buffer
```

`save-buffer` captures the active pane with the requested mode, stores it, and prints the saved buffer name. `list-buffers` prints `name<TAB>byte_count<TAB>preview`. `paste-buffer` writes buffer text bytes to the active pane writer. `delete-buffer` removes a named buffer.

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test --test phase1_cli buffers_save_capture
```

Expected: PASS.

### Task 3: Docs, Verification, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-buffers.md`

- [x] **Step 1: Document buffer commands**

Add commands:

```text
dmux save-buffer -t <name> [-b <buffer>] [--screen|--history|--all]
dmux list-buffers
dmux paste-buffer -t <name> [-b <buffer>]
dmux delete-buffer -b <buffer>
```

Mention that buffers are currently in-memory and fed by pane capture until interactive selection lands.

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
git add README.md src/cli.rs src/main.rs src/protocol.rs src/server.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-buffers.md
git commit -m "feat: add mux buffers"
git push -u origin devmux-buffers
gh pr create --draft --base main --head devmux-buffers --title "Add mux buffers" --body "<summary and validation>"
```
