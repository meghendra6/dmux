# Devmux Attach Mouse Focus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add mouse-click pane focus in unzoomed multi-pane attach.

**Architecture:** Add a new compatible control request that returns one rendered attach snapshot plus pane regions from the same server render pass. The live attach client stores the latest regions, consumes SGR left-click mouse events in input order, maps terminal coordinates into snapshot-body coordinates, and reuses existing `SELECT_PANE` to change focus.

**Tech Stack:** Rust standard library only, existing protocol request decoder, existing server attach renderer, existing client live snapshot loop, existing SGR mouse parser, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Expose Attach Snapshot Regions Through A New Request

**Files:**
- Modify: `src/protocol.rs`
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [ ] **Step 1: Add failing protocol round-trip test**

Add this test beside `round_trips_attach_snapshot_request` in `src/protocol.rs`:

```rust
#[test]
fn round_trips_attach_layout_snapshot_request() {
    let line = encode_attach_layout_snapshot("dev");
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::AttachLayoutSnapshot {
            session: "dev".to_string(),
        }
    );
}
```

- [ ] **Step 2: Run protocol test to verify RED**

Run:

```bash
cargo test round_trips_attach_layout_snapshot_request
```

Expected: FAIL because `encode_attach_layout_snapshot` and
`Request::AttachLayoutSnapshot` do not exist.

- [ ] **Step 3: Implement protocol request**

In `src/protocol.rs`, add the request variant immediately after
`AttachSnapshot`:

```rust
AttachLayoutSnapshot {
    session: String,
},
```

Add the encoder beside `encode_attach_snapshot`:

```rust
pub fn encode_attach_layout_snapshot(session: &str) -> String {
    format!("ATTACH_LAYOUT_SNAPSHOT\t{session}\n")
}
```

Add the decoder arm after `ATTACH_SNAPSHOT`:

```rust
["ATTACH_LAYOUT_SNAPSHOT", session] => Ok(Request::AttachLayoutSnapshot {
    session: (*session).to_string(),
}),
```

- [ ] **Step 4: Run protocol test to verify GREEN**

Run:

```bash
cargo test round_trips_attach_layout_snapshot_request
```

Expected: PASS.

- [ ] **Step 5: Add failing server unit tests for response body**

In `src/server.rs`, add these tests near the existing
`render_attach_layout_*` tests:

```rust
#[test]
fn format_attach_layout_snapshot_response_includes_regions_and_snapshot() {
    let snapshot = RenderedAttachSnapshot {
        text: "left | right\r\n".to_string(),
        regions: vec![
            PaneRegion {
                pane: 0,
                row_start: 0,
                row_end: 1,
                col_start: 0,
                col_end: 4,
            },
            PaneRegion {
                pane: 1,
                row_start: 0,
                row_end: 1,
                col_start: 7,
                col_end: 12,
            },
        ],
    };

    let body = String::from_utf8(format_attach_layout_snapshot_body(&snapshot)).unwrap();

    assert_eq!(
        body,
        "REGIONS\t2\n\
REGION\t0\t0\t1\t0\t4\n\
REGION\t1\t0\t1\t7\t12\n\
SNAPSHOT\t14\n\
left | right\r\n"
    );
}

#[test]
fn render_attach_pane_snapshot_with_regions_returns_fallback_without_regions() {
    let layout = LayoutNode::Pane(0);
    let panes = vec![
        PaneSnapshot {
            index: 0,
            screen: "base-ready\n".to_string(),
        },
        PaneSnapshot {
            index: 1,
            screen: "split-ready\n".to_string(),
        },
    ];

    let snapshot = render_attach_pane_snapshot_with_regions(&layout, &panes);

    assert!(snapshot.regions.is_empty());
    assert!(snapshot.text.contains("-- pane 0 --"), "{:?}", snapshot.text);
    assert!(snapshot.text.contains("-- pane 1 --"), "{:?}", snapshot.text);
}
```

