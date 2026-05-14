# Devmux Event-Driven Redraw Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development to implement this plan task-by-task.
> Follow TDD for every behavior change: write the failing test, run it, then
> implement the smallest passing change.

**Goal:** Replace fixed 100ms polling as the primary unzoomed multi-pane attach
redraw path with a server invalidation stream, while keeping polling fallback
for old daemons and safety.

**Architecture:** Add `ATTACH_EVENTS` as an opt-in event stream that emits
`REDRAW` hints. The client listens for hints and reuses the existing
`STATUS_LINE` + `ATTACH_LAYOUT_SNAPSHOT` redraw path.

**Tech Stack:** Rust standard library, existing Unix socket control protocol,
existing multi-pane snapshot attach loop, integration tests in
`tests/phase1_cli.rs`.

---

### Task 1: Protocol And Server Event Stream

**Files:**
- Modify: `src/protocol.rs`
- Modify: `src/server.rs`
- Test: `src/protocol.rs`
- Test: `src/server.rs`
- Test: `tests/phase1_cli.rs`

- [ ] **Step 1: Write failing protocol test**

Add a protocol round-trip test near existing attach snapshot tests:

```rust
#[test]
fn round_trips_attach_events_request() {
    let line = encode_attach_events("dev");
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::AttachEvents {
            session: "dev".to_string(),
        }
    );
}
```

- [ ] **Step 2: Verify protocol RED**

Run:

```bash
cargo test round_trips_attach_events_request
```

Expected: FAIL because `encode_attach_events` and `Request::AttachEvents` do
not exist.

- [ ] **Step 3: Implement protocol support**

Add:

```rust
Request::AttachEvents { session: String }
```

Add encoder:

```rust
pub fn encode_attach_events(session: &str) -> String {
    format!("ATTACH_EVENTS\t{session}\n")
}
```

Decode:

```rust
["ATTACH_EVENTS", session] => Ok(Request::AttachEvents {
    session: (*session).to_string(),
}),
```

- [ ] **Step 4: Verify protocol GREEN**

Run:

```bash
cargo test round_trips_attach_events_request
```

Expected: PASS.

- [ ] **Step 5: Write failing server unit tests**

Use `UnixStream::pair` for pure unit coverage of the notifier:

```rust
#[test]
fn notify_attach_redraw_writes_redraw_event() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let (server, mut client) = UnixStream::pair().unwrap();
    events.lock().unwrap().push(server);

    notify_attach_redraw(&events);

    let mut buf = [0_u8; 7];
    client.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"REDRAW\n");
}
```

Also add a dead-client cleanup test.

- [ ] **Step 6: Verify server unit RED**

Run:

```bash
cargo test notify_attach_redraw
```

Expected: FAIL because the notifier does not exist.

- [ ] **Step 7: Implement session event registry**

In `src/server.rs`:

1. Add a type alias such as `type AttachEventClients = Arc<Mutex<Vec<UnixStream>>>`.
2. Add `attach_events: AttachEventClients` to `Session`.
3. Add the same `Arc` to `Pane`.
4. Change `spawn_pane` to receive the shared event list.
5. Create the event list before the initial pane in `handle_new`.
6. Reuse `session.attach_event_clients()` for split and new-window panes.
7. Implement `notify_attach_redraw(&AttachEventClients)` that writes
   `REDRAW\n` and drops dead clients.

Keep lock ordering simple: do not write event streams while holding the session
window lock or pane terminal lock.

- [ ] **Step 8: Verify server unit GREEN**

Run:

```bash
cargo test notify_attach_redraw
cargo fmt --check
```

Expected: PASS.

- [ ] **Step 9: Write failing event stream integration tests**

Add focused raw socket tests:

1. `attach_events_stream_sends_initial_redraw`
   - start a detached session;
   - connect raw UnixStream;
   - write `ATTACH_EVENTS`;
   - assert `OK\n`;
   - assert the next line is `REDRAW\n`.

2. `attach_events_stream_redraws_after_pane_output`
   - start a split session whose active pane echoes after input;
   - subscribe to `ATTACH_EVENTS`;
   - consume initial `REDRAW`;
   - send input through `send-keys`;
   - poll/read the event stream until `REDRAW\n`.

