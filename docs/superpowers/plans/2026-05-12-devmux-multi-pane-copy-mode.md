# Devmux Multi-Pane Copy Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `C-b [` copy-mode entry to unzoomed multi-pane attach, using the current server active pane.

**Architecture:** Keep the server unchanged because copy-mode and save-buffer already target the active pane. Extend the live snapshot client input events so `C-b [` asks the redraw loop to pause, waits for acknowledgement, runs the existing copy-mode UI on the input thread, then requests an immediate redraw before normal attach input resumes.

**Tech Stack:** Rust standard library, existing Unix socket control requests, existing client copy-mode UI, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Translate Live Snapshot Copy-Mode Input

**Files:**
- Modify: `src/client.rs`

- [x] **Step 1: Write failing unit tests**

Add tests near the existing live snapshot input tests:

```rust
#[test]
fn live_snapshot_input_enters_copy_mode_on_prefix_bracket() {
    let mut saw_prefix = false;

    let actions = translate_live_snapshot_input(b"\x02[", &mut saw_prefix);

    assert_eq!(
        actions,
        vec![LiveSnapshotInputAction::EnterCopyMode {
            initial_input: Vec::new(),
        }]
    );
    assert!(!saw_prefix);
}

#[test]
fn live_snapshot_input_passes_coalesced_copy_mode_keys_as_initial_input() {
    let mut saw_prefix = false;

    let actions = translate_live_snapshot_input(b"\x02[y", &mut saw_prefix);

    assert_eq!(
        actions,
        vec![LiveSnapshotInputAction::EnterCopyMode {
            initial_input: vec![b'y'],
        }]
    );
    assert!(!saw_prefix);
}

#[test]
fn live_snapshot_input_forwards_bytes_before_copy_mode_prefix() {
    let mut saw_prefix = false;

    let actions = translate_live_snapshot_input(b"abc\x02[y", &mut saw_prefix);

    assert_eq!(
        actions,
        vec![
            LiveSnapshotInputAction::Forward(b"abc".to_vec()),
            LiveSnapshotInputAction::EnterCopyMode {
                initial_input: vec![b'y'],
            },
        ]
    );
    assert!(!saw_prefix);
}
```

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test live_snapshot_input_enters_copy_mode_on_prefix_bracket
cargo test live_snapshot_input_passes_coalesced_copy_mode_keys_as_initial_input
cargo test live_snapshot_input_forwards_bytes_before_copy_mode_prefix
```

Expected: FAIL because `LiveSnapshotInputAction::EnterCopyMode` does not exist.

- [x] **Step 3: Add the action variant and translator branch**

In `src/client.rs`, update `LiveSnapshotInputAction`:

```rust
enum LiveSnapshotInputAction {
    Forward(Vec<u8>),
    Detach,
    SelectNextPane,
    EnterCopyMode { initial_input: Vec<u8> },
}
```

Update `translate_live_snapshot_input` to iterate with indexes and handle `b'['`:

```rust
for (index, byte) in input.iter().enumerate() {
    if *saw_prefix {
        *saw_prefix = false;
        match *byte {
            b'd' => {
                if !output.is_empty() {
                    actions.push(LiveSnapshotInputAction::Forward(std::mem::take(&mut output)));
                }
                actions.push(LiveSnapshotInputAction::Detach);
                return actions;
            }
            b'o' => {
                if !output.is_empty() {
                    actions.push(LiveSnapshotInputAction::Forward(std::mem::take(&mut output)));
                }
                actions.push(LiveSnapshotInputAction::SelectNextPane);
                continue;
            }
            b'[' => {
                if !output.is_empty() {
                    actions.push(LiveSnapshotInputAction::Forward(std::mem::take(&mut output)));
                }
                actions.push(LiveSnapshotInputAction::EnterCopyMode {
                    initial_input: input[index + 1..].to_vec(),
                });
                return actions;
            }
            0x02 => {
                output.push(0x02);
                continue;
            }
            _ => {
                output.push(0x02);
                output.push(*byte);
                continue;
            }
        }
    }
    ...
}
```

- [x] **Step 4: Verify GREEN**

Run:

```bash
cargo test live_snapshot_input_
```

Expected: PASS.

### Task 2: Run Copy Mode From Live Snapshot Attach

**Files:**
- Modify: `src/client.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Write failing integration test**