- [ ] **Step 6: Run server unit tests to verify RED**

Run:

```bash
cargo test format_attach_layout_snapshot_response_includes_regions_and_snapshot
cargo test render_attach_pane_snapshot_with_regions_returns_fallback_without_regions
```

Expected: FAIL because `RenderedAttachSnapshot`,
`format_attach_layout_snapshot_body`, and
`render_attach_pane_snapshot_with_regions` do not exist.

- [ ] **Step 7: Implement rendered snapshot body helpers**

In `src/server.rs`, add the snapshot struct near `RenderedAttachLayout`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedAttachSnapshot {
    text: String,
    regions: Vec<PaneRegion>,
}
```

Replace `render_attach_pane_snapshot` with a wrapper over the new helper:

```rust
fn render_attach_pane_snapshot(layout: &LayoutNode, panes: &[PaneSnapshot]) -> String {
    render_attach_pane_snapshot_with_regions(layout, panes).text
}

fn render_attach_pane_snapshot_with_regions(
    layout: &LayoutNode,
    panes: &[PaneSnapshot],
) -> RenderedAttachSnapshot {
    match render_attach_layout(layout, panes) {
        Some(rendered) => RenderedAttachSnapshot {
            text: render_client_lines(&rendered.lines),
            regions: rendered.regions,
        },
        None => RenderedAttachSnapshot {
            text: render_ordered_pane_sections(panes),
            regions: Vec::new(),
        },
    }
}
```

Add response formatting near the render helpers:

```rust
fn format_attach_layout_snapshot_body(snapshot: &RenderedAttachSnapshot) -> Vec<u8> {
    let mut output = String::new();
    output.push_str("REGIONS\t");
    output.push_str(&snapshot.regions.len().to_string());
    output.push('\n');
    for region in &snapshot.regions {
        output.push_str("REGION\t");
        output.push_str(&region.pane.to_string());
        output.push('\t');
        output.push_str(&region.row_start.to_string());
        output.push('\t');
        output.push_str(&region.row_end.to_string());
        output.push('\t');
        output.push_str(&region.col_start.to_string());
        output.push('\t');
        output.push_str(&region.col_end.to_string());
        output.push('\n');
    }
    output.push_str("SNAPSHOT\t");
    output.push_str(&snapshot.text.as_bytes().len().to_string());
    output.push('\n');

    let mut bytes = output.into_bytes();
    bytes.extend_from_slice(snapshot.text.as_bytes());
    bytes
}
```

- [ ] **Step 8: Add server handler**

In the request dispatch in `src/server.rs`, add:

```rust
Request::AttachLayoutSnapshot { session } => {
    handle_attach_layout_snapshot(&state, &mut stream, &session)
}
```

Add the handler near `handle_attach_snapshot`:

```rust
fn handle_attach_layout_snapshot(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    write_ok(stream)?;
    if let Some(snapshot) = attach_pane_snapshot_with_regions(&session) {
        stream.write_all(&format_attach_layout_snapshot_body(&snapshot))?;
    }
    Ok(())
}

