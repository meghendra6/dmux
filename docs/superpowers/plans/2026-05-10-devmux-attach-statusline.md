# Attach Statusline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render the existing server-side statusline in attached clients so window, pane, and zoom state becomes visible in the attach view.

**Architecture:** Keep the attach stream active-pane based for this slice. The client renders a single attach-only statusline snapshot by reusing the existing `status-line` control request after attach succeeds and before stdin forwarding starts. This does not write status bytes into the child PTY or captured pane text, and it leaves full multi-pane layout composition and live status redraw for later slices.

**Tech Stack:** Rust standard library, existing Unix socket attach stream, existing statusline request path, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Attach Statusline Snapshot

**Files:**
- Modify: `tests/phase1_cli.rs`
- Modify: `src/client.rs`

- [x] **Step 1: Write failing integration test**

Add:

```rust
#[test]
fn attach_renders_status_line_snapshot() {
    let socket = unique_socket("attach-status-line");
    let session = format!("attach-status-line-{}", std::process::id());

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
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .args(["attach", "-t", &session])
        .stdin(Stdio::null())
        .output()
        .expect("run attach");
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("{session} [0] pane 0")),
        "{stdout:?}"
    );

    let captured = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
    assert_success(&captured);
    let captured = String::from_utf8_lossy(&captured.stdout);
    assert!(
        !captured.contains(&format!("{session} [0] pane 0")),
        "{captured:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test --test phase1_cli attach_renders_status_line_snapshot
```

Expected: FAIL because attached clients do not render statusline output.

- [x] **Step 3: Implement attach statusline snapshot**

In `src/client.rs`, add a helper that reuses the existing `status-line` control request:

```rust
fn write_attach_status_line(socket: &Path, session: &str) -> io::Result<()> {
    let body = send_control_request(socket, &protocol::encode_status_line(session, None))?;
    let status = String::from_utf8_lossy(&body);
    let status = status.trim_end();
    if status.is_empty() {
        return Ok(());
    }

    let mut stdout = io::stdout().lock();
    stdout.write_all(format!("{status}\r\n").as_bytes())?;
    stdout.flush()
}
```

Call it in `attach` immediately after the server returns `OK` and before raw mode, output copying, and stdin forwarding begin.

- [x] **Step 4: Verify GREEN**

Run:

```bash
cargo test --test phase1_cli attach_renders_status_line_snapshot
```

Expected: PASS.

### Task 2: Documentation, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-attach-statusline.md`

- [x] **Step 1: Document attach statusline**

Update README implemented bullets to include attach-time statusline snapshot rendering. Update current limits so statusline rendering is described as snapshot-only and layout rendering remains pending.

- [x] **Step 2: Run full verification**

Run:

```bash
cargo fmt --check
git diff --check origin/main
rg -ni "co""dex" .
cargo test
```

Expected: formatting and tests pass; keyword scan prints no matches; whitespace check passes.

- [x] **Step 3: Request subagent review**

Dispatch a read-only review subagent over `git diff origin/main`. Apply technically valid blocking or important findings and rerun full verification.

- [x] **Step 4: Commit and open draft PR**

Run:

```bash
git add README.md src/client.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-attach-statusline.md
git commit -m "feat: render attach statusline"
git push -u origin devmux-attach-statusline
gh pr create --draft --base main --head devmux-attach-statusline --title "Render attach statusline" --body "<summary and validation>"
```
