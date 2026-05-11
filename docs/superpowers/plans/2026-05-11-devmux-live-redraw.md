# Devmux Live Redraw Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make unzoomed multi-pane `attach` stay open and redraw split-layout snapshots repeatedly in read-only mode.

**Architecture:** Add a `LIVE_SNAPSHOT` attach handshake for multiple visible panes. Write failing integration tests before the redraw implementation, then implement a client-side polling redraw loop that repeatedly requests `status-line` and `ATTACH_SNAPSHOT`. Keep existing raw live attach unchanged for single visible panes and zoomed split windows.

**Tech Stack:** Rust standard library, existing Unix socket protocol, existing attach client in `src/client.rs`, existing attach snapshot renderer in `src/server.rs`, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Add Live Snapshot Handshake

**Files:**
- Modify: `src/client.rs`
- Modify: `src/server.rs`

- [x] **Step 1: Write failing parser test**

Add this test to `src/client.rs` in `#[cfg(test)] mod tests`:

```rust
#[test]
fn parses_live_snapshot_attach_ok() {
    assert_eq!(
        parse_attach_ok("OK\tLIVE_SNAPSHOT\n").unwrap(),
        AttachMode::LiveSnapshot
    );
}
```

- [x] **Step 2: Run parser test to verify RED**

Run:

```bash
cargo test client::tests::parses_live_snapshot_attach_ok
```

Expected: FAIL because `AttachMode::LiveSnapshot` does not exist and `parse_attach_ok` rejects `OK\tLIVE_SNAPSHOT\n`.

- [x] **Step 3: Implement handshake mode**

Update `AttachMode` in `src/client.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachMode {
    Live,
    Snapshot,
    LiveSnapshot,
}
```

Update `parse_attach_ok`:

```rust
fn parse_attach_ok(response: &str) -> io::Result<AttachMode> {
    if response == "OK\n" {
        return Ok(AttachMode::Live);
    }

    if response == "OK\tSNAPSHOT\n" {
        return Ok(AttachMode::Snapshot);
    }

    if response == "OK\tLIVE_SNAPSHOT\n" {
        return Ok(AttachMode::LiveSnapshot);
    }

    Err(io::Error::other(format!(
        "unexpected server response: {response:?}"
    )))
}
```

Rename the server helper in `src/server.rs`:

```rust
fn write_attach_live_snapshot_ok(stream: &mut UnixStream) -> io::Result<()> {
    stream.write_all(b"OK\tLIVE_SNAPSHOT\n")
}
```

Update the multi-pane branch in `handle_attach`:

```rust
if has_attach_pane_snapshot(&session) {
    write_attach_live_snapshot_ok(&mut stream)?;
    return Ok(());
}
```

- [x] **Step 4: Run parser test to verify GREEN**

Run:

```bash
cargo test client::tests::parses_live_snapshot_attach_ok
```

Expected: PASS.

- [x] **Step 5: Commit handshake**

Run:

```bash
git add src/client.rs src/server.rs
git commit -m "feat: add live snapshot attach mode"
```

### Task 2: Write Live Redraw Failing Tests

**Files:**
- Modify: `src/client.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Write failing read-only input unit tests**

Add these tests to `src/client.rs`:

```rust
#[test]
fn live_snapshot_input_detaches_on_prefix_d() {
    let mut saw_prefix = false;

    let action = translate_live_snapshot_input(b"\x02d", &mut saw_prefix);

    assert_eq!(action, LiveSnapshotInputAction::Detach);
    assert!(!saw_prefix);
}