fn attach_pane_snapshot_with_regions(session: &Session) -> Option<RenderedAttachSnapshot> {
    let snapshot = session.attach_layout_snapshot();
    if snapshot.panes.is_empty() {
        return None;
    }

    let panes = snapshot
        .panes
        .into_iter()
        .map(|pane| PaneSnapshot {
            index: pane.index,
            screen: capture_pane_text(&pane.pane, CaptureMode::Screen),
        })
        .collect::<Vec<_>>();

    Some(render_attach_pane_snapshot_with_regions(&snapshot.layout, &panes))
}
```

Keep `attach_pane_snapshot` as the old text-only compatibility path:

```rust
fn attach_pane_snapshot(session: &Session) -> Option<String> {
    attach_pane_snapshot_with_regions(session).map(|snapshot| snapshot.text)
}
```

- [ ] **Step 9: Add raw socket integration test for new response and compatibility**

In `tests/phase1_cli.rs`, add this test after
`attach_multi_pane_keeps_snapshot_handshake_for_client_compatibility`:

```rust
#[test]
fn attach_layout_snapshot_response_includes_regions_without_changing_plain_snapshot() {
    let socket = unique_socket("attach-layout-regions");
    let session = format!("attach-layout-regions-{}", std::process::id());

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
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut stream = UnixStream::connect(&socket).expect("connect socket");
    stream
        .write_all(format!("ATTACH_LAYOUT_SNAPSHOT\t{session}\n").as_bytes())
        .expect("write layout snapshot request");
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    let mut body = String::new();
    stream.read_to_string(&mut body).expect("read layout body");

    assert!(body.starts_with("REGIONS\t2\n"), "{body:?}");
    assert!(body.contains("REGION\t0\t0\t1\t0\t10\n"), "{body:?}");
    assert!(body.contains("REGION\t1\t0\t1\t13\t24\n"), "{body:?}");
    assert!(body.contains("SNAPSHOT\t"), "{body:?}");
    assert!(body.contains("base-ready | split-ready\r\n"), "{body:?}");

    let mut plain = UnixStream::connect(&socket).expect("connect socket");
    plain
        .write_all(format!("ATTACH_SNAPSHOT\t{session}\n").as_bytes())
        .expect("write plain snapshot request");
    assert_eq!(read_socket_line(&mut plain), "OK\n");
    let mut plain_body = String::new();
    plain
        .read_to_string(&mut plain_body)
        .expect("read plain snapshot body");
    assert!(!plain_body.contains("REGIONS\t"), "{plain_body:?}");
    assert!(plain_body.contains("base-ready | split-ready\r\n"), "{plain_body:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [ ] **Step 10: Run focused verification**

Run:

```bash
cargo test round_trips_attach_layout_snapshot_request
cargo test format_attach_layout_snapshot_response_includes_regions_and_snapshot
cargo test render_attach_pane_snapshot_with_regions_returns_fallback_without_regions
cargo test --test phase1_cli attach_layout_snapshot_response_includes_regions_without_changing_plain_snapshot
```

Expected: PASS. The `phase1_cli` test may require escalation in this
environment if sandboxed execution reports server readiness failures.

- [ ] **Step 11: Commit protocol/server foundation**

Run:

```bash
git add src/protocol.rs src/server.rs tests/phase1_cli.rs
git commit -m "feat: expose attach layout regions"
```

### Task 2: Parse Snapshot Regions And Live Mouse Input In Client

**Files:**
- Modify: `src/client.rs`

- [ ] **Step 1: Add failing client parser and mapper tests**

Add these tests near the client tests in `src/client.rs`:

```rust
#[test]
fn parses_attach_layout_snapshot_response() {
    let parsed = parse_attach_layout_snapshot_response(
        b"REGIONS\t1\nREGION\t2\t3\t4\t5\t6\nSNAPSHOT\t7\nabc\r\nxy",
    )
    .unwrap();

    assert_eq!(parsed.snapshot, b"abc\r\nxy");
    assert_eq!(
        parsed.regions,
        vec![AttachPaneRegion {
            pane: 2,
            row_start: 3,
            row_end: 4,
            col_start: 5,
            col_end: 6,
        }]
    );
}

#[test]
fn pane_at_mouse_position_subtracts_header_rows() {
    let regions = vec![AttachPaneRegion {
        pane: 1,
        row_start: 0,
        row_end: 1,
        col_start: 13,
        col_end: 24,
    }];

    assert_eq!(pane_at_mouse_position(&regions, 1, MousePosition { col: 14, row: 2 }), Some(1));
    assert_eq!(pane_at_mouse_position(&regions, 2, MousePosition { col: 14, row: 2 }), None);
}
```

- [ ] **Step 2: Add failing live mouse input tests**

Add:

```rust
#[test]
fn live_snapshot_input_emits_mouse_press_and_trailing_bytes() {
    let mut state = LiveSnapshotInputState::default();

    let actions = translate_live_snapshot_input(b"\x1b[<0;14;2Mtyped", &mut state);

    assert_eq!(
        actions,
        vec![
            LiveSnapshotInputAction::MousePress(MousePosition { col: 14, row: 2 }),
            LiveSnapshotInputAction::Forward(b"typed".to_vec()),
        ]
    );
}

#[test]
fn live_snapshot_input_preserves_order_around_mouse_press() {
    let mut state = LiveSnapshotInputState::default();

    let actions = translate_live_snapshot_input(b"abc\x1b[<0;1;2Mdef", &mut state);

    assert_eq!(
        actions,
        vec![
            LiveSnapshotInputAction::Forward(b"abc".to_vec()),
            LiveSnapshotInputAction::MousePress(MousePosition { col: 1, row: 2 }),
            LiveSnapshotInputAction::Forward(b"def".to_vec()),
        ]
    );
}

#[test]
fn live_snapshot_input_buffers_split_sgr_mouse_event() {
    let mut state = LiveSnapshotInputState::default();

    assert!(translate_live_snapshot_input(b"\x1b[<0;1", &mut state).is_empty());
    assert_eq!(
        translate_live_snapshot_input(b";2M", &mut state),
        vec![LiveSnapshotInputAction::MousePress(MousePosition { col: 1, row: 2 })]
    );
}

#[test]
fn live_snapshot_input_consumes_non_press_mouse_events() {
    let mut state = LiveSnapshotInputState::default();

    assert!(translate_live_snapshot_input(b"\x1b[<0;1;2m", &mut state).is_empty());
    assert!(translate_live_snapshot_input(b"\x1b[<32;1;2M", &mut state).is_empty());
    assert!(translate_live_snapshot_input(b"\x1b[<64;1;2M", &mut state).is_empty());
}
```

- [ ] **Step 3: Run client tests to verify RED**

Run:

```bash
cargo test parses_attach_layout_snapshot_response
cargo test pane_at_mouse_position_subtracts_header_rows
cargo test live_snapshot_input_emits_mouse_press_and_trailing_bytes
cargo test live_snapshot_input_preserves_order_around_mouse_press
cargo test live_snapshot_input_buffers_split_sgr_mouse_event
cargo test live_snapshot_input_consumes_non_press_mouse_events
```

Expected: FAIL because client region parsing, `MousePosition`, and mouse input
actions do not exist.

- [ ] **Step 4: Add client structs and response parser**

In `src/client.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MousePosition {
    col: u16,
    row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AttachPaneRegion {
    pane: usize,
    row_start: usize,
    row_end: usize,
    col_start: usize,
    col_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttachLayoutSnapshotResponse {
    snapshot: Vec<u8>,
    regions: Vec<AttachPaneRegion>,
}
```

Add parser helpers near `read_attach_pane_snapshot`:

```rust
fn read_attach_layout_snapshot(
    socket: &Path,
    session: &str,
) -> io::Result<AttachLayoutSnapshotResponse> {
    let body = send_control_request(socket, &protocol::encode_attach_layout_snapshot(session))?;
    parse_attach_layout_snapshot_response(&body)
}

fn parse_attach_layout_snapshot_response(body: &[u8]) -> io::Result<AttachLayoutSnapshotResponse> {
    let mut cursor = 0;
    let header = read_body_line(body, &mut cursor)?;
    let Some(count) = header.strip_prefix("REGIONS\t") else {
        return Err(io::Error::other("missing regions header"));
    };
    let count = count
        .parse::<usize>()
        .map_err(|_| io::Error::other("invalid region count"))?;

    let mut regions = Vec::with_capacity(count);
    for _ in 0..count {
        let line = read_body_line(body, &mut cursor)?;
        regions.push(parse_attach_pane_region(line)?);
    }

    let snapshot = read_body_line(body, &mut cursor)?;
    let Some(len) = snapshot.strip_prefix("SNAPSHOT\t") else {
        return Err(io::Error::other("missing snapshot header"));
    };
    let len = len
        .parse::<usize>()
        .map_err(|_| io::Error::other("invalid snapshot length"))?;
    if body.len().saturating_sub(cursor) < len {
        return Err(io::Error::other("truncated snapshot body"));
    }

    Ok(AttachLayoutSnapshotResponse {
        snapshot: body[cursor..cursor + len].to_vec(),
        regions,
    })
}

fn read_body_line<'a>(body: &'a [u8], cursor: &mut usize) -> io::Result<&'a str> {
    let Some(relative_end) = body[*cursor..].iter().position(|byte| *byte == b'\n') else {
        return Err(io::Error::other("missing response line"));
    };
    let start = *cursor;
    let end = start + relative_end;
    *cursor = end + 1;
    std::str::from_utf8(&body[start..end]).map_err(|_| io::Error::other("non-utf8 response line"))
}

fn parse_attach_pane_region(line: &str) -> io::Result<AttachPaneRegion> {
    let parts = line.split('\t').collect::<Vec<_>>();
    match parts.as_slice() {
        ["REGION", pane, row_start, row_end, col_start, col_end] => Ok(AttachPaneRegion {
            pane: parse_region_field(pane, "invalid region pane")?,
            row_start: parse_region_field(row_start, "invalid region row start")?,
            row_end: parse_region_field(row_end, "invalid region row end")?,
            col_start: parse_region_field(col_start, "invalid region col start")?,
            col_end: parse_region_field(col_end, "invalid region col end")?,
        }),
        _ => Err(io::Error::other("invalid region line")),
    }
}

fn parse_region_field(value: &str, message: &str) -> io::Result<usize> {
    value
        .parse::<usize>()
        .map_err(|_| io::Error::other(message))
}
```

- [ ] **Step 5: Add mouse action and buffering**

Update `LiveSnapshotInputAction`:

```rust
MousePress(MousePosition),
```

Update `LiveSnapshotInputState`:

```rust
mouse_pending: Vec<u8>,
```

At the start of `translate_live_snapshot_input`, merge pending bytes:

```rust
let mut bytes = Vec::new();
if !state.mouse_pending.is_empty() {
    bytes.extend_from_slice(&state.mouse_pending);
    state.mouse_pending.clear();
}
bytes.extend_from_slice(input);
```

Iterate over `bytes` with an offset instead of directly over `input`. Before
normal prefix handling, process SGR mouse sequences:

```rust
if bytes[offset] == 0x1b {
    let remaining = &bytes[offset..];
    if let Some((event, consumed)) = parse_sgr_mouse_event(remaining) {
        if !output.is_empty() {
            actions.push(LiveSnapshotInputAction::Forward(std::mem::take(&mut output)));
        }
        if let Some(position) = live_mouse_press_position(event) {
            actions.push(LiveSnapshotInputAction::MousePress(position));
        }
        offset += consumed;
        continue;
    }
    if is_incomplete_sgr_mouse_event(remaining) {
        if !output.is_empty() {
            actions.push(LiveSnapshotInputAction::Forward(std::mem::take(&mut output)));
        }
        state.mouse_pending.extend_from_slice(remaining);
        break;
    }
}
```

Add:

```rust
fn live_mouse_press_position(event: SgrMouseEvent) -> Option<MousePosition> {
    if event.release || event.col == 0 || event.row == 0 {
        return None;
    }
    if event.code & 64 != 0 || event.code & 32 != 0 || event.code & 3 != 0 {
        return None;
    }
    Some(MousePosition {
        col: event.col,
        row: event.row,
    })
}
```

Update `finish_live_snapshot_input` to clear `mouse_pending`.

- [ ] **Step 6: Add region hit testing**

Add:

```rust
fn pane_at_mouse_position(
    regions: &[AttachPaneRegion],
    header_rows: usize,
    position: MousePosition,
) -> Option<usize> {
    let row = usize::from(position.row).checked_sub(1 + header_rows)?;
    let col = usize::from(position.col).checked_sub(1)?;
    regions
        .iter()
        .find(|region| {
            row >= region.row_start
                && row < region.row_end
                && col >= region.col_start
                && col < region.col_end
        })
        .map(|region| region.pane)
}
```

- [ ] **Step 7: Run client tests to verify GREEN**

Run:

```bash
cargo test parses_attach_layout_snapshot_response
cargo test pane_at_mouse_position_subtracts_header_rows
cargo test live_snapshot_input_emits_mouse_press_and_trailing_bytes
cargo test live_snapshot_input_preserves_order_around_mouse_press
cargo test live_snapshot_input_buffers_split_sgr_mouse_event
cargo test live_snapshot_input_consumes_non_press_mouse_events
cargo test live_snapshot_input_
```

Expected: PASS.

- [ ] **Step 8: Commit client parser/input foundation**

Run:

```bash
git add src/client.rs
git commit -m "feat: parse attach mouse focus input"
```

### Task 3: Select Panes From Live Mouse Clicks

**Files:**
- Modify: `src/client.rs`
- Modify: `tests/phase1_cli.rs`
- Modify: `README.md`

- [ ] **Step 1: Add failing integration test for clicking base pane**

Add this test near the attach live input tests in `tests/phase1_cli.rs`:

```rust
#[test]
fn attach_mouse_click_selects_pane_for_live_input() {
    let socket = unique_socket("attach-mouse-focus");
    let session = format!("attach-mouse-focus-{}", std::process::id());
    let base_file = unique_temp_file("attach-mouse-focus-base");

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
            "printf split-ready; read line; echo split-mouse:$line; sleep 30",
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
            .write_all(b"\x1b[<0;1;2Mbase-mouse\n")
            .expect("write mouse click and base input");
        stdin.flush().expect("flush mouse input");
    }

    assert!(poll_file_contains(&base_file, "base-mouse"));
    let panes = poll_active_pane(&socket, &session, 0);
    assert!(panes.lines().any(|line| line == "0\t1"), "{panes:?}");

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

- [ ] **Step 2: Add failing integration test for coalesced ordering**

Add:

```rust
#[test]
fn attach_mouse_click_preserves_forwarded_input_before_focus_change() {
    let socket = unique_socket("attach-mouse-order");
    let session = format!("attach-mouse-order-{}", std::process::id());
    let base_file = unique_temp_file("attach-mouse-order-base");

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
            "printf split-ready; read line; echo split-before:$line; sleep 30",
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
            .write_all(b"split\x0d\x1b[<0;1;2Mbase-after\n")
            .expect("write coalesced split input mouse click and base input");
        stdin.flush().expect("flush coalesced mouse input");
    }

    assert!(poll_file_contains(&base_file, "base-after"));
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "1"]));
    let split = poll_capture(&socket, &session, "split-before:split");
    assert!(split.contains("split-before:split"), "{split:?}");

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