Add this test after `attach_prefix_o_cycles_active_pane_for_live_input`:

```rust
#[test]
fn attach_prefix_bracket_copies_active_pane_line_in_multi_pane_attach() {
    let socket = unique_socket("attach-copy-mode");
    let session = format!("attach-copy-mode-{}", std::process::id());

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
            "printf base-copy; printf '\\n'; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-copy");
    assert!(base.contains("base-copy"), "{base:?}");

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
            "printf split-copy; printf '\\n'; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-copy");
    assert!(split.contains("split-copy"), "{split:?}");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .args(["attach", "-t", &session])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn attach");

    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(child.try_wait().expect("poll attach").is_none());

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin
            .write_all(b"\x02[y")
            .expect("write copy-mode entry and copy key");
        stdin.flush().expect("flush copy-mode input");
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut listed = String::new();
    while std::time::Instant::now() < deadline {
        let output = dmux(&socket, &["list-buffers"]);
        assert_success(&output);
        listed = String::from_utf8_lossy(&output.stdout).to_string();
        if listed
            .lines()
            .any(|line| line.ends_with("\t11\tsplit-copy"))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        listed.lines().any(|line| line.ends_with("\t11\tsplit-copy")),
        "{listed:?}"
    );

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("-- copy mode --"), "{stdout:?}");
    assert!(stdout.contains("-- copy mode: copied to "), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test --test phase1_cli attach_prefix_bracket_copies_active_pane_line_in_multi_pane_attach
```

Expected: FAIL because `C-b [` is currently forwarded literally in live snapshot attach.

- [x] **Step 3: Add live snapshot events**

In `src/client.rs`, update `LiveSnapshotInputEvent`:

```rust
enum LiveSnapshotInputEvent {
    Forward(Vec<u8>),
    SelectNextPane,
    PauseRedraw(mpsc::Sender<()>),
    RedrawNow,
    Error(String),
    Detach,
    Eof,
}
```

- [x] **Step 4: Split copy-mode reader handling**

Replace `run_copy_mode` with a wrapper plus helper:

```rust
fn run_copy_mode(socket: &Path, session: &str, initial_input: &[u8]) -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    run_copy_mode_with_reader(socket, session, initial_input, &mut stdin)
}

fn run_copy_mode_with_reader<R: Read>(
    socket: &Path,
    session: &str,
    initial_input: &[u8],
    stdin: &mut R,
) -> io::Result<()> {
    let body = send_control_request(
        socket,
        &protocol::encode_copy_mode(session, protocol::CaptureMode::All, None),
    )?;
    let output = String::from_utf8_lossy(&body);
    let mut view = CopyModeView::from_numbered_output(&output)?;

    let _mouse = MouseModeGuard::enable()?;
    write_copy_mode_view(&view)?;
    if view.is_empty() {
        write_copy_mode_message("empty")?;
        return Ok(());
    }

    let mut input_state = CopyModeInputState::default();
    if handle_copy_mode_input(socket, session, &mut view, &mut input_state, initial_input)? {
        return Ok(());
    }

    let mut buf = [0_u8; 1024];
    loop {
        let n = stdin.read(&mut buf)?;
        if n == 0 {
            break;
        }

        if handle_copy_mode_input(socket, session, &mut view, &mut input_state, &buf[..n])? {
            break;
        }
    }

    Ok(())
}
```

- [x] **Step 5: Run copy-mode on the input thread**

Change `spawn_live_snapshot_input_thread` to accept `socket: PathBuf` and
`session: String`. Keep the stdin lock in that thread and handle the new action:

```rust
fn spawn_live_snapshot_input_thread(
    socket: std::path::PathBuf,
    session: String,
) -> mpsc::Receiver<LiveSnapshotInputEvent> {
    ...
    let mut stdin = io::stdin().lock();
    ...
    LiveSnapshotInputAction::EnterCopyMode { initial_input } => {
        let (pause_ack, pause_ready) = mpsc::channel();
        let _ = sender.send(LiveSnapshotInputEvent::PauseRedraw(pause_ack));
        let _ = pause_ready.recv();
        match run_copy_mode_with_reader(&socket, &session, &initial_input, &mut stdin) {
            Ok(()) => {
                let _ = sender.send(LiveSnapshotInputEvent::RedrawNow);
            }
            Err(error) => {
                let _ = sender.send(LiveSnapshotInputEvent::Error(error.to_string()));
                return;
            }
        }
    }
```