#[test]
fn live_snapshot_input_ignores_arbitrary_bytes_without_forwarding() {
    let mut saw_prefix = false;

    let action = translate_live_snapshot_input(b"ignored\n", &mut saw_prefix);

    assert_eq!(action, LiveSnapshotInputAction::Continue);
    assert!(!saw_prefix);
}
```

- [x] **Step 2: Write failing integration test**

Add this helper near the existing poll helpers in `tests/phase1_cli.rs`:

```rust
fn wait_for_child_exit(mut child: std::process::Child) -> Output {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if child.try_wait().expect("poll child").is_some() {
            return child.wait_with_output().expect("wait child output");
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            return child.wait_with_output().expect("wait killed child output");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
```

Add this test after `attach_renders_vertical_split_layout_snapshot`:

```rust
#[test]
fn attach_live_redraws_split_pane_output_after_attach_starts() {
    let socket = unique_socket("attach-live-redraw");
    let session = format!("attach-live-redraw-{}", std::process::id());

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
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo late:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .args(["attach", "-t", &session])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn attach");

    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(child.try_wait().expect("poll attach").is_none());

    assert_success(&dmux(&socket, &["send-keys", "-t", &session, "hello", "Enter"]));
    let late = poll_capture(&socket, &session, "late:hello");
    assert!(late.contains("late:hello"), "{late:?}");
    std::thread::sleep(std::time::Duration::from_millis(250));

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_contains_ordered_line(&stdout, "base-ready", " | ", "late:hello");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 3: Run unit tests to verify RED**

Run:

```bash
cargo test live_snapshot_input_
```

Expected: FAIL because `LiveSnapshotInputAction` and `translate_live_snapshot_input` do not exist.

- [x] **Step 4: Run integration test to verify RED**

Run:

```bash
cargo test --test phase1_cli attach_live_redraws_split_pane_output_after_attach_starts
```

Expected: FAIL because `LiveSnapshot` mode is parsed but not handled by a redraw loop, or because multi-pane attach exits before `late:hello` can appear.

- [x] **Step 5: Keep tests with the implementation commit**

The failing tests were kept in the working tree and folded into the implementation commit so the branch does not contain an intentionally uncompilable intermediate commit.

Final commit command:

```bash
git add src/client.rs tests/phase1_cli.rs
git commit -m "feat: add live split redraw attach"
```

### Task 3: Implement Read-Only Live Redraw

**Files:**
- Modify: `src/client.rs`

- [x] **Step 1: Add input action and translation**

Add near `AttachInputAction`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveSnapshotInputAction {
    Continue,
    Detach,
}

fn translate_live_snapshot_input(
    input: &[u8],
    saw_prefix: &mut bool,
) -> LiveSnapshotInputAction {
    for byte in input {
        if *saw_prefix {
            *saw_prefix = false;
            match *byte {
                b'd' => return LiveSnapshotInputAction::Detach,
                0x02 => {
                    *saw_prefix = true;
                    continue;
                }
                _ => continue,
            }
        }

        if *byte == 0x02 {
            *saw_prefix = true;
        }
    }

    LiveSnapshotInputAction::Continue
}
```

- [x] **Step 2: Add redraw helpers**

Add imports:

```rust
use std::sync::mpsc;
use std::time::Duration;
```

Add constants near existing constants:

```rust
const LIVE_SNAPSHOT_REDRAW_INTERVAL: Duration = Duration::from_millis(100);
const CLEAR_SCREEN: &[u8] = b"\x1b[2J\x1b[H";
```

Add reusable readers and update existing writers:

```rust
fn read_attach_status_line(socket: &Path, session: &str) -> io::Result<String> {
    let body = send_control_request(socket, &protocol::encode_status_line(session, None))?;
    Ok(String::from_utf8_lossy(&body).trim_end().to_string())
}

fn read_attach_pane_snapshot(socket: &Path, session: &str) -> io::Result<Vec<u8>> {
    send_control_request(socket, &protocol::encode_attach_snapshot(session))
}

fn write_attach_status_line(socket: &Path, session: &str) -> io::Result<()> {
    let status = read_attach_status_line(socket, session)?;
    if status.is_empty() {
        return Ok(());
    }

    let mut stdout = io::stdout().lock();
    stdout.write_all(format!("{status}\r\n").as_bytes())?;
    stdout.flush()
}

fn write_attach_pane_snapshot(socket: &Path, session: &str) -> io::Result<()> {
    let body = read_attach_pane_snapshot(socket, session)?;
    if body.is_empty() {
        return Ok(());
    }

    let mut stdout = io::stdout().lock();
    stdout.write_all(&body)?;
    stdout.flush()
}

fn write_live_snapshot_frame(socket: &Path, session: &str) -> io::Result<()> {
    let status = read_attach_status_line(socket, session)?;
    let snapshot = read_attach_pane_snapshot(socket, session)?;

    let mut stdout = io::stdout().lock();
    stdout.write_all(CLEAR_SCREEN)?;
    if !status.is_empty() {
        stdout.write_all(format!("{status}\r\n").as_bytes())?;
    }
    stdout.write_all(&snapshot)?;
    stdout.flush()
}
```

- [x] **Step 3: Add input thread and redraw loop**

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveSnapshotInputEvent {
    Detach,
    Eof,
}

fn spawn_live_snapshot_input_thread() -> mpsc::Receiver<LiveSnapshotInputEvent> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = [0_u8; 1024];
        let mut saw_prefix = false;

        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) => {
                    let _ = sender.send(LiveSnapshotInputEvent::Eof);
                    break;
                }
                Ok(n) => n,
                Err(_) => {
                    let _ = sender.send(LiveSnapshotInputEvent::Eof);
                    break;
                }
            };

            if translate_live_snapshot_input(&buf[..n], &mut saw_prefix)
                == LiveSnapshotInputAction::Detach
            {
                let _ = sender.send(LiveSnapshotInputEvent::Detach);
                break;
            }
        }
    });
    receiver
}

fn run_live_snapshot_attach(socket: &Path, session: &str, stream: &mut UnixStream) -> io::Result<()> {
    let input = spawn_live_snapshot_input_thread();
    write_live_snapshot_frame(socket, session)?;

    loop {
        match input.recv_timeout(LIVE_SNAPSHOT_REDRAW_INTERVAL) {
            Ok(LiveSnapshotInputEvent::Detach) | Ok(LiveSnapshotInputEvent::Eof) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => write_live_snapshot_frame(socket, session)?,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = stream.shutdown(std::net::Shutdown::Both);
    Ok(())
}
```

- [x] **Step 4: Route `LiveSnapshot` mode in `attach`**

Update the attach mode branch:

```rust
if attach_mode == AttachMode::Snapshot {
    write_attach_status_line(socket, session)?;
    write_attach_pane_snapshot(socket, session)?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    return Ok(());
}

if attach_mode == AttachMode::LiveSnapshot {
    let _guard = RawModeGuard::enable();
    return run_live_snapshot_attach(socket, session, &mut stream);
}

write_attach_status_line(socket, session)?;
```

Leave the existing raw live attach code after this block.

- [x] **Step 5: Run unit tests to verify GREEN**

Run:

```bash
cargo test live_snapshot_input_
```

Expected: PASS.

- [x] **Step 6: Run integration tests to verify GREEN**

Run:

```bash
cargo test --test phase1_cli attach_live_redraws_split_pane_output_after_attach_starts
cargo test --test phase1_cli attach_renders_split_pane_snapshot
cargo test --test phase1_cli attach_keeps_zoomed_split_pane_live
```

Expected: PASS.

- [x] **Step 7: Commit live redraw implementation**

Run:

```bash
git add src/client.rs tests/phase1_cli.rs
git commit -m "feat: add live split redraw attach"
```

### Task 4: Documentation, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-11-devmux-live-redraw.md`

- [x] **Step 1: Update README**

Add implemented groundwork:

```text
- polling-based read-only live redraw for multi-pane attach
```

Update the current multi-pane attach limit:

```text
- multi-pane attach live redraw is polling-based and read-only; unzoomed multi-pane input routing is not implemented yet
```

- [x] **Step 2: Run full verification**

Run:

```bash
cargo fmt --check
git diff --check origin/main
rg -ni "co""dex" .
cargo test
```

Expected: formatting and whitespace checks pass; keyword scan prints no matches; all unit and integration tests pass.

- [ ] **Step 3: Request critical review**

Dispatch a read-only review subagent against `git diff origin/main` with this brief:

```text
Review the live redraw attach diff for correctness. Focus on attach mode handshake compatibility, read-only input behavior, redraw loop exit conditions, server snapshot behavior, zoomed single-pane live attach preservation, test reliability, and docs. Report only actionable correctness or maintainability issues.
```

Apply technically valid Critical or Important findings, rerun full verification, and update this plan's checkboxes.

- [ ] **Step 4: Open PR and merge after validation**

Run:

```bash
git push -u origin devmux-live-redraw
gh pr create --base main --head devmux-live-redraw --title "Add live split redraw" --body $'## Summary\n- Keep unzoomed multi-pane attach open in read-only redraw mode.\n- Poll statusline and split-layout snapshots for visible live updates.\n- Preserve raw live attach for single visible panes and zoomed panes.\n\n## Validation\n- cargo fmt --check\n- git diff --check origin/main\n- rg -ni "co""dex" .\n- cargo test\n\n## Review\n- Critical review completed before PR.'
PR_NUMBER=$(gh pr view --head devmux-live-redraw --json number -q .number)
gh pr checks "$PR_NUMBER" --watch=false
gh pr merge "$PR_NUMBER" --squash --delete-branch --subject "Add live split redraw"
```

The PR title and body must not contain the project-assistant keyword requested by the user.
