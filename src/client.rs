use crate::cli;
use crate::protocol;
use crate::pty::PtySize;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

static WINCH_PENDING: AtomicBool = AtomicBool::new(false);
static MOUSE_MODE_DEPTH: AtomicUsize = AtomicUsize::new(0);
const ENABLE_MOUSE_MODE: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h";
const DISABLE_MOUSE_MODE: &[u8] = b"\x1b[?1006l\x1b[?1002l\x1b[?1000l";
const LIVE_SNAPSHOT_REDRAW_INTERVAL: Duration = Duration::from_millis(100);
const PANE_NUMBER_DISPLAY_DURATION: Duration = Duration::from_millis(1000);
const CLEAR_SCREEN: &[u8] = b"\x1b[2J\x1b[H";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachMode {
    Live,
    Snapshot,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveSnapshotFrame {
    regions: Vec<AttachPaneRegion>,
    header_rows: usize,
}

pub fn attach<F>(
    socket: &Path,
    session: &str,
    initial_size: Option<PtySize>,
    mut on_resize: F,
) -> io::Result<()>
where
    F: FnMut(PtySize) -> io::Result<()>,
{
    let mut stream = UnixStream::connect(socket)?;
    stream.write_all(protocol::encode_attach(session).as_bytes())?;

    let response = read_line(&mut stream)?;
    if let Some(message) = response.strip_prefix("ERR ") {
        return Err(io::Error::other(message.trim_end().to_string()));
    }
    let attach_mode = parse_attach_ok(&response)?;

    if attach_mode == AttachMode::Snapshot {
        let _guard = RawModeGuard::enable();
        return run_live_snapshot_attach(socket, session, &mut stream);
    }

    write_attach_status_line(socket, session)?;
    let _guard = RawModeGuard::enable();
    install_winch_handler();
    let mut output_stream = stream.try_clone()?;
    let output = std::thread::spawn(move || copy_attach_output(&mut output_stream));

    let mut last_size = initial_size;
    let copy_mode_socket = socket.to_path_buf();
    let copy_mode_session = session.to_string();
    forward_stdin_until_detach(
        &mut stream,
        || {
            if take_winch_pending() {
                maybe_emit_resize(detect_attach_size(), &mut last_size, &mut on_resize)?;
            }
            Ok(())
        },
        |initial_input| run_copy_mode(&copy_mode_socket, &copy_mode_session, initial_input),
    )?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = output.join();
    Ok(())
}

fn copy_attach_output(output_stream: &mut UnixStream) {
    let mut buf = [0_u8; 8192];
    loop {
        let n = match output_stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };

        let mut stdout = io::stdout().lock();
        if stdout.write_all(&buf[..n]).is_err() {
            break;
        }
        let _ = stdout.flush();
    }
}

fn read_attach_status_line(socket: &Path, session: &str) -> io::Result<String> {
    let body = send_control_request(socket, &protocol::encode_status_line(session, None))?;
    Ok(String::from_utf8_lossy(&body).trim_end().to_string())
}

fn read_attach_layout_snapshot(
    socket: &Path,
    session: &str,
) -> io::Result<AttachLayoutSnapshotResponse> {
    match send_control_request(socket, &protocol::encode_attach_layout_snapshot(session)) {
        Ok(body) => parse_attach_layout_snapshot_response(&body),
        Err(error) if is_unknown_request_error(&error) => {
            let snapshot =
                send_control_request(socket, &protocol::encode_attach_snapshot(session))?;
            Ok(AttachLayoutSnapshotResponse {
                snapshot,
                regions: Vec::new(),
            })
        }
        Err(error) => Err(error),
    }
}

fn is_unknown_request_error(error: &io::Error) -> bool {
    error.to_string().starts_with("unknown request line:")
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

    let mut regions = Vec::new();
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
    let remaining = body.len().saturating_sub(cursor);
    if remaining < len {
        return Err(io::Error::other("truncated snapshot body"));
    }
    if remaining > len {
        return Err(io::Error::other("extra snapshot body bytes"));
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

fn write_attach_status_line(socket: &Path, session: &str) -> io::Result<()> {
    let status = read_attach_status_line(socket, session)?;
    if status.is_empty() {
        return Ok(());
    }

    let mut stdout = io::stdout().lock();
    stdout.write_all(format!("{}\r\n", status).as_bytes())?;
    stdout.flush()
}

fn parse_attach_ok(response: &str) -> io::Result<AttachMode> {
    if response == "OK\n" {
        return Ok(AttachMode::Live);
    }

    if response == "OK\tSNAPSHOT\n" {
        return Ok(AttachMode::Snapshot);
    }

    Err(io::Error::other(format!(
        "unexpected server response: {response:?}"
    )))
}

fn write_live_snapshot_frame(socket: &Path, session: &str) -> io::Result<LiveSnapshotFrame> {
    write_live_snapshot_frame_with_message(socket, session, None)
}

fn write_live_snapshot_frame_with_message(
    socket: &Path,
    session: &str,
    message: Option<&str>,
) -> io::Result<LiveSnapshotFrame> {
    let status = read_attach_status_line(socket, session)?;
    let snapshot = read_attach_layout_snapshot(socket, session)?;
    let header_rows = usize::from(!status.is_empty()) + usize::from(message.is_some());

    let mut stdout = io::stdout().lock();
    stdout.write_all(CLEAR_SCREEN)?;
    if !status.is_empty() {
        stdout.write_all(format!("{status}\r\n").as_bytes())?;
    }
    if let Some(message) = message {
        stdout.write_all(format!("{message}\r\n").as_bytes())?;
    }
    stdout.write_all(&snapshot.snapshot)?;
    stdout.flush()?;

    Ok(LiveSnapshotFrame {
        regions: snapshot.regions,
        header_rows,
    })
}

pub fn detect_attach_size() -> Option<PtySize> {
    if let Ok(value) = std::env::var("DEVMUX_ATTACH_SIZE") {
        return parse_attach_size(&value).ok();
    }

    if !stdin_is_tty() {
        return None;
    }

    let output = ProcessCommand::new("stty")
        .arg("size")
        .stdin(Stdio::inherit())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_stty_size(&String::from_utf8_lossy(&output.stdout)).ok()
}

fn parse_attach_size(value: &str) -> io::Result<PtySize> {
    let (cols, rows) = value
        .split_once('x')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "expected <cols>x<rows>"))?;
    let cols = cols
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid columns"))?;
    let rows = rows
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid rows"))?;
    PtySize::new(cols, rows)
}

fn parse_stty_size(value: &str) -> io::Result<PtySize> {
    let mut parts = value.split_whitespace();
    let rows = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing rows"))?
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid rows"))?;
    let cols = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing columns"))?
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid columns"))?;

    if parts.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unexpected extra size fields",
        ));
    }

    PtySize::new(cols, rows)
}