- [ ] **Step 3: Add failing integration test for separator click**

Add:

```rust
#[test]
fn attach_mouse_click_on_separator_keeps_active_pane() {
    let socket = unique_socket("attach-mouse-separator");
    let session = format!("attach-mouse-separator-{}", std::process::id());

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
            "printf split-ready; read line; echo split-separator:$line; sleep 30",
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
            .write_all(b"\x1b[<0;12;2Mstill\r")
            .expect("write separator click and split input");
        stdin.flush().expect("flush separator input");
    }

    let panes = poll_active_pane(&socket, &session, 1);
    assert!(panes.lines().any(|line| line == "1\t1"), "{panes:?}");
    let split = poll_capture(&socket, &session, "split-separator:still");
    assert!(split.contains("split-separator:still"), "{split:?}");

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

- [ ] **Step 4: Run integration tests to verify RED**

Run:

```bash
cargo test --test phase1_cli attach_mouse_click_selects_pane_for_live_input
cargo test --test phase1_cli attach_mouse_click_preserves_forwarded_input_before_focus_change
cargo test --test phase1_cli attach_mouse_click_on_separator_keeps_active_pane
```

Expected: FAIL because live attach does not yet enable mouse reporting, parse
mouse focus actions, or select panes by clicked regions. These commands may
need escalation if sandboxed execution reports server readiness failures.

- [ ] **Step 5: Wire live attach to snapshot regions**

Change `write_live_snapshot_frame_with_message` to return the latest frame:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveSnapshotFrame {
    regions: Vec<AttachPaneRegion>,
    header_rows: usize,
}
```

