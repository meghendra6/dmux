# Devmux Attach Pane Focus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `C-b o` in unzoomed multi-pane attach to cycle the server active pane.

**Architecture:** Keep this client-side and reuse existing control protocol. The live snapshot input translator emits a select-next-pane event for `C-b o`; the redraw loop calls `LIST_PANES_FORMAT` to find the next pane and `SELECT_PANE` to activate it, then redraws immediately.

**Tech Stack:** Rust standard library, existing `send_control_request`, existing `LIST_PANES_FORMAT` and `SELECT_PANE` protocol helpers, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Parse Pane Lists And Translate Focus Input

**Files:**
- Modify: `src/client.rs`

- [x] **Step 1: Write failing unit tests**

Add these tests near the existing live snapshot input tests in `src/client.rs`:

```rust
#[test]
fn live_snapshot_input_selects_next_pane_on_prefix_o() {
    let mut saw_prefix = false;

    let action = translate_live_snapshot_input(b"\x02o", &mut saw_prefix);

    assert_eq!(action, LiveSnapshotInputAction::SelectNextPane);
    assert!(!saw_prefix);
}

#[test]
fn next_pane_index_wraps_after_active_pane() {
    assert_eq!(
        next_pane_index_from_listing("0\t0\n1\t1\n2\t0\n").unwrap(),
        2
    );
    assert_eq!(
        next_pane_index_from_listing("0\t0\n1\t0\n2\t1\n").unwrap(),
        0
    );
}

#[test]
fn next_pane_index_rejects_missing_active_pane() {
    assert!(next_pane_index_from_listing("0\t0\n1\t0\n").is_err());
}
```

- [x] **Step 2: Run unit tests to verify RED**

Run:

```bash
cargo test live_snapshot_input_selects_next_pane_on_prefix_o next_pane_index_
```

Expected: FAIL because `SelectNextPane` and `next_pane_index_from_listing` do not exist.

- [x] **Step 3: Implement input action and pane list parser**

In `src/client.rs`, add a new action variant:

```rust
enum LiveSnapshotInputAction {
    Forward { bytes: Vec<u8> },
    Detach { forward: Vec<u8> },
    SelectNextPane,
}
```

Update the prefix match in `translate_live_snapshot_input`:

```rust
match *byte {
    b'd' => return LiveSnapshotInputAction::Detach { forward: output },
    b'o' => {
        if output.is_empty() {
            return LiveSnapshotInputAction::SelectNextPane;
        }
        output.push(0x02);
        output.push(*byte);
        continue;
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
```

Add these parser helpers near the live snapshot attach helpers:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PaneListEntry {
    index: usize,
    active: bool,
}

fn next_pane_index_from_listing(listing: &str) -> io::Result<usize> {
    let entries = parse_pane_listing(listing)?;
    if entries.is_empty() {
        return Err(io::Error::other("missing pane"));
    }

    let active = entries
        .iter()
        .position(|entry| entry.active)
        .ok_or_else(|| io::Error::other("missing active pane"))?;
    Ok(entries[(active + 1) % entries.len()].index)
}

fn parse_pane_listing(listing: &str) -> io::Result<Vec<PaneListEntry>> {
    let mut entries = Vec::new();
    for line in listing.lines() {
        let Some((index, active)) = line.split_once('\t') else {
            return Err(io::Error::other("invalid pane listing"));
        };
        let index = index
            .parse::<usize>()
            .map_err(|_| io::Error::other("invalid pane index"))?;
        let active = match active {
            "0" => false,
            "1" => true,
            _ => return Err(io::Error::other("invalid active pane flag")),
        };
        entries.push(PaneListEntry { index, active });
    }
    Ok(entries)
}
```

- [x] **Step 4: Run unit tests to verify GREEN**

Run:

```bash
cargo test live_snapshot_input_selects_next_pane_on_prefix_o
cargo test next_pane_index_
```

Expected: PASS.

- [x] **Step 5: Keep parser and translator with attach pane cycling commit**

The parser and translator were folded into the attach pane cycling commit to avoid an intermediate commit with temporary dead-code warnings.

Final commit command:

```bash
git add src/client.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-12-devmux-attach-pane-focus.md
git commit -m "feat: cycle pane focus in attach"
```

### Task 2: Cycle Active Pane From Live Attach

**Files:**
- Modify: `src/client.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Write failing integration test**

Add this test after `attach_live_input_routes_stdin_to_active_split_pane` in `tests/phase1_cli.rs`:

```rust
#[test]
fn attach_prefix_o_cycles_active_pane_for_live_input() {
    let socket = unique_socket("attach-pane-cycle");
    let session = format!("attach-pane-cycle-{}", std::process::id());
    let base_file = unique_temp_file("attach-pane-cycle-base");

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
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
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
            "printf split-ready; read line; echo split-typed:$line; sleep 30",
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
        stdin.write_all(b"\x02o").expect("write cycle input");
        stdin.flush().expect("flush cycle input");
    }
    std::thread::sleep(std::time::Duration::from_millis(150));

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin.write_all(b"base-cycle\n").expect("write base input");
        stdin.flush().expect("flush base input");
    }
    assert!(poll_file_contains(&base_file, "base-cycle"));

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin.write_all(b"\x02o").expect("write second cycle input");
        stdin.flush().expect("flush second cycle input");
    }
    std::thread::sleep(std::time::Duration::from_millis(150));

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin.write_all(b"split\r").expect("write split input");
        stdin.flush().expect("flush split input");
    }
    let split = poll_capture(&socket, &session, "split-typed:split");
    assert!(split.contains("split-typed:split"), "{split:?}");

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 2: Run integration test to verify RED**

Run:

```bash
cargo test --test phase1_cli attach_prefix_o_cycles_active_pane_for_live_input
```

Expected: FAIL because `C-b o` is currently forwarded literally and does not change active pane.

- [x] **Step 3: Add focus event and control helper**

In `src/client.rs`, add the event variant:

```rust
enum LiveSnapshotInputEvent {
    Forward(Vec<u8>),
    SelectNextPane,
    Detach,
    Eof,
}
```

In `spawn_live_snapshot_input_thread`, handle the new action:

```rust
LiveSnapshotInputAction::SelectNextPane => {
    let _ = sender.send(LiveSnapshotInputEvent::SelectNextPane);
}
```

Add this helper near `write_live_snapshot_frame`:

```rust
fn select_next_pane(socket: &Path, session: &str) -> io::Result<()> {
    let body = send_control_request(
        socket,
        &protocol::encode_list_panes(session, Some("#{pane.index}\t#{pane.active}")),
    )?;
    let listing = String::from_utf8_lossy(&body);
    let next = next_pane_index_from_listing(&listing)?;
    let _ = send_control_request(socket, &protocol::encode_select_pane(session, next))?;
    Ok(())
}
```

- [x] **Step 4: Handle focus event in redraw loop**

Update `run_live_snapshot_attach`:

```rust
Ok(LiveSnapshotInputEvent::SelectNextPane) => {
    select_next_pane(socket, session)?;
    write_live_snapshot_frame(socket, session)?;
    last_redraw = Instant::now();
}
```

- [x] **Step 5: Run focused tests to verify GREEN**

Run:

```bash
cargo test --test phase1_cli attach_prefix_o_cycles_active_pane_for_live_input
cargo test --test phase1_cli attach_live_input_routes_stdin_to_active_split_pane
cargo test --test phase1_cli attach_keeps_zoomed_split_pane_live
cargo test live_snapshot_input_
```

Expected: PASS.

- [x] **Step 6: Commit attach pane cycling**

Run:

```bash
git add src/client.rs tests/phase1_cli.rs
git commit -m "feat: cycle pane focus in attach"
```

### Task 3: Documentation, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-12-devmux-attach-pane-focus.md`

- [x] **Step 1: Update README**

Add implemented groundwork:

```text
- attach-time pane cycling for polling multi-pane attach
```

Update current limits:

```text
- multi-pane attach live redraw is polling-based and routes input to the server active pane; numbered pane selection, mouse focus, and multi-pane copy-mode are not implemented yet
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
Review the attach pane focus diff for correctness. Focus on prefix handling, pane-list parsing, active pane cycling, forwarded input after cycling, redraw timing, snapshot-client compatibility, zoomed raw attach preservation, test reliability, and README/spec consistency. Report only actionable correctness or maintainability issues.
```

Apply technically valid Critical or Important findings, rerun full verification, and update this plan's checkboxes.

- [ ] **Step 4: Open PR and merge after validation**

Run:

```bash
git push -u origin devmux-attach-pane-focus
gh pr create --base main --head devmux-attach-pane-focus --title "Cycle attach pane focus" --body $'## Summary\n- Add C-b o pane cycling in unzoomed multi-pane attach.\n- Reuse list-panes and select-pane control requests.\n- Preserve detach, literal prefix, live input, and zoomed raw attach behavior.\n\n## Validation\n- cargo fmt --check\n- git diff --check origin/main\n- reserved keyword scan: no matches\n- cargo test\n\n## Review\n- Critical review completed before PR.'
PR_NUMBER=$(gh pr view --head devmux-attach-pane-focus --json number -q .number)
gh pr checks "$PR_NUMBER" --watch=false
gh pr merge "$PR_NUMBER" --squash --delete-branch --subject "Cycle attach pane focus"
```

The PR title and body must not contain the project-assistant keyword requested by the user.