fn maybe_emit_resize<F>(
    current: Option<PtySize>,
    last: &mut Option<PtySize>,
    emit: F,
) -> io::Result<()>
where
    F: FnOnce(PtySize) -> io::Result<()>,
{
    let Some(size) = current else {
        return Ok(());
    };

    if *last == Some(size) {
        return Ok(());
    }

    emit(size)?;
    *last = Some(size);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AttachInputAction {
    Forward(Vec<u8>),
    EnterCopyMode { initial_input: Vec<u8> },
    ShowHelp,
    Detach,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveSnapshotInputAction {
    Forward(Vec<u8>),
    MousePress(MousePosition),
    Detach,
    SelectNextPane,
    ShowPaneNumbers,
    ShowHelp,
    SelectPane(usize),
    EnterCopyMode { initial_input: Vec<u8> },
}

#[derive(Default)]
struct LiveSnapshotInputState {
    saw_prefix: bool,
    selecting_pane: bool,
    mouse_pending: Vec<u8>,
}

fn translate_attach_input(input: &[u8], saw_prefix: &mut bool) -> Vec<AttachInputAction> {
    let mut output = Vec::with_capacity(input.len());
    let mut actions = Vec::new();

    for (index, byte) in input.iter().enumerate() {
        if *saw_prefix {
            *saw_prefix = false;
            match *byte {
                b'd' => {
                    if !output.is_empty() {
                        actions.push(AttachInputAction::Forward(std::mem::take(&mut output)));
                    }
                    actions.push(AttachInputAction::Detach);
                    return actions;
                }
                b'[' => {
                    if !output.is_empty() {
                        actions.push(AttachInputAction::Forward(std::mem::take(&mut output)));
                    }
                    actions.push(AttachInputAction::EnterCopyMode {
                        initial_input: input[index + 1..].to_vec(),
                    });
                    return actions;
                }
                b'?' => {
                    if !output.is_empty() {
                        actions.push(AttachInputAction::Forward(std::mem::take(&mut output)));
                    }
                    actions.push(AttachInputAction::ShowHelp);
                    continue;
                }
                0x02 => {
                    output.push(0x02);
                    *saw_prefix = true;
                    continue;
                }
                _ => output.push(0x02),
            }
        }

        if *byte == 0x02 {
            *saw_prefix = true;
        } else {
            output.push(*byte);
        }
    }

    if !output.is_empty() {
        actions.push(AttachInputAction::Forward(output));
    }
    actions
}

#[cfg(test)]
fn translate_live_snapshot_input(
    input: &[u8],
    state: &mut LiveSnapshotInputState,
) -> Vec<LiveSnapshotInputAction> {
    translate_live_snapshot_input_with_mouse(input, state, true)
}

fn translate_live_snapshot_input_with_mouse(
    input: &[u8],
    state: &mut LiveSnapshotInputState,
    mouse_focus_enabled: bool,
) -> Vec<LiveSnapshotInputAction> {
    let mut bytes = Vec::new();
    if !state.mouse_pending.is_empty() {
        bytes.extend_from_slice(&state.mouse_pending);
        state.mouse_pending.clear();
    }
    bytes.extend_from_slice(input);

    let mut output = Vec::with_capacity(bytes.len());
    let mut actions = Vec::new();
    let mut offset = 0;

    while offset < bytes.len() {
        if mouse_focus_enabled && bytes[offset] == 0x1b {
            let remaining = &bytes[offset..];
            if let Some((event, consumed)) = parse_sgr_mouse_event(remaining) {
                if !output.is_empty() {
                    actions.push(LiveSnapshotInputAction::Forward(std::mem::take(
                        &mut output,
                    )));
                }
                clear_live_snapshot_command_state(state);
                if let Some(position) = live_mouse_press_position(event) {
                    actions.push(LiveSnapshotInputAction::MousePress(position));
                }
                offset += consumed;
                continue;
            }
            if is_incomplete_sgr_mouse_event(remaining) {
                if !output.is_empty() {
                    actions.push(LiveSnapshotInputAction::Forward(std::mem::take(
                        &mut output,
                    )));
                }
                state.mouse_pending.extend_from_slice(remaining);
                break;
            }
            if let Some(consumed) = complete_sgr_mouse_event_len(remaining) {
                if !output.is_empty() {
                    actions.push(LiveSnapshotInputAction::Forward(std::mem::take(
                        &mut output,
                    )));
                }
                clear_live_snapshot_command_state(state);
                offset += consumed;
                continue;
            }
        }

        let byte = bytes[offset];
        if state.selecting_pane {
            state.selecting_pane = false;
            if byte.is_ascii_digit() {
                actions.push(LiveSnapshotInputAction::SelectPane(usize::from(
                    byte - b'0',
                )));
                offset += 1;
                continue;
            }
            if byte == 0x02 {
                state.saw_prefix = true;
                offset += 1;
                continue;
            }
            output.push(byte);
            offset += 1;
            continue;
        }

        if state.saw_prefix {
            state.saw_prefix = false;
            match byte {
                b'd' => {
                    if !output.is_empty() {
                        actions.push(LiveSnapshotInputAction::Forward(std::mem::take(
                            &mut output,
                        )));
                    }
                    actions.push(LiveSnapshotInputAction::Detach);
                    return actions;
                }
                b'o' => {
                    if !output.is_empty() {
                        actions.push(LiveSnapshotInputAction::Forward(std::mem::take(
                            &mut output,
                        )));
                    }
                    actions.push(LiveSnapshotInputAction::SelectNextPane);
                    offset += 1;
                    continue;
                }
                b'q' => {
                    if !output.is_empty() {
                        actions.push(LiveSnapshotInputAction::Forward(std::mem::take(
                            &mut output,
                        )));
                    }
                    state.selecting_pane = true;
                    actions.push(LiveSnapshotInputAction::ShowPaneNumbers);
                    offset += 1;
                    continue;
                }
                b'?' => {
                    if !output.is_empty() {
                        actions.push(LiveSnapshotInputAction::Forward(std::mem::take(
                            &mut output,
                        )));
                    }
                    actions.push(LiveSnapshotInputAction::ShowHelp);
                    offset += 1;
                    continue;
                }
                b'[' => {
                    if !output.is_empty() {
                        actions.push(LiveSnapshotInputAction::Forward(std::mem::take(
                            &mut output,
                        )));
                    }
                    actions.push(LiveSnapshotInputAction::EnterCopyMode {
                        initial_input: bytes[offset + 1..].to_vec(),
                    });
                    return actions;
                }
                0x02 => {
                    output.push(0x02);
                    offset += 1;
                    continue;
                }
                _ => {
                    output.push(0x02);
                    output.push(byte);
                    offset += 1;
                    continue;
                }
            }
        }

        if byte == 0x02 {
            state.saw_prefix = true;
        } else {
            output.push(byte);
        }
        offset += 1;
    }

    if !output.is_empty() {
        actions.push(LiveSnapshotInputAction::Forward(output));
    }
    actions
}

fn clear_live_snapshot_command_state(state: &mut LiveSnapshotInputState) {
    state.saw_prefix = false;
    state.selecting_pane = false;
}

fn finish_live_snapshot_input(state: &mut LiveSnapshotInputState) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    if state.saw_prefix {
        state.saw_prefix = false;
        output.push(0x02);
    }
    state.selecting_pane = false;
    if !state.mouse_pending.is_empty() {
        output.extend(std::mem::take(&mut state.mouse_pending));
    }

    if output.is_empty() {
        return None;
    }
    Some(output)
}

fn live_mouse_press_position(event: SgrMouseEvent) -> Option<MousePosition> {
    if event.release || event.col == 0 || event.row == 0 {
        return None;
    }
    if event.code != 0 {
        return None;
    }
    Some(MousePosition {
        col: event.col,
        row: event.row,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PaneListEntry {
    index: usize,
    active: bool,
}

#[cfg(test)]
fn next_pane_index_from_listing(listing: &str) -> io::Result<usize> {
    let entries = parse_pane_listing(listing)?;
    next_pane_index_from_entries(&entries)
}

fn next_pane_index_from_entries(entries: &[PaneListEntry]) -> io::Result<usize> {
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

#[derive(Debug)]
enum LiveSnapshotInputEvent {
    Forward(Vec<u8>),
    MousePress(MousePosition),
    SelectNextPane,
    ShowPaneNumbers,
    ShowHelp,
    SelectPane(usize),
    PauseRedraw(mpsc::Sender<()>),
    RedrawNow,
    Error(String),
    Detach,
    Eof,
}

fn spawn_live_snapshot_input_thread(
    socket: PathBuf,
    session: String,
    mouse_focus_enabled: Arc<AtomicBool>,
) -> mpsc::Receiver<LiveSnapshotInputEvent> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = [0_u8; 1024];
        let mut input_state = LiveSnapshotInputState::default();

        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) => {
                    if let Some(bytes) = finish_live_snapshot_input(&mut input_state) {
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

            let mouse_focus_enabled = mouse_focus_enabled.load(Ordering::SeqCst);
            for action in translate_live_snapshot_input_with_mouse(
                &buf[..n],
                &mut input_state,
                mouse_focus_enabled,
            ) {
                match action {
                    LiveSnapshotInputAction::Forward(bytes) => {
                        let _ = sender.send(LiveSnapshotInputEvent::Forward(bytes));
                    }
                    LiveSnapshotInputAction::Detach => {
                        let _ = sender.send(LiveSnapshotInputEvent::Detach);
                        return;
                    }
                    LiveSnapshotInputAction::SelectNextPane => {
                        let _ = sender.send(LiveSnapshotInputEvent::SelectNextPane);
                    }
                    LiveSnapshotInputAction::ShowPaneNumbers => {
                        let _ = sender.send(LiveSnapshotInputEvent::ShowPaneNumbers);
                    }
                    LiveSnapshotInputAction::ShowHelp => {
                        let _ = sender.send(LiveSnapshotInputEvent::ShowHelp);
                    }
                    LiveSnapshotInputAction::SelectPane(index) => {
                        let _ = sender.send(LiveSnapshotInputEvent::SelectPane(index));
                    }
                    LiveSnapshotInputAction::MousePress(position) => {
                        let _ = sender.send(LiveSnapshotInputEvent::MousePress(position));
                    }
                    LiveSnapshotInputAction::EnterCopyMode { initial_input } => {
                        let (pause_ack, pause_ready) = mpsc::channel();
                        let _ = sender.send(LiveSnapshotInputEvent::PauseRedraw(pause_ack));
                        let _ = pause_ready.recv();
                        match run_copy_mode_with_reader(
                            &socket,
                            &session,
                            &initial_input,
                            &mut stdin,
                        ) {
                            Ok(()) => {
                                let _ = sender.send(LiveSnapshotInputEvent::RedrawNow);
                            }
                            Err(error) => {
                                let _ =
                                    sender.send(LiveSnapshotInputEvent::Error(error.to_string()));
                                return;
                            }
                        }
                    }
                }
            }
        }
    });
    receiver
}

fn run_live_snapshot_attach(
    socket: &Path,
    session: &str,
    stream: &mut UnixStream,
) -> io::Result<()> {
    let mouse_focus_enabled = Arc::new(AtomicBool::new(false));
    let input = spawn_live_snapshot_input_thread(
        socket.to_path_buf(),
        session.to_string(),
        Arc::clone(&mouse_focus_enabled),
    );
    let mut frame = write_live_snapshot_frame(socket, session)?;
    let mut mouse_mode = None;
    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
    let mut last_redraw = Instant::now();
    let mut redraw_paused = false;
    let mut pane_number_message = None;

    loop {
        match input.recv_timeout(LIVE_SNAPSHOT_REDRAW_INTERVAL) {
            Ok(LiveSnapshotInputEvent::Forward(bytes)) => {
                forward_live_snapshot_input(socket, session, &bytes)?;
                if !redraw_paused && last_redraw.elapsed() >= LIVE_SNAPSHOT_REDRAW_INTERVAL {
                    frame = write_live_snapshot_frame_with_pane_number_message(
                        socket,
                        session,
                        &mut pane_number_message,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    last_redraw = Instant::now();
                }
            }
            Ok(LiveSnapshotInputEvent::SelectNextPane) => {
                select_next_pane(socket, session)?;
                pane_number_message = None;
                if !redraw_paused {
                    frame = write_live_snapshot_frame(socket, session)?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                }
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::ShowPaneNumbers) => {
                let message = pane_number_message_text(socket, session)?;
                pane_number_message =
                    Some((message, Instant::now() + PANE_NUMBER_DISPLAY_DURATION));
                frame = write_live_snapshot_frame_with_pane_number_message(
                    socket,
                    session,
                    &mut pane_number_message,
                )?;
                sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::ShowHelp) => {
                pane_number_message = Some((
                    attach_help_message().to_string(),
                    Instant::now() + PANE_NUMBER_DISPLAY_DURATION,
                ));
                frame = write_live_snapshot_frame_with_pane_number_message(
                    socket,
                    session,
                    &mut pane_number_message,
                )?;
                sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::SelectPane(index)) => {
                let _ = select_numbered_pane(socket, session, index)?;
                pane_number_message = None;
                if !redraw_paused {
                    frame = write_live_snapshot_frame(socket, session)?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                }
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::MousePress(position)) => {
                if let Some(pane) =
                    pane_at_mouse_position(&frame.regions, frame.header_rows, position)
                {
                    let _ = select_numbered_pane(socket, session, pane)?;
                    pane_number_message = None;
                    if !redraw_paused {
                        frame = write_live_snapshot_frame(socket, session)?;
                        sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    }
                    last_redraw = Instant::now();
                }
            }
            Ok(LiveSnapshotInputEvent::PauseRedraw(pause_ack)) => {
                redraw_paused = true;
                let _ = pause_ack.send(());
            }
            Ok(LiveSnapshotInputEvent::RedrawNow) => {
                redraw_paused = false;
                frame = write_live_snapshot_frame(socket, session)?;
                sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::Error(message)) => return Err(io::Error::other(message)),
            Ok(LiveSnapshotInputEvent::Detach) | Ok(LiveSnapshotInputEvent::Eof) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !redraw_paused {
                    frame = write_live_snapshot_frame_with_pane_number_message(
                        socket,
                        session,
                        &mut pane_number_message,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    last_redraw = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = stream.shutdown(std::net::Shutdown::Both);
    Ok(())
}

fn sync_live_mouse_mode(
    mouse_focus_enabled: &AtomicBool,
    mouse_mode: &mut Option<MouseModeGuard>,
    frame: &LiveSnapshotFrame,
) -> io::Result<()> {
    let enabled = !frame.regions.is_empty();
    if enabled {
        if mouse_mode.is_none() {
            *mouse_mode = Some(MouseModeGuard::enable()?);
        }
        mouse_focus_enabled.store(true, Ordering::SeqCst);
    } else {
        mouse_focus_enabled.store(false, Ordering::SeqCst);
        *mouse_mode = None;
    }
    Ok(())
}

fn forward_live_snapshot_input(socket: &Path, session: &str, bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }

    let _ = send_control_request(socket, &protocol::encode_send(session, bytes))?;
    Ok(())
}

fn write_live_snapshot_frame_with_pane_number_message(
    socket: &Path,
    session: &str,
    pane_number_message: &mut Option<(String, Instant)>,
) -> io::Result<LiveSnapshotFrame> {
    if pane_number_message
        .as_ref()
        .is_some_and(|(_, until)| Instant::now() >= *until)
    {
        *pane_number_message = None;
    }

    let message = pane_number_message
        .as_ref()
        .map(|(message, _)| message.as_str());
    write_live_snapshot_frame_with_message(socket, session, message)
}

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

fn pane_number_message_text(socket: &Path, session: &str) -> io::Result<String> {
    let entries = pane_entries(socket, session)?;
    if entries.is_empty() {
        return Err(io::Error::other("missing pane"));
    }
    Ok(format_pane_number_message(&entries))
}

fn pane_index_exists(entries: &[PaneListEntry], index: usize) -> bool {
    entries.iter().any(|entry| entry.index == index)
}

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

fn select_numbered_pane(socket: &Path, session: &str, index: usize) -> io::Result<bool> {
    let entries = pane_entries(socket, session)?;
    if entries.is_empty() {
        return Err(io::Error::other("missing pane"));
    }
    if !pane_index_exists(&entries, index) {
        return Ok(false);
    }

    let _ = send_control_request(socket, &protocol::encode_select_pane(session, index))?;
    Ok(true)
}

fn select_next_pane(socket: &Path, session: &str) -> io::Result<()> {
    let entries = pane_entries(socket, session)?;
    let next = next_pane_index_from_entries(&entries)?;
    let _ = send_control_request(socket, &protocol::encode_select_pane(session, next))?;
    Ok(())
}

fn forward_stdin_until_detach<F, C>(
    stream: &mut UnixStream,
    mut tick: F,
    mut enter_copy_mode: C,
) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
    C: FnMut(&[u8]) -> io::Result<()>,
{
    let mut buf = [0_u8; 1024];
    let mut saw_prefix = false;

    loop {
        tick()?;
        let n = io::stdin().lock().read(&mut buf)?;
        if n == 0 {
            break;
        }

        for action in translate_attach_input(&buf[..n], &mut saw_prefix) {
            match action {
                AttachInputAction::Forward(output) => stream.write_all(&output)?,
                AttachInputAction::EnterCopyMode { initial_input } => {
                    enter_copy_mode(&initial_input)?;
                }
                AttachInputAction::ShowHelp => {
                    write_attach_help_message()?;
                }
                AttachInputAction::Detach => return Ok(()),
            }
        }
    }

    if saw_prefix {
        stream.write_all(&[0x02])?;
    }

    Ok(())
}

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

fn handle_copy_mode_input(
    socket: &Path,
    session: &str,
    view: &mut CopyModeView,
    input_state: &mut CopyModeInputState,
    input: &[u8],
) -> io::Result<bool> {
    for action in input_state.apply(view, input) {
        if apply_copy_mode_action(socket, session, view, action)? {
            return Ok(true);
        }
    }

    Ok(false)
}

fn apply_copy_mode_action(
    socket: &Path,
    session: &str,
    view: &mut CopyModeView,
    action: CopyModeAction,
) -> io::Result<bool> {
    match action {
        CopyModeAction::Redraw => {
            write_copy_mode_view(view)?;
            Ok(false)
        }
        CopyModeAction::CopyLine(line) => {
            save_copy_mode_range(socket, session, line, line)?;
            Ok(true)
        }
        CopyModeAction::CopyLineRange { start, end } => {
            save_copy_mode_range(socket, session, start, end)?;
            Ok(true)
        }
        CopyModeAction::Exit => {
            write_copy_mode_message("exit")?;
            Ok(true)
        }
        CopyModeAction::Ignore => Ok(false),
    }
}

fn save_copy_mode_range(socket: &Path, session: &str, start: usize, end: usize) -> io::Result<()> {
    let body = send_control_request(
        socket,
        &protocol::encode_save_buffer(
            session,
            None,
            protocol::CaptureMode::All,
            protocol::BufferSelection::LineRange { start, end },
        ),
    )?;
    let saved = String::from_utf8_lossy(&body);
    let saved = saved.trim_end();
    if saved.is_empty() {
        write_copy_mode_message("copied")?;
    } else {
        write_copy_mode_message(&format!("copied to {saved}"))?;
    }
    Ok(())
}

fn write_copy_mode_view(view: &CopyModeView) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(view.render().as_bytes())?;
    stdout.flush()
}

fn write_copy_mode_message(message: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(format!("\r\n-- copy mode: {message} --\r\n").as_bytes())?;
    stdout.flush()
}

fn attach_help_message() -> &'static str {
    cli::attach_help_summary()
}

fn write_attach_help_message() -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(format!("\r\n-- dmux help: {} --\r\n", attach_help_message()).as_bytes())?;
    stdout.flush()
}

fn send_control_request(socket: &Path, line: &str) -> io::Result<Vec<u8>> {
    let mut stream = UnixStream::connect(socket)?;
    stream.write_all(line.as_bytes())?;

    let response = read_line(&mut stream)?;
    if let Some(message) = response.strip_prefix("ERR ") {
        return Err(io::Error::other(message.trim_end().to_string()));
    }
    if response != "OK\n" {
        return Err(io::Error::other(format!(
            "unexpected server response: {response:?}"
        )));
    }

    let mut body = Vec::new();
    stream.read_to_end(&mut body)?;
    Ok(body)
}

fn parse_sgr_mouse_event(input: &[u8]) -> Option<(SgrMouseEvent, usize)> {
    if !input.starts_with(b"\x1b[<") {
        return None;
    }

    let tail = &input[3..];
    let end = tail
        .iter()
        .position(|byte| *byte == b'M' || *byte == b'm')?;
    let terminator = tail[end];
    let fields = std::str::from_utf8(&tail[..end]).ok()?;
    let mut parts = fields.split(';');
    let code = parts.next()?.parse::<u16>().ok()?;
    let col = parts.next()?.parse::<u16>().ok()?;
    let row = parts.next()?.parse::<u16>().ok()?;
    if parts.next().is_some() {
        return None;
    }

    Some((
        SgrMouseEvent {
            code,
            col,
            row,
            release: terminator == b'm',
        },
        3 + end + 1,
    ))
}

#[derive(Default)]
struct CopyModeInputState {
    pending: Vec<u8>,
}

impl CopyModeInputState {
    fn apply(&mut self, view: &mut CopyModeView, input: &[u8]) -> Vec<CopyModeAction> {
        let mut bytes = Vec::new();
        if !self.pending.is_empty() {
            bytes.extend_from_slice(&self.pending);
            self.pending.clear();
        }
        bytes.extend_from_slice(input);

        let mut actions = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            if bytes[offset] == 0x1b {
                let remaining = &bytes[offset..];
                if let Some((event, consumed)) = parse_sgr_mouse_event(remaining) {
                    offset += consumed;
                    actions.push(view.apply_mouse_event(event));
                } else if is_incomplete_sgr_mouse_event(remaining) {
                    self.pending.extend_from_slice(remaining);
                    break;
                } else {
                    offset += 1;
                    actions.push(CopyModeAction::Exit);
                }
            } else {
                let byte = bytes[offset];
                offset += 1;
                actions.push(view.apply_key(byte));
            }
        }

        actions
    }
}

fn is_incomplete_sgr_mouse_event(input: &[u8]) -> bool {
    input.starts_with(b"\x1b[<")
        && input.len() < 64
        && !input.iter().any(|byte| *byte == b'M' || *byte == b'm')
        && input[3..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b';')
}

fn complete_sgr_mouse_event_len(input: &[u8]) -> Option<usize> {
    if !input.starts_with(b"\x1b[<") {
        return None;
    }

    input[3..]
        .iter()
        .position(|byte| *byte == b'M' || *byte == b'm')
        .map(|end| 3 + end + 1)
}

fn install_winch_handler() {
    const SIGWINCH: i32 = 28;
    unsafe {
        signal(SIGWINCH, handle_winch);
    }
}

fn take_winch_pending() -> bool {
    WINCH_PENDING.swap(false, Ordering::SeqCst)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CopyModeAction {
    Redraw,
    CopyLine(usize),
    CopyLineRange { start: usize, end: usize },
    Exit,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SgrMouseEvent {
    code: u16,
    col: u16,
    row: u16,
    release: bool,
}

struct CopyModeLine {
    number: usize,
    text: String,
}

struct CopyModeView {
    lines: Vec<CopyModeLine>,
    cursor: usize,
    selection_anchor: Option<usize>,
}

impl CopyModeView {
    fn from_numbered_output(output: &str) -> io::Result<Self> {
        let mut lines = Vec::new();
        for line in output.lines() {
            let Some((number, text)) = line.split_once('\t') else {
                continue;
            };
            let number = number.parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid copy-mode line number")
            })?;
            lines.push(CopyModeLine {
                number,
                text: text.to_string(),
            });
        }

        Ok(Self {
            lines,
            cursor: 0,
            selection_anchor: None,
        })
    }

    fn cursor_line_number(&self) -> Option<usize> {
        self.lines.get(self.cursor).map(|line| line.number)
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn apply_key(&mut self, byte: u8) -> CopyModeAction {
        match byte {
            b'j' | 0x0e => {
                if self.cursor + 1 < self.lines.len() {
                    self.cursor += 1;
                    CopyModeAction::Redraw
                } else {
                    CopyModeAction::Ignore
                }
            }
            b'k' | 0x10 => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    CopyModeAction::Redraw
                } else {
                    CopyModeAction::Ignore
                }
            }
            b'y' | b'\r' | b'\n' => self
                .cursor_line_number()
                .map(CopyModeAction::CopyLine)
                .unwrap_or(CopyModeAction::Ignore),
            b'q' | 0x1b => CopyModeAction::Exit,
            _ => CopyModeAction::Ignore,
        }
    }

    fn apply_mouse_event(&mut self, event: SgrMouseEvent) -> CopyModeAction {
        if event.release {
            let Some(anchor) = self.selection_anchor.take() else {
                return CopyModeAction::Ignore;
            };
            if event.col == 0 {
                return CopyModeAction::Ignore;
            }
            let Some(index) = self.line_index_for_mouse_row(event.row) else {
                return CopyModeAction::Ignore;
            };

            self.cursor = index;
            let (start_index, end_index) = normalized_indexes(anchor, index);
            return CopyModeAction::CopyLineRange {
                start: self.lines[start_index].number,
                end: self.lines[end_index].number,
            };
        }

        if event.col == 0 {
            return CopyModeAction::Ignore;
        }
        let Some(index) = self.line_index_for_mouse_row(event.row) else {
            return CopyModeAction::Ignore;
        };

        if event.code & 32 != 0 {
            if self.selection_anchor.is_some() {
                self.cursor = index;
                return CopyModeAction::Redraw;
            }
            return CopyModeAction::Ignore;
        }

        if event.code & 3 == 0 {
            self.selection_anchor = Some(index);
            self.cursor = index;
            return CopyModeAction::Redraw;
        }

        CopyModeAction::Ignore
    }

    fn line_index_for_mouse_row(&self, row: u16) -> Option<usize> {
        if row < 2 {
            return None;
        }
        let index = usize::from(row) - 2;
        self.lines.get(index).map(|_| index)
    }

    fn selected_bounds(&self) -> Option<(usize, usize)> {
        self.selection_anchor
            .map(|anchor| normalized_indexes(anchor, self.cursor))
    }

    fn render(&self) -> String {
        let mut output = String::from("\x1b[2J\x1b[H-- copy mode --\r\n");
        let selected = self.selected_bounds();
        for (index, line) in self.lines.iter().enumerate() {
            if index == self.cursor {
                output.push('>');
            } else if selected
                .map(|(start, end)| index >= start && index <= end)
                .unwrap_or(false)
            {
                output.push('*');
            } else {
                output.push(' ');
            }
            output.push(' ');
            output.push_str(&line.number.to_string());
            output.push('\t');
            output.push_str(&line.text);
            output.push_str("\r\n");
        }
        output
    }
}

fn normalized_indexes(left: usize, right: usize) -> (usize, usize) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

extern "C" fn handle_winch(_: i32) {
    WINCH_PENDING.store(true, Ordering::SeqCst);
}

struct RawModeGuard {
    saved: Option<String>,
}

struct MouseModeGuard;

impl MouseModeGuard {
    fn enable() -> io::Result<Self> {
        if MOUSE_MODE_DEPTH.fetch_add(1, Ordering::SeqCst) == 0 {
            let result = (|| {
                let mut stdout = io::stdout().lock();
                stdout.write_all(ENABLE_MOUSE_MODE)?;
                stdout.flush()
            })();
            if let Err(error) = result {
                MOUSE_MODE_DEPTH.fetch_sub(1, Ordering::SeqCst);
                return Err(error);
            }
        }
        Ok(Self)
    }
}

impl Drop for MouseModeGuard {
    fn drop(&mut self) {
        if MOUSE_MODE_DEPTH.fetch_sub(1, Ordering::SeqCst) == 1 {
            let mut stdout = io::stdout().lock();
            let _ = stdout.write_all(DISABLE_MOUSE_MODE);
            let _ = stdout.flush();
        }
    }
}

impl RawModeGuard {
    fn enable() -> Self {
        if !stdin_is_tty() {
            return Self { saved: None };
        }

        let saved = ProcessCommand::new("stty")
            .arg("-g")
            .stdin(Stdio::inherit())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty());

        if saved.is_some() {
            let _ = ProcessCommand::new("stty")
                .args(["raw", "-echo"])
                .stdin(Stdio::inherit())
                .status();
        }

        Self { saved }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if let Some(saved) = &self.saved {
            let _ = ProcessCommand::new("stty")
                .arg(saved)
                .stdin(Stdio::inherit())
                .status();
        }
    }
}