Update `write_live_snapshot_frame_with_message` so it calls
`read_attach_layout_snapshot`, writes `snapshot.snapshot`, and returns:

```rust
Ok(LiveSnapshotFrame {
    regions: snapshot.regions,
    header_rows,
})
```

Compute `header_rows` as `1` when the status string is non-empty plus `1` when a
message is rendered.

Keep `read_attach_pane_snapshot` only for the old compatibility request and its
tests.

- [ ] **Step 6: Add mouse event handling in the live loop**

Update `LiveSnapshotInputEvent`:

```rust
MousePress(MousePosition),
```

Send that event from `spawn_live_snapshot_input_thread`.

In `run_live_snapshot_attach`, keep:

```rust
let _mouse = MouseModeGuard::enable()?;
let mut frame = write_live_snapshot_frame(socket, session)?;
```

Update every redraw call to assign `frame = ...?`.

Handle mouse press:

```rust
Ok(LiveSnapshotInputEvent::MousePress(position)) => {
    if let Some(pane) = pane_at_mouse_position(&frame.regions, frame.header_rows, position) {
        let _ = select_numbered_pane(socket, session, pane)?;
        pane_number_message = None;
        if !redraw_paused {
            frame = write_live_snapshot_frame(socket, session)?;
        }
        last_redraw = Instant::now();
    }
}
```