- [x] **Step 6: Pause and resume redraw in the attach loop**

Update `run_live_snapshot_attach`:

```rust
let input = spawn_live_snapshot_input_thread(socket.to_path_buf(), session.to_string());
write_live_snapshot_frame(socket, session)?;
let mut last_redraw = Instant::now();
let mut redraw_paused = false;

loop {
    match input.recv_timeout(LIVE_SNAPSHOT_REDRAW_INTERVAL) {
        Ok(LiveSnapshotInputEvent::PauseRedraw(pause_ack)) => {
            redraw_paused = true;
            let _ = pause_ack.send(());
        }
        Ok(LiveSnapshotInputEvent::RedrawNow) => {
            redraw_paused = false;
            write_live_snapshot_frame(socket, session)?;
            last_redraw = Instant::now();
        }
        Ok(LiveSnapshotInputEvent::Error(message)) => {
            return Err(io::Error::other(message));
        }
        Ok(LiveSnapshotInputEvent::Forward(bytes)) => {
            stream.write_all(&bytes)?;
            if !redraw_paused && last_redraw.elapsed() >= LIVE_SNAPSHOT_REDRAW_INTERVAL {
                write_live_snapshot_frame(socket, session)?;
                last_redraw = Instant::now();
            }
        }
        Ok(LiveSnapshotInputEvent::SelectNextPane) => {
            select_next_pane(socket, session)?;
            if !redraw_paused {
                write_live_snapshot_frame(socket, session)?;
                last_redraw = Instant::now();
            }
        }
        Ok(LiveSnapshotInputEvent::Detach) | Ok(LiveSnapshotInputEvent::Eof) => break,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            if !redraw_paused {
                write_live_snapshot_frame(socket, session)?;
                last_redraw = Instant::now();
            }
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => break,
    }
}
```

- [x] **Step 7: Verify GREEN**

Run:

```bash
cargo test live_snapshot_input_
cargo test --test phase1_cli attach_prefix_bracket_copies_active_pane_line_in_multi_pane_attach
cargo test --test phase1_cli attach_live_input_routes_stdin_to_active_split_pane
cargo test --test phase1_cli attach_prefix_o_cycles_active_pane_for_live_input
```

Expected: PASS.

- [x] **Step 8: Commit implementation**

Run:

```bash
git add src/client.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-12-devmux-multi-pane-copy-mode.md
git commit -m "feat: add multi-pane attach copy mode"
```

### Task 3: Documentation, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-12-devmux-multi-pane-copy-mode.md`

- [x] **Step 1: Update README**

Update the copy-mode attach paragraph to say:

```text
Attached clients can enter the current basic copy-mode view with `C-b [`.
```

Update implemented groundwork:

```text
- multi-pane attach copy-mode entry for the active pane
```

Update current limits:

```text
- multi-pane attach live redraw is polling-based and routes input to the server active pane; numbered pane selection, mouse focus, and composed-layout copy-mode are not implemented yet
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
Review the multi-pane attach copy-mode diff for correctness. Focus on stdin ownership, redraw pause/resume, copy-mode initial input ordering, active-pane behavior, detach/EOF handling after copy-mode exits, mouse mode cleanup, live input and C-b o regressions, snapshot-client compatibility, and test reliability. Report only actionable correctness or maintainability issues.
```

Apply technically valid Critical or Important findings, rerun full verification, and update this plan.

- [ ] **Step 4: Open PR and merge after validation**

Run:

```bash
git push -u origin devmux-multi-pane-copy-mode
gh pr create --base main --head devmux-multi-pane-copy-mode --title "Add multi-pane attach copy mode" --body $'## Summary\n- Add C-b [ copy-mode entry for unzoomed multi-pane attach.\n- Pause polling redraw while copy-mode owns stdin and redraw after copy-mode exits.\n- Preserve active-pane input, C-b o pane cycling, and snapshot attach compatibility.\n\n## Validation\n- cargo fmt --check\n- git diff --check origin/main\n- reserved keyword scan: no matches\n- cargo test\n\n## Review\n- Critical review completed before PR; valid findings were addressed.'
gh pr checks <number> --watch=false
gh pr merge <number> --squash --delete-branch --subject "Add multi-pane attach copy mode"
```

The PR title and body must not contain the reserved project-assistant keyword requested by the user.
