# Session Lifecycle And Attach Help Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the basic manual workflow open, detach, shut down, restart, and reveal attach/pane commands.

**Architecture:** Keep server protocol changes out of scope. Reuse the existing client attach path after `NEW` for interactive `new`, make `kill-server` stale-socket cleanup idempotent, and add static help text plus `C-b ?` input handling.

**Tech Stack:** Rust stdlib, Unix domain socket control protocol, existing PTY integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Documented Session Lifecycle Smoke Tests

**Files:**
- Modify: `tests/phase1_cli.rs`

- [ ] **Step 1: Write failing interactive-new attach test**

Add an integration test near the existing attach tests:

```rust
#[test]
fn interactive_new_attaches_and_detaches_created_session() {
    let socket = unique_socket("interactive-new-attach");
    let session = format!("interactive-new-attach-{}", std::process::id());

    let mut child = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .args([
            "new",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf interactive-ready; sleep 30",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn interactive new");

    let ready = poll_capture(&socket, &session, "interactive-ready");
    assert!(ready.contains("interactive-ready"), "{ready:?}");
    assert!(child.try_wait().expect("poll interactive new").is_none());

    {
        let stdin = child.stdin.as_mut().expect("interactive new stdin");
        stdin.write_all(b"\x02d").expect("write detach");
        stdin.flush().expect("flush detach");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("interactive-ready"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [ ] **Step 2: Run and verify RED**

Run:

```bash
cargo test --test phase1_cli interactive_new_attaches_and_detaches_created_session
```

Expected: FAIL because `new -s` exits with `interactive attach is not implemented yet; use -d`.

- [ ] **Step 3: Write lifecycle recreate smoke test**

Add:

```rust
#[test]
fn session_lifecycle_can_attach_detach_kill_shutdown_and_recreate() {
    let socket = unique_socket("session-lifecycle-smoke");
    let session = format!("session-lifecycle-smoke-{}", std::process::id());

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
            "printf smoke-ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "smoke-ready");
    assert!(ready.contains("smoke-ready"), "{ready:?}");

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
        stdin.write_all(b"\x02d").expect("write detach");
        stdin.flush().expect("flush detach");
    }
    assert_success(&wait_for_child_exit(child));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    let listed = dmux(&socket, &["ls"]);
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(!listed.lines().any(|line| line == session), "{listed:?}");

    assert_success(&dmux(&socket, &["kill-server"]));
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
            "printf recreated-ready; sleep 30",
        ],
    ));
    let recreated = poll_capture(&socket, &session, "recreated-ready");
    assert!(recreated.contains("recreated-ready"), "{recreated:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [ ] **Step 4: Run smoke test**

Run:

```bash
cargo test --test phase1_cli session_lifecycle_can_attach_detach_kill_shutdown_and_recreate
```

Expected: PASS before implementation if current lifecycle already supports this path, or FAIL with a concrete lifecycle bug to fix.

### Task 2: Interactive New And Idempotent Shutdown

**Files:**
- Modify: `src/main.rs`
- Modify: `tests/phase1_cli.rs`

- [ ] **Step 1: Implement interactive `new -s` by reusing attach**

In the `Command::New` match arm, replace the non-detached error with a call to a helper that performs the same resize and `client::attach` flow used by `Command::Attach`.

- [ ] **Step 2: Add stale socket test**

Add:

```rust
#[test]
fn kill_server_removes_stale_socket_path() {
    let socket = unique_socket("stale-kill-server");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind stale socket");
    drop(listener);

    let output = dmux(&socket, &["kill-server"]);

    assert_success(&output);
    assert!(!socket.exists(), "stale socket path should be removed");
}
```

Run:

```bash
cargo test --test phase1_cli kill_server_removes_stale_socket_path
```

Expected: FAIL before stale cleanup.

Also add a guard test that a regular file at `DEVMUX_SOCKET` is not removed:

```rust
#[test]
fn kill_server_does_not_remove_non_socket_path() {
    let socket = unique_socket("regular-file-kill-server");
    std::fs::write(&socket, b"not a socket").expect("write regular file");

    let output = dmux(&socket, &["kill-server"]);

    assert!(!output.status.success());
    assert!(socket.exists(), "regular file path should not be removed");
    std::fs::remove_file(&socket).expect("remove regular file");
}
```

- [ ] **Step 3: Implement stale socket cleanup**

In `Command::KillServer`, if the socket path exists and the kill request fails
with a stale socket-style connection refusal, remove it only after verifying the
path is a Unix socket. Keep non-socket paths and protocol/server response errors
visible.

- [ ] **Step 4: Run lifecycle tests**

Run:

```bash
cargo test --test phase1_cli interactive_new_attaches_and_detaches_created_session
cargo test --test phase1_cli session_lifecycle_can_attach_detach_kill_shutdown_and_recreate
cargo test --test phase1_cli kill_server_removes_stale_socket_path
```

Expected: PASS.

### Task 3: CLI Help And Attach-Time Help

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/main.rs`
- Modify: `src/client.rs`
- Modify: `tests/phase1_cli.rs`
- Modify: `README.md`

- [ ] **Step 1: Add failing CLI help tests**

Add unit tests in `src/cli.rs` for `dmux --help`, `dmux help`, `dmux attach --help`, and `dmux help attach`.

- [ ] **Step 2: Implement CLI help parsing and printing**

Add a `Command::Help { topic: Option<HelpTopic> }` command, static general help text, static attach help text, and a `main.rs` arm that prints help without starting the server.

- [ ] **Step 3: Add failing attach `C-b ?` translation tests**

In `src/client.rs` tests, add tests proving `\x02?` maps to help for both raw attach input and live snapshot input.

- [ ] **Step 4: Implement attach-time help**

Add help actions to `AttachInputAction` and `LiveSnapshotInputAction`. Raw attach prints help to stdout and remains attached. Multi-pane snapshot attach renders the help as a message row and remains attached.

- [ ] **Step 5: Add integration help test**

Add an integration test that spawns `attach`, writes `C-b ?`, observes help text in stdout, then writes `C-b d` and asserts a clean detach.

- [ ] **Step 6: Update README**

Document `dmux --help`, `dmux attach --help`, `C-b ?`, and the fact that pane splitting is currently CLI-driven with `split-window -h|-v`.

### Task 4: Verification And PR

**Files:**
- Modify: `HANDOFF.md`

- [ ] **Step 1: Run focused verification**

Run all new/changed focused tests.

- [ ] **Step 2: Run full verification**

Run:

```bash
cargo fmt --check
git diff --check origin/main
rg -ni "co""dex" .
cargo test
```

- [ ] **Step 3: Update `HANDOFF.md`**

Record branch, scope, tests, review findings, PR number, merge result, and retrospective. Do not commit `HANDOFF.md`.

- [ ] **Step 4: Create PR and run critical review**

Create a PR, then dispatch a critical subagent review. Evaluate findings with superpowers:receiving-code-review, fix valid Critical/Important items, rerun verification, and update PR/docs.

- [ ] **Step 5: Merge and retrospective**

After checks and review fixes, merge the PR, delete the remote branch, fetch/prune, and add a post-merge retrospective to `HANDOFF.md`.