fn stdin_is_tty() -> bool {
    unsafe extern "C" {
        fn isatty(fd: i32) -> i32;
    }

    unsafe { isatty(0) == 1 }
}

unsafe extern "C" {
    fn signal(signum: i32, handler: extern "C" fn(i32)) -> extern "C" fn(i32);
}

fn read_line(stream: &mut UnixStream) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];

    loop {
        let n = stream.read(&mut byte)?;
        if n == 0 {
            break;
        }
        bytes.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }

    Ok(String::from_utf8_lossy(&bytes).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_attach_size_override() {
        assert_eq!(
            parse_attach_size("120x40").unwrap(),
            crate::pty::PtySize {
                cols: 120,
                rows: 40
            }
        );
        assert!(parse_attach_size("0x40").is_err());
        assert!(parse_attach_size("120").is_err());
    }

    #[test]
    fn parses_stty_size_output_as_rows_then_cols() {
        assert_eq!(
            parse_stty_size("40 120\n").unwrap(),
            crate::pty::PtySize {
                cols: 120,
                rows: 40
            }
        );
    }

    #[test]
    fn maybe_emit_resize_only_emits_changed_sizes() {
        let mut last = Some(crate::pty::PtySize { cols: 80, rows: 24 });
        let mut emitted = Vec::new();

        maybe_emit_resize(
            Some(crate::pty::PtySize { cols: 80, rows: 24 }),
            &mut last,
            |size| {
                emitted.push(size);
                Ok(())
            },
        )
        .unwrap();
        maybe_emit_resize(
            Some(crate::pty::PtySize {
                cols: 100,
                rows: 40,
            }),
            &mut last,
            |size| {
                emitted.push(size);
                Ok(())
            },
        )
        .unwrap();
        maybe_emit_resize(None, &mut last, |size| {
            emitted.push(size);
            Ok(())
        })
        .unwrap();

        assert_eq!(
            emitted,
            vec![crate::pty::PtySize {
                cols: 100,
                rows: 40
            }]
        );
        assert_eq!(
            last,
            Some(crate::pty::PtySize {
                cols: 100,
                rows: 40
            })
        );
    }

    #[test]
    fn winch_flag_can_be_taken_once() {
        WINCH_PENDING.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(take_winch_pending());
        assert!(!take_winch_pending());
    }

    #[test]
    fn parses_snapshot_attach_ok() {
        assert_eq!(
            parse_attach_ok("OK\tSNAPSHOT\n").unwrap(),
            AttachMode::Snapshot
        );
    }

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
    fn read_attach_layout_snapshot_falls_back_to_plain_snapshot_for_unknown_request() {
        let socket = std::env::temp_dir().join(format!(
            "dmux-layout-fallback-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert_eq!(
                read_line(&mut stream).unwrap(),
                "ATTACH_LAYOUT_SNAPSHOT\tdev\n"
            );
            stream
                .write_all(b"ERR unknown request line: \"ATTACH_LAYOUT_SNAPSHOT\\tdev\\n\"\n")
                .unwrap();

            let (mut stream, _) = listener.accept().unwrap();
            assert_eq!(read_line(&mut stream).unwrap(), "ATTACH_SNAPSHOT\tdev\n");
            stream.write_all(b"OK\nplain snapshot\r\n").unwrap();
        });

        let snapshot = read_attach_layout_snapshot(&socket, "dev").unwrap();

        assert_eq!(snapshot.snapshot, b"plain snapshot\r\n");
        assert!(snapshot.regions.is_empty());
        server.join().unwrap();
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn rejects_extra_bytes_after_attach_layout_snapshot_body() {
        let error =
            parse_attach_layout_snapshot_response(b"REGIONS\t0\nSNAPSHOT\t3\nabcd").unwrap_err();

        assert_eq!(error.to_string(), "extra snapshot body bytes");
    }

    #[test]
    fn rejects_huge_region_count_without_allocating() {
        let body = format!("REGIONS\t{}\n", usize::MAX);

        let result =
            std::panic::catch_unwind(|| parse_attach_layout_snapshot_response(body.as_bytes()));

        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
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

        assert_eq!(
            pane_at_mouse_position(&regions, 1, MousePosition { col: 14, row: 2 }),
            Some(1)
        );
        assert_eq!(
            pane_at_mouse_position(&regions, 2, MousePosition { col: 14, row: 2 }),
            None
        );
    }

    #[test]
    fn live_snapshot_input_forwards_arbitrary_bytes() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"hello\n", &mut state);

        assert_eq!(
            actions,
            vec![LiveSnapshotInputAction::Forward(b"hello\n".to_vec())]
        );
        assert!(!state.saw_prefix);
    }

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
            vec![LiveSnapshotInputAction::MousePress(MousePosition {
                col: 1,
                row: 2
            })]
        );
    }

    #[test]
    fn live_snapshot_input_forwards_standalone_escape_without_waiting() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input(b"\x1b", &mut state),
            vec![LiveSnapshotInputAction::Forward(b"\x1b".to_vec())]
        );
    }

    #[test]
    fn live_snapshot_input_forwards_mouse_sequence_when_mouse_focus_disabled() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input_with_mouse(b"\x1b[<0;1;2Mtyped", &mut state, false,),
            vec![LiveSnapshotInputAction::Forward(
                b"\x1b[<0;1;2Mtyped".to_vec()
            )]
        );
    }

    #[test]
    fn live_snapshot_input_preserves_prefix_when_partial_mouse_prefix_is_not_mouse() {
        let mut state = LiveSnapshotInputState::default();

        assert!(translate_live_snapshot_input(b"\x02", &mut state).is_empty());
        assert_eq!(
            translate_live_snapshot_input(b"\x1b", &mut state),
            vec![LiveSnapshotInputAction::Forward(b"\x02\x1b".to_vec())]
        );
        assert_eq!(
            translate_live_snapshot_input(b"x", &mut state),
            vec![LiveSnapshotInputAction::Forward(b"x".to_vec())]
        );
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_input_flushes_unresolved_mouse_pending_on_eof() {
        let mut state = LiveSnapshotInputState::default();

        assert!(translate_live_snapshot_input(b"\x02", &mut state).is_empty());
        assert!(translate_live_snapshot_input(b"\x1b[<0;1", &mut state).is_empty());

        assert_eq!(
            finish_live_snapshot_input(&mut state),
            Some(b"\x02\x1b[<0;1".to_vec())
        );
    }

    #[test]
    fn live_snapshot_input_consumes_non_press_mouse_events() {
        let mut state = LiveSnapshotInputState::default();

        assert!(translate_live_snapshot_input(b"\x1b[<0;1;2m", &mut state).is_empty());
        assert!(translate_live_snapshot_input(b"\x1b[<32;1;2M", &mut state).is_empty());
        assert!(translate_live_snapshot_input(b"\x1b[<64;1;2M", &mut state).is_empty());
    }

    #[test]
    fn live_snapshot_input_consumes_modified_left_clicks() {
        let mut state = LiveSnapshotInputState::default();

        assert!(translate_live_snapshot_input(b"\x1b[<4;1;2M", &mut state).is_empty());
        assert!(translate_live_snapshot_input(b"\x1b[<8;1;2M", &mut state).is_empty());
        assert!(translate_live_snapshot_input(b"\x1b[<16;1;2M", &mut state).is_empty());
    }

    #[test]
    fn live_snapshot_input_mouse_press_clears_pending_prefix() {
        let mut state = LiveSnapshotInputState::default();

        assert!(translate_live_snapshot_input(b"\x02", &mut state).is_empty());
        let actions = translate_live_snapshot_input(b"\x1b[<0;1;2Md", &mut state);

        assert_eq!(
            actions,
            vec![
                LiveSnapshotInputAction::MousePress(MousePosition { col: 1, row: 2 }),
                LiveSnapshotInputAction::Forward(b"d".to_vec()),
            ]
        );
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_input_mouse_press_clears_number_selection() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input(b"\x02q", &mut state),
            vec![LiveSnapshotInputAction::ShowPaneNumbers]
        );
        let actions = translate_live_snapshot_input(b"\x1b[<0;1;2M1", &mut state);

        assert_eq!(
            actions,
            vec![
                LiveSnapshotInputAction::MousePress(MousePosition { col: 1, row: 2 }),
                LiveSnapshotInputAction::Forward(b"1".to_vec()),
            ]
        );
        assert!(!state.selecting_pane);
    }

    #[test]
    fn live_snapshot_input_detaches_on_prefix_d_without_forwarding_bytes() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"\x02d", &mut state);

        assert_eq!(actions, vec![LiveSnapshotInputAction::Detach]);
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_input_forwards_literal_prefix_with_regular_key() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"\x02x", &mut state);

        assert_eq!(
            actions,
            vec![LiveSnapshotInputAction::Forward(b"\x02x".to_vec())]
        );
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_input_forwards_single_literal_prefix_on_double_prefix() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"\x02\x02", &mut state);

        assert_eq!(actions, vec![LiveSnapshotInputAction::Forward(vec![0x02])]);
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_input_selects_next_pane_on_prefix_o() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"\x02o", &mut state);

        assert_eq!(actions, vec![LiveSnapshotInputAction::SelectNextPane]);
        assert!(!state.saw_prefix);
    }

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

        assert_eq!(
            actions,
            vec![LiveSnapshotInputAction::Forward(b"x".to_vec())]
        );
        assert!(!state.selecting_pane);
    }

    #[test]
    fn live_snapshot_input_enters_copy_mode_on_prefix_bracket() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"\x02[", &mut state);

        assert_eq!(
            actions,
            vec![LiveSnapshotInputAction::EnterCopyMode {
                initial_input: Vec::new(),
            }]
        );
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_input_passes_coalesced_copy_mode_keys_as_initial_input() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"\x02[y", &mut state);

        assert_eq!(
            actions,
            vec![LiveSnapshotInputAction::EnterCopyMode {
                initial_input: vec![b'y'],
            }]
        );
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_input_forwards_bytes_before_copy_mode_prefix() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"abc\x02[y", &mut state);

        assert_eq!(
            actions,
            vec![
                LiveSnapshotInputAction::Forward(b"abc".to_vec()),
                LiveSnapshotInputAction::EnterCopyMode {
                    initial_input: vec![b'y'],
                },
            ]
        );
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_input_preserves_order_when_prefix_o_shares_a_read() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"abc\x02odef", &mut state);

        assert_eq!(
            actions,
            vec![
                LiveSnapshotInputAction::Forward(b"abc".to_vec()),
                LiveSnapshotInputAction::SelectNextPane,
                LiveSnapshotInputAction::Forward(b"def".to_vec()),
            ]
        );
        assert!(!state.saw_prefix);
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

    #[test]
    fn pane_number_message_brackets_active_pane() {
        let entries = parse_pane_listing("0\t0\n1\t1\n2\t0\n").unwrap();

        assert_eq!(format_pane_number_message(&entries), "panes: 0 [1] 2");
    }

    #[test]
    fn pane_index_exists_checks_listing() {
        let entries = parse_pane_listing("0\t0\n1\t1\n").unwrap();

        assert!(pane_index_exists(&entries, 1));
        assert!(!pane_index_exists(&entries, 9));
    }

    #[test]
    fn live_snapshot_input_flushes_pending_prefix_on_eof() {
        let mut state = LiveSnapshotInputState {
            saw_prefix: true,
            selecting_pane: false,
            mouse_pending: Vec::new(),
        };

        let pending = finish_live_snapshot_input(&mut state);

        assert_eq!(pending, Some(vec![0x02]));
        assert!(!state.saw_prefix);
    }

    #[test]
    fn copy_mode_view_moves_with_vi_and_emacs_keys() {
        let mut view = CopyModeView::from_numbered_output("1\tfirst\n2\tsecond\n").unwrap();

        assert_eq!(view.cursor_line_number(), Some(1));
        assert_eq!(view.apply_key(b'j'), CopyModeAction::Redraw);
        assert_eq!(view.cursor_line_number(), Some(2));
        assert_eq!(view.apply_key(0x10), CopyModeAction::Redraw);
        assert_eq!(view.cursor_line_number(), Some(1));
    }

    #[test]
    fn copy_mode_view_copies_current_line() {
        let mut view = CopyModeView::from_numbered_output("7\tselected\n").unwrap();

        assert_eq!(view.apply_key(b'y'), CopyModeAction::CopyLine(7));
    }

    #[test]
    fn copy_mode_view_exits_on_q_or_escape() {
        let mut view = CopyModeView::from_numbered_output("1\tfirst\n").unwrap();

        assert_eq!(view.apply_key(b'q'), CopyModeAction::Exit);
        assert_eq!(view.apply_key(0x1b), CopyModeAction::Exit);
    }

    #[test]
    fn copy_mode_mouse_click_copies_clicked_line() {
        let mut view = CopyModeView::from_numbered_output("3\talpha\n4\tbeta\n").unwrap();

        assert_eq!(
            view.apply_mouse_event(SgrMouseEvent {
                code: 0,
                col: 1,
                row: 3,
                release: false,
            }),
            CopyModeAction::Redraw
        );
        assert_eq!(
            view.apply_mouse_event(SgrMouseEvent {
                code: 0,
                col: 1,
                row: 3,
                release: true,
            }),
            CopyModeAction::CopyLineRange { start: 4, end: 4 }
        );
    }

    #[test]
    fn copy_mode_mouse_drag_copies_inclusive_line_range() {
        let mut view = CopyModeView::from_numbered_output("10\tone\n11\ttwo\n12\tthree\n").unwrap();

        assert_eq!(
            view.apply_mouse_event(SgrMouseEvent {
                code: 0,
                col: 1,
                row: 2,
                release: false,
            }),
            CopyModeAction::Redraw
        );
        assert_eq!(
            view.apply_mouse_event(SgrMouseEvent {
                code: 32,
                col: 1,
                row: 4,
                release: false,
            }),
            CopyModeAction::Redraw
        );
        assert_eq!(
            view.apply_mouse_event(SgrMouseEvent {
                code: 0,
                col: 1,
                row: 4,
                release: true,
            }),
            CopyModeAction::CopyLineRange { start: 10, end: 12 }
        );
    }

    #[test]
    fn parses_sgr_mouse_press_and_release() {
        assert_eq!(
            parse_sgr_mouse_event(b"\x1b[<0;12;3M"),
            Some((
                SgrMouseEvent {
                    code: 0,
                    col: 12,
                    row: 3,
                    release: false,
                },
                10,
            ))
        );
        assert_eq!(
            parse_sgr_mouse_event(b"\x1b[<0;12;3m"),
            Some((
                SgrMouseEvent {
                    code: 0,
                    col: 12,
                    row: 3,
                    release: true,
                },
                10,
            ))
        );
    }

    #[test]
    fn copy_mode_input_buffers_split_sgr_mouse_event() {
        let mut view = CopyModeView::from_numbered_output("3\talpha\n4\tbeta\n").unwrap();
        let mut input = CopyModeInputState::default();

        assert_eq!(input.apply(&mut view, b"\x1b[<0;1"), Vec::new());
        assert_eq!(input.apply(&mut view, b";3M"), vec![CopyModeAction::Redraw]);
        assert_eq!(view.cursor_line_number(), Some(4));
    }

    #[test]
    fn copy_mode_mouse_release_without_anchor_does_not_copy() {
        let mut view = CopyModeView::from_numbered_output("3\talpha\n4\tbeta\n").unwrap();

        assert_eq!(
            view.apply_mouse_event(SgrMouseEvent {
                code: 0,
                col: 1,
                row: 2,
                release: true,
            }),
            CopyModeAction::Ignore
        );
    }

    #[test]
    fn copy_mode_mouse_release_outside_lines_clears_anchor() {
        let mut view = CopyModeView::from_numbered_output("3\talpha\n4\tbeta\n").unwrap();

        assert_eq!(
            view.apply_mouse_event(SgrMouseEvent {
                code: 0,
                col: 1,
                row: 2,
                release: false,
            }),
            CopyModeAction::Redraw
        );
        assert_eq!(
            view.apply_mouse_event(SgrMouseEvent {
                code: 0,
                col: 1,
                row: 99,
                release: true,
            }),
            CopyModeAction::Ignore
        );
        assert_eq!(
            view.apply_mouse_event(SgrMouseEvent {
                code: 0,
                col: 1,
                row: 3,
                release: true,
            }),
            CopyModeAction::Ignore
        );
    }

    #[test]
    fn attach_input_dispatches_copy_mode_prefix_without_forwarding_bytes() {
        let actions = translate_attach_input(b"\x02[", &mut false);

        assert_eq!(
            actions,
            vec![AttachInputAction::EnterCopyMode {
                initial_input: Vec::new(),
            }]
        );
    }

    #[test]
    fn attach_input_shows_help_on_prefix_question() {
        let actions = translate_attach_input(b"\x02?", &mut false);

        assert_eq!(actions, vec![AttachInputAction::ShowHelp]);
    }

    #[test]
    fn attach_input_detaches_on_prefix_d_without_forwarding_bytes() {
        let actions = translate_attach_input(b"\x02d", &mut false);

        assert_eq!(actions, vec![AttachInputAction::Detach]);
    }

    #[test]
    fn attach_input_passes_coalesced_copy_mode_keys_as_initial_input() {
        let actions = translate_attach_input(b"\x02[y", &mut false);

        assert_eq!(
            actions,
            vec![AttachInputAction::EnterCopyMode {
                initial_input: vec![b'y'],
            }]
        );
    }

    #[test]
    fn attach_input_preserves_order_when_prefix_question_shares_a_read() {
        let actions = translate_attach_input(b"abc\x02?def", &mut false);

        assert_eq!(
            actions,
            vec![
                AttachInputAction::Forward(b"abc".to_vec()),
                AttachInputAction::ShowHelp,
                AttachInputAction::Forward(b"def".to_vec()),
            ]
        );
    }

    #[test]
    fn attach_help_message_reuses_cli_summary() {
        assert_eq!(attach_help_message(), crate::cli::attach_help_summary());
        assert!(attach_help_message().contains("C-b C-b literal prefix"));
        assert!(attach_help_message().contains("dmux select-pane"));
    }

    #[test]
    fn live_snapshot_input_shows_help_on_prefix_question() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"\x02?", &mut state);

        assert_eq!(actions, vec![LiveSnapshotInputAction::ShowHelp]);
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_input_preserves_order_when_prefix_question_shares_a_read() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"abc\x02?def", &mut state);

        assert_eq!(
            actions,
            vec![
                LiveSnapshotInputAction::Forward(b"abc".to_vec()),
                LiveSnapshotInputAction::ShowHelp,
                LiveSnapshotInputAction::Forward(b"def".to_vec()),
            ]
        );
        assert!(!state.saw_prefix);
    }
}
