# Devmux Live Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let unzoomed multi-pane `attach` forward stdin bytes to the active pane while keeping polling split-layout redraw.

**Architecture:** Reuse the existing multi-pane `OK\tSNAPSHOT\n` attach handshake. Keep the server-side attach stream open after the handshake and treat bytes read from it as active-pane input. Update the client live snapshot input thread to emit forward/detach events instead of only detach/EOF events.

**Tech Stack:** Rust standard library, existing Unix socket attach stream, existing server `Session::active_pane`, existing client polling redraw loop, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Translate Live Snapshot Input Into Forwardable Events

**Files:**
- Modify: `src/client.rs`

- [x] **Step 1: Replace the read-only translation unit tests**

In `src/client.rs`, replace the existing `live_snapshot_input_...` tests with:

```rust
#[test]
fn live_snapshot_input_forwards_arbitrary_bytes() {
    let mut saw_prefix = false;

    let action = translate_live_snapshot_input(b"hello\n", &mut saw_prefix);

    assert_eq!(
        action,
        LiveSnapshotInputAction::Forward {
            bytes: b"hello\n".to_vec()
        }
    );
    assert!(!saw_prefix);
}

#[test]
fn live_snapshot_input_detaches_on_prefix_d_without_forwarding_bytes() {
    let mut saw_prefix = false;

    let action = translate_live_snapshot_input(b"\x02d", &mut saw_prefix);

    assert_eq!(
        action,
        LiveSnapshotInputAction::Detach {
            forward: Vec::new()
        }
    );
    assert!(!saw_prefix);
}

#[test]
fn live_snapshot_input_forwards_literal_prefix_with_regular_key() {
    let mut saw_prefix = false;

    let action = translate_live_snapshot_input(b"\x02x", &mut saw_prefix);

    assert_eq!(
        action,
        LiveSnapshotInputAction::Forward {
            bytes: b"\x02x".to_vec()
        }
    );
    assert!(!saw_prefix);
}

#[test]
fn live_snapshot_input_flushes_pending_prefix_on_eof() {
    let mut saw_prefix = true;

    let pending = finish_live_snapshot_input(&mut saw_prefix);

    assert_eq!(pending, Some(vec![0x02]));
    assert!(!saw_prefix);
}
```

- [x] **Step 2: Run unit tests to verify RED**

Run:

```bash
cargo test live_snapshot_input_
```

Expected: FAIL because `LiveSnapshotInputAction` still has `Continue`, has no forwarding payload, and `finish_live_snapshot_input` does not exist.

- [x] **Step 3: Implement forwardable input translation**

In `src/client.rs`, replace `LiveSnapshotInputAction` and `translate_live_snapshot_input` with:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveSnapshotInputAction {
    Forward { bytes: Vec<u8> },
    Detach { forward: Vec<u8> },
}

fn translate_live_snapshot_input(input: &[u8], saw_prefix: &mut bool) -> LiveSnapshotInputAction {
    let mut output = Vec::with_capacity(input.len());

    for byte in input {
        if *saw_prefix {
            *saw_prefix = false;
            match *byte {
                b'd' => return LiveSnapshotInputAction::Detach { forward: output },
                _ => {
                    output.push(0x02);
                    output.push(*byte);
                    continue;
                }
            }
        }

        if *byte == 0x02 {
            *saw_prefix = true;
        } else {
            output.push(*byte);
        }
    }

    LiveSnapshotInputAction::Forward { bytes: output }
}