3. `attach_events_stream_redraws_after_select_pane`
   - start split session;
   - subscribe and consume initial `REDRAW`;
   - run `select-pane -p 0`;
   - assert another `REDRAW\n`.

Set a read timeout on the raw event stream to avoid hanging.

- [ ] **Step 10: Verify integration RED**

Run the focused tests:

```bash
cargo test --test phase1_cli attach_events_stream -- --test-threads=1
```

Expected: FAIL because the server does not handle `ATTACH_EVENTS`. If sandboxed
server readiness fails before reaching the assertion, rerun with escalation.

- [ ] **Step 11: Implement server handler and notifications**

Add `Request::AttachEvents` dispatch and `handle_attach_events`:

1. Missing session returns `ERR missing session`.
2. Existing session writes `OK\n`.
3. Pushes a cloned stream into the session event registry.
4. Sends an initial `REDRAW\n`.

Call `session.notify_attach_redraw()` after successful:

- `handle_split`
- `handle_select_pane`
- `handle_kill_pane`
- `handle_new_window`
- `handle_select_window`
- `handle_kill_window`
- `handle_zoom_pane`
- `handle_resize`

Call `notify_attach_redraw(&pane.attach_events)` in `start_output_pump` after
`TerminalState::apply_bytes`.

- [ ] **Step 12: Verify server GREEN**

Run:

```bash
cargo test round_trips_attach_events_request
cargo test notify_attach_redraw
cargo test --test phase1_cli attach_events_stream -- --test-threads=1
cargo fmt --check
git diff --check -- src/protocol.rs src/server.rs tests/phase1_cli.rs
```

Expected: PASS. Escalate focused integration tests if sandbox readiness fails.

- [ ] **Step 13: Commit**

Run:

```bash
git add src/protocol.rs src/server.rs tests/phase1_cli.rs
git commit -m "feat: add attach redraw events"
```

### Task 2: Client Event Listener And Redraw Fallback

**Files:**
- Modify: `src/client.rs`
- Test: `src/client.rs`

- [ ] **Step 1: Write failing event parser tests**

Add tests:

```rust
#[test]
fn parses_live_snapshot_redraw_event() {
    assert_eq!(
        parse_live_snapshot_event_line("REDRAW\n").unwrap(),
        LiveSnapshotServerEvent::Redraw
    );
}

#[test]
fn rejects_unknown_live_snapshot_event() {
    assert!(parse_live_snapshot_event_line("OTHER\n").is_err());
}
```

- [ ] **Step 2: Verify parser RED**

Run:

```bash
cargo test live_snapshot_redraw_event
```

Expected: FAIL because parser/event type does not exist.

- [ ] **Step 3: Implement parser and client event variants**

Add:

- `LiveSnapshotServerEvent::Redraw`
- `LiveSnapshotInputEvent::RedrawHint`
- `LiveSnapshotInputEvent::EventStreamReady`
- `LiveSnapshotInputEvent::EventStreamClosed`

`RedrawHint` must not resume paused copy-mode; only `RedrawNow` does that.

- [ ] **Step 4: Verify parser GREEN**

Run:

```bash
cargo test live_snapshot_redraw_event
```

Expected: PASS.

- [ ] **Step 5: Write failing timeout-selection tests**

Extract the redraw wait calculation into a pure helper and test:

```rust
#[test]
fn live_snapshot_timeout_uses_polling_when_events_are_inactive() {
    assert_eq!(
        live_snapshot_redraw_timeout(false, None, Instant::now()),
        LIVE_SNAPSHOT_REDRAW_INTERVAL
    );
}

#[test]
fn live_snapshot_timeout_uses_safety_interval_when_events_are_active() {
    assert_eq!(
        live_snapshot_redraw_timeout(true, None, Instant::now()),
        LIVE_SNAPSHOT_EVENT_SAFETY_REDRAW_INTERVAL
    );
}

#[test]
fn live_snapshot_timeout_shortens_for_message_expiry() {
    let now = Instant::now();
    assert_eq!(
        live_snapshot_redraw_timeout(true, Some(now + Duration::from_millis(25)), now),
        Duration::from_millis(25)
    );
}
```

