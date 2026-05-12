# Devmux Numbered Pane Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `C-b q` plus a single digit in unzoomed multi-pane attach to show pane indexes and select a pane by number.

**Architecture:** Keep the feature client-side and reuse existing control protocol. The live snapshot input translator emits pane-number UI and selection events; the redraw loop uses `LIST_PANES_FORMAT` to render a transient pane-number message and `SELECT_PANE` to activate a valid requested pane.

**Tech Stack:** Rust standard library, existing `send_control_request`, existing `LIST_PANES_FORMAT` and `SELECT_PANE` protocol helpers, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Add Live Snapshot Numbered-Selection Input State

**Files:**
- Modify: `src/client.rs`

- [x] **Step 1: Write failing unit tests**

Add these tests near the existing live snapshot input tests in `src/client.rs`:

```rust
#[test]
fn live_snapshot_input_shows_pane_numbers_on_prefix_q() {
    let mut state = LiveSnapshotInputState::default();

    let actions = translate_live_snapshot_input(b"\x02q", &mut state);

    assert_eq!(actions, vec![LiveSnapshotInputAction::ShowPaneNumbers]);
    assert!(state.selecting_pane);
}

#[test]
fn live_snapshot_input_selects_numbered_pane_after_prefix_q_across_reads() {
    let mut state = LiveSnapshotInputState::default();

    assert_eq!(
        translate_live_snapshot_input(b"\x02q", &mut state),
        vec![LiveSnapshotInputAction::ShowPaneNumbers]
    );
    let actions = translate_live_snapshot_input(b"0", &mut state);

    assert_eq!(actions, vec![LiveSnapshotInputAction::SelectPane(0)]);
    assert!(!state.selecting_pane);
}

#[test]
fn live_snapshot_input_preserves_order_for_coalesced_number_selection() {
    let mut state = LiveSnapshotInputState::default();

    let actions = translate_live_snapshot_input(b"abc\x02q0def", &mut state);

    assert_eq!(
        actions,
        vec![
            LiveSnapshotInputAction::Forward(b"abc".to_vec()),
            LiveSnapshotInputAction::ShowPaneNumbers,
            LiveSnapshotInputAction::SelectPane(0),
            LiveSnapshotInputAction::Forward(b"def".to_vec()),
        ]
    );
    assert!(!state.selecting_pane);
}

#[test]
fn live_snapshot_input_cancels_number_selection_on_non_digit() {
    let mut state = LiveSnapshotInputState::default();

    assert_eq!(
        translate_live_snapshot_input(b"\x02q", &mut state),
        vec![LiveSnapshotInputAction::ShowPaneNumbers]
    );
    let actions = translate_live_snapshot_input(b"x", &mut state);

    assert_eq!(actions, vec![LiveSnapshotInputAction::Forward(b"x".to_vec())]);
    assert!(!state.selecting_pane);
}
```

- [x] **Step 2: Run unit tests to verify RED**

Run:

```bash
cargo test live_snapshot_input_shows_pane_numbers_on_prefix_q
cargo test live_snapshot_input_selects_numbered_pane_after_prefix_q_across_reads
cargo test live_snapshot_input_preserves_order_for_coalesced_number_selection
cargo test live_snapshot_input_cancels_number_selection_on_non_digit
```

Expected: FAIL because `LiveSnapshotInputState`, `ShowPaneNumbers`, and `SelectPane` do not exist.

- [x] **Step 3: Implement the input state and actions**

In `src/client.rs`, replace the live snapshot translator boolean state with this struct:

```rust
#[derive(Default)]
struct LiveSnapshotInputState {
    saw_prefix: bool,
    selecting_pane: bool,
}
```

Update `LiveSnapshotInputAction`:

```rust
enum LiveSnapshotInputAction {
    Forward(Vec<u8>),
    Detach,
    SelectNextPane,
    ShowPaneNumbers,
    SelectPane(usize),
    EnterCopyMode { initial_input: Vec<u8> },
}
```

Change `translate_live_snapshot_input` to accept `&mut LiveSnapshotInputState`.
At the top of its byte loop, handle pending numbered selection:

```rust
if state.selecting_pane {
    state.selecting_pane = false;
    if byte.is_ascii_digit() {
        actions.push(LiveSnapshotInputAction::SelectPane(usize::from(*byte - b'0')));
        continue;
    }
    if *byte == 0x02 {
        state.saw_prefix = true;
        continue;
    }
    output.push(*byte);
    continue;
}
```

In the prefix match, add:

```rust
b'q' => {
    if !output.is_empty() {
        actions.push(LiveSnapshotInputAction::Forward(std::mem::take(&mut output)));
    }
    state.selecting_pane = true;
    actions.push(LiveSnapshotInputAction::ShowPaneNumbers);
    continue;
}
```

Update existing translator tests to create `LiveSnapshotInputState::default()`
instead of a `saw_prefix` boolean. Update `finish_live_snapshot_input` to accept
`&mut LiveSnapshotInputState`, clear `selecting_pane`, and flush a pending
literal prefix only when `state.saw_prefix` is true.

- [x] **Step 4: Run live snapshot unit tests to verify GREEN**

Run:

```bash
cargo test live_snapshot_input_
```

Expected: PASS.

- [x] **Step 5: Commit input state**

Run:

```bash
git add src/client.rs
git commit -m "feat: add attach pane number input state"
```

### Task 2: Render Pane Numbers And Select A Valid Pane