fn finish_live_snapshot_input(saw_prefix: &mut bool) -> Option<Vec<u8>> {
    if !*saw_prefix {
        return None;
    }

    *saw_prefix = false;
    Some(vec![0x02])
}
```

- [x] **Step 4: Run unit tests to verify GREEN**

Run:

```bash
cargo test live_snapshot_input_
```

Expected: PASS.

- [x] **Step 5: Keep translator with live input forwarding commit**

The translator was folded into the live input forwarding commit to avoid an intermediate commit with a temporary dead-code warning.

Final commit command:

```bash
git add src/client.rs src/server.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-12-devmux-live-input.md
git commit -m "feat: route live attach input"
```

### Task 2: Forward Live Snapshot Input Over The Attach Stream

**Files:**
- Modify: `src/client.rs`
- Modify: `src/server.rs`

- [x] **Step 1: Write failing integration test**

Add this test in `tests/phase1_cli.rs` after `attach_live_redraws_split_pane_output_after_attach_starts`:

```rust
#[test]
fn attach_live_input_routes_stdin_to_active_split_pane() {
    let socket = unique_socket("attach-live-input");
    let session = format!("attach-live-input-{}", std::process::id());

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
            "printf split-ready; read line; echo typed:$line; sleep 30",
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

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin.write_all(b"hello\n").expect("write attach input");
        stdin.flush().expect("flush attach input");
    }

    let typed = poll_capture(&socket, &session, "typed:hello");
    assert!(typed.contains("typed:hello"), "{typed:?}");
    std::thread::sleep(std::time::Duration::from_millis(250));

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("typed:hello"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 2: Run integration test to verify RED**

Run:

```bash
cargo test --test phase1_cli attach_live_input_routes_stdin_to_active_split_pane
```

Expected: FAIL because the client does not forward live snapshot input and the server returns from multi-pane `ATTACH` after the handshake.

- [x] **Step 3: Update live snapshot input events**

In `src/client.rs`, replace `LiveSnapshotInputEvent` with:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveSnapshotInputEvent {
    Forward(Vec<u8>),
    Detach,
    Eof,
}
```

Update `spawn_live_snapshot_input_thread`:

```rust
fn spawn_live_snapshot_input_thread() -> mpsc::Receiver<LiveSnapshotInputEvent> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = [0_u8; 1024];
        let mut saw_prefix = false;

        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) => {
                    if let Some(bytes) = finish_live_snapshot_input(&mut saw_prefix) {
                        let _ = sender.send(LiveSnapshotInputEvent::Forward(bytes));
                    }
                    let _ = sender.send(LiveSnapshotInputEvent::Eof);
                    break;
                }
                Ok(n) => n,
                Err(_) => {
                    let _ = sender.send(LiveSnapshotInputEvent::Eof);
                    break;
                }
            };

            match translate_live_snapshot_input(&buf[..n], &mut saw_prefix) {
                LiveSnapshotInputAction::Forward { bytes } => {
                    if !bytes.is_empty() {
                        let _ = sender.send(LiveSnapshotInputEvent::Forward(bytes));
                    }
                }
                LiveSnapshotInputAction::Detach { forward } => {
                    if !forward.is_empty() {
                        let _ = sender.send(LiveSnapshotInputEvent::Forward(forward));
                    }
                    let _ = sender.send(LiveSnapshotInputEvent::Detach);
                    break;
                }
            }
        }
    });
    receiver
}
```

- [x] **Step 4: Forward events in the redraw loop**

Update `run_live_snapshot_attach`:

```rust
loop {
    match input.recv_timeout(LIVE_SNAPSHOT_REDRAW_INTERVAL) {
        Ok(LiveSnapshotInputEvent::Forward(bytes)) => stream.write_all(&bytes)?,
        Ok(LiveSnapshotInputEvent::Detach) | Ok(LiveSnapshotInputEvent::Eof) => break,
        Err(mpsc::RecvTimeoutError::Timeout) => write_live_snapshot_frame(socket, session)?,
        Err(mpsc::RecvTimeoutError::Disconnected) => break,
    }
}
```

- [x] **Step 5: Keep the server multi-pane attach stream open for input**

In `src/server.rs`, update the multi-pane branch in `handle_attach`:

```rust
if has_attach_pane_snapshot(&session) {
    write_attach_snapshot_ok(&mut stream)?;
    return forward_multi_pane_attach_input(&session, &mut stream);
}
```

Add this helper near `write_attach_snapshot_ok`:

```rust
fn forward_multi_pane_attach_input(session: &Arc<Session>, stream: &mut UnixStream) -> io::Result<()> {
    let mut buf = [0_u8; 8192];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }

        let Some(pane) = session.active_pane() else {
            break;
        };
        pane.writer.lock().unwrap().write_all(&buf[..n])?;
    }

    Ok(())
}
```

- [x] **Step 6: Run focused integration tests to verify GREEN**

Run:

```bash
cargo test --test phase1_cli attach_live_input_routes_stdin_to_active_split_pane
cargo test --test phase1_cli attach_live_redraws_split_pane_output_after_attach_starts
cargo test --test phase1_cli attach_multi_pane_keeps_snapshot_handshake_for_client_compatibility
cargo test --test phase1_cli attach_keeps_zoomed_split_pane_live
```

Expected: PASS.

- [x] **Step 7: Commit live input forwarding**

Run:

```bash
git add src/client.rs src/server.rs tests/phase1_cli.rs
git commit -m "feat: route live attach input"
```

### Task 3: Documentation, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-12-devmux-live-input.md`

- [x] **Step 1: Update README**

Add implemented groundwork:

```text
- active-pane input routing for polling multi-pane attach
```

Update current limits:

```text
- multi-pane attach live redraw is polling-based and routes input to the server active pane; in-attach pane focus switching and multi-pane copy-mode are not implemented yet
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
Review the live attach input diff for correctness. Focus on multi-pane attach stream lifetime, active-pane routing, prefix/detach handling, EOF behavior, older snapshot-client compatibility, zoomed raw attach preservation, test reliability, and README/spec consistency. Report only actionable correctness or maintainability issues.
```

Apply technically valid Critical or Important findings, rerun full verification, and update this plan's checkboxes.

- [ ] **Step 4: Open PR and merge after validation**

Run:

```bash
git push -u origin devmux-live-input
gh pr create --base main --head devmux-live-input --title "Route live attach input" --body $'## Summary\n- Forward unzoomed multi-pane attach stdin to the active pane.\n- Keep polling split-layout redraw and existing snapshot attach handshake.\n- Preserve raw attach behavior for single visible panes and zoomed panes.\n\n## Validation\n- cargo fmt --check\n- git diff --check origin/main\n- reserved keyword scan: no matches\n- cargo test\n\n## Review\n- Critical review completed before PR.'
PR_NUMBER=$(gh pr view --head devmux-live-input --json number -q .number)
gh pr checks "$PR_NUMBER" --watch=false
gh pr merge "$PR_NUMBER" --squash --delete-branch --subject "Route live attach input"
```

The PR title and body must not contain the project-assistant keyword requested by the user.