- [ ] **Step 6: Verify timeout RED**

Run:

```bash
cargo test live_snapshot_timeout_
```

Expected: FAIL because the helper and safety interval do not exist.

- [ ] **Step 7: Implement event listener and timeout helper**

Add a listener thread that:

1. Connects to the socket.
2. Writes `protocol::encode_attach_events(session)`.
3. Reads the first line.
4. On `OK\n`, sends `EventStreamReady`.
5. On unknown request or connection failure, exits silently so polling remains
   active.
6. For each `REDRAW\n`, sends `RedrawHint`.
7. If the stream closes after being ready, sends `EventStreamClosed`.

Change `run_live_snapshot_attach`:

- create one channel shared by stdin and event listener threads;
- start the event listener before or immediately after the initial frame;
- track `event_stream_active`;
- use `live_snapshot_redraw_timeout(event_stream_active, message_expiry, now)`
  for `recv_timeout`;
- on `RedrawHint`, redraw only if not paused;
- on `EventStreamReady`, set event-stream active;
- on `EventStreamClosed`, return to polling fallback.

- [ ] **Step 8: Verify client GREEN**

Run:

```bash
cargo test live_snapshot_redraw_event
cargo test live_snapshot_timeout_
cargo test live_snapshot_input_
cargo test pane_at_mouse_position_subtracts_header_rows
cargo fmt --check
git diff --check -- src/client.rs
```

Expected: PASS.

- [ ] **Step 9: Commit**

Run:

```bash
git add src/client.rs
git commit -m "feat: use attach redraw events"
```

### Task 3: Integration, Docs, And Regression Verification

**Files:**
- Modify: `tests/phase1_cli.rs`
- Modify: `README.md`

- [ ] **Step 1: Add/adjust integration coverage**

Ensure focused coverage includes:

- raw event stream initial redraw;
- raw event stream redraw after pane output;
- raw event stream redraw after status-affecting selection;
- existing live attach output redraw;
- existing copy-mode pause/resume behavior through composed copy-mode tests.

Do not add brittle assertions that require redraw to happen within less than the
old polling interval. Prove the event path at protocol/server level and use
positive behavior checks for attach.

- [ ] **Step 2: Update README**

Update the capabilities/limits sections:

- replace "polling-based live redraw for multi-pane attach" with
  event-driven multi-pane redraw plus fallback wording;
- keep active-pane input routing accurate;
- remove the statement that event-driven live status redraw is not implemented;
- keep the note that attach-time pane splitting is still command-driven unless
  this PR deliberately adds bindings, which it should not.

- [ ] **Step 3: Verification**

Run:

```bash
cargo fmt --check
git diff --check origin/main
cargo test round_trips_attach_events_request
cargo test notify_attach_redraw
cargo test live_snapshot_redraw_event
cargo test live_snapshot_timeout_
cargo test live_snapshot_input_
cargo test pane_at_mouse_position_subtracts_header_rows
cargo test --test phase1_cli attach_events_stream -- --test-threads=1
cargo test --test phase1_cli attach_live_redraws_split_pane_output_after_attach_starts -- --test-threads=1
cargo test --test phase1_cli attach_prefix_bracket_copies_composed_layout_line_in_multi_pane_attach -- --test-threads=1
cargo test --test phase1_cli attach_mouse_click_selects_pane_for_live_input -- --test-threads=1
cargo test
```

Escalate integration/full tests if sandboxed server readiness fails.

- [ ] **Step 4: Commit**

Run:

```bash
git add README.md tests/phase1_cli.rs
git commit -m "docs: document event driven redraw"
```

### PR And Review

- [ ] Push branch `devmux-event-driven-redraw`.
- [ ] Open a PR with summary and verification evidence.
- [ ] Run a post-PR critical review subagent.
- [ ] Assess each finding technically; fix valid Critical/Important findings
      and cheap valid Minor findings.
- [ ] Rerun focused verification after review fixes.
- [ ] Update PR body with review result.
- [ ] Merge PR.
- [ ] Add post-merge retrospective to `HANDOFF.md` for the next continuation.