**Files:**
- Modify: `src/client.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Write failing unit tests for pane-number formatting**

Add this test near the existing pane listing parser tests:

```rust
#[test]
fn pane_number_message_brackets_active_pane() {
    let entries = parse_pane_listing("0\t0\n1\t1\n2\t0\n").unwrap();

    assert_eq!(format_pane_number_message(&entries), "panes: 0 [1] 2");
}
```

- [x] **Step 2: Write failing integration test for attach selection**

Add this test after `attach_prefix_o_cycles_active_pane_for_live_input` in
`tests/phase1_cli.rs`:

```rust
#[test]
fn attach_prefix_q_selects_numbered_pane_for_live_input() {
    let socket = unique_socket("attach-pane-number");
    let session = format!("attach-pane-number-{}", std::process::id());
    let base_file = unique_temp_file("attach-pane-number-base");

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
            "printf split-ready; read line; echo split-number:$line; sleep 30",
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
        stdin
            .write_all(b"\x02q0base-number\n")
            .expect("write coalesced numbered selection and base input");
        stdin.flush().expect("flush numbered base input");
    }
    assert!(poll_file_contains(&base_file, "base-number"));
    let panes = poll_active_pane(&socket, &session, 0);
    assert!(panes.lines().any(|line| line == "0\t1"), "{panes:?}");

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin.write_all(b"\x02q1").expect("write numbered split selection");
        stdin.flush().expect("flush numbered split selection");
    }
    let panes = poll_active_pane(&socket, &session, 1);
    assert!(panes.lines().any(|line| line == "1\t1"), "{panes:?}");

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin.write_all(b"split\r").expect("write split input");
        stdin.flush().expect("flush split input");
    }
    let split = poll_capture(&socket, &session, "split-number:split");
    assert!(split.contains("split-number:split"), "{split:?}");

    {
        let stdin = child.stdin.as_mut().expect("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("panes:"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 3: Run tests to verify RED**

Run:

```bash
cargo test pane_number_message_brackets_active_pane
cargo test --test phase1_cli attach_prefix_q_selects_numbered_pane_for_live_input
```

Expected: FAIL because pane-number message formatting and `C-b q` event handling do not exist.

- [x] **Step 4: Implement pane-number helpers and events**

In `src/client.rs`, add:

```rust
const PANE_NUMBER_DISPLAY_DURATION: Duration = Duration::from_millis(1000);
```

Update `LiveSnapshotInputEvent`:

```rust
enum LiveSnapshotInputEvent {
    Forward(Vec<u8>),
    SelectNextPane,
    ShowPaneNumbers,
    SelectPane(usize),
    PauseRedraw(mpsc::Sender<()>),
    RedrawNow,
    Error(String),
    Detach,
    Eof,
}
```

In `spawn_live_snapshot_input_thread`, map the new actions to the new events.

Add helpers near `select_next_pane`:

```rust
fn pane_entries(socket: &Path, session: &str) -> io::Result<Vec<PaneListEntry>> {
    let body = send_control_request(
        socket,
        &protocol::encode_list_panes(session, Some("#{pane.index}\t#{pane.active}")),
    )?;
    let listing = String::from_utf8_lossy(&body);
    parse_pane_listing(&listing)
}

fn format_pane_number_message(entries: &[PaneListEntry]) -> String {
    let indexes = entries
        .iter()
        .map(|entry| {
            if entry.active {
                format!("[{}]", entry.index)
            } else {
                entry.index.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("panes: {indexes}")
}

fn pane_number_message(socket: &Path, session: &str) -> io::Result<String> {
    let entries = pane_entries(socket, session)?;
    if entries.is_empty() {
        return Err(io::Error::other("missing pane"));
    }
    Ok(format_pane_number_message(&entries))
}

fn select_numbered_pane(socket: &Path, session: &str, index: usize) -> io::Result<bool> {
    let entries = pane_entries(socket, session)?;
    if !entries.iter().any(|entry| entry.index == index) {
        return Ok(false);
    }
    let _ = send_control_request(socket, &protocol::encode_select_pane(session, index))?;
    Ok(true)
}
```

Refactor frame rendering:

```rust
fn write_live_snapshot_frame(socket: &Path, session: &str) -> io::Result<()> {
    write_live_snapshot_frame_with_message(socket, session, None)
}

fn write_live_snapshot_frame_with_message(
    socket: &Path,
    session: &str,
    message: Option<&str>,
) -> io::Result<()> {
    let status = read_attach_status_line(socket, session)?;
    let snapshot = read_attach_pane_snapshot(socket, session)?;

    let mut stdout = io::stdout().lock();
    stdout.write_all(CLEAR_SCREEN)?;
    if !status.is_empty() {
        stdout.write_all(format!("{status}\r\n").as_bytes())?;
    }
    if let Some(message) = message {
        stdout.write_all(format!("{message}\r\n").as_bytes())?;
    }
    stdout.write_all(&snapshot)?;
    stdout.flush()
}
```

In `run_live_snapshot_attach`, track:

```rust
let mut pane_number_message: Option<(String, Instant)> = None;
```

Use a helper expression before frame writes:

```rust
let current_pane_message = pane_number_message
    .as_ref()
    .filter(|(_, until)| Instant::now() < *until)
    .map(|(message, _)| message.as_str());
```

Handle events:

```rust
Ok(LiveSnapshotInputEvent::ShowPaneNumbers) => {
    let message = pane_number_message(socket, session)?;
    pane_number_message = Some((message, Instant::now() + PANE_NUMBER_DISPLAY_DURATION));
    let current = pane_number_message.as_ref().map(|(message, _)| message.as_str());
    write_live_snapshot_frame_with_message(socket, session, current)?;
    last_redraw = Instant::now();
}
Ok(LiveSnapshotInputEvent::SelectPane(index)) => {
    let _selected = select_numbered_pane(socket, session, index)?;
    pane_number_message = None;
    if !redraw_paused {
        write_live_snapshot_frame(socket, session)?;
    }
    last_redraw = Instant::now();
}
```

Update timeout and forward-triggered redraw paths to call
`write_live_snapshot_frame_with_message` while the message has not expired.

- [x] **Step 5: Run focused tests to verify GREEN**

Run:

```bash
cargo test pane_number_message_brackets_active_pane
cargo test live_snapshot_input_
cargo test --test phase1_cli attach_prefix_q_selects_numbered_pane_for_live_input
cargo test --test phase1_cli attach_prefix_o_cycles_active_pane_for_live_input
cargo test --test phase1_cli attach_prefix_bracket_copies_active_pane_line_in_multi_pane_attach
```

Expected: PASS.

- [x] **Step 6: Commit pane selection behavior**

Run:

```bash
git add src/client.rs tests/phase1_cli.rs
git commit -m "feat: select attach panes by number"
```

### Task 3: Documentation, Verification, PR, Review, Merge

**Files:**
- Modify: `README.md`
- Modify: `HANDOFF.md`
- Modify: `docs/superpowers/specs/2026-05-12-devmux-numbered-pane-selection-design.md`
- Modify: `docs/superpowers/plans/2026-05-12-devmux-numbered-pane-selection.md`

- [x] **Step 1: Update README**

In `README.md`, update the attach paragraph to mention `C-b q` numbered pane
selection:

```text
Unzoomed multi-pane attach routes input and copy-mode to the server active pane,
handles `C-b d` to detach, `C-b o` to cycle the server active pane, and `C-b q`
followed by a single digit to select a pane by number.
```

Add implemented groundwork:

```text
- attach-time numbered pane selection for polling multi-pane attach
```

Update current limits:

```text
- multi-pane attach live redraw is polling-based and routes input to the server active pane; mouse focus and composed-layout copy-mode are not implemented yet
```

- [ ] **Step 2: Update HANDOFF progress**

Record branch, scope, tests run, PR number after creation, review findings, merge
status, and retrospective notes. Do not stage or commit `HANDOFF.md`.

- [x] **Step 3: Run full verification before commit/PR**

Run:

```bash
cargo fmt --check
git diff --check origin/main
rg -ni "co""dex" .
cargo test
```

Expected: formatting and whitespace checks pass; reserved keyword scan prints no matches; all unit and integration tests pass.

- [x] **Step 4: Commit documentation and plan state**

Run:

```bash
git add README.md src/client.rs tests/phase1_cli.rs docs/superpowers/specs/2026-05-12-devmux-numbered-pane-selection-design.md docs/superpowers/plans/2026-05-12-devmux-numbered-pane-selection.md
git commit -m "docs: document attach pane number selection"
```

- [ ] **Step 5: Push and open PR**

Run:

```bash
git push -u origin devmux-numbered-pane-selection
gh pr create --base main --head devmux-numbered-pane-selection --title "Select attach panes by number" --body $'## Summary\n- Add C-b q pane-number display in unzoomed multi-pane attach.\n- Let the next single digit select a valid pane without forwarding the prefix or digit.\n- Preserve detach, pane cycling, copy-mode, live input, and zoomed raw attach behavior.\n\n## Validation\n- cargo fmt --check\n- git diff --check origin/main\n- reserved keyword scan: no matches\n- cargo test\n\n## Review\n- Critical review will run after PR creation per workflow.'
```

The PR title and body must not contain the reserved assistant/project keyword requested by the user.

- [ ] **Step 6: Run critical subagent review after PR creation**

Dispatch a read-only review subagent against `git diff origin/main` and the PR
metadata with this brief:

```text
Critically review the numbered pane selection PR for correctness. Focus on live snapshot input state transitions, coalesced read ordering, pending selection cancellation, pane-list parsing, invalid digit behavior, redraw/message timing, interaction with C-b d/C-b o/C-b [/C-b C-b, zoomed raw attach preservation, test reliability, README/spec/plan consistency, and whether the PR title/body avoid the reserved keyword. Report only actionable Critical/Important/Minor issues with file and line references.
```

Evaluate each finding technically. Apply valid Critical and Important findings,
rerun full verification, and update `HANDOFF.md`.

- [ ] **Step 7: Merge PR and record retrospective**

After review-driven fixes and verification:

```bash
PR_NUMBER=$(gh pr view --head devmux-numbered-pane-selection --json number -q .number)
gh pr checks "$PR_NUMBER" --watch=false
gh pr merge "$PR_NUMBER" --squash --delete-branch --subject "Select attach panes by number"
git fetch origin --prune
```

If GitHub merges remotely but local branch update fails because `main` is owned
by another worktree, verify the PR state with:

```bash
gh pr view "$PR_NUMBER" --json state,mergedAt,mergeCommit,headRefName
```

Then delete the remote branch if needed, fetch with prune, and record the merge
commit and retrospective in `HANDOFF.md`.
