# DevMux Send Keys Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `dmux send-keys` for the current single-pane session so scripts and tests can write input into a detached PTY.

**Architecture:** The CLI parses `send-keys -t <session> <keys...>`, translates simple key tokens into bytes, and sends a `SEND` request over the existing Unix socket protocol. The server owns PTY input and writes decoded bytes into the session PTY writer.

**Tech Stack:** Rust standard library only. Protocol payloads encode bytes as hex so control bytes and carriage returns can travel safely over the existing line protocol.

---

### Task 1: CLI Key Translation And Protocol

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write failing parser/protocol tests**

Add parser test:

```rust
#[test]
fn parses_send_keys_target_and_keys() {
    let command = parse_args(["dmux", "send-keys", "-t", "dev", "echo hi", "Enter"]).unwrap();
    assert_eq!(
        command,
        Command::SendKeys {
            session: "dev".to_string(),
            keys: vec!["echo hi".to_string(), "Enter".to_string()],
        }
    );
}
```

Add protocol test:

```rust
#[test]
fn round_trips_send_request() {
    let line = encode_send("dev", b"hello\r");
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::Send {
            session: "dev".to_string(),
            bytes: b"hello\r".to_vec(),
        }
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test cli::tests::parses_send_keys_target_and_keys
cargo test protocol::tests::round_trips_send_request
```

Expected: compilation fails because send variants/helpers are missing.

- [ ] **Step 3: Implement parser, key translation, and protocol**

Add:

```rust
Command::SendKeys { session: String, keys: Vec<String> }
Request::Send { session: String, bytes: Vec<u8> }
```

Support:

```bash
dmux send-keys -t <session> <keys...>
```

Translate key tokens in `main.rs`:

- literal strings append UTF-8 bytes
- `Enter` appends `\r`
- `Space` appends ` `
- `Tab` appends `\t`
- `Escape` appends `\x1b`
- `C-c` appends `\x03`

Protocol:

```text
SEND\t<session>\t<hex-bytes>\n
```

- [ ] **Step 4: Run tests**

Run: `cargo test cli::tests protocol::tests`

Expected: parser/protocol tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/cli.rs src/protocol.rs src/main.rs
git commit -m "feat: add send-keys command protocol"
```

---

### Task 2: Server Writes Sent Keys To PTY

**Files:**
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [ ] **Step 1: Write failing integration test**

Add:

```rust
#[test]
fn send_keys_writes_input_to_detached_session() {
    let socket = unique_socket("send-keys");
    let session = format!("send-keys-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "read line; echo got:$line; sleep 30"],
    ));

    assert_success(&dmux(&socket, &["send-keys", "-t", &session, "hello", "Enter"]));

    let captured = poll_capture(&socket, &session, "got:hello");
    assert!(captured.contains("got:hello"), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test phase1_cli send_keys_writes_input_to_detached_session`

Expected: failure because server does not handle `SEND`.

- [ ] **Step 3: Implement server handling**

Add `Request::Send` handling:

- find session
- write bytes to `session.writer`
- return `OK`
- return `ERR missing session` for unknown sessions

- [ ] **Step 4: Run tests**

Run: `cargo test`

Expected: all tests pass.

- [ ] **Step 5: Commit**

Run:

```bash
git add src/server.rs tests/phase1_cli.rs
git commit -m "feat: send keys to detached sessions"
```

---

### Task 3: Documentation And Verification

**Files:**
- Modify: `README.md`
- Add: `docs/superpowers/plans/2026-05-10-devmux-send-keys.md`

- [ ] **Step 1: Document send-keys**

Add `dmux send-keys -t <name> <keys...>` to implemented commands and mention the supported key tokens.

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
git add README.md docs/superpowers/plans/2026-05-10-devmux-send-keys.md
git commit -m "docs: plan send-keys slice"
```

---

## Self-Review

- This plan only implements single-session input injection.
- It does not implement pane targeting, key tables, repeat counts, or full tmux key-name compatibility.
- This is a prerequisite control primitive for future split-pane and scripted acceptance tests.