- [ ] **Step 7: Make mouse guard nesting safe**

Add a process-local depth counter near `WINCH_PENDING`:

```rust
static MOUSE_MODE_DEPTH: AtomicUsize = AtomicUsize::new(0);
```

Import `AtomicUsize`.

Update `MouseModeGuard::enable`:

```rust
fn enable() -> io::Result<Self> {
    if MOUSE_MODE_DEPTH.fetch_add(1, Ordering::SeqCst) == 0 {
        let mut stdout = io::stdout().lock();
        stdout.write_all(ENABLE_MOUSE_MODE)?;
        stdout.flush()?;
    }
    Ok(Self)
}
```

Update `Drop`:

```rust
fn drop(&mut self) {
    if MOUSE_MODE_DEPTH.fetch_sub(1, Ordering::SeqCst) == 1 {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(DISABLE_MOUSE_MODE);
        let _ = stdout.flush();
    }
}
```

- [ ] **Step 8: Update README**

In `README.md`, change the attach behavior paragraph to include mouse focus:

```text
`C-b o` to cycle the server active pane, `C-b q` followed by a single digit to
select a pane by number, mouse click to select a pane, and `C-b [` to enter
basic copy-mode for the current server active pane.
```

Add implemented groundwork:

```text
- attach-time mouse focus for polling multi-pane attach
```

Update current limits:

```text
- multi-pane attach live redraw is polling-based and routes input to the server active pane; composed-layout copy-mode is not implemented yet
```

- [ ] **Step 9: Run focused verification**

Run:

```bash
cargo test live_snapshot_input_
cargo test parses_attach_layout_snapshot_response
cargo test pane_at_mouse_position_subtracts_header_rows
cargo test --test phase1_cli attach_mouse_click_selects_pane_for_live_input
cargo test --test phase1_cli attach_mouse_click_preserves_forwarded_input_before_focus_change
cargo test --test phase1_cli attach_mouse_click_on_separator_keeps_active_pane
cargo test --test phase1_cli attach_prefix_bracket_copies_active_pane_line_in_multi_pane_attach
cargo test --test phase1_cli attach_prefix_q_selects_numbered_pane_for_live_input
cargo test --test phase1_cli attach_prefix_o_cycles_active_pane_for_live_input
```

Expected: PASS. The `phase1_cli` tests may require escalation in this
environment if sandboxed execution reports server readiness failures.

- [ ] **Step 10: Commit live mouse focus**

Run:

```bash
git add src/client.rs tests/phase1_cli.rs README.md
git commit -m "feat: select attach panes by mouse"
```

### Task 4: Verification, Reviews, PR, Merge

**Files:**
- Modify: `HANDOFF.md`

- [ ] **Step 1: Update HANDOFF progress**

Record branch, scope, tests run, subagent review results, PR number, merge
status, and retrospective notes. Do not stage or commit `HANDOFF.md`.

- [ ] **Step 2: Run full verification before PR**

Run:

```bash
cargo fmt --check
git diff --check origin/main
rg -ni "co""dex" .
cargo test
```

Expected: formatting and whitespace checks pass; reserved keyword scan prints
no matches; all unit and integration tests pass. Use escalation for
`cargo test` if sandboxed PTY/server integration tests fail with server
readiness errors.

- [ ] **Step 3: Run subagent-driven review gates before push**

Dispatch a spec compliance subagent against `git diff origin/main..HEAD` and
the attach mouse focus spec. Fix valid findings and re-review.

After spec compliance passes, dispatch a code-quality subagent against the same
diff. Fix valid Critical/Important findings and re-review if needed.

- [ ] **Step 4: Push and open PR**

Run:

```bash
git push -u origin devmux-attach-mouse-focus
gh pr create --base main --head devmux-attach-mouse-focus --title "Select attach panes by mouse" --body $'## Summary\n- Add a compatible attach layout snapshot response with pane regions.\n- Select panes from SGR mouse clicks in unzoomed multi-pane attach.\n- Preserve existing snapshot, prefix, numbered selection, copy-mode, and zoomed attach behavior.\n\n## Validation\n- cargo fmt --check\n- git diff --check origin/main\n- reserved keyword scan: no matches\n- cargo test\n\n## Review\n- Spec compliance and code-quality subagent reviews completed before PR.\n- Critical review will run after PR creation per workflow.'
```

The PR title and body must not contain the reserved assistant/project keyword
requested by the user.

- [ ] **Step 5: Run required critical subagent review after PR creation**

Dispatch a read-only critical subagent review against `git diff origin/main..HEAD`
and PR metadata. Ask it to focus on:

```text
snapshot compatibility, snapshot-region response parsing, mouse coordinate offsets,
separator/out-of-region clicks, coalesced input ordering, mouse-mode guard nesting,
copy-mode mouse regression, zoomed/raw attach behavior, README/spec/plan consistency,
and PR title/body reserved keyword check.
```

Evaluate each finding technically. Apply valid Critical and Important findings,
rerun full verification, push fixes, and update `HANDOFF.md`.

- [ ] **Step 6: Merge PR and record retrospective**

After review-driven fixes and verification:

```bash
PR_NUMBER=$(gh pr view --head devmux-attach-mouse-focus --json number -q .number)
gh pr checks "$PR_NUMBER" --watch=false
gh pr merge "$PR_NUMBER" --squash --delete-branch --subject "Select attach panes by mouse"
git fetch origin --prune
```

If GitHub merges remotely but local branch update fails because `main` is owned
by another worktree, verify the PR state with:

```bash
gh pr view "$PR_NUMBER" --json state,mergedAt,mergeCommit,headRefName
```

Then delete the remote branch if needed, fetch with prune, and record the merge
commit and retrospective in `HANDOFF.md`.
